#![cfg(feature = "sqlite")]

#[test]
fn pascal_relationship_semantics_compile_as_a_complete_query_capability() {
    let manifest = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../graphql-orm-macros/fixtures/relationship-capability/Cargo.toml"
    );
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let target_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("runtime crate must be inside the workspace")
        .join("target/relationship-capability-fixture");
    let status = std::process::Command::new(cargo)
        .env("CARGO_TARGET_DIR", target_dir)
        .args([
            "test",
            "--manifest-path",
            manifest,
            "--no-default-features",
            "--features",
            "sqlite",
        ])
        .status()
        .expect("run relationship capability fixture");

    assert!(status.success());
}
