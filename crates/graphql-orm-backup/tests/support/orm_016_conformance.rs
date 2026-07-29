use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm::graphql::orm::{
    DatabaseBackend, Entity, EntityMetadata, Migration, MigrationRunner, OrmSchemaModule,
    SchemaModuleCatalog, SchemaModuleDescriptor, SchemaModel, build_migration_plan,
};
use graphql_orm::prelude::*;
use graphql_orm::sqlx::Row as _;
use graphql_orm_backup::{
    BackupError, BackupObjectIndex, BackupObjectRef, BackupTableExport, ColumnBackupPolicy,
    FullBackupRequest, GraphqlOrmBackupAdapter, LocalBackupRepository, OrmBackupAdapter,
    RestoreContext, RestoreObjectSink, bytes_sha256_hex, create_full_backup, restore_objects,
    restore_snapshot, verify_manifest_and_objects,
};
use tempfile::TempDir;
use uuid::Uuid;

#[cfg(all(feature = "orm-sqlite", feature = "orm-postgres"))]
compile_error!("ORM conformance tests require exactly one database backend");

mod identity_module {
    use super::*;

    #[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
    #[graphql_entity(
        table = "conformance_identity_accounts",
        plural = "ConformanceIdentityAccounts",
        default_sort = "name ASC"
    )]
    pub(super) struct ConformanceAccount {
        #[primary_key]
        #[filterable(type = "uuid")]
        pub id: graphql_orm::uuid::Uuid,

        pub name: String,

        #[json_field]
        pub profile: Option<serde_json::Value>,

        #[backup(redact)]
        pub secret: String,

        #[backup(exclude)]
        pub ephemeral_token: Option<String>,
    }

    static DESCRIPTOR: SchemaModuleDescriptor = SchemaModuleDescriptor::new(
        "com.example.backup.identity",
        "1.0.0",
        "conformance_identity_",
    );

    pub(super) struct IdentityModule;

    impl OrmSchemaModule for IdentityModule {
        fn descriptor(&self) -> &SchemaModuleDescriptor {
            &DESCRIPTOR
        }

        fn entities(&self) -> &[&'static EntityMetadata] {
            static ENTITIES: OnceLock<Vec<&'static EntityMetadata>> = OnceLock::new();
            ENTITIES.get_or_init(|| vec![ConformanceAccount::metadata()])
        }
    }
}

mod activity_module {
    use super::*;

    #[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
    #[graphql_entity(
        table = "conformance_activity_events",
        plural = "ConformanceActivityEvents",
        default_sort = "id ASC"
    )]
    pub(super) struct ConformanceEvent {
        #[primary_key]
        #[filterable(type = "uuid")]
        pub id: graphql_orm::uuid::Uuid,

        #[filterable(type = "uuid")]
        pub account_id: Option<graphql_orm::uuid::Uuid>,

        #[json_field]
        pub payload: Option<serde_json::Value>,

        pub note: Option<String>,

        #[graphql(skip)]
        #[relation(
            target = "ConformanceAccount",
            from = "account_id",
            to = "id",
            on_delete = "restrict"
        )]
        pub account: Option<String>,
    }

    static DESCRIPTOR: SchemaModuleDescriptor = SchemaModuleDescriptor::new(
        "com.example.backup.activity",
        "1.0.0",
        "conformance_activity_",
    );

    pub(super) struct ActivityModule;

    impl OrmSchemaModule for ActivityModule {
        fn descriptor(&self) -> &SchemaModuleDescriptor {
            &DESCRIPTOR
        }

        fn entities(&self) -> &[&'static EntityMetadata] {
            static ENTITIES: OnceLock<Vec<&'static EntityMetadata>> = OnceLock::new();
            ENTITIES.get_or_init(|| vec![ConformanceEvent::metadata()])
        }
    }
}

#[cfg(feature = "orm-sqlite")]
type TestPool = graphql_orm::sqlx::SqlitePool;
#[cfg(feature = "orm-postgres")]
type TestPool = graphql_orm::sqlx::PgPool;

