#![allow(dead_code)]

use graphql_orm::prelude::*;
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "external_records",
    plural = "ExternalRecords",
    default_sort = "name ASC"
)]
struct ExternalRecord {
    #[primary_key]
    #[graphql(skip)]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "credentials",
    plural = "Credentials",
    default_sort = "principal ASC"
)]
struct Credential {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub principal: String,

    #[graphql_orm(private)]
    pub password_hash: String,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "blob_assets",
    plural = "BlobAssets",
    default_sort = "label ASC"
)]
struct BlobAsset {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub label: String,

    pub payload: Vec<u8>,

    pub thumbnail: Option<Vec<u8>>,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "server_managed_logs",
    plural = "ServerManagedLogs",
    default_sort = "message ASC"
)]
struct ServerManagedLog {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub message: String,

    #[graphql_orm(write = false, default = "CURRENT_TIMESTAMP")]
    pub created_at: String,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "sync_profiles",
    plural = "SyncProfiles",
    default_sort = "name ASC"
)]
struct SyncProfile {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[graphql_orm(skip_input)]
    #[filterable(type = "string")]
    #[sortable]
    pub sync_status: String,
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(table = "relation_parents", plural = "RelationParents")]
struct RelationParent {
    #[primary_key]
    pub id: String,
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(table = "relation_children_no_fk", plural = "RelationChildrenNoFk")]
struct RelationChildNoPhysicalFk {
    #[primary_key]
    pub id: String,

    pub parent_id: String,

    #[graphql(skip)]
    #[relation(
        target = "RelationParent",
        from = "parent_id",
        to = "id",
        emit_fk = false
    )]
    pub parent: Option<String>,
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(table = "relation_children_with_fk", plural = "RelationChildrenWithFk")]
struct RelationChildWithPhysicalFk {
    #[primary_key]
    pub id: String,

    pub parent_id: String,

    #[graphql(skip)]
    #[relation(target = "RelationParent", from = "parent_id", to = "id")]
    pub parent: Option<String>,
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(table = "sql_default_examples", plural = "SqlDefaultExamples")]
struct SqlDefaultExample {
    #[primary_key]
    pub id: String,

    #[graphql_orm(write = false, default = "CURRENT_TIMESTAMP")]
    pub created_at: String,

    #[graphql_orm(write = false, default = "lower(hex(randomblob(16)))")]
    pub token: String,
}

schema_roots! {
    query_custom_ops: [],
    entities: [ExternalRecord, Credential, BlobAsset, ServerManagedLog, SyncProfile],
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
    for table in [
        "external_records",
        "credentials",
        "blob_assets",
        "server_managed_logs",
        "sync_profiles",
        "relation_children_no_fk",
        "relation_children_with_fk",
        "relation_parents",
        "sql_default_examples",
    ] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {table} CASCADE"))
            .execute(&pool)
            .await?;
    }
    Ok(pool)
}

fn test_mutex() -> &'static tokio::sync::Mutex<()> {
    static TEST_MUTEX: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    TEST_MUTEX.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn sdl_input_block<'a>(sdl: &'a str, input_name: &str) -> Option<&'a str> {
    let marker = format!("input {input_name} {{");
    let start = sdl.find(&marker)?;
    let rest = &sdl[start..];
    let end = rest.find("\n}")?;
    Some(&rest[..end + 2])
}

async fn apply_schema(
    database: &graphql_orm::db::Database,
    entities: &[&graphql_orm::graphql::orm::EntityMetadata],
) -> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{SchemaStage, SchemaStageRunner};

    let version = format!(
        "2026040901_contract_gaps_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    database
        .apply_schema_stages(&[SchemaStage::from_entities(
            version,
            "contract_gaps",
            entities,
        )])
        .await?;
    Ok(())
}

#[derive(Clone, Default)]
struct SyncProfileTransform {
    calls: Arc<Mutex<Vec<String>>>,
}

impl SyncProfileTransform {
    fn snapshot(&self) -> Vec<String> {
        self.calls.lock().expect("transform calls lock").clone()
    }
}

