use async_graphql::SimpleObject;
#[cfg(feature = "change-journal")]
use graphql_orm::graphql::orm::{BackupChangeAction, ChangeWindow};
use graphql_orm::graphql::orm::{
    Entity, GraphqlOrmBackupRuntime, Migration, MigrationRunner, RestoreContext,
    build_migration_plan,
};
use graphql_orm::prelude::*;
use sqlx::Row;
use std::collections::BTreeMap;

#[derive(SimpleObject, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Default)]
struct BackupProfile {
    display_name: String,
    flags: Vec<String>,
}

#[cfg(feature = "change-journal")]
#[tokio::test]
async fn change_journal_records_generated_repository_writes()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone()).with_change_journal();
    apply_schema(&database, "2026051403_backup_change_journal").await?;
    database.ensure_change_journal_table().await?;

    let account = BackupRtAccount::insert(
        &database,
        CreateBackupRtAccountInput {
            email: "journal@example.com".to_string(),
            password_hash: "secret-hash".to_string(),
            reset_token: None,
            profile: BackupProfile {
                display_name: "Journal".to_string(),
                flags: vec![],
            },
            avatar: vec![9, 8, 7],
            nickname: None,
        },
    )
    .await?;

    let until = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let changes = database
        .export_changes(ChangeWindow {
            after_snapshot_id: None,
            until,
        })
        .await?;

    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].entity_name, "BackupRtAccount");
    assert_eq!(changes[0].table_name, "backup_rt_accounts");
    assert_eq!(changes[0].primary_key, account.id.to_string());
    assert_eq!(changes[0].action, BackupChangeAction::Create);
    Ok(())
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "backup_rt_accounts",
    plural = "BackupRtAccounts",
    default_sort = "email ASC"
)]
struct BackupRtAccount {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub email: String,

    #[backup(redact)]
    pub password_hash: String,

    #[backup(exclude)]
    pub reset_token: Option<String>,

    #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
    pub profile: BackupProfile,

    pub avatar: Vec<u8>,

    pub nickname: Option<String>,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql_entity(
    table = "backup_rt_notes",
    plural = "BackupRtNotes",
    default_sort = "body ASC"
)]
#[graphql(complex)]
struct BackupRtNote {
    #[primary_key]
    #[filterable(type = "string")]
    pub id: String,

    #[filterable(type = "uuid")]
    pub account_id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub body: String,

    #[graphql(skip)]
    #[relation(target = "BackupRtAccount", from = "account_id", to = "id")]
    pub account: Option<BackupRtAccount>,
}

impl graphql_orm::graphql::loaders::BatchLoadEntity for BackupRtAccount {
    fn batch_column() -> &'static str {
        "id"
    }

    #[cfg(feature = "sqlite")]
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        row.try_get("id")
    }

    #[cfg(feature = "postgres")]
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        let id: graphql_orm::uuid::Uuid = row.try_get("id")?;
        Ok(id.to_string())
    }
}

schema_roots! {
    query_custom_ops: [],
    entities: [BackupRtAccount, BackupRtNote],
}

#[cfg(feature = "sqlite")]
type TestPool = sqlx::SqlitePool;
#[cfg(feature = "postgres")]
type TestPool = sqlx::PgPool;

#[cfg(feature = "sqlite")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    Ok(sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?)
}

#[cfg(feature = "postgres")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:55432/postgres".to_string());
    let pool = sqlx::PgPool::connect(&database_url).await?;
    sqlx::query("DROP TABLE IF EXISTS backup_rt_notes")
        .execute(&pool)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS backup_rt_accounts")
        .execute(&pool)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS __graphql_orm_migrations")
        .execute(&pool)
        .await?;
    Ok(pool)
}

