#![cfg(feature = "sqlite")]

use super::fixture_target;

#[test]
fn pascal_relationship_semantics_compile_as_a_complete_query_capability() {
    let manifest = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../graphql-orm-macros/fixtures/relationship-capability/Cargo.toml"
    );
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    // The fixture's services use the PascalCase naming contract (#508). A
    // separate target directory keeps the camelCase fixtures' graphql-orm
    // build from being rebuilt on every switch.
    let target_dir = fixture_target::target_dir().join("pascal-case");
    let status = std::process::Command::new(cargo)
        .env("CARGO_TARGET_DIR", target_dir)
        .env("GRAPHQL_ORM_RESOLVER_CASE", "PascalCase")
        .env("GRAPHQL_ORM_ARGUMENT_CASE", "PascalCase")
        .env("GRAPHQL_ORM_FIELD_CASE", "PascalCase")
        .env_remove("GRAPHQL_ORM_AI_CASE")
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
