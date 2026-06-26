#[test]
fn sqlite_and_mssql_services_can_share_one_graphql_orm_build() {
    let manifest = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/backend-coexistence/Cargo.toml"
    );
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = std::process::Command::new(cargo)
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
}
