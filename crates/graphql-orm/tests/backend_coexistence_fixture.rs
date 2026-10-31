#[test]
fn sqlite_and_mssql_services_can_share_one_graphql_orm_build() {
    let manifest = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/backend-coexistence/Cargo.toml"
    );
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    // Every fixture workspace builds into one shared directory, so dependency
    // builds they have in common are compiled once.
    let target_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("runtime crate must be inside the workspace")
        .join("target/fixtures");
    let status = std::process::Command::new(cargo)
        .env("CARGO_TARGET_DIR", target_dir)
        .args([
            "test",
            "--locked",
            "--manifest-path",
            manifest,
            "-p",
            "auth-service",
            "-p",
            "coordinated-service",
            "-p",
            "legacy-service",
            "-p",
            "ai-runtime",
            "--features",
            "legacy-service/graphql-orm-mssql-poc",
        ])
        .status()
        .expect("run backend coexistence fixture cargo check");

    assert!(status.success());
}

#[test]
fn direct_host_dependency_resolves_one_exact_agql_auth_universe() {
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
    // The standalone host must adopt the same reviewed identity as the
    // canonical workspace dependency, rather than defining another type universe.
    let dependency = include_str!("../../../Cargo.toml")
        .lines()
        .find(|line| line.starts_with("agql-auth = "))
        .expect("canonical workspace auth dependency");
    let optional_field = |name: &str| {
        let prefix = format!("{name} = \"");
        dependency
            .split_once(prefix.as_str())
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(value, _)| value)
    };
    let field = |name: &str| optional_field(name).expect("reviewed auth dependency field");
    // The selector is either an exact reviewed full revision or a published
    // version tag; never a moving branch.
    let selector = match (optional_field("rev"), optional_field("tag")) {
        (Some(rev), None) => {
            assert!(
                rev.len() == 40 && rev.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "agql-auth rev must be a full commit SHA: {rev}"
            );
            format!("rev={rev}")
        }
        (None, Some(tag)) => {
            assert_eq!(tag, format!("v{}", field("version")));
            format!("tag={tag}")
        }
        _ => panic!("agql-auth must select exactly one of rev or tag: {dependency}"),
    };
    assert_eq!(agql_auth[0]["version"], field("version"));
    // The host graph must resolve the same locked identity as the workspace.
    let workspace_lock = include_str!("../../../Cargo.lock");
    let locked = workspace_lock
        .split("[[package]]")
        .find(|package| package.contains("\nname = \"agql-auth\"\n"))
        .and_then(|package| package.lines().find(|line| line.starts_with("source = \"")))
        .and_then(|line| line.strip_prefix("source = \"")?.strip_suffix('"'))
        .expect("workspace lock records agql-auth");
    let source = agql_auth[0]["source"]
        .as_str()
        .expect("agql-auth source must be present");
    assert!(
        source.starts_with(&format!("git+{}?{selector}#", field("git"))),
        "unexpected agql-auth selector: {source}",
    );
    assert_eq!(source, locked, "unexpected agql-auth source");
}
