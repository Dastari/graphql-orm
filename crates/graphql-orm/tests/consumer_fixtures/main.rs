//! Standalone consumer workspaces exercised by the CI fixture lane.

#[path = "../support/fixture_target.rs"]
mod fixture_target;

mod backend_coexistence_fixture;
mod migration_api_compatibility_fixture;
mod relationship_capability_fixture;
mod repository_aggregate_consumer_fixture;

fn fixture_build_dir(fixture: &str) -> std::path::PathBuf {
    let path = fixture_target::target_dir().join(fixture);
    // Each build gets its own leaf beneath `fixtures`. Create it before launching
    // Cargo so its cold-start rename cannot replace another build's parent.
    std::fs::create_dir_all(&path).expect("create consumer fixture build directory");
    path
}
