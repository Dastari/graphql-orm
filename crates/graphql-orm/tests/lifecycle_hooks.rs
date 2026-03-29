use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "lifecycle_records",
    plural = "LifecycleRecords",
    default_sort = "title ASC"
)]
struct LifecycleRecord {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub title: String,

    #[filterable(type = "boolean")]
    pub archived: bool,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [LifecycleRecord],
}

#[derive(Clone, Debug, PartialEq)]
struct ObservedLifecycle {
    phase: graphql_orm::graphql::orm::MutationPhase,
    action: graphql_orm::graphql::orm::ChangeAction,
    entity_name: &'static str,
    actor: Option<String>,
    before_title: Option<String>,
    after_title: Option<String>,
}

#[derive(Clone, Default)]
struct RecordingLifecycleHook {
    events: Arc<Mutex<Vec<ObservedLifecycle>>>,
    fail_after_update: bool,
}

impl RecordingLifecycleHook {
    fn snapshot(&self) -> Vec<ObservedLifecycle> {
        self.events.lock().expect("hook lock poisoned").clone()
    }
}

impl graphql_orm::graphql::orm::MutationHook for RecordingLifecycleHook {
    fn on_mutation<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut graphql_orm::graphql::orm::MutationContext<'_>,
        event: &'a graphql_orm::graphql::orm::MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            let before = event.before::<LifecycleRecord>()?;
            let after = event.after::<LifecycleRecord>()?;
            self.events
                .lock()
                .expect("hook lock poisoned")
                .push(ObservedLifecycle {
                    phase: event.phase.clone(),
                    action: event.action.clone(),
                    entity_name: event.entity_name,
                    actor: hook_ctx.actor::<String>(ctx),
                    before_title: before.map(|record| record.title.clone()),
                    after_title: after.map(|record| record.title.clone()),
                });

