//! Caller allocation and schema ownership stay outside the repository.
use graphql_orm::prelude::*;
use repository_aggregate_consumer::{
    Backend,
    host_primary_keys::{CreateHostI32Input, HostI32},
};

pub async fn insert_allocated(
    database: &Database<Backend>,
    allocated: i32,
) -> Result<HostI32, Box<dyn std::error::Error>> {
    let created = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                Ok(tx
                    .insert::<HostI32>(CreateHostI32Input {
                        id: allocated,
                        label: "allocated externally".into(),
                    })
                    .await?)
            })
        })
        .await?;
    assert_eq!(created.id, allocated);
    Ok(created)
}

#[cfg(feature = "sqlite")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut database = Database::<Backend>::connect_sqlite("sqlite::memory:").await?;
    database
        .set_entity_policy(repository_aggregate_consumer::host_primary_keys::HostKeyFixturePolicy);
    {
        let plan = database
            .schema()
            .plan_migration_to_entities(
                "host-key",
                "owned consumer fixture",
                &[HostI32::metadata()],
            )
            .await?;
        database
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
    }
    insert_allocated(&database, 123_456).await?;
    database.pool().close().await;
    Ok(())
}

#[cfg(not(feature = "sqlite"))]
fn main() {
    println!(
        "Compile-only entry point; owned backend tests execute insert_allocated. External schemas are never installed by this example."
    );
}
