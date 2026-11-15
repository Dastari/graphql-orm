//! A request whose error buffer was poisoned by a panic still ends with a
//! typed error (#511).
//!
//! async-graphql keeps a request's field errors behind a standard mutex and
//! locks it again when it builds the response. A generated mutation's
//! post-commit output check cuts that buffer back when it withholds the row.
//! If a host callback panicked while holding the lock, the check must report
//! `INTERNAL_ERROR` and leave the buffer usable, or the response itself
//! panics.
#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "poison_notes",
    plural = "PoisonNotes",
    default_sort = "name ASC",
    read_policy = "note.read",
    write_policy = "note.write"
)]
struct PoisonNote {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [PoisonNote],
}

/// Allows every write. When armed, the read check of a mutation's output
/// buffers a private diagnostic, poisons the request's error buffer the way a
/// panicking holder of its lock does, and withholds the row.
#[derive(Clone, Default)]
struct PoisoningPolicy {
    armed: Arc<AtomicUsize>,
    poisoned: Arc<AtomicUsize>,
}

impl graphql_orm::graphql::orm::EntityPolicy for PoisoningPolicy {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        kind: graphql_orm::graphql::orm::EntityAccessKind,
        surface: graphql_orm::graphql::orm::EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let output_read = kind == graphql_orm::graphql::orm::EntityAccessKind::Read
                && surface == graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation;
            let Some(ctx) = ctx.filter(|_| output_read) else {
                return Ok(true);
            };
            if self.armed.load(Ordering::SeqCst) == 0 {
                return Ok(true);
            }
            ctx.add_error(async_graphql::ServerError::new("private diagnostic", None));
            let errors = &ctx.query_env.errors;
            std::thread::scope(|scope| {
                let panicked = scope
                    .spawn(|| {
                        let _held = errors.lock().unwrap();
                        panic!("a callback panics while it holds the request's error buffer");
                    })
                    .join();
                assert!(panicked.is_err());
            });
            assert!(errors.is_poisoned());
            self.poisoned.fetch_add(1, Ordering::SeqCst);
            Ok(false)
        })
    }
}

struct AllRows;
impl RowPolicy for AllRows {
    fn can_read_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn can_write_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

fn extension(error: &async_graphql::ServerError, name: &str) -> Option<String> {
    match error.extensions.as_ref()?.get(name)? {
        async_graphql::Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

#[tokio::test]
async fn a_poisoned_error_buffer_is_a_typed_response_not_a_panic()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    sqlx::query(
        "CREATE TABLE poison_notes (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
    )
    .execute(&pool)
    .await?;
    let policy = PoisoningPolicy::default();
    let mut database = graphql_orm::db::Database::with_entity_policy(pool.clone(), policy.clone());
    database.set_row_policy(AllRows);
    let schema = schema_builder(database.clone())
        .data(graphql_orm::graphql::auth::AuthSubject::new("test-user"))
        .finish();
    const CREATE: &str = "mutation($name: String!) {
        createPoisonNote(input: { name: $name }) { success poisonNote { id name } }
    }";
    let create = |name: &str| {
        async_graphql::Request::new(CREATE).variables(async_graphql::Variables::from_json(
            serde_json::json!({ "name": name }),
        ))
    };

    // Unarmed, the mutation returns its row.
    let served = schema.execute(create("first")).await;
    assert!(served.errors.is_empty(), "{:?}", served.errors);

    // Armed: the buffer is poisoned inside the output check. Executing on a
    // task turns a panic of the request into a join error.
    policy.armed.store(1, Ordering::SeqCst);
    let request = create("second");
    let response = tokio::spawn({
        let schema = schema.clone();
        async move { schema.execute(request).await }
    })
    .await
    .expect("the request must not panic");
    assert_eq!(policy.poisoned.load(Ordering::SeqCst), 1);
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    let error = &response.errors[0];
    assert_eq!(extension(error, "code").as_deref(), Some("INTERNAL_ERROR"));
    assert_eq!(error.message, "internal error");
    assert_eq!(extension(error, "outcome").as_deref(), Some("committed"));
    // The diagnostic buffered before the poison is not in the response.
    assert!(!format!("{response:?}").contains("private diagnostic"));
    // The write itself committed.
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM poison_notes ORDER BY name")
        .fetch_all(&pool)
        .await?;
    assert_eq!(names, ["first", "second"]);

    // The schema keeps serving.
    policy.armed.store(0, Ordering::SeqCst);
    let after = schema.execute(create("third")).await;
    assert!(after.errors.is_empty(), "{:?}", after.errors);
    Ok(())
}
