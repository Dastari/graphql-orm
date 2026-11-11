use std::fs;
use std::path::{Path, PathBuf};

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).expect("source directory should be readable") {
            let path = entry.expect("source entry should be readable").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files
}

#[test]
fn crate_source_has_no_direct_database_dependency() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest =
        fs::read_to_string(root.join("Cargo.toml")).expect("manifest should be readable");
    assert!(!manifest.contains("sqlx ="));
    assert!(!manifest.contains("tiberius ="));

    for path in rust_files(&root.join("src")) {
        let source = fs::read_to_string(&path).expect("Rust source should be readable");
        for forbidden in ["sqlx::", "tiberius::", "DATABASE_URL", "TEST_DATABASE_URL"] {
            assert!(
                !source.contains(forbidden),
                "{} contains forbidden boundary reference {forbidden}",
                path.display()
            );
        }
    }
}

/// Every principal-reference resolution goes through
/// `principal_projection::resolve_current_reference`, which refuses legacy
/// delegated and sessionless stored references before the host resolver runs.
/// A direct `CurrentPrincipalResolver::resolve` call elsewhere would bypass it.
#[test]
fn principal_references_resolve_only_through_the_delegation_refusal() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut direct = Vec::new();
    for path in rust_files(&root) {
        let source: String = fs::read_to_string(&path)
            .expect("Rust source should be readable")
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        assert!(
            !source.contains("CurrentPrincipalResolver::resolve("),
            "{} resolves a principal reference without the delegation refusal",
            path.display()
        );
        let mut rest = source.as_str();
        while let Some(start) = rest.find(".resolve(") {
            rest = &rest[start + ".resolve(".len()..];
            let argument = rest.split(')').next().unwrap_or_default();
            if !argument.contains(',') && argument.to_ascii_lowercase().contains("reference") {
                direct.push(path.clone());
            }
        }
    }
    assert_eq!(
        direct,
        vec![root.join("principal_projection.rs")],
        "only resolve_current_reference may call the host resolver"
    );
}