#[cfg(feature = "orm-sqlite")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    let pool = graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    graphql_orm::sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;
    Ok(pool)
}

#[cfg(feature = "orm-postgres")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    let database_url =
        std::env::var("GRAPHQL_ORM_BACKUP_TEST_DATABASE_URL").map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "GRAPHQL_ORM_BACKUP_TEST_DATABASE_URL must name an owned disposable database",
            )
        })?;
    let pool = graphql_orm::sqlx::PgPool::connect(&database_url).await?;
    graphql_orm::sqlx::query(
        "DROP TABLE IF EXISTS conformance_activity_events, conformance_identity_accounts CASCADE",
    )
    .execute(&pool)
    .await?;
    graphql_orm::sqlx::query("DROP TABLE IF EXISTS __graphql_orm_migrations")
        .execute(&pool)
        .await?;
    Ok(pool)
}

fn catalog() -> Result<SchemaModuleCatalog, Box<dyn std::error::Error>> {
    let identity = identity_module::IdentityModule;
    let activity = activity_module::ActivityModule;
    Ok(SchemaModuleCatalog::compose(&[&identity, &activity])?)
}

fn backend() -> DatabaseBackend {
    #[cfg(feature = "orm-sqlite")]
    {
        DatabaseBackend::Sqlite
    }
    #[cfg(feature = "orm-postgres")]
    {
        DatabaseBackend::Postgres
    }
}

