use graphql_orm::prelude::*;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(table = "file_rows", plural = "FileRows", default_sort = "path ASC")]
struct FileRow {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    pub path: String,

    #[sortable]
    pub created_at: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [HookUser, RefreshSession, FileRow],
}

fn test_mutex() -> &'static tokio::sync::Mutex<()> {
    static TEST_MUTEX: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    TEST_MUTEX.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[derive(Clone, Default)]
struct CleanupHook {
    fail_after_session_cleanup: Arc<AtomicBool>,
    fail_after_file_delete: Arc<AtomicBool>,
    fail_deferred_cleanup: Arc<AtomicBool>,
    deferred_runs: Arc<Mutex<Vec<String>>>,
}

impl graphql_orm::graphql::orm::MutationHook for CleanupHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut graphql_orm::graphql::orm::MutationContext<'_>,
        event: &'a graphql_orm::graphql::orm::MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            use graphql_orm::graphql::orm::{ChangeAction, MutationPhase};

            match (event.phase.clone(), event.action.clone(), event.entity_name) {
                (MutationPhase::After, ChangeAction::Updated, "HookUser") => {
                    let before = event
                        .before::<HookUser>()?
                        .ok_or_else(|| async_graphql::Error::new("missing previous user"))?;
                    let after = event
                        .after::<HookUser>()?
                        .ok_or_else(|| async_graphql::Error::new("missing updated user"))?;

                    if !before.disabled && after.disabled {
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

                        if self.fail_after_session_cleanup.load(Ordering::SeqCst) {
                            return Err(async_graphql::Error::new("forced cleanup failure"));
                        }
                    }
                }
                (MutationPhase::After, ChangeAction::Deleted, "HookUser") => {
                    let before = event
                        .before::<HookUser>()?
                        .ok_or_else(|| async_graphql::Error::new("missing deleted user"))?;

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
                (MutationPhase::After, ChangeAction::Deleted, "FileRow") => {
                    let before = event
                        .before::<FileRow>()?
                        .ok_or_else(|| async_graphql::Error::new("missing deleted file"))?;
                    let file_id = before.id.to_string();
                    let deferred_runs = self.deferred_runs.clone();
                    let fail_deferred = self.fail_deferred_cleanup.clone();

                    hook_ctx.defer(move |_db| async move {
                        deferred_runs
                            .lock()
                            .expect("deferred runs lock")
                            .push(file_id);
                        if fail_deferred.load(Ordering::SeqCst) {
                            Err("forced deferred cleanup failure".to_string())
                        } else {
                            Ok(())
                        }
                    });

                    if self.fail_after_file_delete.load(Ordering::SeqCst) {
                        return Err(async_graphql::Error::new("forced file delete failure"));
                    }
                }
                _ => {}
            }

            Ok(())
        })
    }
}

#[derive(Clone, Default)]
struct RecordingPostCommitErrors {
    errors: Arc<Mutex<Vec<String>>>,
}

impl RecordingPostCommitErrors {
    fn snapshot(&self) -> Vec<String> {
        self.errors.lock().expect("post-commit errors lock").clone()
    }
}

