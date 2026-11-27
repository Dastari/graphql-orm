#![cfg(any(feature = "sqlite", feature = "postgres"))]

use super::fixture_target;

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
    let target_dir = fixture_target::target_dir();
    let status = std::process::Command::new(&cargo)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target_dir)
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

    let output = std::process::Command::new(cargo)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", target_dir)
        .args([
            "check",
            "--manifest-path",
            manifest,
            "--locked",
            "-p",
            "cross-crate-source-models",
            "--no-default-features",
            "--features",
            &format!("{backend},unreadable-target-key"),
        ])
        .output()
        .expect("check unreadable target across crate boundaries");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "private cross-crate key compiled");
    assert!(stderr.contains("readable relation `Session.endpoint` uses unreadable or unresolved target key `Endpoint.tenant_id`"), "{stderr}");

    // A public alias of a server-only column in the target crate withdraws
    // its certification, so the source crate's server_only_key is refused:
    // the same name, a delimited name, and a case-folded name.
    for feature in [
        "server-only-public-alias",
        "server-only-qualified-alias",
        "server-only-folded-alias",
    ] {
        let output = std::process::Command::new(
            std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()),
        )
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", fixture_target::target_dir())
        .args([
            "check",
            "--manifest-path",
            manifest,
            "--locked",
            "-p",
            "cross-crate-source-models",
            "--no-default-features",
            "--features",
            &format!("{backend},{feature}"),
        ])
        .output()
        .expect("check an alias of a server-only key across crate boundaries");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "{feature}: aliased server-only key compiled"
        );
        assert!(
            stderr.contains("relation `PartitionedLink.endpoint` declares server_only_key target `PartitionedEndpoint.partition_id`, which is not a server-only column"),
            "{feature}: {stderr}"
        );
    }
}