impl graphql_orm::graphql::orm::WriteInputTransform for SyncProfileTransform {
    fn before_create<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if entity_name == "SyncProfile" {
                let input = input
                    .downcast_mut::<CreateSyncProfileInput>()
                    .ok_or_else(|| async_graphql::Error::new("unexpected create input type"))?;
                if input.sync_status.is_empty() {
                    input.sync_status = "created-by-transform".to_string();
                }
                self.calls
                    .lock()
                    .expect("transform calls lock")
                    .push(format!("create:{}", input.sync_status));
            }
            Ok(())
        })
    }

    fn before_update<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        _existing_row: Option<&'a (dyn std::any::Any + Send + Sync)>,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if entity_name == "SyncProfile" {
                let input = input
                    .downcast_mut::<UpdateSyncProfileInput>()
                    .ok_or_else(|| async_graphql::Error::new("unexpected update input type"))?;
                if input.sync_status.is_none() {
                    input.sync_status = Some("updated-by-transform".to_string());
                }
                self.calls
                    .lock()
                    .expect("transform calls lock")
                    .push(format!("update:{:?}", input.sync_status));
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn hidden_primary_key_is_excluded_from_graphql_input_but_settable_via_repo_insert()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    apply_schema(&database, &[<ExternalRecord as Entity>::metadata()]).await?;

    let schema = schema_builder(database.clone())
        .data("system".to_string())
        .finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("type ExternalRecord"));
    assert!(sdl.contains("id: String!"));
    let create_input = sdl_input_block(&sdl, "CreateExternalRecordInput")
        .expect("CreateExternalRecordInput block");
    assert!(!create_input.contains("id:"));

    let created = ExternalRecord::insert(
        &pool,
        CreateExternalRecordInput {
            id: "remote-42".to_string(),
            name: "Remote mirror".to_string(),
        },
    )
    .await?;

    assert_eq!(created.id, "remote-42");
    assert_eq!(
        ExternalRecord::get(&pool, &"remote-42".to_string())
            .await?
            .expect("external record exists")
            .name,
        "Remote mirror"
    );

    Ok(())
}

#[tokio::test]
async fn skip_input_fields_remain_publicly_readable_but_stay_out_of_graphql_write_inputs()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let transform = SyncProfileTransform::default();
    let mut database = graphql_orm::db::Database::new(pool.clone());
    database.set_write_input_transform(transform.clone());
    apply_schema(&database, &[<SyncProfile as Entity>::metadata()]).await?;

    let schema = schema_builder(database.clone())
        .data("system".to_string())
        .finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("type SyncProfile"));
    assert!(sdl.contains("syncStatus: String!"));

    let create_input =
        sdl_input_block(&sdl, "CreateSyncProfileInput").expect("CreateSyncProfileInput block");
    assert!(create_input.contains("name: String!"));
    assert!(!create_input.contains("syncStatus:"));

    let update_input =
        sdl_input_block(&sdl, "UpdateSyncProfileInput").expect("UpdateSyncProfileInput block");
    assert!(update_input.contains("name: String"));
    assert!(!update_input.contains("syncStatus:"));

    let direct_create = CreateSyncProfileInput {
        name: "Repo created".to_string(),
        sync_status: "repo-managed".to_string(),
    };
    let repo_created = SyncProfile::insert(&database, direct_create).await?;
    assert_eq!(repo_created.sync_status, "repo-managed");

    let repo_updated = SyncProfile::update_by_id(
        &database,
        &repo_created.id,
        UpdateSyncProfileInput {
            sync_status: Some("repo-updated".to_string()),
            ..Default::default()
        },
    )
    .await?
    .expect("updated sync profile");
    assert_eq!(repo_updated.sync_status, "repo-updated");

    let graphql_created = schema
        .execute(
            "mutation {
                createSyncProfile(input: { name: \"GraphQL created\" }) {
                    success
                    syncProfile {
                        id
                        name
                        syncStatus
                    }
                }
            }",
        )
        .await;
    assert!(
        graphql_created.errors.is_empty(),
        "{:?}",
        graphql_created.errors
    );
    let created_json = graphql_created.data.into_json()?;
    assert_eq!(
        created_json["createSyncProfile"]["syncProfile"]["syncStatus"].as_str(),
        Some("created-by-transform")
    );
    let graphql_created_id = graphql_orm::uuid::Uuid::parse_str(
        created_json["createSyncProfile"]["syncProfile"]["id"]
            .as_str()
            .expect("created sync profile id"),
    )?;

    let graphql_updated = schema
        .execute(format!(
            "mutation {{
                updateSyncProfile(id: \"{}\", input: {{ name: \"Renamed from GraphQL\" }}) {{
                    success
                    syncProfile {{
                        id
                        name
                        syncStatus
                    }}
                }}
            }}",
            graphql_created_id
        ))
        .await;
    assert!(
        graphql_updated.errors.is_empty(),
        "{:?}",
        graphql_updated.errors
    );
    let updated_json = graphql_updated.data.into_json()?;
    assert_eq!(
        updated_json["updateSyncProfile"]["syncProfile"]["syncStatus"].as_str(),
        Some("updated-by-transform")
    );
    assert_eq!(
        updated_json["updateSyncProfile"]["syncProfile"]["name"].as_str(),
        Some("Renamed from GraphQL")
    );

    let stored_graphql_profile = SyncProfile::get(&pool, &graphql_created_id)
        .await?
        .expect("stored graphql-created sync profile");
    assert_eq!(stored_graphql_profile.sync_status, "updated-by-transform");

    let transform_calls = transform.snapshot();
    assert!(
        transform_calls
            .iter()
            .any(|entry| entry == "create:repo-managed")
    );
    assert!(
        transform_calls
            .iter()
            .any(|entry| entry == "update:Some(\"repo-updated\")")
    );
    assert!(
        transform_calls
            .iter()
            .any(|entry| entry == "create:created-by-transform")
    );
    assert!(
        transform_calls
            .iter()
            .any(|entry| entry == "update:Some(\"updated-by-transform\")")
    );

    Ok(())
}

