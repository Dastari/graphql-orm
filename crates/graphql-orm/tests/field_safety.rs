#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

type PolicyLog = Arc<Mutex<Vec<(String, String, Option<String>)>>>;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[serde(rename_all = "camelCase")]
#[graphql_entity(table = "users", plural = "Users", default_sort = "display_name ASC")]
pub struct User {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    pub principal: String,

    #[filterable(type = "string")]
    #[sortable]
    pub display_name: String,

    #[graphql_orm(private)]
    pub password_hash: String,

    #[graphql_orm(private)]
    pub totp_secret: Option<String>,

    #[serde(rename = "emailAddress")]
    #[graphql_orm(read_policy = "user.email.read", write_policy = "user.email.write")]
    #[filterable(type = "string")]
    pub email: Option<String>,

    pub created_at: i64,
    pub updated_at: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [User],
}

#[derive(Clone, Default)]
struct RecordingPolicy {
    reads: PolicyLog,
    writes: PolicyLog,
    allowed_reads: Arc<Mutex<HashSet<String>>>,
    allowed_writes: Arc<Mutex<HashSet<String>>>,
}

impl RecordingPolicy {
    fn allow_read(&self, key: &str) {
        self.allowed_reads
            .lock()
            .expect("read policy lock")
            .insert(key.to_string());
    }

    fn allow_write(&self, key: &str) {
        self.allowed_writes
            .lock()
            .expect("write policy lock")
            .insert(key.to_string());
    }

    fn reads(&self) -> Vec<(String, String, Option<String>)> {
        self.reads.lock().expect("read log lock").clone()
    }
}

impl graphql_orm::graphql::orm::FieldPolicy for RecordingPolicy {
    fn can_read_field<'a>(
        &'a self,
        _ctx: &'a async_graphql::Context<'_>,
        _db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            self.reads.lock().expect("read log lock").push((
                entity_name.to_string(),
                field_name.to_string(),
                policy_key.map(str::to_string),
            ));
            Ok(policy_key.is_none_or(|key| {
                self.allowed_reads
                    .lock()
                    .expect("allowed reads lock")
                    .contains(key)
            }))
        })
    }

    fn can_write_field<'a>(
        &'a self,
        _ctx: &'a async_graphql::Context<'_>,
        _db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _value: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            self.writes.lock().expect("write log lock").push((
                entity_name.to_string(),
                field_name.to_string(),
                policy_key.map(str::to_string),
            ));
            Ok(policy_key.is_none_or(|key| {
                self.allowed_writes
                    .lock()
                    .expect("allowed writes lock")
                    .contains(key)
            }))
        })
    }
}

async fn setup_schema(
    policy: RecordingPolicy,
) -> Result<
    async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>,
    Box<dyn std::error::Error>,
> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
    sqlx::query(
        "CREATE TABLE users (
            id TEXT PRIMARY KEY,
            principal TEXT NOT NULL,
            display_name TEXT NOT NULL,
            password_hash TEXT NOT NULL DEFAULT '',
            totp_secret TEXT,
            email TEXT,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
    )
    .execute(&pool)
    .await?;

    let mut database = graphql_orm::db::Database::new(pool);
    database.set_field_policy(policy);
    Ok(schema_builder(database)
        .data("test-user".to_string())
        .finish())
}

#[tokio::test]
async fn private_fields_are_hidden_and_names_follow_serde() -> Result<(), Box<dyn std::error::Error>>
{
    let schema = setup_schema(RecordingPolicy::default()).await?;
    let sdl = schema.sdl();

    assert!(sdl.contains("type User"));
    assert!(sdl.contains("displayName: String!"));
    assert!(sdl.contains("emailAddress: String"));
    assert!(!sdl.contains("passwordHash"));
    assert!(!sdl.contains("totpSecret"));
    assert!(sdl.contains("input CreateUserInput"));
    assert!(sdl.contains("displayName: String!"));
    assert!(sdl.contains("emailAddress: String"));
    assert!(!sdl.contains("passwordHash:"));
    assert!(!sdl.contains("totpSecret:"));
    assert!(sdl.contains("input UserWhereInput"));
    assert!(sdl.contains("displayName: StringFilter"));
    assert!(!sdl.contains("passwordHash: StringFilter"));
    assert!(sdl.contains("input UserOrderByInput"));
    assert!(sdl.contains("displayName: OrderDirection"));
    assert!(!sdl.contains("passwordHash: OrderDirection"));

    let user = User {
        id: graphql_orm::uuid::Uuid::new_v4(),
        principal: "principal-1".to_string(),
        display_name: "Visible Name".to_string(),
        password_hash: "secret-hash".to_string(),
        totp_secret: Some("totp".to_string()),
        email: Some("hidden@example.com".to_string()),
        created_at: 0,
        updated_at: 0,
    };
    assert_eq!(user.password_hash, "secret-hash");
    assert_eq!(user.totp_secret.as_deref(), Some("totp"));

    Ok(())
}

#[tokio::test]
async fn field_policy_callbacks_gate_generated_read_and_write_paths()
-> Result<(), Box<dyn std::error::Error>> {
    let policy = RecordingPolicy::default();
    let schema = setup_schema(policy.clone()).await?;

    let denied_write = schema
        .execute(
            "mutation {
                createUser(input: {
                    principal: \"principal-1\"
                    displayName: \"Visible Name\"
                    emailAddress: \"user@example.com\"
                }) {
                    success
                    error
                }
            }",
        )
        .await;
    assert!(denied_write.errors.is_empty(), "{:?}", denied_write.errors);
    let denied_write_json = denied_write.data.into_json()?;
    assert_eq!(
        denied_write_json["createUser"]["success"].as_bool(),
        Some(true)
    );

    policy.allow_write("user.email.write");
    let created = schema
        .execute(
            "mutation {
                createUser(input: {
                    principal: \"principal-1\"
                    displayName: \"Visible Name\"
                    emailAddress: \"user@example.com\"
                }) {
                    success
                    user {
                        id
                        principal
                        displayName
                    }
                }
            }",
        )
        .await;
    assert!(created.errors.is_empty(), "{:?}", created.errors);
    let created_json = created.data.into_json()?;
    let user_id = created_json["createUser"]["user"]["id"]
        .as_str()
        .expect("created user id should be present");

    let denied_read = schema
        .execute(format!(
            "query {{
                user(id: \"{user_id}\") {{
                    principal
                    emailAddress
                }}
            }}"
        ))
        .await;
    assert!(!denied_read.errors.is_empty());

    policy.allow_read("user.email.read");
    let allowed_read = schema
        .execute(
            "query {
                users(where: { displayName: { eq: \"Visible Name\" } }, orderBy: [{ displayName: ASC }]) {
                    edges {
                        node {
                            principal
                            displayName
                            emailAddress
                        }
                    }
                }
            }",
        )
        .await;
    assert!(allowed_read.errors.is_empty(), "{:?}", allowed_read.errors);
    let allowed_json = allowed_read.data.into_json()?;
    assert_eq!(
        allowed_json["users"]["edges"][0]["node"]["displayName"].as_str(),
        Some("Visible Name")
    );
    assert_eq!(
        allowed_json["users"]["edges"][0]["node"]["emailAddress"].as_str(),
        Some("user@example.com")
    );
    assert!(!policy.reads().is_empty());

    Ok(())
}
