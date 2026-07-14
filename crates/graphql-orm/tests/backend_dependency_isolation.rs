use std::path::Path;
use std::process::Command;

fn resolved_tree(features: &str) -> String {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_dir = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("runtime crate must be inside the workspace");
    let output = Command::new(env!("CARGO"))
        .current_dir(workspace_dir)
        .args([
            "tree",
            "--locked",
            "--edges",
            "normal",
            "--package",
            "graphql-orm",
            "--no-default-features",
            "--features",
            features,
        ])
        .output()
        .expect("cargo tree must run");
    assert!(
        output.status.success(),
        "cargo tree failed for {features}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("cargo tree output must be UTF-8")
}

fn has_package(tree: &str, package: &str) -> bool {
    tree.lines().any(|line| {
        line.trim_start_matches([' ', '│', '├', '└', '─'])
            .starts_with(&format!("{package} v"))
    })
}

#[test]
fn backend_features_activate_only_their_sqlx_drivers() {
    let sqlite = resolved_tree("sqlite");
    assert!(has_package(&sqlite, "sqlx-sqlite"));
    assert!(!has_package(&sqlite, "sqlx-postgres"));

    let postgres = resolved_tree("postgres");
    assert!(has_package(&postgres, "sqlx-postgres"));
    assert!(!has_package(&postgres, "sqlx-sqlite"));

    let mssql = resolved_tree("mssql");
    assert!(!has_package(&mssql, "sqlx-postgres"));
    assert!(!has_package(&mssql, "sqlx-sqlite"));

    let combined = resolved_tree("sqlite,postgres");
    assert!(has_package(&combined, "sqlx-sqlite"));
    assert!(has_package(&combined, "sqlx-postgres"));
}

#[cfg(all(feature = "sqlite", feature = "postgres"))]
mod combined_backend_compile {
    use graphql_orm::prelude::*;

    #[derive(GraphQLSchemaEntity, Clone, Debug)]
    #[graphql_entity(
        backend = "sqlite",
        table = "combined_sqlite_records",
        plural = "CombinedSqliteRecords"
    )]
    struct CombinedSqliteRecord {
        #[primary_key]
        id: String,
    }

    #[derive(GraphQLSchemaEntity, Clone, Debug)]
    #[graphql_entity(
        backend = "postgres",
        table = "combined_postgres_records",
        plural = "CombinedPostgresRecords"
    )]
    struct CombinedPostgresRecord {
        #[primary_key]
        id: String,
    }

    #[test]
    fn explicit_entities_compile_with_both_write_backends_enabled() {
        let sqlite = CombinedSqliteRecord {
            id: "sqlite".to_owned(),
        };
        let postgres = CombinedPostgresRecord {
            id: "postgres".to_owned(),
        };
        assert_eq!(sqlite.id, "sqlite");
        assert_eq!(postgres.id, "postgres");
        assert!(
            CombinedSqliteRecord::metadata()
                .table_name
                .contains("combined_sqlite_records")
        );
        assert!(
            CombinedPostgresRecord::metadata()
                .table_name
                .contains("combined_postgres_records")
        );
    }
}
