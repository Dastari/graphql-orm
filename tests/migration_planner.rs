use graphql_orm::graphql::orm::{
    ColumnModel, DatabaseBackend, ForeignKeyModel, IndexDef, MigrationPlan, MigrationStep,
    SchemaModel, TableModel, build_migration_plan, diff_schema_models, migration_filename,
    render_migration_file,
};

fn users_v1() -> TableModel {
    TableModel {
        entity_name: "User".to_string(),
        table_name: "users".to_string(),
        primary_key: "id".to_string(),
        default_sort: "name ASC".to_string(),
        columns: vec![
            ColumnModel {
                name: "id".to_string(),
                sql_type: "TEXT".to_string(),
                nullable: false,
                is_primary_key: true,
                is_unique: false,
                default: None,
            },
            ColumnModel {
                name: "name".to_string(),
                sql_type: "TEXT".to_string(),
                nullable: false,
                is_primary_key: false,
                is_unique: false,
                default: None,
            },
        ],
        indexes: vec![],
        composite_unique_indexes: vec![],
        foreign_keys: vec![],
    }
}

fn users_v2() -> TableModel {
    TableModel {
        columns: vec![
            ColumnModel {
                name: "id".to_string(),
                sql_type: "TEXT".to_string(),
                nullable: false,
                is_primary_key: true,
                is_unique: false,
                default: None,
            },
            ColumnModel {
                name: "name".to_string(),
                sql_type: "VARCHAR(255)".to_string(),
                nullable: false,
                is_primary_key: false,
                is_unique: false,
                default: None,
            },
            ColumnModel {
                name: "active".to_string(),
                sql_type: "BOOLEAN".to_string(),
                nullable: false,
                is_primary_key: false,
                is_unique: false,
                default: Some("false".to_string()),
            },
        ],
        indexes: vec![IndexDef::new("idx_users_name", &["name"])],
        ..users_v1()
    }
}

fn posts_with_fk() -> TableModel {
    TableModel {
        entity_name: "Post".to_string(),
        table_name: "posts".to_string(),
        primary_key: "id".to_string(),
        default_sort: "title ASC".to_string(),
        columns: vec![
            ColumnModel {
                name: "id".to_string(),
                sql_type: "TEXT".to_string(),
                nullable: false,
                is_primary_key: true,
                is_unique: false,
                default: None,
            },
            ColumnModel {
                name: "author_id".to_string(),
                sql_type: "TEXT".to_string(),
                nullable: false,
                is_primary_key: false,
                is_unique: false,
                default: None,
            },
        ],
        indexes: vec![],
        composite_unique_indexes: vec![],
        foreign_keys: vec![ForeignKeyModel {
            source_column: "author_id".to_string(),
            target_table: "users".to_string(),
            target_column: "id".to_string(),
            is_multiple: false,
        }],
    }
}

#[test]
fn diff_detects_create_table_for_new_schema() {
    let current = SchemaModel { tables: vec![] };
    let target = SchemaModel {
        tables: vec![users_v1()],
    };

    let diff = diff_schema_models(&current, &target);
    assert_eq!(diff.steps.len(), 1);
    assert!(matches!(diff.steps[0], MigrationStep::CreateTable(_)));
}

#[test]
fn diff_detects_add_alter_and_create_index() {
    let current = SchemaModel {
        tables: vec![users_v1()],
    };
    let target = SchemaModel {
        tables: vec![users_v2()],
    };

    let diff = diff_schema_models(&current, &target);
    assert!(diff.steps.iter().any(|step| matches!(
        step,
        MigrationStep::AddColumn { table_name, column }
            if table_name == "users" && column.name == "active"
    )));
    assert!(diff.steps.iter().any(|step| matches!(
        step,
        MigrationStep::AlterColumn { table_name, after, .. }
            if table_name == "users" && after.name == "name" && after.sql_type == "VARCHAR(255)"
    )));
    assert!(diff.steps.iter().any(|step| matches!(
        step,
        MigrationStep::CreateIndex { table_name, index }
            if table_name == "users" && index.name == "idx_users_name"
    )));
}

#[test]
fn diff_detects_drop_column_and_drop_table() {
    let current = SchemaModel {
        tables: vec![users_v2()],
    };
    let target = SchemaModel { tables: vec![] };

    let diff = diff_schema_models(&current, &target);
    assert!(diff.steps.iter().any(|step| matches!(
        step,
        MigrationStep::DropTable { table_name } if table_name == "users"
    )));
}

