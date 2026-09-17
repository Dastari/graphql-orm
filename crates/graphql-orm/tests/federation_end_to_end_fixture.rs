#![cfg(feature = "sqlite")]
//! Drives the federation end-to-end fixture workspace.
//!
//! The fixture is its own workspace because it needs the router, a second ORM
//! subgraph, and an Ntex runtime, none of which belong in this crate's test
//! dependencies.

#[test]
fn two_orm_subgraphs_compose_and_join_through_the_router() {
    let manifest = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/federation-end-to-end/Cargo.toml"
    );
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let target_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("runtime crate must be inside the workspace")
        .join("target/federation-end-to-end-fixture");
    let status = std::process::Command::new(cargo)
        .env("CARGO_TARGET_DIR", target_dir)
        .args([
            "test",
            "--manifest-path",
            manifest,
            "-p",
            "federation-e2e",
            "-p",
            "readings-subgraph",
            "-p",
            "subgraph-http",
            "-p",
            "zones-subgraph",
        ])
        .status()
        .expect("run the federation end-to-end fixture tests");

    assert!(status.success());
}