async fn apply_schema(
    database: &graphql_orm::db::Database,
    version: &'static str,
) -> Result<(), Box<dyn std::error::Error>> {
    let target_schema = graphql_orm::graphql::orm::SchemaModel::from_entities(&[
        <BackupRtAccount as Entity>::metadata(),
        <BackupRtNote as Entity>::metadata(),
    ]);
    let plan = build_migration_plan(
        if cfg!(feature = "postgres") {
            graphql_orm::graphql::orm::DatabaseBackend::Postgres
        } else {
            graphql_orm::graphql::orm::DatabaseBackend::Sqlite
        },
        &graphql_orm::graphql::orm::SchemaModel { tables: Vec::new() },
        &target_schema,
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
            version,
            description: "backup_roundtrip_schema",
            statements,
        }])
        .await?;
    Ok(())
}

#[tokio::test]
async fn full_logical_backup_restores_into_empty_database() -> Result<(), Box<dyn std::error::Error>>
{
    let source_pool = setup_pool().await?;
    let source = graphql_orm::db::Database::new(source_pool.clone());
    apply_schema(&source, "2026051401_backup_roundtrip_source").await?;

    let account = BackupRtAccount::insert(
        &source_pool,
        CreateBackupRtAccountInput {
            email: "ada@example.com".to_string(),
            password_hash: "secret-hash".to_string(),
            reset_token: Some("one-time".to_string()),
            profile: BackupProfile {
                display_name: "Ada".to_string(),
                flags: vec!["admin".to_string(), "research".to_string()],
            },
            avatar: vec![0, 1, 2, 3, 255],
            nickname: Some("countess".to_string()),
        },
    )
    .await?;
    let note = BackupRtNote::insert(
        &source_pool,
        CreateBackupRtNoteInput {
            account_id: account.id,
            body: "first note".to_string(),
        },
    )
    .await?;

    let source_snapshot = source.schema_snapshot(
        "2026051401_backup_roundtrip",
        &graphql_orm_entity_metadata(),
    );
    assert!(!source_snapshot.schema_hash.is_empty());

    let mut descriptors = source.list_backup_entities(&graphql_orm_entity_metadata());
    descriptors.sort_by_key(|descriptor| (descriptor.export_order, descriptor.table_name.clone()));
    let mut snapshot = source.begin_consistent_snapshot().await?;
    let mut exported = Vec::new();
    for descriptor in &descriptors {
        exported.push((
            descriptor.clone(),
            source.export_table_rows(&mut snapshot, descriptor).await?,
        ));
    }

    let dest_pool = setup_pool().await?;
    let dest = graphql_orm::db::Database::new(dest_pool.clone());
    apply_schema(&dest, "2026051402_backup_roundtrip_dest").await?;

    let rows_by_table = exported
        .into_iter()
        .map(|(descriptor, rows)| (descriptor.table_name, rows))
        .collect::<BTreeMap<_, _>>();
    let dest_snapshot = dest.schema_snapshot(
        "2026051401_backup_roundtrip",
        &graphql_orm_entity_metadata(),
    );
    let reports = dest
        .restore_backup_rows(
            &source_snapshot,
            &dest_snapshot,
            &rows_by_table,
            &RestoreContext::empty_database(),
        )
        .await?;
    for report in &reports {
        let expected = rows_by_table
            .get(&report.table_name)
            .map(Vec::len)
            .unwrap_or(0);
        assert_eq!(report.rows_imported, expected);
        assert_eq!(report.rows_validated, expected);
    }

    let restored_account = BackupRtAccount::get(&dest_pool, &account.id)
        .await?
        .expect("account should restore");
    assert_eq!(restored_account.email, account.email);
    assert_eq!(restored_account.password_hash, "[graphql-orm:redacted]");
    assert_eq!(restored_account.reset_token, None);
    assert_eq!(restored_account.profile, account.profile);
    assert_eq!(restored_account.avatar, account.avatar);
    assert_eq!(restored_account.nickname, account.nickname);
    assert_eq!(restored_account.created_at, account.created_at);
    assert_eq!(restored_account.updated_at, account.updated_at);

    let restored_note = BackupRtNote::get(&dest_pool, &note.id)
        .await?
        .expect("note should restore");
    assert_eq!(restored_note.id, note.id);
    assert_eq!(restored_note.account_id, account.id);
    assert_eq!(restored_note.body, note.body);

    Ok(())
}
