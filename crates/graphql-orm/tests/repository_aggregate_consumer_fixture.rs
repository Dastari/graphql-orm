#[test]
fn external_repository_aggregate_consumer_requires_no_direct_graphql_dependency() {
    let manifest = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/repository-aggregate-consumer/Cargo.toml"
    );
    let target_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/repository-aggregate-consumer");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let lane = if cfg!(feature = "sqlite") {
        "sqlite"
    } else if cfg!(feature = "postgres") {
        "postgres"
    } else {
        "mssql"
    };
    let status = std::process::Command::new(&cargo)
        .env("CARGO_TARGET_DIR", &target_dir)
        .args([
            "test",
            "--manifest-path",
            manifest,
            "--locked",
            "--no-default-features",
            "--features",
            lane,
        ])
        .status()
        .expect("run external repository aggregate consumer");
    assert!(status.success());

    let output = std::process::Command::new(cargo)
        .args([
            "metadata",
            "--manifest-path",
            manifest,
            "--locked",
            "--format-version",
            "1",
            "--no-deps",
        ])
        .output()
        .expect("inspect external consumer manifest");
    assert!(output.status.success());
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("Cargo metadata JSON");
    let dependencies = metadata["packages"][0]["dependencies"]
        .as_array()
        .expect("direct dependencies");
    assert!(
        dependencies
            .iter()
            .all(|dependency| dependency["name"] != "async-graphql")
    );
}
