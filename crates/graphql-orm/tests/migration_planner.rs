use graphql_orm::graphql::orm::{
    ColumnModel, DatabaseBackend, DeletePolicy, ForeignKeyModel, IndexDef, MigrationPlan,
    MigrationStep, SchemaModel, SpatialColumnDef, SpatialGeometryType, TableModel,
    build_migration_plan, diff_schema_models, migration_filename, render_migration_file,
};

fn users_v1() -> TableModel {
    TableModel {
        entity_name: "User".to_string(),
        table_name: "users".to_string(),
        primary_key: "id".to_string(),
        primary_keys: vec!["id".to_string()],
        default_sort: "name ASC".to_string(),
        columns: vec![
            ColumnModel {
                name: "id".to_string(),
                sql_type: "TEXT".to_string(),
                spatial: None,
                nullable: false,
                is_primary_key: true,
                is_unique: false,
                default: None,
            },
            ColumnModel {
                name: "name".to_string(),
                sql_type: "TEXT".to_string(),
                spatial: None,
                nullable: false,
                is_primary_key: false,
                is_unique: false,
                default: None,
            },
        ],
        indexes: vec![],
        composite_unique_indexes: vec![],
        foreign_keys: vec![],
        search_indexes: vec![],
        append_only: false,
        retention_purge: false,
        check_constraints: vec![],
    }
}

fn users_v2() -> TableModel {
    TableModel {
        columns: vec![
            ColumnModel {
                name: "id".to_string(),
                sql_type: "TEXT".to_string(),
                spatial: None,
                nullable: false,
                is_primary_key: true,
                is_unique: false,
                default: None,
            },
            ColumnModel {
                name: "name".to_string(),
                sql_type: "VARCHAR(255)".to_string(),
                spatial: None,
                nullable: false,
                is_primary_key: false,
                is_unique: false,
                default: None,
            },
            ColumnModel {
                name: "active".to_string(),
                sql_type: "BOOLEAN".to_string(),
                spatial: None,
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
        primary_keys: vec!["id".to_string()],
        default_sort: "title ASC".to_string(),
        columns: vec![
            ColumnModel {
                name: "id".to_string(),
                sql_type: "TEXT".to_string(),
                spatial: None,
                nullable: false,
                is_primary_key: true,
                is_unique: false,
                default: None,
            },
            ColumnModel {
                name: "author_id".to_string(),
                sql_type: "TEXT".to_string(),
                spatial: None,
                nullable: false,
                is_primary_key: false,
                is_unique: false,
                default: None,
            },
        ],
        indexes: vec![],
        composite_unique_indexes: vec![],
        foreign_keys: vec![ForeignKeyModel::single(
            "author_id",
            "users",
            "id",
            false,
            DeletePolicy::Restrict,
        )],
        search_indexes: vec![],
        append_only: false,
        retention_purge: false,
        check_constraints: vec![],
    }
}

fn places_with_spatial_index() -> TableModel {
    TableModel {
        entity_name: "Place".to_string(),
        table_name: "places".to_string(),
        primary_key: "id".to_string(),
        primary_keys: vec!["id".to_string()],
        default_sort: "id".to_string(),
        columns: vec![
            ColumnModel {
                name: "id".to_string(),
                sql_type: "TEXT".to_string(),
                spatial: None,
                nullable: false,
                is_primary_key: true,
                is_unique: false,
                default: None,
            },
            ColumnModel {
                name: "location".to_string(),
                sql_type: "geometry(Point,4326)".to_string(),
                spatial: Some(SpatialColumnDef::geometry(SpatialGeometryType::Point, 4326)),
                nullable: false,
                is_primary_key: false,
                is_unique: false,
                default: None,
            },
        ],
        indexes: vec![IndexDef::spatial_gist(
            "idx_places_location_spatial",
            &["location"],
        )],
        composite_unique_indexes: vec![],
        foreign_keys: vec![],
        search_indexes: vec![],
        append_only: false,
        retention_purge: false,
        check_constraints: vec![],
    }
}

fn sqlite_places_with_spatial_field() -> TableModel {
    TableModel {
        columns: vec![
            ColumnModel {
                name: "id".to_string(),
                sql_type: "TEXT".to_string(),
                spatial: None,
                nullable: false,
                is_primary_key: true,
                is_unique: false,
                default: None,
            },
            ColumnModel {
                name: "location".to_string(),
                sql_type: "TEXT".to_string(),
                spatial: Some(SpatialColumnDef::geometry(SpatialGeometryType::Point, 4326)),
                nullable: false,
                is_primary_key: false,
                is_unique: false,
                default: None,
            },
        ],
        indexes: vec![],
        ..places_with_spatial_index()
    }
}

#[test]
fn diff_detects_create_table_for_new_schema() {
    let current = SchemaModel {
        extensions: Vec::new(),
        tables: vec![],
    };
    let target = SchemaModel {
        extensions: Vec::new(),
        tables: vec![users_v1()],
    };

    let diff = diff_schema_models(&current, &target);
    assert_eq!(diff.steps.len(), 1);
    assert!(matches!(diff.steps[0], MigrationStep::CreateTable(_)));
}

#[test]
fn diff_detects_add_alter_and_create_index() {
    let current = SchemaModel {
        extensions: Vec::new(),
        tables: vec![users_v1()],
    };
    let target = SchemaModel {
        extensions: Vec::new(),
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
        extensions: Vec::new(),
        tables: vec![users_v2()],
    };
    let target = SchemaModel {
        extensions: Vec::new(),
        tables: vec![],
    };

    let diff = diff_schema_models(&current, &target);
    assert!(diff.steps.iter().any(|step| matches!(
        step,
        MigrationStep::DropTable { table_name } if table_name == "users"
    )));
}

#[test]
fn postgres_plan_renders_backend_specific_statements() {
    let current = SchemaModel {
        extensions: Vec::new(),
        tables: vec![users_v1()],
    };
    let target = SchemaModel {
        extensions: Vec::new(),
        tables: vec![users_v2()],
    };

    let plan: MigrationPlan = build_migration_plan(DatabaseBackend::Postgres, &current, &target);
    assert!(plan.statements.iter().any(|statement| statement
        == "ALTER TABLE users ADD COLUMN active BOOLEAN NOT NULL DEFAULT false"));
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement == "ALTER TABLE users ALTER COLUMN name TYPE VARCHAR(255)")
    );
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement == "CREATE INDEX \"idx_users_name\" ON \"users\" (\"name\")")
    );
}

