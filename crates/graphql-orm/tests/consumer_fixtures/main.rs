//! Standalone consumer workspaces exercised by the CI fixture lane.

#[path = "../support/fixture_target.rs"]
mod fixture_target;

mod backend_coexistence_fixture;
mod migration_api_compatibility_fixture;
mod relationship_capability_fixture;
mod repository_aggregate_consumer_fixture;
