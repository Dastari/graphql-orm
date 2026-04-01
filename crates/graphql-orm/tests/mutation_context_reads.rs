use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::time::{Duration, timeout};

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(table = "hook_users", plural = "HookUsers", default_sort = "email ASC")]
struct HookUser {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub email: String,

    #[filterable(type = "boolean")]
    pub disabled: bool,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "refresh_sessions",
    plural = "RefreshSessions",
    default_sort = "created_at ASC"
)]
struct RefreshSession {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "uuid")]
    pub user_id: graphql_orm::uuid::Uuid,

    #[filterable(type = "boolean")]
    pub revoked: bool,

    #[sortable]
    pub created_at: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [HookUser, RefreshSession],
}

fn test_mutex() -> &'static tokio::sync::Mutex<()> {
    static TEST_MUTEX: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    TEST_MUTEX.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[derive(Clone, Debug, PartialEq)]
struct ReadObservation {
    stage: &'static str,
    count: i64,
    found: bool,
    disabled: Option<bool>,
}

#[derive(Clone, Default)]
struct TransactionalReadHook {
    observations: Arc<Mutex<Vec<ReadObservation>>>,
}

impl TransactionalReadHook {
    fn record(&self, observation: ReadObservation) {
        self.observations
            .lock()
            .expect("observations lock poisoned")
            .push(observation);
    }

    fn snapshot(&self) -> Vec<ReadObservation> {
        self.observations
            .lock()
            .expect("observations lock poisoned")
            .clone()
    }
}