#[tokio::test]
async fn private_fields_stay_out_of_graphql_inputs_but_are_writable_in_trusted_code()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    apply_schema(&database, &[<Credential as Entity>::metadata()]).await?;

    let schema = schema_builder(database.clone()).finish();
    let sdl = schema.sdl();
    let create_input =
        sdl_input_block(&sdl, "CreateCredentialInput").expect("CreateCredentialInput block");
    assert!(!create_input.contains("passwordHash:"));
    let update_input =
        sdl_input_block(&sdl, "UpdateCredentialInput").expect("UpdateCredentialInput block");
    assert!(!update_input.contains("passwordHash:"));

    let created = Credential::insert(
        &pool,
        CreateCredentialInput {
            principal: "owner@example.com".to_string(),
            password_hash: "hash-v1".to_string(),
        },
    )
    .await?;
    assert_eq!(created.password_hash, "hash-v1");

    let updated = Credential::update_by_id(
        &database,
        &created.id,
        UpdateCredentialInput {
            principal: None,
            password_hash: Some("hash-v2".to_string()),
        },
    )
    .await?
    .expect("credential updated");
    assert_eq!(updated.password_hash, "hash-v2");

    Ok(())
}

#[tokio::test]
async fn bytes_blob_fields_round_trip_through_generated_repo_helpers()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    apply_schema(&database, &[<BlobAsset as Entity>::metadata()]).await?;

    let created = BlobAsset::insert(
        &pool,
        CreateBlobAssetInput {
            label: "asset-a".to_string(),
            payload: vec![1, 2, 3, 4],
            thumbnail: Some(vec![8, 9]),
        },
    )
    .await?;
    assert_eq!(created.payload, vec![1, 2, 3, 4]);
    assert_eq!(created.thumbnail, Some(vec![8, 9]));

    let updated = BlobAsset::update_by_id(
        &database,
        &created.id,
        UpdateBlobAssetInput {
            label: None,
            payload: Some(vec![5, 6, 7]),
            thumbnail: Some(None),
        },
    )
    .await?
    .expect("blob asset updated");
    assert_eq!(updated.payload, vec![5, 6, 7]);
    assert_eq!(updated.thumbnail, None);

    Ok(())
}