            if self.fail_after_update
                && event.phase == graphql_orm::graphql::orm::MutationPhase::After
                && event.action == graphql_orm::graphql::orm::ChangeAction::Updated
            {
                return Err(async_graphql::Error::new("forced lifecycle failure"));
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
        "CREATE TABLE lifecycle_records (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            archived INTEGER NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
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
    sqlx::query("DROP TABLE IF EXISTS lifecycle_records")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE lifecycle_records (
            id UUID PRIMARY KEY,
            title TEXT NOT NULL,
            archived BOOLEAN NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

#[tokio::test]
async fn lifecycle_hooks_expose_before_after_and_roll_back_on_failure()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let hook = RecordingLifecycleHook::default();
    let db = graphql_orm::db::Database::with_mutation_hook(pool.clone(), hook.clone());

    let created = LifecycleRecord::insert(
        &db,
        CreateLifecycleRecordInput {
            title: "alpha".to_string(),
            archived: false,
        },
    )
    .await?;

    let updated = LifecycleRecord::update_by_id(
        &db,
        &created.id,
        UpdateLifecycleRecordInput {
            title: Some("beta".to_string()),
            ..Default::default()
        },
    )
    .await?
    .expect("record should update");
    assert_eq!(updated.title, "beta");

    let deleted = LifecycleRecord::delete_by_id(&db, &created.id).await?;
    assert!(deleted);

    let schema = schema_builder(db.clone())
        .data("test-user".to_string())
        .finish();
    let created_graphql = schema
        .execute(
            "mutation {
                createLifecycleRecord(input: { title: \"gamma\", archived: false }) {
                    success
                    lifecycleRecord { id title }
                }
            }",
        )
        .await;
    assert!(
        created_graphql.errors.is_empty(),
        "{:?}",
        created_graphql.errors
    );
    let created_graphql_json = created_graphql.data.into_json()?;
    let graphql_id = created_graphql_json["createLifecycleRecord"]["lifecycleRecord"]["id"]
        .as_str()
        .expect("graphql id missing")
        .to_string();

    let updated_graphql = schema
        .execute(format!(
            "mutation {{
                updateLifecycleRecord(id: \"{graphql_id}\", input: {{ title: \"delta\" }}) {{
                    success
                    lifecycleRecord {{ id title }}
                }}
            }}"
        ))
        .await;
    assert!(
        updated_graphql.errors.is_empty(),
        "{:?}",
        updated_graphql.errors
    );

    let deleted_graphql = schema
        .execute(format!(
            "mutation {{
                deleteLifecycleRecord(id: \"{graphql_id}\") {{
                    success
                }}
            }}"
        ))
        .await;
    assert!(
        deleted_graphql.errors.is_empty(),
        "{:?}",
        deleted_graphql.errors
    );

    let events = hook.snapshot();
    assert_eq!(events.len(), 12);

    assert_eq!(
        events[0].phase,
        graphql_orm::graphql::orm::MutationPhase::Before
    );
    assert_eq!(
        events[0].action,
        graphql_orm::graphql::orm::ChangeAction::Created
    );
    assert_eq!(events[0].before_title, None);
    assert_eq!(events[0].after_title, None);
    assert_eq!(events[0].actor, None);

    assert_eq!(
        events[1].phase,
        graphql_orm::graphql::orm::MutationPhase::After
    );
    assert_eq!(
        events[1].action,
        graphql_orm::graphql::orm::ChangeAction::Created
    );
    assert_eq!(events[1].after_title.as_deref(), Some("alpha"));

    assert_eq!(
        events[2].phase,
        graphql_orm::graphql::orm::MutationPhase::Before
    );
    assert_eq!(
        events[2].action,
        graphql_orm::graphql::orm::ChangeAction::Updated
    );
    assert_eq!(events[2].before_title.as_deref(), Some("alpha"));
    assert_eq!(events[2].after_title, None);

    assert_eq!(
        events[3].phase,
        graphql_orm::graphql::orm::MutationPhase::After
    );
    assert_eq!(
        events[3].action,
        graphql_orm::graphql::orm::ChangeAction::Updated
    );
    assert_eq!(events[3].before_title.as_deref(), Some("alpha"));
    assert_eq!(events[3].after_title.as_deref(), Some("beta"));

    assert_eq!(
        events[4].phase,
        graphql_orm::graphql::orm::MutationPhase::Before
    );
    assert_eq!(
        events[4].action,
        graphql_orm::graphql::orm::ChangeAction::Deleted
    );
    assert_eq!(events[4].before_title.as_deref(), Some("beta"));

    assert_eq!(
        events[5].phase,
        graphql_orm::graphql::orm::MutationPhase::After
    );
    assert_eq!(
        events[5].action,
        graphql_orm::graphql::orm::ChangeAction::Deleted
    );
    assert_eq!(events[5].before_title.as_deref(), Some("beta"));
    assert_eq!(events[5].after_title, None);

    assert_eq!(events[6].actor.as_deref(), Some("test-user"));
    assert_eq!(events[9].after_title.as_deref(), Some("delta"));
    assert_eq!(events[9].actor.as_deref(), Some("test-user"));
    assert_eq!(events[11].before_title.as_deref(), Some("delta"));
    assert_eq!(events[11].actor.as_deref(), Some("test-user"));

    let failing_hook = RecordingLifecycleHook {
        fail_after_update: true,
        ..Default::default()
    };
    let failing_db = graphql_orm::db::Database::with_mutation_hook(pool.clone(), failing_hook);
    let failing = LifecycleRecord::insert(
        &failing_db,
        CreateLifecycleRecordInput {
            title: "rollback-target".to_string(),
            archived: false,
        },
    )
    .await?;

    let failed_update = LifecycleRecord::update_by_id(
        &failing_db,
        &failing.id,
        UpdateLifecycleRecordInput {
            title: Some("should-not-stick".to_string()),
            ..Default::default()
        },
    )
    .await;
    assert!(failed_update.is_err());

    let after_failure = LifecycleRecord::get(&pool, &failing.id)
        .await?
        .expect("record should still exist");
    assert_eq!(after_failure.title, "rollback-target");

    Ok(())
}
