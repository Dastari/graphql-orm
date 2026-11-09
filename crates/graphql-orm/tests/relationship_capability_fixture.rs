#![cfg(feature = "sqlite")]

#[path = "support/fixture_target.rs"]
mod fixture_target;

#[test]
fn pascal_relationship_semantics_compile_as_a_complete_query_capability() {
    let manifest = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../graphql-orm-macros/fixtures/relationship-capability/Cargo.toml"
    );
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let target_dir = fixture_target::target_dir();
    let status = std::process::Command::new(cargo)
        .env("CARGO_TARGET_DIR", target_dir)
        .args([
            "test",
            "--locked",
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
