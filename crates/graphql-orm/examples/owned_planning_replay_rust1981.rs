//! New native recipe: run with exact qualified Rust 1.98.1; prints hashes only.
use graphql_orm::{db::Database, graphql::orm::*};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let target = OwnedSchemaModel::default();
    let ownership = ManagedTableSet::new([])?;
    let limits = OwnedPlanningReplayLimits::default();
    let recipe = OwnedPlanningReplayRecipe::Rust1981V1;
    let (plan, witness) = db
        .schema()
        .plan_owned_migration_with_replay_witness_for_recipe(
            "example-v1",
            "empty owned schema",
            &target,
            &ownership,
            PlanOptions::strict(),
            recipe,
            limits,
            OwnedPlanningReplayCaptureConsent::accept_sensitive_schema_metadata_and_private_custody(
            ),
        )
        .await?;
    let private_bytes = witness.to_json(limits)?;
    db.pool().close().await;
    let facts = OwnedPlanningReplayWitness::from_json_for_recipe(&private_bytes, limits, recipe)?
        .verify()?;
    assert_eq!(facts.plan_hash(), plan.plan_hash());
    println!(
        "verified recipe={} plan={}",
        facts.recipe(),
        facts.plan_hash()
    );
    Ok(())
}