impl graphql_orm::graphql::orm::MutationHook for TransactionalReadHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut graphql_orm::graphql::orm::MutationContext<'_>,
        event: &'a graphql_orm::graphql::orm::MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            use graphql_orm::graphql::orm::{ChangeAction, MutationPhase};

            match (event.phase.clone(), event.action, event.entity_name) {
                (MutationPhase::After, ChangeAction::Updated, "HookUser") => {
                    let before = event
                        .before::<HookUser>()?
                        .ok_or_else(|| async_graphql::Error::new("missing previous user"))?;
                    let after = event
                        .after::<HookUser>()?
                        .ok_or_else(|| async_graphql::Error::new("missing updated user"))?;

                    if !before.disabled && after.disabled {
                        let open_filter = RefreshSessionWhereInput {
                            user_id: Some(UuidFilter {
                                eq: Some(after.id),
                                ..Default::default()
                            }),
                            revoked: Some(BoolFilter {
                                eq: Some(false),
                                ..Default::default()
                            }),
                            ..Default::default()
                        };
                        let open_count = hook_ctx
                            .query::<RefreshSession>()
                            .filter(open_filter.clone())
                            .count()
                            .await
                            .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                        let first_open = hook_ctx
                            .query::<RefreshSession>()
                            .filter(open_filter)
                            .limit(1)
                            .fetch_one()
                            .await
                            .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                        let current_user = hook_ctx
                            .find_by_id::<HookUser>(&after.id)
                            .await
                            .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                        self.record(ReadObservation {
                            stage: "after_update",
                            count: open_count,
                            found: first_open.is_some(),
                            disabled: current_user.as_ref().map(|user| user.disabled),
                        });

                        hook_ctx
                            .update_where::<RefreshSession>(
                                RefreshSessionWhereInput {
                                    user_id: Some(UuidFilter {
                                        eq: Some(after.id),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                },
                                UpdateRefreshSessionInput {
                                    revoked: Some(true),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                    }
                }
                (MutationPhase::Before, ChangeAction::Deleted, "HookUser") => {
                    let before = event
                        .before::<HookUser>()?
                        .ok_or_else(|| async_graphql::Error::new("missing deleted user"))?;
                    let existing_user = hook_ctx
                        .find_by_id::<HookUser>(&before.id)
                        .await
                        .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                    let sessions_exist = hook_ctx
                        .query::<RefreshSession>()
                        .filter(RefreshSessionWhereInput {
                            user_id: Some(UuidFilter {
                                eq: Some(before.id),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .exists()
                        .await
                        .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                    self.record(ReadObservation {
                        stage: "before_delete",
                        count: if sessions_exist { 1 } else { 0 },
                        found: existing_user.is_some(),
                        disabled: existing_user.as_ref().map(|user| user.disabled),
                    });

                    hook_ctx
                        .delete_where::<RefreshSession>(RefreshSessionWhereInput {
                            user_id: Some(UuidFilter {
                                eq: Some(before.id),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .await
                        .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                }
                (MutationPhase::After, ChangeAction::Deleted, "HookUser") => {
                    let before = event
                        .before::<HookUser>()?
                        .ok_or_else(|| async_graphql::Error::new("missing deleted user"))?;
                    let remaining = hook_ctx
                        .query::<RefreshSession>()
                        .filter(RefreshSessionWhereInput {
                            user_id: Some(UuidFilter {
                                eq: Some(before.id),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .count()
                        .await
                        .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                    self.record(ReadObservation {
                        stage: "after_delete",
                        count: remaining,
                        found: false,
                        disabled: None,
                    });
                }
                _ => {}
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
        "CREATE TABLE hook_users (
            id TEXT PRIMARY KEY,
            email TEXT NOT NULL,
            disabled INTEGER NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE refresh_sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            revoked INTEGER NOT NULL,
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
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS refresh_sessions")
        .execute(&pool)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS hook_users")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE hook_users (
            id UUID PRIMARY KEY,
            email TEXT NOT NULL,
            disabled BOOLEAN NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query(
        "CREATE TABLE refresh_sessions (
            id UUID PRIMARY KEY,
            user_id UUID NOT NULL,
            revoked BOOLEAN NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

#[tokio::test]
async fn mutation_hooks_can_read_related_rows_on_the_active_transaction()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let hook = TransactionalReadHook::default();
    let db = graphql_orm::db::Database::with_mutation_hook(pool.clone(), hook.clone());
    let schema = schema_builder(db.clone())
        .data("test-user".to_string())
        .finish();

    let user = HookUser::insert(
        &db,
        CreateHookUserInput {
            email: "user@example.com".to_string(),
            disabled: false,
        },
    )
    .await?;
    RefreshSession::insert(
        &db,
        CreateRefreshSessionInput {
            user_id: user.id,
            revoked: false,
        },
    )
    .await?;
    RefreshSession::insert(
        &db,
        CreateRefreshSessionInput {
            user_id: user.id,
            revoked: false,
        },
    )
    .await?;

    let updated = timeout(
        Duration::from_secs(2),
        HookUser::update_by_id(
            &db,
            &user.id,
            UpdateHookUserInput {
                disabled: Some(true),
                ..Default::default()
            },
        ),
    )
    .await??;
    assert!(updated.expect("user should update").disabled);

    let revoked_sessions = RefreshSession::query(db.pool())
        .filter(RefreshSessionWhereInput {
            user_id: Some(UuidFilter {
                eq: Some(user.id),
                ..Default::default()
            }),
            revoked: Some(BoolFilter {
                eq: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        })
        .count()
        .await?;
    assert_eq!(revoked_sessions, 2);

    let deleted = timeout(
        Duration::from_secs(2),
        schema.execute(format!(
            "mutation {{
                deleteHookUser(id: \"{}\") {{
                    success
                }}
            }}",
            user.id
        )),
    )
    .await?;
    assert!(deleted.errors.is_empty(), "{:?}", deleted.errors);
    let deleted_json = deleted.data.into_json()?;
    assert_eq!(
        deleted_json["deleteHookUser"]["success"].as_bool(),
        Some(true)
    );

    let remaining_sessions = RefreshSession::query(db.pool())
        .filter(RefreshSessionWhereInput {
            user_id: Some(UuidFilter {
                eq: Some(user.id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .count()
        .await?;
    assert_eq!(remaining_sessions, 0);

    let observations = hook.snapshot();
    assert!(observations.iter().any(|observation| {
        observation.stage == "after_update"
            && observation.count == 2
            && observation.found
            && observation.disabled == Some(true)
    }));
    assert!(observations.iter().any(|observation| {
        observation.stage == "before_delete"
            && observation.count == 1
            && observation.found
            && observation.disabled == Some(true)
    }));
    assert!(observations.iter().any(|observation| {
        observation.stage == "after_delete" && observation.count == 0 && !observation.found
    }));

    Ok(())
}
