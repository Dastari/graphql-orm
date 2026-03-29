use graphql_orm::prelude::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "collections",
    plural = "Collections",
    default_sort = "name ASC",
    read_policy = "collection.read",
    write_policy = "collection.write"
)]
struct Collection {
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
    entities: [Collection],
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EntityPolicyCall {
    entity_name: String,
    policy_key: Option<String>,
    kind: graphql_orm::graphql::orm::EntityAccessKind,
    surface: graphql_orm::graphql::orm::EntityAccessSurface,
}

#[derive(Clone, Default)]
struct RecordingEntityPolicy {
    allowed_reads: Arc<Mutex<HashSet<String>>>,
    allowed_writes: Arc<Mutex<HashSet<String>>>,
    calls: Arc<Mutex<Vec<EntityPolicyCall>>>,
}

impl RecordingEntityPolicy {
    fn allow_read(&self, key: &str) {
        self.allowed_reads
            .lock()
            .expect("allowed reads lock")
            .insert(key.to_string());
    }

    fn allow_write(&self, key: &str) {
        self.allowed_writes
            .lock()
            .expect("allowed writes lock")
            .insert(key.to_string());
    }

    fn calls(&self) -> Vec<EntityPolicyCall> {
        self.calls.lock().expect("calls lock").clone()
    }
}

impl graphql_orm::graphql::orm::EntityPolicy for RecordingEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        kind: graphql_orm::graphql::orm::EntityAccessKind,
        surface: graphql_orm::graphql::orm::EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("calls lock")
                .push(EntityPolicyCall {
                    entity_name: entity_name.to_string(),
                    policy_key: policy_key.map(str::to_string),
                    kind,
                    surface,
                });

            let allowed = match kind {
                graphql_orm::graphql::orm::EntityAccessKind::Read => policy_key.is_none_or(|key| {
                    self.allowed_reads
                        .lock()
                        .expect("allowed reads lock")
                        .contains(key)
                }),
                graphql_orm::graphql::orm::EntityAccessKind::Write => {
                    policy_key.is_none_or(|key| {
                        self.allowed_writes
                            .lock()
                            .expect("allowed writes lock")
                            .contains(key)
                    })
                }
            };

            Ok(allowed)
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
    sqlx::query(
        "CREATE TABLE collections (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
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
    sqlx::query("DROP TABLE IF EXISTS collections")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE collections (
            id UUID PRIMARY KEY,
            name TEXT NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

#[tokio::test]
async fn host_declared_entity_policy_gates_generated_surfaces()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let policy = RecordingEntityPolicy::default();
    let database = graphql_orm::db::Database::with_entity_policy(pool.clone(), policy.clone());
    let schema = schema_builder(database.clone())
        .data("test-user".to_string())
        .finish();

    let denied_graphql_create = schema
        .execute(
            "mutation {
                createCollection(input: { name: \"Docs\" }) {
                    success
                    error
                }
            }",
        )
        .await;
    assert!(!denied_graphql_create.errors.is_empty());

    let denied_repo_insert = Collection::insert(
        &database,
        CreateCollectionInput {
            name: "Repo Docs".to_string(),
        },
    )
    .await;
    assert!(denied_repo_insert.is_err());

    policy.allow_write("collection.write");

    let created = Collection::insert(
        &database,
        CreateCollectionInput {
            name: "Repo Docs".to_string(),
        },
    )
    .await?;

    let denied_read = schema
        .execute(format!(
            "query {{
                collection(id: \"{}\") {{
                    id
                    name
                }}
            }}",
            created.id
        ))
        .await;
    assert!(!denied_read.errors.is_empty());

    policy.allow_read("collection.read");

    let allowed_read = schema
        .execute(format!(
            "query {{
                collection(id: \"{}\") {{
                    id
                    name
                }}
            }}",
            created.id
        ))
        .await;
    assert!(allowed_read.errors.is_empty(), "{:?}", allowed_read.errors);
    let allowed_json = allowed_read.data.into_json()?;
    assert_eq!(allowed_json["collection"]["name"], "Repo Docs");

    let calls = policy.calls();
    assert!(calls.iter().any(|call| {
        call.entity_name == "Collection"
            && call.policy_key.as_deref() == Some("collection.write")
            && call.kind == graphql_orm::graphql::orm::EntityAccessKind::Write
            && call.surface == graphql_orm::graphql::orm::EntityAccessSurface::GraphqlMutation
    }));
    assert!(calls.iter().any(|call| {
        call.entity_name == "Collection"
            && call.policy_key.as_deref() == Some("collection.write")
            && call.kind == graphql_orm::graphql::orm::EntityAccessKind::Write
            && call.surface == graphql_orm::graphql::orm::EntityAccessSurface::Repository
    }));
    assert!(calls.iter().any(|call| {
        call.entity_name == "Collection"
            && call.policy_key.as_deref() == Some("collection.read")
            && call.kind == graphql_orm::graphql::orm::EntityAccessKind::Read
            && call.surface == graphql_orm::graphql::orm::EntityAccessSurface::GraphqlQuery
    }));

    Ok(())
}