#[tokio::test]
async fn repo_inserts_use_server_managed_defaults_when_graphql_inputs_omit_fields()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    apply_schema(&database, &[<ServerManagedLog as Entity>::metadata()]).await?;

    let schema = schema_builder(database.clone()).finish();
    let sdl = schema.sdl();
    let create_input = sdl_input_block(&sdl, "CreateServerManagedLogInput")
        .expect("CreateServerManagedLogInput block");
    assert!(!create_input.contains("createdAt:"));

    let created = ServerManagedLog::insert(
        &pool,
        CreateServerManagedLogInput {
            message: "hello".to_string(),
        },
    )
    .await?;
    assert_eq!(created.message, "hello");
    assert!(!created.created_at.is_empty());

    Ok(())
}

#[tokio::test]
async fn relation_metadata_can_disable_physical_foreign_key_emission()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    use graphql_orm::graphql::orm::{Entity, SchemaModel, introspect_schema};

    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let target = SchemaModel::from_entities(&[
        <RelationParent as Entity>::metadata(),
        <RelationChildNoPhysicalFk as Entity>::metadata(),
    ]);

    let child_metadata = <RelationChildNoPhysicalFk as Entity>::metadata();
    assert_eq!(child_metadata.relations.len(), 1);
    assert!(!child_metadata.relations[0].emit_foreign_key);
    let child_table = target
        .tables
        .iter()
        .find(|table| table.table_name == "relation_children_no_fk")
        .expect("child table metadata");
    assert!(child_table.foreign_keys.is_empty());

    apply_schema(
        &database,
        &[
            <RelationParent as Entity>::metadata(),
            <RelationChildNoPhysicalFk as Entity>::metadata(),
        ],
    )
    .await?;
    let introspected = introspect_schema(database.pool()).await?;
    let introspected_child = introspected
        .tables
        .iter()
        .find(|table| table.table_name == "relation_children_no_fk")
        .expect("introspected child table");
    assert!(introspected_child.foreign_keys.is_empty());

    Ok(())
}

#[tokio::test]
async fn relation_metadata_emits_physical_foreign_key_by_default()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    use graphql_orm::graphql::orm::{Entity, SchemaModel, introspect_schema};

    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let target = SchemaModel::from_entities(&[
        <RelationParent as Entity>::metadata(),
        <RelationChildWithPhysicalFk as Entity>::metadata(),
    ]);

    let child_metadata = <RelationChildWithPhysicalFk as Entity>::metadata();
    assert_eq!(child_metadata.relations.len(), 1);
    assert!(child_metadata.relations[0].emit_foreign_key);
    let child_table = target
        .tables
        .iter()
        .find(|table| table.table_name == "relation_children_with_fk")
        .expect("child table metadata");
    assert_eq!(child_table.foreign_keys.len(), 1);

    apply_schema(
        &database,
        &[
            <RelationParent as Entity>::metadata(),
            <RelationChildWithPhysicalFk as Entity>::metadata(),
        ],
    )
    .await?;
    let introspected = introspect_schema(database.pool()).await?;
    let introspected_child = introspected
        .tables
        .iter()
        .find(|table| table.table_name == "relation_children_with_fk")
        .expect("introspected child table");
    assert_eq!(introspected_child.foreign_keys.len(), 1);

    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_sql_expression_defaults_survive_schema_stage_generation()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    use graphql_orm::graphql::orm::{Entity, introspect_schema};

    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    apply_schema(&database, &[<SqlDefaultExample as Entity>::metadata()]).await?;

    let introspected = introspect_schema(database.pool()).await?;
    let table = introspected
        .tables
        .iter()
        .find(|table| table.table_name == "sql_default_examples")
        .expect("sql default example table");

    let created_at = table
        .columns
        .iter()
        .find(|column| column.name == "created_at")
        .and_then(|column| column.default.as_deref())
        .expect("created_at default");
    assert!(created_at.contains("CURRENT_TIMESTAMP"));

    let token = table
        .columns
        .iter()
        .find(|column| column.name == "token")
        .and_then(|column| column.default.as_deref())
        .expect("token default");
    assert!(token.contains("randomblob(16)"));
    assert!(token.contains("lower"));

    Ok(())
}
