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

#[test]
fn current_principal_resolution_is_owned_by_the_guarded_handle() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    for path in rust_files(&root) {
        if path
            .file_name()
            .is_some_and(|name| name == "principal_projection.rs")
        {
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        let compact: String = source.chars().filter(|ch| !ch.is_whitespace()).collect();
        for declaration in compact.split("struct").skip(1) {
            let Some(open) = declaration.find('{') else {
                continue;
            };
            if declaration[..open].contains(';') || declaration.starts_with("AiRuntimeBuilder{") {
                continue;
            }
            let mut depth = 1;
            let mut end = open + 1;
            for byte in declaration.as_bytes()[open + 1..].iter() {
                end += 1;
                match byte {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    break;
                }
            }
            let fields = &declaration[open + 1..end - 1];
            for raw in [
                "Arc<dynCurrentPrincipalResolver>",
                "Arc<dynagql_auth::CurrentPrincipalResolver>",
            ] {
                assert!(
                    !fields.contains(raw),
                    "{} stores raw principal authority outside the guarded owner",
                    path.display()
                );
            }
        }
        // Constructors may accept the host trait, but engine calls must use
        // resolve_current_reference and the private, non-Deref resolver handle.
        for forbidden in [
            "principal_resolver.resolve(",
            "resolver.resolve(",
            "CurrentPrincipalResolver::resolve(",
            "CurrentPrincipalResolver>::resolve(",
        ] {
            assert!(
                !compact.contains(forbidden),
                "{} bypasses the current-reference guard: {forbidden}",
                path.display()
            );
        }
    }
}
