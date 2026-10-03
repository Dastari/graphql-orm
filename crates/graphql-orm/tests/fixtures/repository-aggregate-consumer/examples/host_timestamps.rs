//! Framework-neutral host timestamp inputs, including a physical updated_at alias.
use graphql_orm::prelude::*;
use repository_aggregate_consumer::Backend;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "host_clock_example",
        plural = "HostClockExamples",
        upsert = "id"
    )
)]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    repository_entity(
        backend = "postgres",
        table = "host_clock_example",
        plural = "HostClockExamples",
        upsert = "id"
    )
)]
#[cfg_attr(
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
    repository_entity(
        backend = "mssql",
        table = "host_clock_example",
        plural = "HostClockExamples",
        schema_policy = "external_writable",
        upsert = "id"
    )
)]
pub struct HostClockExample {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: graphql_orm::uuid::Uuid,
    pub label: String,
    #[graphql_orm(timestamp = "host", default = "0")]
    pub created_at: i64,
    #[graphql_orm(timestamp = "host", db_column = "updated_at", default = "0")]
    pub modified_ms: i64,
}

// Ordinary write=false restrictions are respected; host mode grants no authority.
#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "restricted_host_clock",
        plural = "RestrictedHostClocks"
    )
)]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    repository_entity(
        backend = "postgres",
        table = "restricted_host_clock",
        plural = "RestrictedHostClocks"
    )
)]
#[cfg_attr(
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
    repository_entity(
        backend = "mssql",
        table = "restricted_host_clock",
        plural = "RestrictedHostClocks",
        schema_policy = "external_writable"
    )
)]
pub struct RestrictedHostClock {
    #[primary_key]
    pub id: String,
    pub label: String,
    #[graphql_orm(timestamp = "host", write = false, default = "0")]
    pub updated_at: i64,
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub async fn run(db: &Database<Backend>) -> Result<(), Box<dyn std::error::Error>> {
    // Only call this setup on an owned disposable database, never an external source.
    let plan = db
        .schema()
        .plan_migration_to_entities(
            "example-init",
            "owned host clock example",
            &[
                HostClockExample::metadata(),
                RestrictedHostClock::metadata(),
            ],
        )
        .await?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let id = graphql_orm::uuid::Uuid::new_v4();
    db.transaction(TransactionMode::StateMachine, |tx| {
        Box::pin(async move {
            let row = tx
                .insert::<HostClockExample>(CreateHostClockExampleInput {
                    id,
                    label: "caller-clock".into(),
                    created_at: 1_765_432_100_123,
                    modified_ms: -123,
                })
                .await?;
            assert_eq!(row.id, id);
            let row = tx
                .update_by_id::<HostClockExample>(
                    &id,
                    UpdateHostClockExampleInput {
                        modified_ms: Some(0),
                        ..Default::default()
                    },
                )
                .await?
                .expect("own inserted record");
            assert_eq!((row.created_at, row.modified_ms), (1_765_432_100_123, 0));
            Ok::<_, OrmPublicError>(())
        })
    })
    .await?;
    let row = HostClockExample::update_by_id(
        db,
        &id,
        UpdateHostClockExampleInput {
            label: Some("omitted-timestamps".into()),
            ..Default::default()
        },
    )
    .await?
    .expect("committed record");
    assert_eq!((row.created_at, row.modified_ms), (1_765_432_100_123, 0));
    let restricted = RestrictedHostClock::insert(
        db,
        CreateRestrictedHostClockInput {
            label: "ordinary-restrictions".into(),
        },
    )
    .await?;
    let restricted = RestrictedHostClock::update_by_id(
        db,
        &restricted.id,
        UpdateRestrictedHostClockInput {
            label: Some("still-restricted".into()),
        },
    )
    .await?
    .expect("restricted row");
    assert_eq!(restricted.updated_at, 0);
    assert!(HostClockExample::delete_by_id(db, &id).await?);
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::<Backend>::connect_sqlite("sqlite::memory:").await?;
    run(&db).await
}
#[cfg(not(feature = "sqlite"))]
#[allow(dead_code)]
fn main() {
    // PostgreSQL executes through tests/host_timestamps.rs with owned infrastructure.
    // MSSQL declarations compile only; this example issues no external schema operations.
}
