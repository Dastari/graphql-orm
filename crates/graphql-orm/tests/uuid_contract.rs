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
#[graphql_entity(table = "accounts", plural = "Accounts", default_sort = "name ASC")]
pub struct Account {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[filterable(type = "boolean")]
    pub active: bool,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [Account],
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
        _ctx: &'a async_graphql::Context<'_>,
        _db: &'a graphql_orm::db::Database,
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
    sqlx::query("PRAGMA foreign_keys = ON").execute(&pool).await?;
    sqlx::query(
        "CREATE TABLE accounts (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            active INTEGER NOT NULL,
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
    sqlx::query("DROP TABLE IF EXISTS accounts")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE accounts (
            id UUID PRIMARY KEY,
            name TEXT NOT NULL,
            active BOOLEAN NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

#[cfg(feature = "sqlite")]
fn expected_uuid_sql_type() -> &'static str {
    "TEXT"
}

#[cfg(feature = "postgres")]
fn expected_uuid_sql_type() -> &'static str {
    "UUID"
}

#[tokio::test]
async fn uuid_ids_are_first_class_across_crud_migrations_and_hooks(
) -> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let hook = RecordingHook::default();
    let database = graphql_orm::db::Database::with_mutation_hook(pool.clone(), hook.clone());
    let schema = schema_builder(database).data("test-user".to_string()).finish();

    let created = schema
        .execute(
            "mutation {
                CreateAccount(Input: { name: \"Primary\", active: true }) {
                    Success
                    Account { id name active }
                }
            }",
        )
        .await;
    assert!(created.errors.is_empty(), "{:?}", created.errors);
    let created_json = created.data.into_json()?;
    let account_id = created_json["CreateAccount"]["Account"]["id"]
        .as_str()
        .expect("account id should be present")
        .to_string();
    let account_uuid = graphql_orm::uuid::Uuid::parse_str(&account_id)?;

    let by_id = schema
        .execute(format!(
            "query {{
                Account(Id: \"{account_id}\") {{
                    id
                    name
                    active
                }}
            }}"
        ))
        .await;
    assert!(by_id.errors.is_empty(), "{:?}", by_id.errors);
    let by_id_json = by_id.data.into_json()?;
    assert_eq!(by_id_json["Account"]["id"].as_str(), Some(account_id.as_str()));
    assert_eq!(by_id_json["Account"]["name"].as_str(), Some("Primary"));

    let filtered = schema
        .execute(format!(
            "query {{
                Accounts(Where: {{ id: {{ Eq: \"{account_id}\" }} }}) {{
                    Edges {{
                        Node {{ id name }}
                    }}
                }}
            }}"
        ))
        .await;
    assert!(filtered.errors.is_empty(), "{:?}", filtered.errors);
    let filtered_json = filtered.data.into_json()?;
    assert_eq!(filtered_json["Accounts"]["Edges"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        filtered_json["Accounts"]["Edges"][0]["Node"]["id"].as_str(),
        Some(account_id.as_str())
    );

    let updated = schema
        .execute(format!(
            "mutation {{
                UpdateAccount(Id: \"{account_id}\", Input: {{ name: \"Renamed\" }}) {{
                    Success
                    Account {{ id name }}
                }}
            }}"
        ))
        .await;
    assert!(updated.errors.is_empty(), "{:?}", updated.errors);
    let updated_json = updated.data.into_json()?;
    assert_eq!(
        updated_json["UpdateAccount"]["Account"]["name"].as_str(),
        Some("Renamed")
    );

    let deleted = schema
        .execute(format!(
            "mutation {{
                DeleteAccount(Id: \"{account_id}\") {{
                    Success
                }}
            }}"
        ))
        .await;
    assert!(deleted.errors.is_empty(), "{:?}", deleted.errors);
    let deleted_json = deleted.data.into_json()?;
    assert_eq!(deleted_json["DeleteAccount"]["Success"].as_bool(), Some(true));

    let metadata = <Account as graphql_orm::graphql::orm::Entity>::metadata();
    let id_field = metadata
        .fields
        .iter()
        .find(|field| field.name == "id")
        .expect("id field metadata should exist");
    assert_eq!(id_field.sql_type, expected_uuid_sql_type());

    let target_schema = graphql_orm::graphql::orm::SchemaModel::from_entities(&[metadata]);
    let plan = graphql_orm::graphql::orm::build_migration_plan(
        graphql_orm::graphql::orm::current_backend(),
        &graphql_orm::graphql::orm::SchemaModel { tables: Vec::new() },
        &target_schema,
    );
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement.contains(expected_uuid_sql_type())),
        "migration plan should use backend UUID storage: {:?}",
        plan.statements
    );

    let events = hook.snapshot();
    assert_eq!(events.len(), 6);
    assert_eq!(
        events
            .iter()
            .map(|event| (&event.phase, &event.action))
            .collect::<Vec<_>>(),
        vec![
            (
                &graphql_orm::graphql::orm::MutationPhase::Before,
                &graphql_orm::graphql::orm::ChangeAction::Created
            ),
            (
                &graphql_orm::graphql::orm::MutationPhase::After,
                &graphql_orm::graphql::orm::ChangeAction::Created
            ),
            (
                &graphql_orm::graphql::orm::MutationPhase::Before,
                &graphql_orm::graphql::orm::ChangeAction::Updated
            ),
            (
                &graphql_orm::graphql::orm::MutationPhase::After,
                &graphql_orm::graphql::orm::ChangeAction::Updated
            ),
            (
                &graphql_orm::graphql::orm::MutationPhase::Before,
                &graphql_orm::graphql::orm::ChangeAction::Deleted
            ),
            (
                &graphql_orm::graphql::orm::MutationPhase::After,
                &graphql_orm::graphql::orm::ChangeAction::Deleted
            ),
        ]
    );
    assert!(events.iter().all(|event| event.id == account_id));
    assert_eq!(events[0].entity_name, "Account");
    assert_eq!(events[0].table_name, "accounts");
    assert_eq!(
        events[0]
            .changes
            .iter()
            .map(|field| field.field.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "name", "active"]
    );
    assert!(matches!(
        events[0].changes[0].value,
        graphql_orm::graphql::orm::SqlValue::Uuid(id) if id == account_uuid
    ));
    assert_eq!(
        events[2]
            .changes
            .iter()
            .map(|field| field.field.as_str())
            .collect::<Vec<_>>(),
        vec!["name"]
    );
    assert!(events[4].changes.is_empty());

    Ok(())
}