#[test]
fn postgres_plan_renders_backend_specific_statements() {
    let current = SchemaModel {
        tables: vec![users_v1()],
    };
    let target = SchemaModel {
        tables: vec![users_v2()],
    };

    let plan: MigrationPlan = build_migration_plan(DatabaseBackend::Postgres, &current, &target);
    assert!(plan
        .statements
        .iter()
        .any(|statement| statement == "ALTER TABLE users ADD COLUMN active BOOLEAN NOT NULL DEFAULT false"));
    assert!(plan
        .statements
        .iter()
        .any(|statement| statement == "ALTER TABLE users ALTER COLUMN name TYPE VARCHAR(255)"));
    assert!(plan
        .statements
        .iter()
        .any(|statement| statement == "CREATE INDEX idx_users_name ON users (name)"));
}

#[test]
fn sqlite_plan_rebuilds_tables_for_column_alterations() {
    let current = SchemaModel {
        tables: vec![users_v1()],
    };
    let target = SchemaModel {
        tables: vec![users_v2()],
    };

    let plan = build_migration_plan(DatabaseBackend::Sqlite, &current, &target);
    assert!(plan
        .statements
        .iter()
        .any(|statement| statement == "PRAGMA foreign_keys = OFF"));
    assert!(plan
        .statements
        .iter()
        .any(|statement| statement.starts_with("CREATE TABLE __graphql_orm_users_new")));
    assert!(plan
        .statements
        .iter()
        .any(|statement| statement
            == "INSERT INTO __graphql_orm_users_new (id, name) SELECT id, name FROM users"));
    assert!(plan
        .statements
        .iter()
        .any(|statement| statement == "DROP TABLE users"));
    assert!(plan
        .statements
        .iter()
        .any(|statement| statement == "ALTER TABLE __graphql_orm_users_new RENAME TO users"));
    assert!(plan
        .statements
        .iter()
        .any(|statement| statement == "CREATE INDEX idx_users_name ON users (name)"));
    assert!(plan
        .statements
        .iter()
        .any(|statement| statement == "PRAGMA foreign_keys = ON"));
}

#[test]
fn diff_detects_foreign_key_addition() {
    let current = SchemaModel {
        tables: vec![users_v1(), TableModel { foreign_keys: vec![], ..posts_with_fk() }],
    };
    let target = SchemaModel {
        tables: vec![users_v1(), posts_with_fk()],
    };

    let diff = diff_schema_models(&current, &target);
    assert!(diff.steps.iter().any(|step| matches!(
        step,
        MigrationStep::AddForeignKey { table_name, foreign_key }
            if table_name == "posts"
                && foreign_key.source_column == "author_id"
                && foreign_key.target_table == "users"
                && foreign_key.target_column == "id"
    )));
}

#[test]
fn postgres_plan_renders_foreign_key_statement() {
    let current = SchemaModel {
        tables: vec![users_v1(), TableModel { foreign_keys: vec![], ..posts_with_fk() }],
    };
    let target = SchemaModel {
        tables: vec![users_v1(), posts_with_fk()],
    };

    let plan = build_migration_plan(DatabaseBackend::Postgres, &current, &target);
    assert!(plan.statements.iter().any(|statement| {
        statement.contains("ALTER TABLE posts ADD CONSTRAINT fk_posts_author_id_users_id")
            && statement.contains("FOREIGN KEY (author_id) REFERENCES users(id)")
    }));
}

#[test]
fn sqlite_plan_rebuilds_tables_for_foreign_key_changes() {
    let current = SchemaModel {
        tables: vec![users_v1(), TableModel { foreign_keys: vec![], ..posts_with_fk() }],
    };
    let target = SchemaModel {
        tables: vec![users_v1(), posts_with_fk()],
    };

    let plan = build_migration_plan(DatabaseBackend::Sqlite, &current, &target);
    assert!(plan
        .statements
        .iter()
        .any(|statement| statement.starts_with("CREATE TABLE __graphql_orm_posts_new")));
    assert!(plan.statements.iter().any(|statement| {
        statement.contains("FOREIGN KEY (author_id) REFERENCES users(id)")
    }));
    assert!(plan
        .statements
        .iter()
        .any(|statement| statement
            == "INSERT INTO __graphql_orm_posts_new (id, author_id) SELECT id, author_id FROM posts"));
}

#[test]
fn migration_file_renderer_includes_headers_and_semicolons() {
    let plan = MigrationPlan {
        backend: DatabaseBackend::Postgres,
        steps: vec![],
        statements: vec![
            "CREATE TABLE users (id TEXT PRIMARY KEY)".to_string(),
            "-- comment only".to_string(),
        ],
    };

    let file = render_migration_file(&plan, "2026032501", "Create Users");
    assert!(file.contains("-- version: 2026032501"));
    assert!(file.contains("-- description: Create Users"));
    assert!(file.contains("CREATE TABLE users (id TEXT PRIMARY KEY);"));
    assert!(file.contains("-- comment only;"));
    assert_eq!(migration_filename("2026032501", "Create Users"), "2026032501_create_users.sql");
}
