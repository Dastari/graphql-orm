use graphql_orm::prelude::*;
use std::sync::OnceLock;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Default)]
struct Identity {
    subject: String,
    namespace: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Default)]
struct Content {
    title: String,
    body: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Default)]
struct Tag {
    label: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Default)]
struct RecordMetadata {
    revision: i32,
    published: bool,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(table = "records", plural = "Records", default_sort = "created_at ASC")]
struct Record {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub slug: String,

    #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
    pub identity: Identity,

    #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
    pub content: Content,

    #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
    pub tags: Vec<Tag>,

    #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
    pub metadata: Option<RecordMetadata>,

    #[filterable(type = "number")]
    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [Record],
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
    sqlx::query("DROP TABLE IF EXISTS records")
        .execute(&pool)
        .await?;
    Ok(pool)
}

#[cfg(feature = "sqlite")]
fn expected_json_sql_type() -> &'static str {
    "TEXT"
}

#[cfg(feature = "postgres")]
fn expected_json_sql_type() -> &'static str {
    "JSONB"
}

fn test_mutex() -> &'static tokio::sync::Mutex<()> {
    static TEST_MUTEX: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    TEST_MUTEX.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[tokio::test]
async fn typed_json_fields_round_trip_through_runtime_and_migrations()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    use graphql_orm::graphql::orm::{
        DatabaseBackend, Entity, MigrationRunner, build_migration_plan, introspect_schema,
    };

    let pool = setup_pool().await?;
    let target_schema =
        graphql_orm::graphql::orm::SchemaModel::from_entities(&[<Record as Entity>::metadata()]);
    let version = format!(
        "2026032805_json_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let plan = build_migration_plan(
        if cfg!(feature = "postgres") {
            DatabaseBackend::Postgres
        } else {
            DatabaseBackend::Sqlite
        },
        &graphql_orm::graphql::orm::SchemaModel { tables: Vec::new() },
        &target_schema,
    );
    let leaked_statements: &'static [&'static str] = Box::leak(
        plan.statements
            .iter()
            .map(|statement| Box::leak(statement.clone().into_boxed_str()) as &'static str)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    let migration = graphql_orm::graphql::orm::Migration {
        version: Box::leak(version.into_boxed_str()),
        description: "json_fields_contract",
        statements: leaked_statements,
    };

    let database = graphql_orm::db::Database::new(pool.clone());
    database.apply_migrations(&[migration]).await?;

    let metadata = <Record as Entity>::metadata();
    assert_eq!(
        metadata
            .fields
            .iter()
            .find(|field| field.name == "identity")
            .expect("identity field metadata should exist")
            .sql_type,
        expected_json_sql_type()
    );

    let schema = schema_builder(database.clone()).finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("input CreateRecordInput"));
    assert!(sdl.contains("identity: JSON!"));
    assert!(sdl.contains("content: JSON!"));
    assert!(sdl.contains("tags: JSON!"));
    assert!(sdl.contains("metadata: JSON"));

    let created = Record::insert(
        &pool,
        CreateRecordInput {
            slug: "record-1".to_string(),
            identity: Identity {
                subject: "record-1".to_string(),
                namespace: "tenant-a".to_string(),
            },
            content: Content {
                title: "Original".to_string(),
                body: "First body".to_string(),
            },
            tags: vec![
                Tag {
                    label: "draft".to_string(),
                },
                Tag {
                    label: "legal".to_string(),
                },
            ],
            metadata: Some(RecordMetadata {
                revision: 1,
                published: false,
            }),
        },
    )
    .await?;

    let fetched = Record::get(&pool, &created.id)
        .await?
        .expect("record should exist");
    assert_eq!(fetched.slug, "record-1");
    assert_eq!(fetched.identity.subject, "record-1");
    assert_eq!(fetched.content.title, "Original");
    assert_eq!(fetched.tags.len(), 2);
    assert_eq!(
        fetched.metadata,
        Some(RecordMetadata {
            revision: 1,
            published: false,
        })
    );

    let queried = Record::query(&pool).fetch_all().await?;
    assert_eq!(queried.len(), 1);
    assert_eq!(queried[0].content.body, "First body");

    let updated = Record::update_by_id(
        &database,
        &created.id,
        UpdateRecordInput {
            slug: Some("record-1-updated".to_string()),
            content: Some(Content {
                title: "Updated".to_string(),
                body: "Second body".to_string(),
            }),
            tags: Some(vec![Tag {
                label: "published".to_string(),
            }]),
            metadata: Some(Some(RecordMetadata {
                revision: 2,
                published: true,
            })),
            ..Default::default()
        },
    )
    .await?
    .expect("updated record should be returned");
    assert_eq!(updated.slug, "record-1-updated");
    assert_eq!(updated.content.title, "Updated");
    assert_eq!(updated.tags[0].label, "published");
    assert_eq!(
        updated.metadata,
        Some(RecordMetadata {
            revision: 2,
            published: true,
        })
    );

    let updated_count = Record::update_where(
        &database,
        RecordWhereInput {
            id: Some(UuidFilter {
                eq: Some(created.id),
                ..Default::default()
            }),
            ..Default::default()
        },
        UpdateRecordInput {
            metadata: Some(None),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(updated_count, 1);

    let fetched_again = Record::get(&pool, &created.id)
        .await?
        .expect("record should still exist");
    assert_eq!(fetched_again.metadata, None);

    let deleted = Record::delete_by_id(&database, &created.id).await?;
    assert!(deleted);
    assert!(Record::get(&pool, &created.id).await?.is_none());

    let introspected = introspect_schema(&pool).await?;
    let records_table = introspected
        .tables
        .iter()
        .find(|table| table.table_name == "records")
        .expect("records table should exist");
    for column_name in ["identity", "content", "tags", "metadata"] {
        let column = records_table
            .columns
            .iter()
            .find(|column| column.name == column_name)
            .expect("json column should exist");
        assert_eq!(column.sql_type.to_uppercase(), expected_json_sql_type());
    }

    Ok(())
}

#[tokio::test]
async fn typed_json_fields_are_writable_through_generated_graphql_mutations()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    use async_graphql::{Request, Variables};
    use graphql_orm::graphql::orm::{DatabaseBackend, Entity, Migration, build_migration_plan};

    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let target_schema =
        graphql_orm::graphql::orm::SchemaModel::from_entities(&[<Record as Entity>::metadata()]);
    let version = format!(
        "2026040102_json_graphql_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let plan = build_migration_plan(
        if cfg!(feature = "postgres") {
            DatabaseBackend::Postgres
        } else {
            DatabaseBackend::Sqlite
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
            version: Box::leak(version.into_boxed_str()),
            description: "json_fields_graphql_write_contract",
            statements,
        }])
        .await?;

    let schema = schema_builder(database.clone())
        .data("editor-1".to_string())
        .finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("input CreateRecordInput"));
    assert!(sdl.contains("identity: JSON!"));
    assert!(sdl.contains("content: JSON!"));
    assert!(sdl.contains("tags: JSON!"));
    assert!(sdl.contains("metadata: JSON"));

    let created = schema
        .execute(
            Request::new(
                "mutation CreateRecord($input: CreateRecordInput!) {
                    createRecord(input: $input) {
                        success
                        record { id slug }
                    }
                }",
            )
            .variables(Variables::from_json(serde_json::json!({
                "input": {
                    "slug": "record-graphql",
                    "identity": { "subject": "rec-1", "namespace": "tenant-a" },
                    "content": { "title": "GraphQL Title", "body": "GraphQL Body" },
                    "tags": [{ "label": "graphql" }, { "label": "json" }],
                    "metadata": { "revision": 1, "published": false }
                }
            }))),
        )
        .await;
    assert!(created.errors.is_empty(), "{:?}", created.errors);
    let created_json = created.data.into_json()?;
    let record_id = graphql_orm::uuid::Uuid::parse_str(
        created_json["createRecord"]["record"]["id"]
            .as_str()
            .expect("record id missing"),
    )?;

    let created_record = Record::get(&pool, &record_id)
        .await?
        .expect("record should exist after GraphQL create");
    assert_eq!(created_record.identity.subject, "rec-1");
    assert_eq!(created_record.content.title, "GraphQL Title");
    assert_eq!(created_record.tags.len(), 2);
    assert_eq!(
        created_record.metadata,
        Some(RecordMetadata {
            revision: 1,
            published: false,
        })
    );

    let updated = schema
        .execute(
            Request::new(
                "mutation UpdateRecord($id: UUID!, $input: UpdateRecordInput!) {
                    updateRecord(id: $id, input: $input) {
                        success
                        record { id slug }
                    }
                }",
            )
            .variables(Variables::from_json(serde_json::json!({
                "id": record_id,
                "input": {
                    "content": { "title": "Updated Title", "body": "Updated Body" },
                    "tags": [{ "label": "updated" }],
                    "metadata": null
                }
            }))),
        )
        .await;
    assert!(updated.errors.is_empty(), "{:?}", updated.errors);

    let updated_record = Record::get(&pool, &record_id)
        .await?
        .expect("record should exist after GraphQL update");
    assert_eq!(updated_record.content.title, "Updated Title");
    assert_eq!(updated_record.tags.len(), 1);
    assert_eq!(updated_record.tags[0].label, "updated");
    assert_eq!(updated_record.metadata, None);

    Ok(())
}
