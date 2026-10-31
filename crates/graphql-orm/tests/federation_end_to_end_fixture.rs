#![cfg(feature = "sqlite")]
//! Drives the federation end-to-end fixture workspace.
//!
//! The fixture is its own workspace because it needs the router, a second ORM
//! subgraph, and an Ntex runtime, none of which belong in this crate's test
//! dependencies.

fn run_fixture(arguments: &[&str]) {
    let manifest = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/federation-end-to-end/Cargo.toml"
    );
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    // Every fixture workspace builds into one shared directory, so dependency
    // builds they have in common are compiled once.
    let target_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("runtime crate must be inside the workspace")
        .join("target/fixtures");
    let status = std::process::Command::new(cargo)
        .env("CARGO_TARGET_DIR", target_dir)
        .args(["test", "--locked", "--manifest-path", manifest])
        .args(arguments)
        .status()
        .expect("run the federation end-to-end fixture tests");

    assert!(status.success());
}

#[test]
fn two_orm_subgraphs_compose_and_join_through_the_router() {
    run_fixture(&[
        "-p",
        "federation-e2e",
        "-p",
        "fixture-database",
        "-p",
        "readings-subgraph",
        "-p",
        "subgraph-http",
        "-p",
        "zones-subgraph",
    ]);
}

/// The global node lookup with both subgraphs on test-owned PostgreSQL.
#[test]
#[ignore = "starts test-owned PostgreSQL containers"]
fn node_lookup_through_the_router_on_owned_postgres() {
    run_fixture(&[
        "-p",
        "federation-e2e",
        "--no-default-features",
        "--features",
        "postgres",
        "--test",
        "node_lookup",
    ]);
}

/// The global node lookup with both subgraphs on test-owned SQL Server.
#[test]
#[ignore = "starts test-owned SQL Server containers"]
fn node_lookup_through_the_router_on_owned_mssql() {
    run_fixture(&[
        "-p",
        "federation-e2e",
        "--no-default-features",
        "--features",
        "mssql",
        "--test",
        "node_lookup",
    ]);
}
