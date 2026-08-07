use graphql_orm::async_graphql::{Request, Schema};
use graphql_orm::futures::{Stream, StreamExt};
use graphql_orm::prelude::*;
use std::sync::OnceLock;
use std::task::Poll;
use tokio::time::{Duration, timeout};

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "subscription_records",
    plural = "SubscriptionRecords",
    default_sort = "title ASC"
)]
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

schema_roots! {
    query_custom_ops: [],
    entities: [Record],
}

type TestSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

fn test_mutex() -> &'static tokio::sync::Mutex<()> {
    static TEST_MUTEX: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    TEST_MUTEX.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[cfg(feature = "sqlite")]
type TestPool = sqlx::SqlitePool;
#[cfg(feature = "postgres")]
type TestPool = sqlx::PgPool;

#[cfg(feature = "sqlite")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    // Keep the in-memory database and its per-connection foreign-key setting
    // on one connection. The rollback test below relies on a deferred foreign
    // key that fails at commit, after generated mutation code has queued its
    // change event.
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE subscription_titles (
            title TEXT PRIMARY KEY
        )",
    )
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO subscription_titles (title) VALUES ('Alpha')")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE subscription_records (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL REFERENCES subscription_titles(title) DEFERRABLE INITIALLY DEFERRED,
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
    sqlx::query("DROP TABLE IF EXISTS subscription_records")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE subscription_records (
            id UUID PRIMARY KEY,
            title TEXT NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

#[tokio::test]
async fn generated_subscriptions_work_without_manual_sender_registration()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let pool = setup_pool().await?;
    let schema: TestSchema = schema_builder(graphql_orm::db::Database::new(pool))
        .data("test-user".to_string())
        .finish();

    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new(
                "subscription {
                recordChanged {
                    action
                    record { id title }
                }
            }",
            )
            .data("test-user".to_string()),
        ),
    );
    graphql_orm::futures::future::poll_fn(|cx| match stream.as_mut().poll_next(cx) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(Some(response)) => {
            panic!(
                "subscription yielded before mutation: {:?}",
                response.errors
            )
        }
        Poll::Ready(None) => panic!("subscription stream ended before mutation"),
    })
    .await;

    let created = schema
        .execute(
            Request::new(
                "mutation {
                    createRecord(input: { title: \"Alpha\" }) {
                        success
                        record { id title }
                    }
                }",
            )
            .data("test-user".to_string()),
        )
        .await;
    assert!(created.errors.is_empty(), "{:?}", created.errors);

    let response = timeout(Duration::from_secs(2), stream.next())
        .await?
        .expect("subscription stream ended unexpectedly");
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json()?;
    assert_eq!(json["recordChanged"]["action"].as_str(), Some("CREATED"));
    assert_eq!(
        json["recordChanged"]["record"]["title"].as_str(),
        Some("Alpha")
    );

    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn generated_subscription_events_are_emitted_only_after_commit()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let database = graphql_orm::db::Database::new(setup_pool().await?);
    let schema: TestSchema = schema_builder(database.clone())
        .data("test-user".to_string())
        .finish();

    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new(
                "subscription {
                    recordChanged {
                        action
                        record { title }
                    }
                }",
            )
            .data("test-user".to_string()),
        ),
    );
    graphql_orm::futures::future::poll_fn(|cx| match stream.as_mut().poll_next(cx) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(Some(response)) => panic!(
            "subscription yielded before mutation: {:?}",
            response.errors
        ),
        Poll::Ready(None) => panic!("subscription stream ended before mutation"),
    })
    .await;

    let committed = schema
        .execute(
            Request::new(
                "mutation {
                    createRecord(input: { title: \"Alpha\" }) {
                        success
                    }
                }",
            )
            .data("test-user".to_string()),
        )
        .await;
    assert!(committed.errors.is_empty(), "{:?}", committed.errors);
    let committed_event = timeout(Duration::from_secs(2), stream.next())
        .await?
        .expect("subscription stream ended unexpectedly");
    assert!(
        committed_event.errors.is_empty(),
        "{:?}",
        committed_event.errors
    );
    assert_eq!(
        committed_event.data.into_json()?["recordChanged"]["record"]["title"].as_str(),
        Some("Alpha")
    );

    // `Rejected` has no matching parent row. SQLite defers this foreign-key
    // check until COMMIT, so the generated mutation has already inserted and
    // read the row, run its hooks, and queued RecordChangedEvent. A failed
    // commit must discard that queued event.
    let rolled_back = schema
        .execute(
            Request::new(
                "mutation {
                    createRecord(input: { title: \"Rejected\" }) {
                        success
                        error
                    }
                }",
            )
            .data("test-user".to_string()),
        )
        .await;
    let rolled_back_json = rolled_back.data.into_json()?;
    assert!(
        !rolled_back.errors.is_empty()
            || rolled_back_json["createRecord"]["success"].as_bool() == Some(false),
        "deferred foreign-key violation unexpectedly committed: {rolled_back_json}"
    );

    let rejected_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM subscription_records WHERE title = 'Rejected'")
            .fetch_one(database.pool())
            .await?;
    assert_eq!(rejected_rows, 0, "failed generated write was committed");
    assert!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .is_err(),
        "rolled-back generated write emitted a subscription event"
    );

    Ok(())
}

#[tokio::test]
async fn generated_subscriptions_fail_explicitly_when_database_runtime_is_missing()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = test_mutex().lock().await;
    let schema: TestSchema = graphql_orm::async_graphql::Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .data("test-user".to_string())
    .finish();

    let mut stream = Box::pin(
        schema.execute_stream(
            Request::new(
                "subscription {
                recordChanged {
                    action
                }
            }",
            )
            .data("test-user".to_string()),
        ),
    );

    let response = timeout(Duration::from_secs(2), stream.next())
        .await?
        .expect("subscription stream ended unexpectedly");
    assert!(!response.errors.is_empty());
    assert!(
        response.errors[0]
            .message
            .contains("Database runtime not registered")
    );

    Ok(())
}
