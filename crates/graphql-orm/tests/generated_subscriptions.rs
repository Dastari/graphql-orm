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
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE subscription_records (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
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