#[test]
fn postgres_plan_enables_postgis_and_renders_spatial_index() {
    let current = SchemaModel {
        extensions: Vec::new(),
        tables: vec![],
    };
    let target = SchemaModel {
        extensions: vec!["postgis".to_string()],
        tables: vec![places_with_spatial_index()],
    };

    let plan = build_migration_plan(DatabaseBackend::Postgres, &current, &target);
    assert_eq!(
        plan.statements.first().map(String::as_str),
        Some("CREATE EXTENSION IF NOT EXISTS postgis")
    );
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement.contains("location geometry(Point,4326) NOT NULL"))
    );
    assert!(plan.statements.iter().any(|statement| {
        statement == "CREATE INDEX \"idx_places_location_spatial\" ON \"places\" USING GIST (\"location\")"
    }));
}

#[test]
fn sqlite_plan_stores_spatial_fields_as_text_without_spatial_indexes() {
    let current = SchemaModel {
        extensions: Vec::new(),
        tables: vec![],
    };
    let target = SchemaModel {
        extensions: vec!["postgis".to_string()],
        tables: vec![sqlite_places_with_spatial_field()],
    };

    let plan = build_migration_plan(DatabaseBackend::Sqlite, &current, &target);
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement.contains("location TEXT NOT NULL"))
    );
    assert!(
        !plan
            .statements
            .iter()
            .any(|statement| statement.contains("postgis") || statement.contains("USING GIST"))
    );

    let mut introspected = sqlite_places_with_spatial_field();
    introspected.columns[1].spatial = None;
    let current = SchemaModel {
        extensions: Vec::new(),
        tables: vec![introspected],
    };
    let plan = build_migration_plan(DatabaseBackend::Sqlite, &current, &target);
    assert!(plan.statements.is_empty());
}

