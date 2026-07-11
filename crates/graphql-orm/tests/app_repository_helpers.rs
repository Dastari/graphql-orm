use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql_entity(
    table = "app_helper_users",
    plural = "Users",
    default_sort = "principal ASC"
)]
pub struct User {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub principal: String,

    #[graphql_orm(private)]
    pub password_hash: String,

    #[filterable(type = "boolean")]
    pub disabled: bool,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [User],
}

#[derive(Clone, Default)]
struct RecordingHook {
    events: Arc<Mutex<Vec<graphql_orm::graphql::orm::MutationEvent>>>,
}

impl RecordingHook {
    fn snapshot(&self) -> Vec<graphql_orm::graphql::orm::MutationEvent> {
        self.events.lock().expect("hook lock poisoned").clone()
    }
}

impl graphql_orm::graphql::orm::MutationHook for RecordingHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _hook_ctx: &'a mut graphql_orm::graphql::orm::MutationContext<'_>,
        event: &'a graphql_orm::graphql::orm::MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            self.events
                .lock()
                .expect("hook lock poisoned")
                .push(event.clone());
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
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE app_helper_users (
            id TEXT PRIMARY KEY,
            principal TEXT NOT NULL,
            password_hash TEXT NOT NULL,
            disabled INTEGER NOT NULL,
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
    let database_url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| {
        "postgres://graphql_orm:graphql_orm@127.0.0.1:55433/graphql_orm_test".to_string()
    });
    let pool = sqlx::PgPool::connect(&database_url).await?;
    sqlx::query("DROP TABLE IF EXISTS app_helper_users CASCADE")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE app_helper_users (
            id UUID PRIMARY KEY,
            principal TEXT NOT NULL,
            password_hash TEXT NOT NULL,
            disabled BOOLEAN NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

#[cfg(feature = "sqlite")]
async fn insert_user_row(
    pool: &TestPool,
    principal: &str,
    disabled: bool,
    created_at: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO app_helper_users (
            id, principal, password_hash, disabled, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(graphql_orm::uuid::Uuid::new_v4().to_string())
    .bind(principal)
    .bind(format!("hash-{principal}"))
    .bind(disabled)
    .bind(created_at)
    .bind(created_at)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_user_row(
    pool: &TestPool,
    principal: &str,
    disabled: bool,
    created_at: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO app_helper_users (
            id, principal, password_hash, disabled, created_at, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(graphql_orm::uuid::Uuid::new_v4())
    .bind(principal)
    .bind(format!("hash-{principal}"))
    .bind(disabled)
    .bind(created_at)
    .bind(created_at)
    .execute(pool)
    .await?;
    Ok(())
}

#[tokio::test]
async fn app_side_helpers_update_delete_and_emit_side_effects()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let hook = RecordingHook::default();
    let db = graphql_orm::db::Database::with_mutation_hook(pool.clone(), hook.clone());
    let mut rx = db.ensure_event_sender::<UserChangedEvent>().subscribe();

    let alpha = User::insert(
        db.pool(),
        CreateUserInput {
            principal: "alpha".to_string(),
            password_hash: "hash-1".to_string(),
            disabled: false,
        },
    )
    .await?;

    let beta = User::insert(
        db.pool(),
        CreateUserInput {
            principal: "beta".to_string(),
            password_hash: "hash-2".to_string(),
            disabled: false,
        },
    )
    .await?;

    let updated = User::update_by_id(
        &db,
        &alpha.id,
        UpdateUserInput {
            password_hash: Some("hash-1b".to_string()),
            disabled: Some(true),
            ..Default::default()
        },
    )
    .await?
    .expect("alpha should update");
    assert_eq!(updated.password_hash, "hash-1b");
    assert!(updated.disabled);

    let event = rx.recv().await?;
    assert_eq!(
        event.action,
        graphql_orm::graphql::orm::ChangeAction::Updated
    );
    assert_eq!(event.id, alpha.id);
    assert_eq!(
        event.entity.expect("updated entity missing").password_hash,
        "hash-1b"
    );

    let affected = User::update_where(
        &db,
        UserWhereInput {
            disabled: Some(BoolFilter {
                eq: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        },
        UpdateUserInput {
            disabled: Some(true),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(affected, 1);

    let bulk_update_event = rx.recv().await?;
    assert_eq!(
        bulk_update_event.action,
        graphql_orm::graphql::orm::ChangeAction::Updated
    );
    assert_eq!(bulk_update_event.id, beta.id);

    let deleted_alpha = User::delete_by_id(&db, &alpha.id).await?;
    assert!(deleted_alpha);

    let delete_event = rx.recv().await?;
    assert_eq!(
        delete_event.action,
        graphql_orm::graphql::orm::ChangeAction::Deleted
    );
    assert_eq!(delete_event.id, alpha.id);

    let deleted_count = User::delete_where(
        &db,
        UserWhereInput {
            principal: Some(StringFilter {
                eq: Some("beta".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(deleted_count, 1);

    let bulk_delete_event = rx.recv().await?;
    assert_eq!(
        bulk_delete_event.action,
        graphql_orm::graphql::orm::ChangeAction::Deleted
    );
    assert_eq!(bulk_delete_event.id, beta.id);

    assert!(User::get(db.pool(), &alpha.id).await?.is_none());
    assert!(User::get(db.pool(), &beta.id).await?.is_none());

    let events = hook.snapshot();
    assert_eq!(events.len(), 8);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.action == graphql_orm::graphql::orm::ChangeAction::Updated)
            .count(),
        4
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.action == graphql_orm::graphql::orm::ChangeAction::Deleted)
            .count(),
        4
    );

    Ok(())
}

#[tokio::test]
async fn single_key_bulk_mutations_require_and_enforce_explicit_bounds()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let db = graphql_orm::db::Database::new(pool.clone());
    insert_user_row(&pool, "bounded-a", false, 1).await?;
    insert_user_row(&pool, "bounded-b", false, 2).await?;
    let filter = || UserWhereInput {
        disabled: Some(BoolFilter {
            eq: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    };

    let overflow = User::update_where_bounded(
        &db,
        filter(),
        UpdateUserInput {
            disabled: Some(true),
            ..Default::default()
        },
        MutationLimit::new(1)?,
    )
    .await?;
    assert_eq!(
        overflow,
        BoundedMutationOutcome::LimitExceeded { maximum: 1 }
    );
    assert_eq!(User::count_query(&pool).filter(&filter()).count().await?, 2);

    let applied = User::update_where_bounded(
        &db,
        filter(),
        UpdateUserInput {
            disabled: Some(true),
            ..Default::default()
        },
        MutationLimit::new(2)?,
    )
    .await?;
    assert_eq!(applied, BoundedMutationOutcome::Applied { affected: 2 });

    let deleted = User::delete_where_bounded(
        &db,
        UserWhereInput {
            disabled: Some(BoolFilter {
                eq: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        },
        MutationLimit::new(2)?,
    )
    .await?;
    assert_eq!(deleted, BoundedMutationOutcome::Applied { affected: 2 });
    Ok(())
}

#[tokio::test]
async fn filtered_repository_aggregates_run_in_sql() -> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    insert_user_row(&pool, "alpha", false, 10).await?;
    insert_user_row(&pool, "beta", false, 30).await?;
    insert_user_row(&pool, "disabled", true, 50).await?;

    let active_filter = UserWhereInput {
        disabled: Some(BoolFilter {
            eq: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    };

    assert_eq!(
        User::query(&pool)
            .filter(active_filter.clone())
            .max_i64("created_at")
            .await?,
        Some(30)
    );
    assert_eq!(
        User::query(&pool)
            .filter(active_filter.clone())
            .min_i64("created_at")
            .await?,
        Some(10)
    );
    assert_eq!(
        User::query(&pool)
            .filter(active_filter)
            .count_column("created_at")
            .await?,
        2
    );
    assert!(User::query(&pool).max_i64("missing_column").await.is_err());

    Ok(())
}