impl graphql_orm::graphql::orm::PostCommitErrorHandler for RecordingPostCommitErrors {
    fn on_post_commit_error<'a>(
        &'a self,
        _db: &'a graphql_orm::db::Database,
        error: &'a str,
    ) -> graphql_orm::futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            self.errors
                .lock()
                .expect("post-commit errors lock")
                .push(error.to_string());
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
    sqlx::query(
        "CREATE TABLE file_rows (
            id TEXT PRIMARY KEY,
            path TEXT NOT NULL,
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
    sqlx::query("DROP TABLE IF EXISTS file_rows")
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
    sqlx::query(
        "CREATE TABLE file_rows (
            id UUID PRIMARY KEY,
            path TEXT NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

#[tokio::test]
async fn hook_side_update_and_delete_cleanup_are_transactional()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let hook = CleanupHook::default();
    let db = graphql_orm::db::Database::with_mutation_hook(pool.clone(), hook.clone());

    let user = HookUser::insert(
        &db,
        CreateHookUserInput {
            email: "user@example.com".to_string(),
            disabled: false,
        },
    )
    .await?;
    let session_a = RefreshSession::insert(
        &db,
        CreateRefreshSessionInput {
            user_id: user.id,
            revoked: false,
        },
    )
    .await?;
    let session_b = RefreshSession::insert(
        &db,
        CreateRefreshSessionInput {
            user_id: user.id,
            revoked: false,
        },
    )
    .await?;

    let updated = HookUser::update_by_id(
        &db,
        &user.id,
        UpdateHookUserInput {
            disabled: Some(true),
            ..Default::default()
        },
    )
    .await?
    .expect("user should update");
    assert!(updated.disabled);

    let sessions = RefreshSession::query(db.pool())
        .filter(RefreshSessionWhereInput {
            user_id: Some(UuidFilter {
                eq: Some(user.id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .fetch_all()
        .await?;
    assert_eq!(sessions.len(), 2);
    assert!(sessions.iter().all(|session| session.revoked));

    let deleted = HookUser::delete_by_id(&db, &user.id).await?;
    assert!(deleted);
    let remaining = RefreshSession::query(db.pool())
        .filter(RefreshSessionWhereInput {
            id: Some(UuidFilter {
                in_list: Some(vec![session_a.id, session_b.id]),
                ..Default::default()
            }),
            ..Default::default()
        })
        .fetch_all()
        .await?;
    assert!(remaining.is_empty());

    let failing_hook = CleanupHook {
        fail_after_session_cleanup: Arc::new(AtomicBool::new(true)),
        ..Default::default()
    };
    let failing_db = graphql_orm::db::Database::with_mutation_hook(pool.clone(), failing_hook);
    let rollback_user = HookUser::insert(
        &failing_db,
        CreateHookUserInput {
            email: "rollback@example.com".to_string(),
            disabled: false,
        },
    )
    .await?;
    let rollback_session = RefreshSession::insert(
        &failing_db,
        CreateRefreshSessionInput {
            user_id: rollback_user.id,
            revoked: false,
        },
    )
    .await?;

    let failed = HookUser::update_by_id(
        &failing_db,
        &rollback_user.id,
        UpdateHookUserInput {
            disabled: Some(true),
            ..Default::default()
        },
    )
    .await;
    assert!(failed.is_err());

    let stored_user = HookUser::get(failing_db.pool(), &rollback_user.id)
        .await?
        .expect("rollback user should still exist");
    assert!(!stored_user.disabled);
    let stored_session = RefreshSession::get(failing_db.pool(), &rollback_session.id)
        .await?
        .expect("rollback session should still exist");
    assert!(!stored_session.revoked);

    Ok(())
}

#[tokio::test]
async fn deferred_post_commit_actions_run_only_after_commit_and_report_failures()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let hook = CleanupHook::default();
    let errors = RecordingPostCommitErrors::default();
    let mut db = graphql_orm::db::Database::with_mutation_hook(pool.clone(), hook.clone());
    db.set_post_commit_error_handler(errors.clone());

    let file = FileRow::insert(
        &db,
        CreateFileRowInput {
            path: "/tmp/a.bin".to_string(),
        },
    )
    .await?;

    let schema = schema_builder(db.clone())
        .data("test-user".to_string())
        .finish();
    let deleted = schema
        .execute(format!(
            "mutation {{
                deleteFileRow(id: \"{}\") {{
                    success
                }}
            }}",
            file.id
        ))
        .await;
    assert!(deleted.errors.is_empty(), "{:?}", deleted.errors);
    assert!(FileRow::get(db.pool(), &file.id).await?.is_none());
    assert_eq!(
        hook.deferred_runs.lock().expect("deferred runs lock").len(),
        1
    );

    let mut failing_db = graphql_orm::db::Database::with_mutation_hook(
        pool.clone(),
        CleanupHook {
            fail_after_file_delete: Arc::new(AtomicBool::new(true)),
            ..Default::default()
        },
    );
    failing_db.set_post_commit_error_handler(errors.clone());
    let rollback_file = FileRow::insert(
        &failing_db,
        CreateFileRowInput {
            path: "/tmp/b.bin".to_string(),
        },
    )
    .await?;
    let failed_delete = FileRow::delete_by_id(&failing_db, &rollback_file.id).await;
    assert!(failed_delete.is_err());
    assert!(
        FileRow::get(failing_db.pool(), &rollback_file.id)
            .await?
            .is_some()
    );
    assert_eq!(
        hook.deferred_runs.lock().expect("deferred runs lock").len(),
        1
    );

    let mut deferred_fail_db = graphql_orm::db::Database::with_mutation_hook(
        pool.clone(),
        CleanupHook {
            fail_deferred_cleanup: Arc::new(AtomicBool::new(true)),
            ..Default::default()
        },
    );
    deferred_fail_db.set_post_commit_error_handler(errors.clone());
    let deferred_fail_file = FileRow::insert(
        &deferred_fail_db,
        CreateFileRowInput {
            path: "/tmp/c.bin".to_string(),
        },
    )
    .await?;
    let deleted = FileRow::delete_by_id(&deferred_fail_db, &deferred_fail_file.id).await?;
    assert!(deleted);
    assert!(
        FileRow::get(deferred_fail_db.pool(), &deferred_fail_file.id)
            .await?
            .is_none()
    );

    let recorded_errors = errors.snapshot();
    assert!(
        recorded_errors
            .iter()
            .any(|error| error.contains("forced deferred cleanup failure"))
    );

    Ok(())
}