#[test]
fn index_method_mismatch_drops_and_recreates_index() {
    let current = SchemaModel {
        extensions: vec!["postgis".to_string()],
        tables: vec![TableModel {
            indexes: vec![IndexDef::new("idx_places_location_spatial", &["location"])],
            ..places_with_spatial_index()
        }],
    };
    let target = SchemaModel {
        extensions: vec!["postgis".to_string()],
        tables: vec![places_with_spatial_index()],
    };

    let diff = diff_schema_models(&current, &target);
    let drop_position = diff.steps.iter().position(|step| {
        matches!(
            step,
            MigrationStep::DropIndex { table_name, index_name }
                if table_name == "places" && index_name == "idx_places_location_spatial"
        )
    });
    let create_position = diff.steps.iter().position(|step| {
        matches!(
            step,
            MigrationStep::CreateIndex { table_name, index }
                if table_name == "places" && index.name == "idx_places_location_spatial" && index.is_spatial
        )
    });

    assert!(drop_position.is_some());
    assert!(create_position.is_some());
    assert!(drop_position < create_position);
}

#[test]
fn sqlite_plan_rebuilds_tables_for_column_alterations() {
    let current = SchemaModel {
        extensions: Vec::new(),
        tables: vec![users_v1()],
    };
    let target = SchemaModel {
        extensions: Vec::new(),
        tables: vec![users_v2()],
    };

    let plan = build_migration_plan(DatabaseBackend::Sqlite, &current, &target);
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement == "PRAGMA foreign_keys = OFF")
    );
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement.starts_with("CREATE TABLE __graphql_orm_users_new"))
    );
    assert!(plan.statements.iter().any(|statement| statement
        == "INSERT INTO __graphql_orm_users_new (id, name) SELECT id, name FROM users"));
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement == "DROP TABLE users")
    );
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement == "ALTER TABLE __graphql_orm_users_new RENAME TO users")
    );
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement == "CREATE INDEX \"idx_users_name\" ON \"users\" (\"name\")")
    );
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement == "PRAGMA foreign_keys = ON")
    );
}

#[test]
fn diff_detects_foreign_key_addition() {
    let current = SchemaModel {
        extensions: Vec::new(),
        tables: vec![
            users_v1(),
            TableModel {
                foreign_keys: vec![],
                ..posts_with_fk()
            },
        ],
    };
    let target = SchemaModel {
        extensions: Vec::new(),
        tables: vec![users_v1(), posts_with_fk()],
    };

    let diff = diff_schema_models(&current, &target);
    assert!(diff.steps.iter().any(|step| matches!(
        step,
        MigrationStep::AddForeignKey { table_name, foreign_key }
            if table_name == "posts"
                && foreign_key.source_columns().eq(["author_id"].into_iter())
                && foreign_key.target_table == "users"
                && foreign_key.target_columns().eq(["id"].into_iter())
                && foreign_key.on_delete == DeletePolicy::Restrict
    )));
}

#[test]
fn postgres_plan_renders_foreign_key_statement() {
    let current = SchemaModel {
        extensions: Vec::new(),
        tables: vec![
            users_v1(),
            TableModel {
                foreign_keys: vec![],
                ..posts_with_fk()
            },
        ],
    };
    let target = SchemaModel {
        extensions: Vec::new(),
        tables: vec![users_v1(), posts_with_fk()],
    };

    let plan = build_migration_plan(DatabaseBackend::Postgres, &current, &target);
    assert!(plan.statements.iter().any(|statement| {
        statement.contains("ALTER TABLE posts ADD CONSTRAINT \"fk_posts_author_id_users_id\"")
            && statement.contains("FOREIGN KEY (author_id) REFERENCES users(id) ON DELETE RESTRICT")
    }));
}

