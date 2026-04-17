use async_graphql::{Request, Schema};
use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};
use tokio::time::{Duration, sleep};

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "discovered_devices",
    plural = "DiscoveredDevices",
    default_sort = "name ASC",
    upsert = "mac"
)]
struct DiscoveredDevice {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[unique]
    #[filterable(type = "string")]
    #[sortable]
    pub mac: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[graphql_orm(private)]
    pub last_seen_by: String,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [DiscoveredDevice],
}

type TestSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

#[derive(Clone, Default)]
struct UpsertAudit {
    entries: Arc<Mutex<Vec<String>>>,
}

impl UpsertAudit {
    fn snapshot(&self) -> Vec<String> {
        self.entries.lock().expect("audit lock poisoned").clone()
    }
}

impl graphql_orm::graphql::orm::WriteInputTransform for UpsertAudit {
    fn before_upsert_with_context<'a>(
        &'a self,
        write_ctx: &'a mut graphql_orm::graphql::orm::WriteInputContext<'_, '_>,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if write_ctx.entity_name() != "DiscoveredDevice" {
                return Ok(());
            }

            let actor = write_ctx
                .actor::<String>()
                .unwrap_or_else(|| "system".to_string());
            let input = input
                .downcast_mut::<CreateDiscoveredDeviceInput>()
                .ok_or_else(|| async_graphql::Error::new("unexpected upsert input type"))?;
            input.last_seen_by = actor.clone();
            self.entries
                .lock()
                .expect("audit lock poisoned")
                .push(format!("upsert:{:?}:{actor}", write_ctx.origin()));
            Ok(())
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ObservedUpsert {
    phase: graphql_orm::graphql::orm::MutationPhase,
    action: graphql_orm::graphql::orm::ChangeAction,
    before_name: Option<String>,
    after_name: Option<String>,
}

#[derive(Clone, Default)]
struct RecordingUpsertHook {
    events: Arc<Mutex<Vec<ObservedUpsert>>>,
}

impl RecordingUpsertHook {
    fn snapshot(&self) -> Vec<ObservedUpsert> {
        self.events.lock().expect("hook lock poisoned").clone()
    }
}

impl graphql_orm::graphql::orm::MutationHook for RecordingUpsertHook {
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
                .push(ObservedUpsert {
                    phase: event.phase.clone(),
                    action: event.action,
                    before_name: event
                        .before::<DiscoveredDevice>()?
                        .map(|device| device.name.clone()),
                    after_name: event
                        .after::<DiscoveredDevice>()?
                        .map(|device| device.name.clone()),
                });
            Ok(())
        })
    }
}

#[derive(Clone, Default)]
struct DeviceRowPolicy;

impl graphql_orm::graphql::orm::RowPolicy for DeviceRowPolicy {
    fn can_read_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: graphql_orm::graphql::orm::EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn can_write_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: graphql_orm::graphql::orm::EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if entity_name != "DiscoveredDevice" {
                return Ok(true);
            }
            let Some(device) = row.downcast_ref::<DiscoveredDevice>() else {
                return Ok(false);
            };
            let actor = ctx.and_then(|ctx| ctx.data_opt::<String>()).cloned();
            Ok(actor.is_none()
                || actor.as_deref() == Some("admin")
                || actor.as_deref() == Some(device.last_seen_by.as_str()))
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
        "CREATE TABLE discovered_devices (
            id TEXT PRIMARY KEY,
            mac TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            last_seen_by TEXT NOT NULL,
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
    sqlx::query("DROP TABLE IF EXISTS discovered_devices")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE discovered_devices (
            id UUID PRIMARY KEY,
            mac TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            last_seen_by TEXT NOT NULL,
            created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint),
            updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::bigint)
        )",
    )
    .execute(&pool)
    .await?;
    Ok(pool)
}