async fn apply_schema(
    database: &graphql_orm::db::Database,
    catalog: &SchemaModuleCatalog,
) -> Result<(), Box<dyn std::error::Error>> {
    let plan = build_migration_plan(
        backend(),
        &SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
        &catalog.schema_model(),
    );
    let statements: &'static [&'static str] = Box::leak(
        plan.statements
            .iter()
            .map(|statement| Box::leak(statement.clone().into_boxed_str()) as &'static str)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    database
        .apply_migrations(&[Migration {
            version: "orm-0.15-conformance",
            description: "private module backup conformance",
            statements,
        }])
        .await?;
    Ok(())
}

fn account_id() -> Uuid {
    Uuid::parse_str("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa").expect("valid account uuid")
}

fn event_id() -> Uuid {
    Uuid::parse_str("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb").expect("valid event uuid")
}

fn object_id() -> Uuid {
    Uuid::parse_str("cccccccc-cccc-4ccc-cccc-cccccccccccc").expect("valid object uuid")
}

fn snapshot_id() -> Uuid {
    Uuid::parse_str("dddddddd-dddd-4ddd-dddd-dddddddddddd").expect("valid snapshot uuid")
}

#[cfg(feature = "orm-sqlite")]
async fn insert_fixture(pool: &TestPool) -> Result<(), Box<dyn std::error::Error>> {
    graphql_orm::sqlx::query(
        "INSERT INTO conformance_identity_accounts
         (id, name, profile, secret, ephemeral_token)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(account_id().to_string())
    .bind("Ada")
    .bind(serde_json::json!({"tier": "research"}).to_string())
    .bind("source-secret")
    .bind("one-time-token")
    .execute(pool)
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO conformance_activity_events
         (id, account_id, payload, note)
         VALUES (?, ?, ?, ?)",
    )
    .bind(event_id().to_string())
    .bind(account_id().to_string())
    .bind(serde_json::json!({"kind": "created"}).to_string())
    .bind("source-note")
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(feature = "orm-postgres")]
async fn insert_fixture(pool: &TestPool) -> Result<(), Box<dyn std::error::Error>> {
    graphql_orm::sqlx::query(
        "INSERT INTO conformance_identity_accounts
         (id, name, profile, secret, ephemeral_token)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(account_id())
    .bind("Ada")
    .bind(serde_json::json!({"tier": "research"}))
    .bind("source-secret")
    .bind("one-time-token")
    .execute(pool)
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO conformance_activity_events
         (id, account_id, payload, note)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(event_id())
    .bind(account_id())
    .bind(serde_json::json!({"kind": "created"}))
    .bind("source-note")
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(feature = "orm-sqlite")]
async fn assert_restored_rows(pool: &TestPool) -> Result<(), Box<dyn std::error::Error>> {
    let account = graphql_orm::sqlx::query(
        "SELECT id, profile, secret, ephemeral_token
         FROM conformance_identity_accounts",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(account.try_get::<String, _>("id")?, account_id().to_string());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&account.try_get::<String, _>("profile")?)?,
        serde_json::json!({"tier": "research"})
    );
    assert_eq!(
        account.try_get::<String, _>("secret")?,
        "[graphql-orm:redacted]"
    );
    assert_eq!(
        account.try_get::<Option<String>, _>("ephemeral_token")?,
        None
    );

    let event = graphql_orm::sqlx::query(
        "SELECT id, account_id, payload, note
         FROM conformance_activity_events",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(event.try_get::<String, _>("id")?, event_id().to_string());
    assert_eq!(
        event.try_get::<Option<String>, _>("account_id")?,
        Some(account_id().to_string())
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&event.try_get::<String, _>("payload")?)?,
        serde_json::json!({"kind": "created"})
    );
    assert_eq!(
        event.try_get::<Option<String>, _>("note")?,
        Some("[graphql-orm:redacted]".to_string())
    );
    Ok(())
}

#[cfg(feature = "orm-postgres")]
async fn assert_restored_rows(pool: &TestPool) -> Result<(), Box<dyn std::error::Error>> {
    let account = graphql_orm::sqlx::query(
        "SELECT id, profile, secret, ephemeral_token
         FROM conformance_identity_accounts",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(account.try_get::<Uuid, _>("id")?, account_id());
    assert_eq!(
        account.try_get::<serde_json::Value, _>("profile")?,
        serde_json::json!({"tier": "research"})
    );
    assert_eq!(
        account.try_get::<String, _>("secret")?,
        "[graphql-orm:redacted]"
    );
    assert_eq!(
        account.try_get::<Option<String>, _>("ephemeral_token")?,
        None
    );

    let event = graphql_orm::sqlx::query(
        "SELECT id, account_id, payload, note
         FROM conformance_activity_events",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(event.try_get::<Uuid, _>("id")?, event_id());
    assert_eq!(
        event.try_get::<Option<Uuid>, _>("account_id")?,
        Some(account_id())
    );
    assert_eq!(
        event.try_get::<serde_json::Value, _>("payload")?,
        serde_json::json!({"kind": "created"})
    );
    assert_eq!(
        event.try_get::<Option<String>, _>("note")?,
        Some("[graphql-orm:redacted]".to_string())
    );
    Ok(())
}

struct FixedObjectIndex {
    object: BackupObjectRef,
    bytes: Bytes,
}

#[async_trait]
impl BackupObjectIndex for FixedObjectIndex {
    async fn list_objects_for_full_backup(&self) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(vec![self.object.clone()])
    }

    async fn list_objects_for_incremental_backup(
        &self,
        _since_snapshot_id: Uuid,
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        Err(BackupError::UnsupportedOperation {
            operation: "conformance fixture has no change journal".to_string(),
        })
    }

    async fn load_object(&self, _object: &BackupObjectRef) -> Result<Bytes, BackupError> {
        Ok(self.bytes.clone())
    }
}

#[derive(Default)]
struct RecordingObjectSink {
    restored: Arc<Mutex<Vec<(BackupObjectRef, Bytes)>>>,
}

#[async_trait]
impl RestoreObjectSink for RecordingObjectSink {
    async fn restore_object(
        &self,
        object: BackupObjectRef,
        bytes: Bytes,
    ) -> Result<(), BackupError> {
        self.restored
            .lock()
            .expect("restored object lock")
            .push((object, bytes));
        Ok(())
    }
}

#[tokio::test]
#[cfg_attr(
    feature = "orm-postgres",
    ignore = "requires GRAPHQL_ORM_BACKUP_TEST_DATABASE_URL for an owned disposable database"
)]
async fn private_modules_round_trip_through_graphql_orm_016(
) -> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let database = Arc::new(graphql_orm::db::Database::new(pool.clone()));
    let catalog = catalog()?;
    assert_eq!(catalog.modules().len(), 2);
    assert_eq!(catalog.entities().len(), 2);
    apply_schema(database.as_ref(), &catalog).await?;
    insert_fixture(&pool).await?;

    let base_adapter = OrmBackupAdapter::new(database.clone(), catalog.entities().to_vec());
    let base_snapshot = base_adapter.current_schema_snapshot().await?;
    let adapter = OrmBackupAdapter::new(database.clone(), catalog.entities().to_vec())
        .with_column_backup_policy(
            "conformance_activity_events",
            "note",
            ColumnBackupPolicy::Redact,
        );
    let overridden_snapshot = adapter.current_schema_snapshot().await?;
    assert_ne!(base_snapshot.schema_hash, overridden_snapshot.schema_hash);

    let weakening_adapter =
        OrmBackupAdapter::new(database.clone(), catalog.entities().to_vec())
            .with_column_backup_policy(
                "conformance_identity_accounts",
                "secret",
                ColumnBackupPolicy::Include,
            );
    assert!(matches!(
        weakening_adapter.current_schema_snapshot().await,
        Err(BackupError::InvalidColumnBackupPolicyOverride { .. })
    ));

    let mut incompatible_schema = adapter.schema_snapshot().await?;
    incompatible_schema.schema_hash.push_str("-different");
    assert!(matches!(
        adapter
            .restore_full(
                incompatible_schema,
                Vec::<BackupTableExport>::new(),
                RestoreContext::empty_database(),
            )
            .await,
        Err(BackupError::RestoreSchemaMismatch { .. })
    ));

    let object_bytes = Bytes::from_static(b"exact-conformance-object");
    let object = BackupObjectRef {
        object_id: object_id(),
        storage_key: "conformance/original.bin".to_string(),
        sha256_hex: bytes_sha256_hex(&object_bytes),
        size_bytes: object_bytes.len() as u64,
        mime_type: Some("application/octet-stream".to_string()),
    };
    let objects = FixedObjectIndex {
        object: object.clone(),
        bytes: object_bytes.clone(),
    };
    let temp = TempDir::new()?;
    let repository = LocalBackupRepository::new(temp.path());
    let backup = create_full_backup(
        &repository,
        &adapter,
        &objects,
        FullBackupRequest {
            snapshot_id: snapshot_id(),
            created_at: 1_775_174_400,
            app_id: "orm-private-module-conformance".to_string(),
            app_version: "0.6.0".to_string(),
        },
    )
    .await?;
    verify_manifest_and_objects(&repository, &backup.manifest).await?;

    adapter.clear_restore_target().await?;
    assert!(adapter.restore_target_is_empty().await?);
    let result = restore_snapshot(
        &repository,
        &adapter,
        snapshot_id(),
        RestoreContext::empty_database(),
    )
    .await?;
    assert_eq!(result.full_table_count, 2);
    assert_eq!(result.full_row_count, 2);
    assert_restored_rows(&pool).await?;

    let sink = RecordingObjectSink::default();
    restore_objects(&repository, &backup.manifest, &sink).await?;
    assert_eq!(
        sink.restored
            .lock()
            .expect("restored object lock")
            .as_slice(),
        &[(object, object_bytes)]
    );

    let invalid_context = RestoreContext {
        mode: graphql_orm_backup::RestoreMode::EmptyDatabase,
        disable_policies: false,
        disable_change_journal: true,
    };
    let current_schema = adapter.schema_snapshot().await?;
    assert!(matches!(
        adapter
            .restore_full(current_schema, Vec::<BackupTableExport>::new(), invalid_context)
            .await,
        Err(BackupError::InvalidRestoreContext { .. })
    ));

    Ok(())
}
