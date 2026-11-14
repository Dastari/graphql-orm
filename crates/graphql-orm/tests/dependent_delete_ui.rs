#[path = "support/ui.rs"]
mod ui;

static BUILDS: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[test]
fn unsupported_dependent_delete_shapes_are_rejected() {
    let _build = BUILDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let t = ui::test_cases();
    #[cfg(not(feature = "mssql"))]
    t.compile_fail("tests/ui/dependent_delete_composite.rs");
    // The expected output names the backend, so each backend has its own case.
    #[cfg(feature = "sqlite")]
    t.compile_fail("tests/ui/dependent_delete_parent.rs");
    #[cfg(feature = "postgres")]
    t.compile_fail("tests/ui/dependent_delete_parent_postgres.rs");
    #[cfg(feature = "mssql")]
    t.compile_fail("tests/ui/dependent_delete_mssql.rs");
}

#[cfg(not(feature = "mssql"))]
#[test]
fn declared_parent_opt_in_is_enforced_across_crates() {
    let _build = BUILDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
    let check = |feature: &str| {
        std::process::Command::new(&cargo)
            .args([
                "check",
                "--manifest-path",
                manifest,
                "--locked",
                "-p",
                "cross-crate-target-models",
                "--no-default-features",
                "--features",
                &format!("{backend},{feature}"),
            ])
            .output()
            .expect("run external parent marker fixture")
    };
    let negative = check("dependent-child");
    assert!(
        !negative.status.success(),
        "opted-in child accepted an opted-out parent from another crate"
    );
    let error = String::from_utf8_lossy(&negative.stderr);
    assert!(
        error.contains("dependent_deletes requires") && error.contains("OptInParent"),
        "{error}"
    );
    let positive = check("dependent-parent-opt-in");
    assert!(
        positive.status.success(),
        "{}",
        String::from_utf8_lossy(&positive.stderr)
    );
}