#[tokio::test]
async fn generated_upserts_work_for_graphql_and_repository_paths()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let audit = UpsertAudit::default();
    let hook = RecordingUpsertHook::default();
    let mut db = graphql_orm::db::Database::with_mutation_hook(pool.clone(), hook.clone());
    db.set_write_input_transform(audit.clone());
    db.set_row_policy(DeviceRowPolicy);
    let mut rx = db
        .ensure_event_sender::<DiscoveredDeviceChangedEvent>()
        .subscribe();

    let actor_a_schema: TestSchema = schema_builder(db.clone())
        .data("actor-a".to_string())
        .finish();
    let actor_b_schema: TestSchema = schema_builder(db.clone())
        .data("actor-b".to_string())
        .finish();

    let created = actor_a_schema
        .execute(
            Request::new(
                "mutation {
                    upsertDiscoveredDevice(input: { mac: \"AA:BB\", name: \"Alpha\" }) {
                        success
                        action
                        discoveredDevice { id mac name }
                    }
                }",
            )
            .data("actor-a".to_string()),
        )
        .await;
    assert!(created.errors.is_empty(), "{:?}", created.errors);
    let created_json = created.data.into_json()?;
    assert_eq!(
        created_json["upsertDiscoveredDevice"]["action"].as_str(),
        Some("CREATED")
    );
    let created_id = created_json["upsertDiscoveredDevice"]["discoveredDevice"]["id"]
        .as_str()
        .expect("created id missing")
        .to_string();

    let created_event = rx.recv().await?;
    assert_eq!(
        created_event.action,
        graphql_orm::graphql::orm::ChangeAction::Created
    );

    sleep(Duration::from_secs(1)).await;

    let updated = actor_a_schema
        .execute(
            Request::new(
                "mutation {
                    upsertDiscoveredDevice(input: { mac: \"AA:BB\", name: \"Alpha Two\" }) {
                        success
                        action
                        discoveredDevice { id mac name }
                    }
                }",
            )
            .data("actor-a".to_string()),
        )
        .await;
    assert!(updated.errors.is_empty(), "{:?}", updated.errors);
    let updated_json = updated.data.into_json()?;
    assert_eq!(
        updated_json["upsertDiscoveredDevice"]["action"].as_str(),
        Some("UPDATED")
    );
    assert_eq!(
        updated_json["upsertDiscoveredDevice"]["discoveredDevice"]["id"].as_str(),
        Some(created_id.as_str())
    );

    let updated_event = rx.recv().await?;
    assert_eq!(
        updated_event.action,
        graphql_orm::graphql::orm::ChangeAction::Updated
    );

    sleep(Duration::from_secs(1)).await;

    let repository_outcome = DiscoveredDevice::upsert(
        &db,
        CreateDiscoveredDeviceInput {
            mac: "AA:BB".to_string(),
            name: "Repo Update".to_string(),
            last_seen_by: String::new(),
        },
    )
    .await?;
    assert_eq!(
        repository_outcome.action,
        graphql_orm::graphql::orm::ChangeAction::Updated
    );
    assert_eq!(repository_outcome.entity.id.to_string(), created_id);
    assert_eq!(repository_outcome.entity.last_seen_by, "system");

    let repo_event = rx.recv().await?;
    assert_eq!(
        repo_event.action,
        graphql_orm::graphql::orm::ChangeAction::Updated
    );

    let denied = actor_b_schema
        .execute(
            Request::new(
                "mutation {
                    upsertDiscoveredDevice(input: { mac: \"AA:BB\", name: \"Blocked\" }) {
                        success
                        error
                    }
                }",
            )
            .data("actor-b".to_string()),
        )
        .await;
    assert!(!denied.errors.is_empty());
    assert!(
        denied.errors[0].message.contains("Write denied"),
        "{:?}",
        denied.errors
    );

    let stored =
        DiscoveredDevice::get(db.pool(), &graphql_orm::uuid::Uuid::parse_str(&created_id)?)
            .await?
            .expect("stored device missing");
    assert_eq!(stored.name, "Repo Update");
    assert_eq!(stored.last_seen_by, "system");

    let audit_entries = audit.snapshot();
    assert_eq!(
        audit_entries,
        vec![
            "upsert:GraphqlMutation:actor-a".to_string(),
            "upsert:GraphqlMutation:actor-a".to_string(),
            "upsert:Repository:system".to_string(),
            "upsert:GraphqlMutation:actor-b".to_string(),
        ]
    );

    let observed = hook.snapshot();
    assert_eq!(observed.len(), 6);
    assert_eq!(
        observed
            .iter()
            .filter(|event| event.action == graphql_orm::graphql::orm::ChangeAction::Created)
            .count(),
        2
    );
    assert_eq!(
        observed
            .iter()
            .filter(|event| event.action == graphql_orm::graphql::orm::ChangeAction::Updated)
            .count(),
        4
    );
    assert_eq!(observed[0].before_name, None);
    assert_eq!(observed[1].after_name.as_deref(), Some("Alpha"));
    assert_eq!(observed[2].before_name.as_deref(), Some("Alpha"));
    assert_eq!(observed[3].after_name.as_deref(), Some("Alpha Two"));
    assert_eq!(observed[4].before_name.as_deref(), Some("Alpha Two"));
    assert_eq!(observed[5].after_name.as_deref(), Some("Repo Update"));

    Ok(())
}
