//! Complete sensitive evidence stays in memory; this example prints hashes only.
use graphql_orm::{db::Database, graphql::orm::*};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let target = OwnedSchemaModel::default();
    let ownership = ManagedTableSet::new([])?;
    let limits = OwnedPlanningReplayLimits::default();
    let (plan, witness) = db
        .schema()
        .plan_owned_migration_with_replay_witness(
            "example-v1",
            "empty owned schema",
            &target,
            &ownership,
            PlanOptions::strict(),
            limits,
            OwnedPlanningReplayCaptureConsent::accept_sensitive_schema_metadata_and_private_custody(
            ),
        )
        .await?;
    let private_bytes = witness.to_json(limits)?;
    db.pool().close().await;
    let facts = OwnedPlanningReplayWitness::from_json(&private_bytes, limits)?.verify()?;
    assert_eq!(facts.plan_hash(), plan.plan_hash());
    println!(
        "verified recipe={} plan={}",
        facts.recipe(),
        facts.plan_hash()
    );
    Ok(())
}
