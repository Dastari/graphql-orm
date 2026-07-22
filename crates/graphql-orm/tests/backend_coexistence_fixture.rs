#[test]
fn sqlite_and_mssql_services_can_share_one_graphql_orm_build() {
    let manifest = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/backend-coexistence/Cargo.toml"
    );
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let target_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("runtime crate must be inside the workspace")
        .join("target/backend-coexistence-fixture");
    let status = std::process::Command::new(cargo)
        .env("CARGO_TARGET_DIR", target_dir)
        .args([
            "check",
            "--manifest-path",
            manifest,
            "-p",
            "auth-service",
            "-p",
            "jim-service",
            "--features",
            "jim-service/graphql-orm-mssql-poc",
        ])
        .status()
        .expect("run backend coexistence fixture cargo check");

    assert!(status.success());
    assert_direct_host_dependency_resolves_one_exact_agql_auth_universe();
}

fn assert_direct_host_dependency_resolves_one_exact_agql_auth_universe() {
    let manifest = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/backend-coexistence/Cargo.toml"
    );
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--manifest-path",
            manifest,
        ])
        .output()
        .expect("run auth-service dependency metadata inspection");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata must be JSON");
    let agql_auth = metadata["packages"]
        .as_array()
        .expect("metadata packages")
        .iter()
        .filter(|package| package["name"] == "agql-auth")
        .collect::<Vec<_>>();
    assert_eq!(agql_auth.len(), 1, "resolved metadata:\n{metadata}");
    assert_eq!(agql_auth[0]["version"], "0.12.0");
    let source = agql_auth[0]["source"]
        .as_str()
        .expect("agql-auth source must be present");
    assert!(
        source.contains("rev=3f3b0c5365adfbe436514a681d977b600991b797")
            && source.ends_with("#3f3b0c5365adfbe436514a681d977b600991b797"),
        "unexpected agql-auth source: {source}",
    );
}