#[test]
fn a_changed_delete_policy_drops_the_old_foreign_key_before_adding_the_new_one() {
    // The generated constraint name ignores the policy, so the replacement must free it first,
    // whichever policy sorts first.
    let policies = [
        DeletePolicy::Restrict,
        DeletePolicy::Cascade,
        DeletePolicy::SetNull,
    ];
    for from in &policies {
        for to in policies.iter().filter(|to| *to != from) {
            let with = |policy: &DeletePolicy| {
                let mut posts = posts_with_fk();
                posts.columns[1].nullable = true;
                posts.foreign_keys[0].on_delete = policy.clone();
                SchemaModel {
                    extensions: Vec::new(),
                    tables: vec![users_v1(), posts],
                }
            };
            let (current, target) = (with(from), with(to));
            let diff = diff_schema_models(&current, &target);
            let kinds = diff
                .steps
                .iter()
                .filter_map(|step| match step {
                    MigrationStep::DropForeignKey { foreign_key, .. } => {
                        Some(("drop", foreign_key.on_delete.clone()))
                    }
                    MigrationStep::AddForeignKey { foreign_key, .. } => {
                        Some(("add", foreign_key.on_delete.clone()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                kinds,
                [("drop", from.clone()), ("add", to.clone())],
                "{from:?} -> {to:?}"
            );
            let plan = build_migration_plan(DatabaseBackend::Postgres, &current, &target);
            let position = |needle: &str| {
                plan.statements
                    .iter()
                    .position(|statement| statement.contains(needle))
                    .unwrap_or_else(|| panic!("{needle} in {:?}", plan.statements))
            };
            assert!(
                position("DROP CONSTRAINT \"fk_posts_author_id_users_id\"")
                    < position("ADD CONSTRAINT \"fk_posts_author_id_users_id\""),
                "{from:?} -> {to:?}: {:?}",
                plan.statements
            );
        }
    }
}

#[test]
fn postgres_fk_drop_uses_the_exact_quoted_observed_constraint_name() {
    let mut posts = posts_with_fk();
    posts.foreign_keys[0].constraint_name = Some("Legacy \"Posts\" Author FK".to_string());
    let current = SchemaModel {
        extensions: Vec::new(),
        tables: vec![users_v1(), posts],
    };
    let target = SchemaModel {
        extensions: Vec::new(),
        tables: vec![
            users_v1(),
            TableModel {
                foreign_keys: vec![],
                ..posts_with_fk()
            },
        ],
    };

    let plan = build_migration_plan(DatabaseBackend::Postgres, &current, &target);
    assert!(plan.statements.iter().any(|statement| {
        statement == "ALTER TABLE posts DROP CONSTRAINT \"Legacy \"\"Posts\"\" Author FK\""
    }));
}

#[test]
fn sqlite_plan_rebuilds_tables_for_foreign_key_changes() {
    let current = SchemaModel {
        extensions: Vec::new(),
        tables: vec![
            users_v1(),
            TableModel {
                foreign_keys: vec![],
                ..posts_with_fk()
            },
        ],
    };
    let target = SchemaModel {
        extensions: Vec::new(),
        tables: vec![users_v1(), posts_with_fk()],
    };

    let plan = build_migration_plan(DatabaseBackend::Sqlite, &current, &target);
    assert!(
        plan.statements
            .iter()
            .any(|statement| statement.starts_with("CREATE TABLE __graphql_orm_posts_new"))
    );
    assert!(plan.statements.iter().any(|statement| {
        statement.contains("FOREIGN KEY (author_id) REFERENCES users(id) ON DELETE RESTRICT")
    }));
    assert!(plan.statements.iter().any(|statement| statement
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
    assert_eq!(
        migration_filename("2026032501", "Create Users"),
        "2026032501_create_users.sql"
    );
}

#[test]
fn known_postgres_epoch_defaults_compare_without_widening_sql_equivalence() {
    use graphql_orm::graphql::orm::canonicalize_column_default_expression;
    let generated = "EXTRACT(EPOCH FROM NOW())::bigint";
    for deparsed in [
        "(EXTRACT(epoch FROM now()))::bigint",
        "((EXTRACT(epoch FROM now()))::bigint)",
        "extract(epoch from now())::BIGINT",
    ] {
        assert!(
            canonicalize_column_default_expression(deparsed) == generated,
            "known default representation mismatch"
        );
    }
    for different in [
        "(EXTRACT(epoch FROM now()))::bigint * 1000",
        "EXTRACT(EPOCH FROM clock_timestamp())::bigint",
        "EXTRACT(EPOCH FROM NOW())::integer",
        "'EXTRACT(EPOCH FROM NOW())::bigint'",
    ] {
        assert!(
            canonicalize_column_default_expression(different) != generated,
            "different default collapsed"
        );
    }
}

#[test]
fn postgres_timestamp_aliases_share_planning_without_rewriting_hashes_or_real_types() {
    fn model(kind: &str) -> SchemaModel {
        let mut table = users_v1();
        table.columns[1].sql_type = kind.into();
        SchemaModel {
            extensions: vec![],
            tables: vec![table],
        }
    }
    let short = model("TIMESTAMPTZ");
    let deparsed = model("timestamp with time zone");
    // The public hash contract retains the caller's original physical spelling.
    assert_ne!(short.stable_hash(), deparsed.stable_hash());
    for (before, after) in [(&short, &deparsed), (&deparsed, &short)] {
        let plan = build_migration_plan(DatabaseBackend::Postgres, before, after);
        assert!(plan.steps.is_empty());
        assert!(plan.statements.is_empty());
        assert!(
            !build_migration_plan(DatabaseBackend::Sqlite, before, after)
                .steps
                .is_empty()
        );
        assert!(
            !build_migration_plan(DatabaseBackend::Mssql, before, after)
                .steps
                .is_empty()
        );
    }
    for distinct in [
        "timestamp without time zone",
        "TIMESTAMP",
        "DATE",
        "TEXT",
        "TIMESTAMPTZ(3)",
        "timestamp(3) with time zone",
        "TIMESTAMPTZ[]",
        "public.timestamptz",
        "\"TIMESTAMPTZ\"",
    ] {
        let plan = build_migration_plan(DatabaseBackend::Postgres, &short, &model(distinct));
        assert!(
            plan.steps
                .iter()
                .any(|step| matches!(step, MigrationStep::AlterColumn { .. }))
        );
    }
    assert!(
        build_migration_plan(
            DatabaseBackend::Postgres,
            &model("timestamptz"),
            &model("TIMESTAMP WITH TIME ZONE")
        )
        .steps
        .is_empty()
    );
    assert!(
        !build_migration_plan(
            DatabaseBackend::Postgres,
            &model("TIMESTAMPTZ(3)"),
            &model("TIMESTAMPTZ(6)")
        )
        .steps
        .is_empty()
    );
}

#[test]
fn postgres_text_literal_deparse_shares_planning_without_rewriting_hashes() {
    fn model(default: &str) -> SchemaModel {
        let mut table = users_v1();
        table.columns[1].default = Some(default.into());
        SchemaModel {
            extensions: vec![],
            tables: vec![table],
        }
    }
    let plain = model("'O''Reilly::text'");
    let deparsed = model("'O''Reilly::text'::text");
    let plain_hash = plain.stable_hash();
    let deparsed_hash = deparsed.stable_hash();
    assert_ne!(
        plain_hash, deparsed_hash,
        "legacy representation hashes remain distinct"
    );
    for (before, after) in [(&plain, &deparsed), (&deparsed, &plain)] {
        let plan = build_migration_plan(DatabaseBackend::Postgres, before, after);
        assert!(plan.steps.is_empty());
        assert!(plan.statements.is_empty());
        for backend in [DatabaseBackend::Sqlite, DatabaseBackend::Mssql] {
            assert!(
                !build_migration_plan(backend, before, after)
                    .steps
                    .is_empty()
            );
        }
    }
    assert_eq!(plain.stable_hash(), plain_hash);
    assert_eq!(deparsed.stable_hash(), deparsed_hash);
    let changed = build_migration_plan(DatabaseBackend::Postgres, &deparsed, &model("'different'"));
    assert!(
        changed
            .steps
            .iter()
            .any(|step| matches!(step, MigrationStep::AlterColumn { .. }))
    );
}

#[test]
fn postgres_alter_column_renders_real_default_changes_without_unrelated_ddl() {
    let model = |default: Option<&str>| {
        let mut table = users_v1();
        table.columns[1].default = default.map(str::to_owned);
        SchemaModel {
            extensions: vec![],
            tables: vec![table],
        }
    };
    for (before, after, expected) in [
        (None, Some("'old'"), "SET DEFAULT 'old'"),
        (
            Some("'old'"),
            Some("'O''Brien 雪'"),
            "SET DEFAULT 'O''Brien 雪'",
        ),
        (Some("'old'"), None, "DROP DEFAULT"),
    ] {
        let plan = build_migration_plan(DatabaseBackend::Postgres, &model(before), &model(after));
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(
            plan.statements,
            [format!("ALTER TABLE users ALTER COLUMN name {expected}")]
        );
        assert_ne!(model(before).stable_hash(), model(after).stable_hash());
    }
}

#[test]
fn postgres_alter_type_detaches_old_default_then_restores_desired_expression() {
    for (desired, restore) in [
        (Some("'new'"), Some("SET DEFAULT 'new'")),
        (Some("'old'"), Some("SET DEFAULT 'old'")),
        (None, None),
    ] {
        let mut current = users_v1();
        current.columns[1].default = Some("'old'".into());
        let mut target = current.clone();
        target.columns[1].sql_type = "VARCHAR(255)".into();
        target.columns[1].default = desired.map(str::to_owned);
        let plan = build_migration_plan(
            DatabaseBackend::Postgres,
            &SchemaModel {
                extensions: vec![],
                tables: vec![current],
            },
            &SchemaModel {
                extensions: vec![],
                tables: vec![target],
            },
        );
        let mut expected = vec![
            "ALTER TABLE users ALTER COLUMN name DROP DEFAULT".to_owned(),
            "ALTER TABLE users ALTER COLUMN name TYPE VARCHAR(255)".to_owned(),
        ];
        if let Some(restore) = restore {
            expected.push(format!("ALTER TABLE users ALTER COLUMN name {restore}"));
        }
        assert_eq!(plan.statements, expected);
    }
}

#[test]
fn postgres_nullability_only_preserves_equivalent_defaults_and_type_aliases() {
    for (before_type, after_type, before_default, after_default) in [
        ("TEXT", "TEXT", "'same'::text", "'same'"),
        (
            "TIMESTAMPTZ",
            "timestamp with time zone",
            "CURRENT_TIMESTAMP",
            "CURRENT_TIMESTAMP",
        ),
    ] {
        let mut current = users_v1();
        current.columns[1].sql_type = before_type.into();
        current.columns[1].default = Some(before_default.into());
        let mut target = current.clone();
        target.columns[1].sql_type = after_type.into();
        target.columns[1].default = Some(after_default.into());
        target.columns[1].nullable = true;
        let plan = build_migration_plan(
            DatabaseBackend::Postgres,
            &SchemaModel {
                extensions: vec![],
                tables: vec![current],
            },
            &SchemaModel {
                extensions: vec![],
                tables: vec![target],
            },
        );
        assert_eq!(
            plan.statements,
            ["ALTER TABLE users ALTER COLUMN name DROP NOT NULL"]
        );
    }
}
