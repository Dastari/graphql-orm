#[test]
fn external_legacy_migration_surface_compiles() {
    let manifest = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/migration-api-compatibility/Cargo.toml"
    );
    let target_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/migration-api-compatibility");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = std::process::Command::new(cargo);
    command.env("CARGO_TARGET_DIR", target_dir).args([
        "test",
        "--manifest-path",
        manifest,
        "--locked",
        "--no-default-features",
        "--features",
    ]);
    if cfg!(feature = "sqlite") {
        command.arg("sqlite");
    } else if cfg!(feature = "postgres") {
        command.arg("postgres");
    } else {
        command.arg("mssql");
    }
    let status = command
        .status()
        .expect("run external migration API fixture");
    assert!(status.success());
}
