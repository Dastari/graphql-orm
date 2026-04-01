use graphql_orm::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(table = "records", plural = "Records", default_sort = "title ASC")]
struct Record {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub title: String,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "record_versions",
    plural = "RecordVersions",
    default_sort = "title_snapshot ASC"
)]
struct RecordVersion {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "uuid")]
    pub record_id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    pub title_snapshot: String,

    #[filterable(type = "string")]
    pub source_action: String,

    #[sortable]
    pub created_at: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [Record, RecordVersion],
}

#[derive(Clone, Default)]
struct RecordVersionHook {
    fail_after_insert: Arc<AtomicBool>,
}

impl graphql_orm::graphql::orm::MutationHook for RecordVersionHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut graphql_orm::graphql::orm::MutationContext<'_>,
        event: &'a graphql_orm::graphql::orm::MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.phase != graphql_orm::graphql::orm::MutationPhase::After
                || event.action != graphql_orm::graphql::orm::ChangeAction::Updated
                || event.entity_name != "Record"
            {
                return Ok(());
            }

            let after = event
                .after::<Record>()?
                .ok_or_else(|| async_graphql::Error::new("missing updated record state"))?;

            hook_ctx
                .insert::<RecordVersion>(CreateRecordVersionInput {
                    record_id: after.id,
                    title_snapshot: after.title.clone(),
                    source_action: "updated".to_string(),
                })
                .await
                .map_err(|error| async_graphql::Error::new(error.to_string()))?;

            if self.fail_after_insert.load(Ordering::SeqCst) {
                return Err(async_graphql::Error::new("forced hook failure"));
            }

            Ok(())
        })
    }
}

#[cfg(feature = "sqlite")]
type TestPool = sqlx::SqlitePool;
#[cfg(feature = "postgres")]
type TestPool = sqlx::PgPool;

#[cfg(feature = "sqlite")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE records (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE record_versions (
            id TEXT PRIMARY KEY,
            record_id TEXT NOT NULL,
            title_snapshot TEXT NOT NULL,
            source_action TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

#[cfg(feature = "postgres")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:55432/postgres".to_string());
    let pool = sqlx::PgPool::connect(&database_url).await?;
    sqlx::query("DROP TABLE IF EXISTS record_versions")
        .execute(&pool)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS records")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE records (
            id UUID PRIMARY KEY,
            title TEXT NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE record_versions (
            id UUID PRIMARY KEY,
            record_id UUID NOT NULL,
            title_snapshot TEXT NOT NULL,
            source_action TEXT NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

#[tokio::test]
async fn hook_related_writes_are_transactional_for_app_and_graphql_paths()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let hook = RecordVersionHook::default();
    let db = graphql_orm::db::Database::with_mutation_hook(pool.clone(), hook.clone());

    let created = Record::insert(
        &db,
        CreateRecordInput {
            title: "alpha".to_string(),
        },
    )
    .await?;

    let updated = Record::update_by_id(
        &db,
        &created.id,
        UpdateRecordInput {
            title: Some("beta".to_string()),
        },
    )
    .await?
    .expect("record should update");
    assert_eq!(updated.title, "beta");

    let versions = RecordVersion::query(db.pool()).fetch_all().await?;
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].record_id, created.id);
    assert_eq!(versions[0].title_snapshot, "beta");

    let created_graphql = Record::insert(
        &db,
        CreateRecordInput {
            title: "gamma".to_string(),
        },
    )
    .await?;

    let schema = schema_builder(db.clone())
        .data("test-user".to_string())
        .finish();
    let update_result = schema
        .execute(format!(
            "mutation {{
                updateRecord(id: \"{}\", input: {{ title: \"delta\" }}) {{
                    success
                    record {{ id title }}
                }}
            }}",
            created_graphql.id
        ))
        .await;
    assert!(
        update_result.errors.is_empty(),
        "{:?}",
        update_result.errors
    );

    let versions = RecordVersion::query(db.pool()).fetch_all().await?;
    assert_eq!(versions.len(), 2);
    assert!(versions.iter().any(|version| {
        version.record_id == created_graphql.id && version.title_snapshot == "delta"
    }));

    let failing_hook = RecordVersionHook {
        fail_after_insert: Arc::new(AtomicBool::new(true)),
    };
    let failing_db = graphql_orm::db::Database::with_mutation_hook(pool.clone(), failing_hook);
    let rollback_record = Record::insert(
        &failing_db,
        CreateRecordInput {
            title: "rollback".to_string(),
        },
    )
    .await?;

    let failed = Record::update_by_id(
        &failing_db,
        &rollback_record.id,
        UpdateRecordInput {
            title: Some("rollback-failed".to_string()),
        },
    )
    .await;
    assert!(failed.is_err());

    let stored = Record::get(failing_db.pool(), &rollback_record.id)
        .await?
        .expect("record should still exist");
    assert_eq!(stored.title, "rollback");

    let rollback_versions = RecordVersion::query(failing_db.pool())
        .filter(RecordVersionWhereInput {
            record_id: Some(UuidFilter {
                eq: Some(rollback_record.id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .fetch_all()
        .await?;
    assert!(rollback_versions.is_empty());

    Ok(())
}
