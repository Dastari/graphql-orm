#![cfg(any(feature = "sqlite", feature = "postgres"))]

#[test]
fn generated_relationships_execute_across_real_crate_boundaries() {
    let manifest = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../graphql-orm-macros/fixtures/cross-crate-relations/Cargo.toml"
    );
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let backend = if cfg!(feature = "postgres") {
        "postgres"
    } else {
        "sqlite"
    };
    let status = std::process::Command::new(cargo)
        .env("CARGO_BUILD_JOBS", "2")
        .env("CARGO_INCREMENTAL", "0")
        .args([
            "test",
            "--manifest-path",
            manifest,
            "--locked",
            "-p",
            "cross-crate-source-models",
            "--no-default-features",
            "--features",
            backend,
            "--lib",
        ])
        .status()
        .expect("run cross-crate fixture");
    assert!(status.success(), "external fixture failed");
}
