#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
use graphql_orm::graphql::orm::*;
use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "unique_delta",
        schema_policy = "managed",
        unique_composite = "owner_id,name"
    )
)]
#[cfg_attr(
    feature = "postgres",
    repository_entity(
        backend = "postgres",
        table = "unique_delta",
        schema_policy = "managed",
        unique_composite = "owner_id,name"
    )
)]
#[cfg_attr(
    feature = "mssql",
    repository_entity(
        backend = "mssql",
        table = "dbo.unique_delta",
        schema_policy = "managed",
        unique_composite = "owner_id,name"
    )
)]
struct Record {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    owner_id: String,
    name: String,
}

#[test]
fn every_backend_renders_composite_unique_deltas() {
    let before = SchemaModel::from_entities(&[Record::metadata()]);
    let mut after = before.clone();
    after.tables[0].composite_unique_indexes = vec![vec!["owner_id".into(), "id".into()]];
    for backend in [
        DatabaseBackend::Sqlite,
        DatabaseBackend::Postgres,
        DatabaseBackend::Mssql,
    ] {
        let diff = diff_schema_models_for_backend(backend, &before, &after);
        assert!(matches!(
            diff.steps.as_slice(),
            [MigrationStep::SetCompositeUniques { .. }]
        ));
        let sql = render_migration_step(backend, &diff.steps[0]).join("; ");
        match backend {
            DatabaseBackend::Sqlite => assert!(sql.contains("rebuild")),
            _ => {
                assert!(sql.contains("DROP CONSTRAINT"));
                assert!(sql.contains("ADD UNIQUE"));
            }
        }
    }
}

#[test]
fn primary_key_unique_normalization_is_backend_specific_and_ordered() {
    let mut target = SchemaModel::from_entities(&[Record::metadata()]);
    target.tables[0].primary_keys = vec!["owner_id".into(), "name".into()];
    target.tables[0].primary_key = "owner_id".into();
    for column in &mut target.tables[0].columns {
        column.is_primary_key = column.name != "id";
    }
    let mut physical = target.clone();
    physical.tables[0].composite_unique_indexes.clear();
    for backend in [DatabaseBackend::Sqlite, DatabaseBackend::Postgres] {
        assert!(
            diff_schema_models_for_backend(backend, &physical, &target)
                .steps
                .is_empty()
        );
        assert!(
            diff_schema_models_for_backend(backend, &target, &physical)
                .steps
                .is_empty()
        );
        let mut reversed = target.clone();
        reversed.tables[0].composite_unique_indexes[0].reverse();
        assert!(matches!(
            diff_schema_models_for_backend(backend, &physical, &reversed)
                .steps
                .as_slice(),
            [MigrationStep::SetCompositeUniques { .. }]
        ));
    }
    assert!(matches!(
        diff_schema_models_for_backend(DatabaseBackend::Mssql, &physical, &target)
            .steps
            .as_slice(),
        [MigrationStep::SetCompositeUniques { .. }]
    ));
}

#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[cfg(any(feature = "sqlite", feature = "postgres"))]
#[tokio::test]
async fn populated_table_remove_add_change_and_duplicate_refusal()
-> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    #[cfg(feature = "postgres")]
    let mut server = owned_postgres::OwnedPostgres::start("composite-uniques")?;
    #[cfg(feature = "postgres")]
    let db = Database::<PostgresBackend>::connect_postgres(&server.url).await?;
    let manager = db.schema();
    let mut metadata = Record::metadata().clone();
    metadata.indexes = vec![IndexDef::new("preserved_index", &["name"])].into_boxed_slice();
    let initial = manager
        .plan_migration_to_entities_with_options(
            "v1",
            "initial",
            &[Box::leak(Box::new(metadata.clone()))],
            PlanOptions::managed_tables_only(),
        )
        .await?;
    manager
        .apply_migration(&initial, ApplyOptions::default())
        .await?;
    sqlx::query("INSERT INTO unique_delta(id,owner_id,name) VALUES ('one','owner','name'),('two','other','name')").execute(db.pool()).await?;

    #[cfg(feature = "sqlite")]
    for sql in [
        "CREATE TABLE audit (id TEXT)",
        "CREATE TRIGGER preserved_trigger AFTER INSERT ON unique_delta BEGIN INSERT INTO audit VALUES (NEW.id); END",
        "CREATE TABLE child (id TEXT PRIMARY KEY, parent_id TEXT REFERENCES unique_delta(id))",
        "INSERT INTO child VALUES ('child','one')",
    ] {
        sqlx::query(sql).execute(db.pool()).await?;
    }
    // Removing the declaration must physically permit previously conflicting rows.
    metadata.composite_unique_indexes = Box::new([]);
    let remove = manager
        .plan_migration_to_entities_with_options(
            "v2",
            "remove",
            &[Box::leak(Box::new(metadata.clone()))],
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(!remove.steps.is_empty());
    manager
        .apply_migration(&remove, ApplyOptions::default())
        .await?;
    let same = manager
        .plan_migration_to_entities_with_options(
            "removed-check",
            "same",
            &[Box::leak(Box::new(metadata.clone()))],
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(same.steps.is_empty(), "{same:?}");
    sqlx::query("INSERT INTO unique_delta VALUES ('three','owner','name')")
        .execute(db.pool())
        .await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM unique_delta")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(count, 3);
    metadata.composite_unique_indexes = Record::metadata().composite_unique_indexes.clone();
    let add = manager
        .plan_migration_to_entities_with_options(
            "v3",
            "add duplicates",
            &[Box::leak(Box::new(metadata.clone()))],
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(!add.steps.is_empty());
    let error = manager
        .apply_migration(&add, ApplyOptions::default())
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("existing rows violate a unique constraint"),
        "{error}"
    );
    assert!(!error.to_string().contains("owner_id"));
    let recorded: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM __graphql_orm_migrations WHERE version='v3'")
            .fetch_one(db.pool())
            .await?;
    assert_eq!(recorded, 0);
    #[cfg(feature = "sqlite")]
    {
        let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(enabled, 1);
        let temporary: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='__graphql_orm_unique_delta_new'",
        )
        .fetch_one(db.pool())
        .await?;
        assert_eq!(temporary, 0);
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM unique_delta")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(count, 3);
    sqlx::query("UPDATE unique_delta SET name='different' WHERE id='three'")
        .execute(db.pool())
        .await?;
    manager
        .apply_migration(&add, ApplyOptions::default())
        .await?;
    let same = manager
        .plan_migration_to_entities_with_options(
            "added-check",
            "same",
            &[Box::leak(Box::new(metadata.clone()))],
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(same.steps.is_empty(), "{same:?}");
    assert!(
        sqlx::query("INSERT INTO unique_delta VALUES ('four','owner','name')")
            .execute(db.pool())
            .await
            .is_err()
    );
    // Changing the member set removes the old enforcement and installs the new one.
    metadata.composite_unique_indexes =
        vec![vec!["owner_id", "id"].into_boxed_slice()].into_boxed_slice();
    let change = manager
        .plan_migration_to_entities_with_options(
            "v4",
            "change",
            &[Box::leak(Box::new(metadata.clone()))],
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(!change.steps.is_empty());
    manager
        .apply_migration(&change, ApplyOptions::default())
        .await?;
    sqlx::query("INSERT INTO unique_delta VALUES ('four','owner','name')")
        .execute(db.pool())
        .await?;
    let replan = manager
        .plan_migration_to_entities_with_options(
            "v5",
            "same",
            &[Box::leak(Box::new(metadata.clone()))],
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(replan.steps.is_empty(), "{replan:?}");
    let rows: Vec<String> = sqlx::query_scalar("SELECT id FROM unique_delta ORDER BY id")
        .fetch_all(db.pool())
        .await?;
    assert_eq!(rows, ["four", "one", "three", "two"]);
    #[cfg(feature = "sqlite")]
    {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(count, 2);
        let parent: String = sqlx::query_scalar("SELECT parent_id FROM child")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(parent, "one");
        let index: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name='preserved_index'")
                .fetch_one(db.pool())
                .await?;
        assert_eq!(index, 1);
        let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(enabled, 1);
    }
    #[cfg(feature = "postgres")]
    {
        db.pool().close().await;
        server.cleanup()?;
    }
    Ok(())
}

#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;

// MSSQL has no MigrationBackend implementation. Exercise the rendered DDL on
// its test-owned disposable server without implying managed apply support.
#[cfg(feature = "mssql")]
#[tokio::test]
async fn mssql_composite_constraint_ddl_and_introspection() -> Result<(), Box<dyn std::error::Error>>
{
    let mut server = owned_mssql::OwnedSqlServer::start().await?;
    let db = Database::<MssqlBackend>::connect_ado(&server.connection_string).await?;
    owned_mssql::execute_batch(&server.connection_string,
        "CREATE TABLE dbo.unique_delta (id nvarchar(100) NOT NULL PRIMARY KEY, owner_id nvarchar(100) NOT NULL, name nvarchar(100) NOT NULL, UNIQUE(owner_id,name)); INSERT INTO dbo.unique_delta VALUES ('one','o','n'),('two','other','n');"
    ).await?;
    // Unlike PostgreSQL/SQLite, SQL Server keeps a redundant UNIQUE over
    // the PK. Its introspection must round-trip and removal must stay visible.
    owned_mssql::execute_batch(&server.connection_string,
        "CREATE TABLE dbo.primary_unique (partition nvarchar(100) NOT NULL, name nvarchar(100) NOT NULL, created_at bigint, PRIMARY KEY(partition,name), UNIQUE(partition,name));"
    ).await?;
    let physical = introspect_mssql_schema(db.pool()).await?;
    let mut target = physical.clone();
    let table = target
        .tables
        .iter_mut()
        .find(|t| t.table_name.contains("primary_unique"))
        .unwrap();
    assert_eq!(
        table.composite_unique_indexes,
        vec![vec!["[partition]", "[name]"]]
    );
    table.composite_unique_indexes = vec![vec!["partition".into(), "name".into()]];
    assert!(
        diff_schema_models_for_backend(DatabaseBackend::Mssql, &physical, &target)
            .steps
            .is_empty()
    );
    target
        .tables
        .iter_mut()
        .find(|t| t.table_name.contains("primary_unique"))
        .unwrap()
        .composite_unique_indexes
        .clear();
    assert!(matches!(
        diff_schema_models_for_backend(DatabaseBackend::Mssql, &physical, &target)
            .steps
            .as_slice(),
        [MigrationStep::SetCompositeUniques { .. }]
    ));
    let mut current = introspect_mssql_schema(db.pool()).await?;
    let index = current
        .tables
        .iter()
        .position(|t| t.table_name.contains("unique_delta"))
        .unwrap();
    assert_eq!(current.tables[index].composite_unique_indexes.len(), 1);
    assert!(!current.tables[index].columns.iter().any(|c| c.is_unique));
    let mut target = current.clone();
    target.tables[index].composite_unique_indexes.clear();
    for step in diff_schema_models_for_backend(DatabaseBackend::Mssql, &current, &target).steps {
        for sql in render_migration_step(DatabaseBackend::Mssql, &step) {
            owned_mssql::execute_batch(&server.connection_string, &sql).await?;
        }
    }
    owned_mssql::execute_batch(
        &server.connection_string,
        "INSERT INTO dbo.unique_delta VALUES ('three','o','n')",
    )
    .await?;
    assert!(
        introspect_mssql_schema(db.pool()).await?.tables[index]
            .composite_unique_indexes
            .is_empty()
    );
    let add = MigrationStep::SetCompositeUniques {
        table_name: current.tables[index].table_name.clone(),
        before: vec![],
        after: current.tables[index].composite_unique_indexes.clone(),
    };
    let sql = render_migration_step(DatabaseBackend::Mssql, &add).join("; ");
    assert!(
        owned_mssql::execute_batch(&server.connection_string, &sql)
            .await
            .is_err()
    );
    owned_mssql::execute_batch(
        &server.connection_string,
        "UPDATE dbo.unique_delta SET name='different' WHERE id='three'",
    )
    .await?;
    owned_mssql::execute_batch(&server.connection_string, &sql).await?;
    current = introspect_mssql_schema(db.pool()).await?;
    assert_eq!(current.tables[index].composite_unique_indexes.len(), 1);
    let change = MigrationStep::SetCompositeUniques {
        table_name: current.tables[index].table_name.clone(),
        before: current.tables[index].composite_unique_indexes.clone(),
        after: vec![vec!["[owner_id]".into(), "[id]".into()]],
    };
    for sql in render_migration_step(DatabaseBackend::Mssql, &change) {
        owned_mssql::execute_batch(&server.connection_string, &sql).await?;
    }
    owned_mssql::execute_batch(&server.connection_string, "INSERT INTO dbo.unique_delta VALUES ('four','o','n'); IF (SELECT COUNT(*) FROM dbo.unique_delta) <> 4 THROW 50000, 'row loss', 1;").await?;
    assert_eq!(
        introspect_mssql_schema(db.pool()).await?.tables[index].composite_unique_indexes,
        vec![vec!["[owner_id]", "[id]"]]
    );
    server.cleanup()?;
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_composite_rebuild_refuses_mixed_policy_changes()
-> Result<(), Box<dyn std::error::Error>> {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let mut metadata = Record::metadata().clone();
    metadata.append_only = true;
    let plan = db
        .schema()
        .plan_migration_to_entities_with_options(
            "initial",
            "append only",
            &[Box::leak(Box::new(metadata.clone()))],
            PlanOptions::managed_tables_only(),
        )
        .await?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    sqlx::query("INSERT INTO unique_delta VALUES ('one','o','n')")
        .execute(db.pool())
        .await?;
    metadata.append_only = false;
    metadata.composite_unique_indexes = Box::new([]);
    let plan = db
        .schema()
        .plan_migration_to_entities_with_options(
            "remove",
            "remove both",
            &[Box::leak(Box::new(metadata))],
            PlanOptions::managed_tables_only(),
        )
        .await?;
    let error = db
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("UnsupportedCapability"));
    assert!(
        sqlx::query("UPDATE unique_delta SET name='changed'")
            .execute(db.pool())
            .await
            .is_err()
    );
    Ok(())
}

/// #409 dependency shapes. Names deliberately make the old reverse-current
/// traversal visit `records` before `accounts`, although only the target
/// contains the dependency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// The issue reproduction: a new key on `accounts(owner_id,name)` and a
    /// new `records(owner_id,name) REFERENCES accounts(owner_id,name)`.
    Issue,
    /// The reproduction plus a self reference on `accounts`.
    SelfReference,
    /// Both tables gain a key and reference each other.
    Cycle,
}

const SHAPES: [Shape; 3] = [Shape::Issue, Shape::SelfReference, Shape::Cycle];

fn dependency_schemas(constraint: bool, shape: Shape) -> (SchemaModel, SchemaModel) {
    let mut before = SchemaModel::from_entities(&[Record::metadata()]);
    before.tables[0].table_name = "accounts".into();
    before.tables[0].composite_unique_indexes.clear();
    #[cfg(feature = "mssql")]
    for column in &mut before.tables[0].columns {
        column.sql_type = "nvarchar(100)".into();
    }
    let mut records = before.tables[0].clone();
    records.table_name = "records".into();
    before.tables.push(records);
    let mut after = before.clone();
    for (i, name) in ["accounts_key", "records_key"].into_iter().enumerate() {
        if i == 1 && shape != Shape::Cycle {
            continue;
        }
        if constraint {
            after.tables[i].composite_unique_indexes = vec![vec!["owner_id".into(), "name".into()]];
        } else {
            after.tables[i].indexes = vec![IndexDef::new(name, &["owner_id", "name"]).unique()];
        }
    }
    let fk = |target: &str| ForeignKeyModel {
        constraint_name: None,
        column_pairs: ["owner_id", "name"]
            .into_iter()
            .map(|name| ForeignKeyColumnPairModel {
                source_column: name.into(),
                target_column: name.into(),
            })
            .collect(),
        target_table: target.into(),
        is_multiple: false,
        on_delete: DeletePolicy::Restrict,
    };
    after.tables[1].foreign_keys.push(fk("accounts"));
    match shape {
        Shape::Issue => {}
        Shape::SelfReference => after.tables[0].foreign_keys.push(fk("accounts")),
        Shape::Cycle => after.tables[0].foreign_keys.push(fk("records")),
    }
    (before, after)
}

#[test]
fn foreign_keys_surround_key_changes_for_every_backend() {
    let empty = SchemaModel {
        tables: vec![],
        extensions: vec![],
    };
    for backend in [DatabaseBackend::Postgres, DatabaseBackend::Mssql] {
        for constraint in [false, true] {
            for shape in SHAPES {
                let (before, after) = dependency_schemas(constraint, shape);
                let add = diff_schema_models_for_backend(backend, &before, &after);
                let first_fk = add
                    .steps
                    .iter()
                    .position(|s| matches!(s, MigrationStep::AddForeignKey { .. }))
                    .unwrap();
                assert!(
                    add.steps[first_fk..]
                        .iter()
                        .all(|s| matches!(s, MigrationStep::AddForeignKey { .. })),
                    "{backend:?} {constraint} {shape:?}: {add:?}"
                );
                let drop = diff_schema_models_for_backend(backend, &after, &before);
                let last_fk = drop
                    .steps
                    .iter()
                    .rposition(|s| matches!(s, MigrationStep::DropForeignKey { .. }))
                    .unwrap();
                assert!(
                    drop.steps[..=last_fk]
                        .iter()
                        .all(|s| matches!(s, MigrationStep::DropForeignKey { .. })),
                    "{backend:?} {constraint} {shape:?}: {drop:?}"
                );
                assert!(
                    diff_schema_models_for_backend(backend, &after, &after)
                        .steps
                        .is_empty()
                );
                // New tables keep inline FKs whose key exists at CREATE TABLE.
                // Deferred: a self reference to an index created after its
                // table, and the first FK of a cycle among new tables.
                let created = diff_schema_models_for_backend(backend, &empty, &after);
                let deferred = created
                    .steps
                    .iter()
                    .filter(|s| matches!(s, MigrationStep::AddForeignKey { .. }))
                    .count();
                let inline = created
                    .steps
                    .iter()
                    .filter_map(|s| match s {
                        MigrationStep::CreateTable(t) => Some(t.foreign_keys.len()),
                        _ => None,
                    })
                    .sum::<usize>();
                let total = if shape == Shape::Issue { 1 } else { 2 };
                let expected = match shape {
                    Shape::Issue => 0,
                    Shape::SelfReference => usize::from(!constraint),
                    Shape::Cycle => 1,
                };
                assert_eq!(
                    (deferred, inline),
                    (expected, total - expected),
                    "{backend:?} {constraint} {shape:?}: {created:?}"
                );
                let first_fk = created
                    .steps
                    .iter()
                    .position(|s| matches!(s, MigrationStep::AddForeignKey { .. }))
                    .unwrap_or(created.steps.len());
                assert!(
                    created.steps[first_fk..]
                        .iter()
                        .all(|s| matches!(s, MigrationStep::AddForeignKey { .. }))
                );
            }
        }
    }
}

#[test]
fn creating_tables_with_available_keys_stays_additive() {
    let empty = SchemaModel {
        tables: vec![],
        extensions: vec![],
    };
    let (_, after) = dependency_schemas(true, Shape::Issue);
    for backend in [
        DatabaseBackend::Postgres,
        DatabaseBackend::Mssql,
        DatabaseBackend::Sqlite,
    ] {
        let plan = build_migration_plan(backend, &empty, &after);
        assert!(
            plan.steps
                .iter()
                .all(|s| matches!(s, MigrationStep::CreateTable(_))),
            "{backend:?}: {:?}",
            plan.steps
        );
        assert!(
            classify_migration_steps(&plan.steps)
                .iter()
                .all(|s| s.risk == MigrationRisk::Additive)
        );
    }
}

#[test]
fn sqlite_keeps_per_table_foreign_key_rebuild_steps() {
    // SQLite installs each rebuilt table's complete target FK set with
    // enforcement suspended and checks it after the final statement.
    let (before, after) = dependency_schemas(false, Shape::Cycle);
    let add = diff_schema_models_for_backend(DatabaseBackend::Sqlite, &before, &after);
    assert_eq!(
        add.steps
            .iter()
            .filter(|s| matches!(s, MigrationStep::AddForeignKey { .. }))
            .count(),
        2
    );
    let (_, created) = dependency_schemas(true, Shape::Cycle);
    let plan = diff_schema_models_for_backend(
        DatabaseBackend::Sqlite,
        &SchemaModel {
            tables: vec![],
            extensions: vec![],
        },
        &created,
    );
    assert!(
        plan.steps
            .iter()
            .all(|s| matches!(s, MigrationStep::CreateTable(t) if t.foreign_keys.len() == 1))
    );
}

/// Column-only changes to `accounts.owner_id`, a member of both cycle FKs:
/// it is referenced by `records -> accounts` and is the source of
/// `accounts -> records`.
fn member_column_changes() -> (SchemaModel, Vec<(&'static str, SchemaModel)>) {
    let (_, before) = dependency_schemas(false, Shape::Cycle);
    let change = |label, edit: fn(&mut ColumnModel)| {
        let mut changed = before.clone();
        edit(&mut changed.tables[0].columns[1]);
        assert_eq!(changed.tables[0].columns[1].name, "owner_id");
        (label, changed)
    };
    let changes = vec![
        change("nullability", |c| c.nullable = !c.nullable),
        change("default", |c| c.default = Some("'owner'".into())),
        change("type", |c| c.sql_type = "varchar(200)".into()),
    ];
    (before, changes)
}

fn foreign_key_steps(diff: &SchemaDiff) -> (usize, usize) {
    let count = |f: fn(&MigrationStep) -> bool| diff.steps.iter().filter(|s| f(s)).count();
    (
        count(|s| matches!(s, MigrationStep::DropForeignKey { .. })),
        count(|s| matches!(s, MigrationStep::AddForeignKey { .. })),
    )
}

#[test]
fn postgres_column_changes_keep_retained_foreign_keys_attached() {
    // PostgreSQL defaults and nullability do not interact with FKs, and a
    // one-sided type change (here a varchar widening on accounts.owner_id
    // only) stays metadata-only when binary coercible. The FK stays attached,
    // so no referencing rows are revalidated.
    let (before, changes) = member_column_changes();
    for (label, changed) in changes {
        let diff = diff_schema_models_for_backend(DatabaseBackend::Postgres, &before, &changed);
        assert!(
            matches!(diff.steps.as_slice(), [MigrationStep::AlterColumn { .. }]),
            "{label}: {diff:?}"
        );
    }
}

#[test]
fn postgres_paired_type_changes_detach_only_their_foreign_keys() {
    // Retyping both members of an FK pair would fail on the first ALTER
    // COLUMN TYPE (42804), so that FK is detached and re-added, which forces a
    // full revalidation of its referencing rows.
    let (_, before) = dependency_schemas(false, Shape::Cycle);
    let mut paired = before.clone();
    for table in &mut paired.tables {
        table.columns[1].sql_type = "varchar(200)".into();
    }
    let diff = diff_schema_models_for_backend(DatabaseBackend::Postgres, &before, &paired);
    // Both cycle FKs pair accounts.owner_id with records.owner_id.
    assert_eq!(foreign_key_steps(&diff), (2, 2), "{diff:?}");
    assert!(matches!(
        diff.steps.first(),
        Some(MigrationStep::DropForeignKey { .. })
    ));
    assert!(matches!(
        diff.steps.last(),
        Some(MigrationStep::AddForeignKey { .. })
    ));
    // Retyping two columns that are not paired with each other keeps the FK.
    let mut unpaired = before.clone();
    unpaired.tables[0].columns[1].sql_type = "varchar(200)".into();
    unpaired.tables[1].columns[2].sql_type = "varchar(200)".into();
    let diff = diff_schema_models_for_backend(DatabaseBackend::Postgres, &before, &unpaired);
    assert_eq!(foreign_key_steps(&diff), (0, 0), "{diff:?}");
}

/// `records.owner_id REFERENCES accounts.id`, with the given member types.
#[cfg(feature = "postgres")]
fn single_key_schema(referenced: &str, source: &str) -> SchemaModel {
    let (before, _) = dependency_schemas(false, Shape::Issue);
    let mut schema = before;
    for column in &mut schema.tables[0].columns {
        if column.name == "id" {
            column.sql_type = referenced.into();
        }
    }
    for column in &mut schema.tables[1].columns {
        if column.name == "owner_id" {
            column.sql_type = source.into();
        }
    }
    schema.tables[1].foreign_keys.push(ForeignKeyModel {
        constraint_name: None,
        column_pairs: vec![ForeignKeyColumnPairModel {
            source_column: "owner_id".into(),
            target_column: "id".into(),
        }],
        target_table: "accounts".into(),
        is_multiple: false,
        on_delete: DeletePolicy::Restrict,
    });
    schema
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_paired_fk_type_change_detaches_and_applies()
-> Result<(), Box<dyn std::error::Error>> {
    // With the FK attached, the first ALTER COLUMN TYPE re-adds it and fails
    // with 42804 (incompatible types: text and uuid).
    let mut server = owned_postgres::OwnedPostgres::start("fk-paired-type")?;
    let db = Database::<PostgresBackend>::connect_postgres(&server.url).await?;
    let (uuid, text) = (
        single_key_schema("uuid", "uuid"),
        single_key_schema("text", "text"),
    );
    let parent = "7c1a1b9e-3f4d-4a43-9a51-2a0f5b1c6d01";
    for (label, target) in [("create", &uuid), ("to-text", &text)] {
        let current = introspect_postgres_schema(db.pool()).await?;
        let plan = db.schema().plan_migration_with_options(
            format!("paired-{label}"),
            label,
            &current,
            target,
            PlanOptions::managed_tables_only(),
        )?;
        if label == "to-text" {
            let steps = plan.steps.iter().map(|s| &s.step).collect::<Vec<_>>();
            assert!(
                matches!(
                    steps.as_slice(),
                    [
                        MigrationStep::DropForeignKey { .. },
                        MigrationStep::AlterColumn { .. },
                        MigrationStep::AlterColumn { .. },
                        MigrationStep::AddForeignKey { .. },
                    ]
                ),
                "{plan:?}"
            );
        }
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
        if label == "create" {
            sqlx::query(&format!(
                "INSERT INTO accounts VALUES ('{parent}', 'owner', 'name')"
            ))
            .execute(db.pool())
            .await?;
            sqlx::query(&format!(
                "INSERT INTO records VALUES ('one', '{parent}', 'name')"
            ))
            .execute(db.pool())
            .await?;
        }
        let physical = introspect_postgres_schema(db.pool()).await?;
        let same = db.schema().plan_migration_with_options(
            "same",
            "same",
            &physical,
            target,
            PlanOptions::managed_tables_only(),
        )?;
        assert!(same.steps.is_empty(), "{label}: {same:?}");
    }
    // The re-added FK still enforces the converted key.
    assert_foreign_key_violation(
        db.pool(),
        "INSERT INTO records VALUES ('two', 'missing', 'name')",
    )
    .await;
    db.pool().close().await;
    server.cleanup()?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn assert_foreign_key_violation(pool: &sqlx::PgPool, sql: &str) {
    let error = sqlx::query(sql)
        .execute(pool)
        .await
        .expect_err("the FK must reject an orphan row");
    assert_eq!(
        error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23503"),
        "{error:?}"
    );
}

#[cfg(feature = "postgres")]
async fn relfilenode(pool: &sqlx::PgPool, table: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT relfilenode::bigint FROM pg_class WHERE oid = $1::regclass")
        .bind(table)
        .fetch_one(pool)
        .await
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_one_sided_widening_keeps_foreign_key_attached()
-> Result<(), Box<dyn std::error::Error>> {
    // A managed target requires equal FK member types, so a one-sided type
    // change is the alignment of a live schema whose members differ. A
    // binary-coercible widening is metadata-only: no table rewrite and no FK
    // recheck. Detaching would scan every referencing row under SHARE ROW
    // EXCLUSIVE locks on both tables.
    let mut server = owned_postgres::OwnedPostgres::start("fk-one-sided")?;
    let db = Database::<PostgresBackend>::connect_postgres(&server.url).await?;
    let empty = SchemaModel {
        tables: vec![],
        extensions: vec![],
    };
    // (widened table, live referenced/source types, aligned type).
    let cases = [
        (
            "accounts",
            ("character varying(50)", "character varying(100)"),
            "character varying(100)",
        ),
        ("records", ("text", "character varying(50)"), "text"),
    ];
    for (widened, (referenced, source), aligned) in cases {
        // The legacy renderer does not validate member types, so it can
        // reproduce a mismatched live schema.
        let live = build_migration_plan(
            DatabaseBackend::Postgres,
            &empty,
            &single_key_schema(referenced, source),
        );
        for statement in &live.statements {
            sqlx::query(statement).execute(db.pool()).await?;
        }
        sqlx::query("INSERT INTO accounts VALUES ('parent', 'owner', 'name')")
            .execute(db.pool())
            .await?;
        sqlx::query("INSERT INTO records VALUES ('one', 'parent', 'name')")
            .execute(db.pool())
            .await?;
        let target = single_key_schema(aligned, aligned);
        let current = introspect_postgres_schema(db.pool()).await?;
        let plan = db.schema().plan_migration_with_options(
            format!("one-sided-{widened}"),
            "one-sided",
            &current,
            &target,
            PlanOptions::managed_tables_only(),
        )?;
        assert!(
            matches!(plan.steps.as_slice(), [step] if matches!(
                &step.step,
                MigrationStep::AlterColumn { table_name, .. } if table_name == widened
            )),
            "{widened}: {plan:?}"
        );
        let before = relfilenode(db.pool(), widened).await?;
        db.schema()
            .apply_migration(
                &plan,
                ApplyOptions {
                    require_clean_schema: false,
                    ..Default::default()
                },
            )
            .await?;
        assert_eq!(
            relfilenode(db.pool(), widened).await?,
            before,
            "{widened} was rewritten"
        );
        let physical = introspect_postgres_schema(db.pool()).await?;
        let same = db.schema().plan_migration_with_options(
            "same",
            "same",
            &physical,
            &target,
            PlanOptions::managed_tables_only(),
        )?;
        assert!(same.steps.is_empty(), "{widened}: {same:?}");
        assert_foreign_key_violation(
            db.pool(),
            "INSERT INTO records VALUES ('two', 'missing', 'name')",
        )
        .await;
        sqlx::query("DROP TABLE records, accounts")
            .execute(db.pool())
            .await?;
    }
    db.pool().close().await;
    server.cleanup()?;
    Ok(())
}

#[test]
fn mssql_column_changes_detach_and_reattach_dependent_foreign_keys() {
    // SQL Server refuses ALTER COLUMN on a column that an FK references or uses.
    let (before, changes) = member_column_changes();
    for (label, changed) in changes {
        let diff = diff_schema_models_for_backend(DatabaseBackend::Mssql, &before, &changed);
        assert_eq!(foreign_key_steps(&diff), (2, 2), "{label}: {diff:?}");
        assert!(
            matches!(
                diff.steps.as_slice(),
                [
                    MigrationStep::DropForeignKey { .. },
                    MigrationStep::DropForeignKey { .. },
                    MigrationStep::AlterColumn { .. },
                    MigrationStep::AddForeignKey { .. },
                    MigrationStep::AddForeignKey { .. },
                ]
            ),
            "{label}: {diff:?}"
        );
    }
    // A column outside every FK never detaches, on either backend.
    let (_, before) = dependency_schemas(false, Shape::Cycle);
    let mut after = before.clone();
    after.tables[0].columns.push(ColumnModel {
        name: "note".into(),
        sql_type: "TEXT".into(),
        spatial: None,
        nullable: true,
        is_primary_key: false,
        is_unique: false,
        default: None,
    });
    let mut altered = after.clone();
    altered.tables[0].columns.last_mut().unwrap().nullable = false;
    for backend in [DatabaseBackend::Postgres, DatabaseBackend::Mssql] {
        let diff = diff_schema_models_for_backend(backend, &after, &altered);
        assert!(
            matches!(diff.steps.as_slice(), [MigrationStep::AlterColumn { .. }]),
            "{backend:?}: {diff:?}"
        );
    }
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_member_column_change_applies_without_detaching_foreign_keys()
-> Result<(), Box<dyn std::error::Error>> {
    let mut server = owned_postgres::OwnedPostgres::start("fk-member-alter")?;
    let db = Database::<PostgresBackend>::connect_postgres(&server.url).await?;
    // accounts.owner_id is referenced by both FKs and is the source of the
    // self reference; populated rows insert parent-first.
    let (_, before) = dependency_schemas(false, Shape::SelfReference);
    let mut changed = before.clone();
    changed.tables[0].columns[1].nullable = !changed.tables[0].columns[1].nullable;
    for (label, target) in [
        ("create", &before),
        ("alter", &changed),
        ("restore", &before),
    ] {
        let current = introspect_postgres_schema(db.pool()).await?;
        let plan = db.schema().plan_migration_with_options(
            format!("member-{label}"),
            label,
            &current,
            target,
            PlanOptions::managed_tables_only(),
        )?;
        if label != "create" {
            assert!(
                matches!(plan.steps.as_slice(), [step] if matches!(step.step, MigrationStep::AlterColumn { .. })),
                "{label}: {plan:?}"
            );
        }
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
        if label == "create" {
            for table in ["accounts", "records"] {
                sqlx::query(&format!(
                    "INSERT INTO {table} VALUES ('one','owner','name')"
                ))
                .execute(db.pool())
                .await?;
            }
        }
        let physical = introspect_postgres_schema(db.pool()).await?;
        let same = db.schema().plan_migration_with_options(
            "same",
            "same",
            &physical,
            target,
            PlanOptions::managed_tables_only(),
        )?;
        assert!(same.steps.is_empty(), "{label}: {same:?}");
    }
    db.pool().close().await;
    server.cleanup()?;
    Ok(())
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_exclusion_constraint_index_is_not_an_ordinary_index()
-> Result<(), Box<dyn std::error::Error>> {
    // An EXCLUDE constraint owns its backing index. Introspecting that index as
    // ordinary would plan a DropIndex that PostgreSQL refuses ("constraint
    // ... requires it"), blocking every later migration.
    let mut server = owned_postgres::OwnedPostgres::start("fk-exclusion")?;
    let db = Database::<PostgresBackend>::connect_postgres(&server.url).await?;
    let (_, target) = dependency_schemas(false, Shape::Issue);
    let current = introspect_postgres_schema(db.pool()).await?;
    let plan = db.schema().plan_migration_with_options(
        "exclusion-create",
        "create",
        &current,
        &target,
        PlanOptions::managed_tables_only(),
    )?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    sqlx::query(
        "ALTER TABLE accounts ADD CONSTRAINT accounts_name_exclusive \
         EXCLUDE USING btree (name WITH =)",
    )
    .execute(db.pool())
    .await?;
    let physical = introspect_postgres_schema(db.pool()).await?;
    let accounts = physical
        .tables
        .iter()
        .find(|t| t.table_name == "accounts")
        .unwrap();
    assert!(
        accounts
            .indexes
            .iter()
            .all(|index| index.name != "accounts_name_exclusive"),
        "{:?}",
        accounts.indexes
    );
    // The FK-referenced ordinary unique index remains visible.
    assert!(
        accounts
            .indexes
            .iter()
            .any(|index| index.name == "accounts_key")
    );
    let same = db.schema().plan_migration_with_options(
        "same",
        "same",
        &physical,
        &target,
        PlanOptions::managed_tables_only(),
    )?;
    assert!(same.steps.is_empty(), "{same:?}");
    db.pool().close().await;
    server.cleanup()?;
    Ok(())
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_fk_target_keys_apply_drop_and_replan() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = owned_postgres::OwnedPostgres::start("fk-key-order")?;
    let db = Database::<PostgresBackend>::connect_postgres(&server.url).await?;
    for constraint in [false, true] {
        for shape in SHAPES {
            let (before, after) = dependency_schemas(constraint, shape);
            for (label, target) in [("initial", &before), ("add", &after), ("drop", &before)] {
                let current = introspect_postgres_schema(db.pool()).await?;
                let plan = db.schema().plan_migration_with_options(
                    format!("{constraint}-{shape:?}-{label}"),
                    label,
                    &current,
                    target,
                    PlanOptions::managed_tables_only(),
                )?;
                db.schema()
                    .apply_migration(
                        &plan,
                        ApplyOptions {
                            allow_destructive: true,
                            ..Default::default()
                        },
                    )
                    .await?;
                if label == "initial" {
                    for table in ["accounts", "records"] {
                        sqlx::query(&format!(
                            "INSERT INTO {table} VALUES ('one','owner','name')"
                        ))
                        .execute(db.pool())
                        .await?;
                    }
                }
                let physical = introspect_postgres_schema(db.pool()).await?;
                let same = db.schema().plan_migration_with_options(
                    "same",
                    "same",
                    &physical,
                    target,
                    PlanOptions::managed_tables_only(),
                )?;
                assert!(same.steps.is_empty(), "{same:?}");
            }
            // A dangling source row must roll back all newly established keys,
            // not leave a partially applied structural phase behind.
            sqlx::query("INSERT INTO records VALUES ('dangling','missing','key')")
                .execute(db.pool())
                .await?;
            let physical = introspect_postgres_schema(db.pool()).await?;
            let failed = db.schema().plan_migration_with_options(
                format!("{constraint}-{shape:?}-refused"),
                "refused",
                &physical,
                &after,
                PlanOptions::managed_tables_only(),
            )?;
            assert!(
                db.schema()
                    .apply_migration(&failed, ApplyOptions::default())
                    .await
                    .is_err()
            );
            let physical = introspect_postgres_schema(db.pool()).await?;
            let unchanged = db.schema().plan_migration_with_options(
                "unchanged",
                "unchanged",
                &physical,
                &before,
                PlanOptions::managed_tables_only(),
            )?;
            assert!(unchanged.steps.is_empty(), "{unchanged:?}");
            let recorded: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM __graphql_orm_migrations WHERE description='refused'",
            )
            .fetch_one(db.pool())
            .await?;
            assert_eq!(recorded, 0);
            sqlx::query("DROP TABLE records, accounts")
                .execute(db.pool())
                .await?;
            // New tables need the same deferred attachment, even in a cycle.
            let current = introspect_postgres_schema(db.pool()).await?;
            let created = db.schema().plan_migration_with_options(
                format!("{constraint}-{shape:?}-create"),
                "create",
                &current,
                &after,
                PlanOptions::managed_tables_only(),
            )?;
            db.schema()
                .apply_migration(&created, ApplyOptions::default())
                .await?;
            if !constraint {
                let mut replacement = after.clone();
                replacement.tables[0].indexes[0].name = "replacement_accounts_key";
                let current = introspect_postgres_schema(db.pool()).await?;
                let replaced = db.schema().plan_migration_with_options(
                    format!("{constraint}-{shape:?}-replace"),
                    "replace",
                    &current,
                    &replacement,
                    PlanOptions::managed_tables_only(),
                )?;
                db.schema()
                    .apply_migration(
                        &replaced,
                        ApplyOptions {
                            allow_destructive: true,
                            ..Default::default()
                        },
                    )
                    .await?;
                let current = introspect_postgres_schema(db.pool()).await?;
                assert!(
                    db.schema()
                        .plan_migration_with_options(
                            "same",
                            "same",
                            &current,
                            &replacement,
                            PlanOptions::managed_tables_only()
                        )?
                        .steps
                        .is_empty()
                );
            }
            sqlx::query("DROP TABLE records, accounts")
                .execute(db.pool())
                .await?;
        }
    }
    db.pool().close().await;
    server.cleanup()?;
    Ok(())
}

#[cfg(feature = "mssql")]
#[tokio::test]
async fn mssql_fk_target_keys_including_cycles() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = owned_mssql::OwnedSqlServer::start().await?;
    for constraint in [false, true] {
        for shape in SHAPES {
            let (before, after) = dependency_schemas(constraint, shape);
            let empty = SchemaModel {
                tables: vec![],
                extensions: vec![],
            };
            for (current, target) in [
                (&empty, &before),
                (&before, &after),
                (&after, &before),
                (&before, &empty),
            ] {
                let plan = build_migration_plan(DatabaseBackend::Mssql, current, target);
                // #537: normalize the unsupported delete-action spelling.
                // #546: isolate batch-local variables per rendered statement,
                // while retaining one transaction for the complete plan.
                let statements = plan
                    .statements
                    .iter()
                    .map(|sql| {
                        format!(
                            "EXEC(N'{}')",
                            sql.replace("ON DELETE RESTRICT", "ON DELETE NO ACTION")
                                .replace('\'', "''")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                let sql = format!("BEGIN TRANSACTION; {statements}; COMMIT TRANSACTION;");
                owned_mssql::execute_batch(&server.connection_string, &sql).await?;
            }
        }
    }
    server.cleanup()?;
    Ok(())
}

#[test]
fn retained_foreign_keys_detach_when_their_index_is_replaced() {
    let (_, before) = dependency_schemas(false, Shape::Cycle);
    let mut after = before.clone();
    // Renaming the referenced unique index requires detaching even a semantically
    // unchanged FK: PostgreSQL tracks its dependency on the physical index.
    after.tables[0].indexes[0].name = "replacement_accounts_key";
    for backend in [DatabaseBackend::Postgres, DatabaseBackend::Mssql] {
        let diff = diff_schema_models_for_backend(backend, &before, &after);
        assert!(matches!(
            diff.steps.first(),
            Some(MigrationStep::DropForeignKey { .. })
        ));
        assert!(matches!(
            diff.steps.last(),
            Some(MigrationStep::AddForeignKey { .. })
        ));
    }
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_fk_key_rebuild_order_and_safe_refusal() -> Result<(), Box<dyn std::error::Error>> {
    for constraint in [false, true] {
        let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
        // The constraint case keeps the composite-unique-only rebuild of
        // `accounts` separate from FK rebuilds: mixed same-table changes
        // deliberately remain unsupported.
        let (before, after) = dependency_schemas(
            constraint,
            if constraint {
                Shape::Issue
            } else {
                Shape::Cycle
            },
        );
        for (label, target) in [("initial", &before), ("add", &after), ("drop", &before)] {
            let current = introspect_sqlite_schema(db.pool()).await?;
            let plan = db.schema().plan_migration_with_options(
                label,
                label,
                &current,
                target,
                PlanOptions::managed_tables_only(),
            )?;
            let applied = db
                .schema()
                .apply_migration(
                    &plan,
                    ApplyOptions {
                        allow_destructive: true,
                        ..Default::default()
                    },
                )
                .await;
            // #547: the constraint "drop" removes the child FK and then the
            // parent's composite UNIQUE it referenced. The preflight accepts
            // it because the child rebuild scheduled first removes the FK.
            applied?;
            let physical = introspect_sqlite_schema(db.pool()).await?;
            let same = db.schema().plan_migration_with_options(
                "same",
                "same",
                &physical,
                target,
                PlanOptions::managed_tables_only(),
            )?;
            assert!(same.steps.is_empty(), "{same:?}");
            let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
                .fetch_one(db.pool())
                .await?;
            assert_eq!(enabled, 1);
        }
    }
    Ok(())
}

/// `records(owner_id,name) REFERENCES accounts(owner_id,name)` over a composite
/// UNIQUE constraint on `accounts`, applied to an in-memory database with one
/// parent row and one referencing row. Returns the schema without the FK and
/// the key, and the schema with them.
#[cfg(feature = "sqlite")]
async fn sqlite_scheduled_removal_fixture()
-> Result<(Database<SqliteBackend>, SchemaModel, SchemaModel), Box<dyn std::error::Error>> {
    let db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let (without, with) = dependency_schemas(true, Shape::Issue);
    for (label, target) in [("initial", &without), ("add", &with)] {
        let current = introspect_sqlite_schema(db.pool()).await?;
        let plan = db.schema().plan_migration_with_options(
            label,
            label,
            &current,
            target,
            PlanOptions::managed_tables_only(),
        )?;
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
    }
    sqlx::query("INSERT INTO accounts VALUES ('parent', 'owner', 'name')")
        .execute(db.pool())
        .await?;
    sqlx::query("INSERT INTO records VALUES ('one', 'owner', 'name')")
        .execute(db.pool())
        .await?;
    Ok((db, without, with))
}

/// Assert the database still has the FK, the key, both rows and FK
/// enforcement, exactly as [`sqlite_scheduled_removal_fixture`] left them.
#[cfg(feature = "sqlite")]
async fn assert_sqlite_scheduled_removal_untouched(
    db: &Database<SqliteBackend>,
    with: &SchemaModel,
) -> Result<(), Box<dyn std::error::Error>> {
    let physical = introspect_sqlite_schema(db.pool()).await?;
    let same = db.schema().plan_migration_with_options(
        "same",
        "same",
        &physical,
        with,
        PlanOptions::managed_tables_only(),
    )?;
    assert!(same.steps.is_empty(), "{same:?}");
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id, owner_id, name FROM records")
            .fetch_all(db.pool())
            .await?;
    assert_eq!(rows, [("one".into(), "owner".into(), "name".into())]);
    let parents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(parents, 1);
    let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(enabled, 1);
    // The FK and the key it references still enforce.
    assert!(
        sqlx::query("INSERT INTO records VALUES ('orphan', 'missing', 'name')")
            .execute(db.pool())
            .await
            .is_err()
    );
    assert!(
        sqlx::query("INSERT INTO accounts VALUES ('duplicate', 'owner', 'name')")
            .execute(db.pool())
            .await
            .is_err()
    );
    let leftovers: i64 = sqlx::query_scalar(
        r"SELECT COUNT(*) FROM sqlite_master WHERE name LIKE '\_\_graphql\_orm\_%\_new' ESCAPE '\'",
    )
    .fetch_one(db.pool())
    .await?;
    assert_eq!(leftovers, 0);
    Ok(())
}

/// Whether `sql` is the rebuild directive of `table`.
#[cfg(feature = "sqlite")]
fn sqlite_rebuilds(sql: &str, table: &str) -> bool {
    sql.starts_with("SELECT graphql_orm_rebuild") && sql.contains(&format!("('[\"{table}\","))
}

#[cfg(feature = "sqlite")]
async fn sqlite_scheduled_removal_plan(
    db: &Database<SqliteBackend>,
    without: &SchemaModel,
) -> Result<PlannedMigration, Box<dyn std::error::Error>> {
    let current = introspect_sqlite_schema(db.pool()).await?;
    let plan = db.schema().plan_migration_with_options(
        "drop",
        "drop the child FK and the parent key",
        &current,
        without,
        PlanOptions::managed_tables_only(),
    )?;
    // The child rebuild that removes the FK is scheduled before the parent
    // rebuild that removes the key.
    let position = |table: &str| {
        plan.statements
            .iter()
            .position(|sql| sqlite_rebuilds(sql, table))
            .unwrap_or_else(|| panic!("no rebuild of {table}: {plan:?}"))
    };
    assert!(position("records") < position("accounts"), "{plan:?}");
    Ok(plan)
}

/// #547: a plan that removes a child FK and the parent composite UNIQUE it
/// referenced applies to populated tables and keeps every row.
#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_scheduled_fk_removal_with_parent_key_removal_preserves_rows()
-> Result<(), Box<dyn std::error::Error>> {
    let (db, without, _) = sqlite_scheduled_removal_fixture().await?;
    let plan = sqlite_scheduled_removal_plan(&db, &without).await?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let physical = introspect_sqlite_schema(db.pool()).await?;
    let same = db.schema().plan_migration_with_options(
        "same",
        "same",
        &physical,
        &without,
        PlanOptions::managed_tables_only(),
    )?;
    assert!(same.steps.is_empty(), "{same:?}");
    let records: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id, owner_id, name FROM records")
            .fetch_all(db.pool())
            .await?;
    assert_eq!(records, [("one".into(), "owner".into(), "name".into())]);
    let accounts: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id, owner_id, name FROM accounts")
            .fetch_all(db.pool())
            .await?;
    assert_eq!(accounts, [("parent".into(), "owner".into(), "name".into())]);
    // Both constraints are gone, and FK enforcement is back on.
    let foreign_keys: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_list('records')")
            .fetch_one(db.pool())
            .await?;
    assert_eq!(foreign_keys, 0);
    sqlx::query("INSERT INTO accounts VALUES ('duplicate', 'owner', 'name')")
        .execute(db.pool())
        .await?;
    let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(enabled, 1);
    Ok(())
}

/// #547: when a later statement of the same plan fails, the child and parent
/// rebuilds roll back with it.
#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_scheduled_fk_removal_rolls_back_with_a_failed_plan()
-> Result<(), Box<dyn std::error::Error>> {
    let (db, without, with) = sqlite_scheduled_removal_fixture().await?;
    let mut plan = sqlite_scheduled_removal_plan(&db, &without).await?;
    let restore = plan
        .statements
        .iter()
        .position(|sql| sql == "PRAGMA foreign_keys = ON")
        .unwrap_or(plan.statements.len());
    plan.statements.insert(
        restore,
        "INSERT INTO scheduled_removal_missing_table VALUES (1)".into(),
    );
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect_err("the failing statement must fail the plan");
    assert_sqlite_scheduled_removal_untouched(&db, &with).await
}

/// #547: the refusal stays wherever the plan does not itself remove the FK
/// before the key: a parent rebuild scheduled first, and an FK on a table the
/// plan does not rebuild. Neither changes the schema or the rows.
#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_parent_key_removal_is_still_refused_without_a_scheduled_fk_removal()
-> Result<(), Box<dyn std::error::Error>> {
    // The parent rebuild runs before the child rebuild.
    let (db, without, with) = sqlite_scheduled_removal_fixture().await?;
    let mut plan = sqlite_scheduled_removal_plan(&db, &without).await?;
    let rebuild = |plan: &PlannedMigration, table: &str| {
        plan.statements
            .iter()
            .position(|sql| sqlite_rebuilds(sql, table))
            .unwrap()
    };
    let (child, parent) = (rebuild(&plan, "records"), rebuild(&plan, "accounts"));
    plan.statements.swap(child, parent);
    let error = db
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("UnsupportedCapability"),
        "{error}"
    );
    assert_sqlite_scheduled_removal_untouched(&db, &with).await?;

    // Only the parent key is removed; the child keeps its FK. Planning
    // already refuses a target whose FK references no unique key.
    let mut parent_only = with.clone();
    parent_only.tables[0].composite_unique_indexes.clear();
    let current = introspect_sqlite_schema(db.pool()).await?;
    let error = db
        .schema()
        .plan_migration_with_options(
            "parent-only",
            "parent only",
            &current,
            &parent_only,
            PlanOptions::managed_tables_only(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("exact unique key"), "{error}");

    // A second, unmanaged table also references the key. The plan removes
    // only the managed child's FK, so the key must stay.
    sqlx::query(
        "CREATE TABLE host_notes (id TEXT PRIMARY KEY NOT NULL, owner_id TEXT NOT NULL, name TEXT NOT NULL, \
         FOREIGN KEY (owner_id, name) REFERENCES accounts (owner_id, name))",
    )
    .execute(db.pool())
    .await?;
    let plan = sqlite_scheduled_removal_plan(&db, &without).await?;
    let error = db
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("UnsupportedCapability"),
        "{error}"
    );
    sqlx::query("DROP TABLE host_notes")
        .execute(db.pool())
        .await?;
    assert_sqlite_scheduled_removal_untouched(&db, &with).await
}

/// Both members of `records.owner_id REFERENCES accounts.owner_id` with the
/// given type, over a unique index on the referenced column.
fn paired_member_schema(member_type: &str) -> SchemaModel {
    let (mut schema, _) = dependency_schemas(false, Shape::Issue);
    for table in &mut schema.tables {
        assert_eq!(table.columns[1].name, "owner_id");
        table.columns[1].sql_type = member_type.into();
    }
    schema.tables[0].indexes = vec![IndexDef::new("accounts_owner_key", &["owner_id"]).unique()];
    schema.tables[1].foreign_keys.push(ForeignKeyModel {
        constraint_name: None,
        column_pairs: vec![ForeignKeyColumnPairModel {
            source_column: "owner_id".into(),
            target_column: "owner_id".into(),
        }],
        target_table: "accounts".into(),
        is_multiple: false,
        on_delete: DeletePolicy::Restrict,
    });
    schema
}

/// #561: a model may declare any alias of the type name PostgreSQL reports.
#[test]
fn postgres_type_aliases_are_the_same_type() {
    // (declared alias, spelling `format_type` reports)
    let aliases = [
        ("varchar(50)", "character varying(50)"),
        ("VARCHAR", "character varying"),
        ("char(3)", "character(3)"),
        ("char", "character(1)"),
        ("int", "integer"),
        ("INT4", "integer"),
        ("int8", "bigint"),
        ("int2", "smallint"),
        ("bool", "boolean"),
        ("decimal(10,2)", "numeric(10,2)"),
        ("numeric(10, 2)", "numeric(10,2)"),
        ("numeric(10)", "numeric(10,0)"),
        ("decimal", "numeric"),
        ("float8", "double precision"),
        ("float", "double precision"),
        ("float(24)", "real"),
        ("float4", "real"),
        ("timestamptz", "timestamp with time zone"),
        ("timestamptz(3)", "timestamp(3) with time zone"),
        ("timestamp", "timestamp without time zone"),
        ("timetz", "time with time zone"),
        ("time(6)", "time(6) without time zone"),
        ("varbit(8)", "bit varying(8)"),
        ("int[]", "integer[]"),
        ("varchar(50)[]", "character varying(50)[]"),
    ];
    for (declared, live) in aliases {
        let current = paired_member_schema(live);
        let target = paired_member_schema(declared);
        // The FK member-type check accepts the alias on both sides and mixed.
        target.validate_physical_contract().unwrap();
        let mut mixed = target.clone();
        mixed.tables[0].columns[1].sql_type = live.into();
        mixed
            .validate_physical_contract()
            .unwrap_or_else(|error| panic!("{declared} / {live}: {error}"));
        let diff = diff_schema_models_for_backend(DatabaseBackend::Postgres, &current, &target);
        assert!(diff.steps.is_empty(), "{declared} / {live}: {diff:?}");
    }
    // Different types still differ, including a different modifier of one
    // base type and a quoted name that only looks like an alias.
    let different = [
        ("varchar(50)", "character varying(60)"),
        ("varchar", "character varying(50)"),
        ("int", "bigint"),
        ("timestamp", "timestamp with time zone"),
        ("timestamptz(3)", "timestamp with time zone"),
        ("numeric(10,2)", "numeric(10,3)"),
        ("float(25)", "real"),
        ("int[]", "integer"),
        ("\"int\"", "integer"),
        ("text", "character varying"),
    ];
    for (declared, live) in different {
        let current = paired_member_schema(live);
        let target = paired_member_schema(declared);
        let diff = diff_schema_models_for_backend(DatabaseBackend::Postgres, &current, &target);
        assert!(!diff.steps.is_empty(), "{declared} / {live}");
    }
    // Alias folding is PostgreSQL's: SQL Server keeps comparing the spelling.
    let diff = diff_schema_models_for_backend(
        DatabaseBackend::Mssql,
        &paired_member_schema("character varying(50)"),
        &paired_member_schema("varchar(50)"),
    );
    assert!(!diff.steps.is_empty());
}

/// #566: a pair whose members both only widen within one base type keeps its
/// FK attached; every other paired type change still detaches it.
#[test]
fn postgres_paired_widenings_keep_their_foreign_keys() {
    let plan = |before: &str, after: (&str, &str)| {
        let current = paired_member_schema(before);
        let mut target = paired_member_schema(after.0);
        target.tables[1].columns[1].sql_type = after.1.into();
        diff_schema_models_for_backend(DatabaseBackend::Postgres, &current, &target)
    };
    for (before, after) in [
        ("character varying(50)", "varchar(100)"),
        ("varchar(50)", "character varying"),
        ("numeric(10,2)", "numeric(12,2)"),
        ("decimal(10,2)", "numeric"),
        ("bit varying(8)", "varbit(16)"),
    ] {
        let diff = plan(before, (after, after));
        assert!(
            matches!(
                diff.steps.as_slice(),
                [
                    MigrationStep::AlterColumn { .. },
                    MigrationStep::AlterColumn { .. }
                ]
            ),
            "{before} -> {after}: {diff:?}"
        );
    }
    for (before, after) in [
        // Narrowing, a changed scale, and a change of base type.
        ("character varying(100)", ("varchar(50)", "varchar(50)")),
        ("numeric(10,2)", ("numeric(12,4)", "numeric(12,4)")),
        ("numeric(10,2)", ("numeric(8,2)", "numeric(8,2)")),
        ("character varying(50)", ("text", "text")),
        ("character(5)", ("character(10)", "character(10)")),
        ("integer", ("bigint", "bigint")),
        // One member widens and the other changes base type.
        ("character varying(50)", ("varchar(100)", "text")),
    ] {
        let diff = plan(before, after);
        assert_eq!(
            foreign_key_steps(&diff),
            (1, 1),
            "{before} -> {after:?}: {diff:?}"
        );
        assert!(matches!(
            diff.steps.first(),
            Some(MigrationStep::DropForeignKey { .. })
        ));
        assert!(matches!(
            diff.steps.last(),
            Some(MigrationStep::AddForeignKey { .. })
        ));
    }
    // SQL Server refuses ALTER COLUMN on an FK member, so it detaches for
    // every change, widening included.
    let diff = diff_schema_models_for_backend(
        DatabaseBackend::Mssql,
        &paired_member_schema("nvarchar(50)"),
        &paired_member_schema("nvarchar(100)"),
    );
    assert_eq!(foreign_key_steps(&diff), (1, 1), "{diff:?}");
}

/// #561 and #566 on a live server: a schema declared with alias spellings
/// replans to nothing, and a paired widening applies with the FK attached and
/// without rewriting either table.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_alias_types_replan_empty_and_paired_widening_keeps_the_foreign_key()
-> Result<(), Box<dyn std::error::Error>> {
    let mut server = owned_postgres::OwnedPostgres::start("fk-alias-widening")?;
    let db = Database::<PostgresBackend>::connect_postgres(&server.url).await?;
    for (declared, widened, key) in [
        ("varchar(50)", "varchar(100)", "'owner'"),
        ("decimal(10,2)", "numeric(12,2)", "1.50"),
    ] {
        for (label, member_type) in [("create", declared), ("widen", widened)] {
            let target = paired_member_schema(member_type);
            let current = introspect_postgres_schema(db.pool()).await?;
            let plan = db.schema().plan_migration_with_options(
                format!("alias-{label}-{declared}"),
                label,
                &current,
                &target,
                PlanOptions::managed_tables_only(),
            )?;
            let mut files = Vec::new();
            if label == "widen" {
                let steps = plan.steps.iter().map(|s| &s.step).collect::<Vec<_>>();
                assert!(
                    matches!(
                        steps.as_slice(),
                        [
                            MigrationStep::AlterColumn { .. },
                            MigrationStep::AlterColumn { .. }
                        ]
                    ),
                    "{declared}: {plan:?}"
                );
                for table in ["accounts", "records"] {
                    files.push(relfilenode(db.pool(), table).await?);
                }
            }
            db.schema()
                .apply_migration(
                    &plan,
                    ApplyOptions {
                        require_clean_schema: false,
                        ..Default::default()
                    },
                )
                .await?;
            if label == "create" {
                sqlx::query(&format!(
                    "INSERT INTO accounts VALUES ('parent', {key}, 'name')"
                ))
                .execute(db.pool())
                .await?;
                sqlx::query(&format!(
                    "INSERT INTO records VALUES ('one', {key}, 'name')"
                ))
                .execute(db.pool())
                .await?;
            } else {
                for (table, before) in ["accounts", "records"].into_iter().zip(files) {
                    assert_eq!(
                        relfilenode(db.pool(), table).await?,
                        before,
                        "{declared}: {table} was rewritten"
                    );
                }
            }
            // #561: the live `format_type` spelling equals the declared alias.
            let physical = introspect_postgres_schema(db.pool()).await?;
            let same = db.schema().plan_migration_with_options(
                "same",
                "same",
                &physical,
                &target,
                PlanOptions::managed_tables_only(),
            )?;
            assert!(same.steps.is_empty(), "{declared} {label}: {same:?}");
        }
        // The FK that stayed attached still enforces.
        let orphan = if key.starts_with('\'') {
            "'missing'"
        } else {
            "9.75"
        };
        assert_foreign_key_violation(
            db.pool(),
            &format!("INSERT INTO records VALUES ('two', {orphan}, 'name')"),
        )
        .await;
        sqlx::query("DROP TABLE records, accounts")
            .execute(db.pool())
            .await?;
    }
    db.pool().close().await;
    server.cleanup()?;
    Ok(())
}

/// #598: dropping a table is `Destructive` on every backend. PostgreSQL and
/// SQL Server first drop the table's own foreign keys, a `Risky` step for the
/// same table, so the table's risk is the highest of its steps.
#[test]
fn dropping_a_table_with_foreign_keys_is_destructive_on_every_backend() {
    let (_, with) = dependency_schemas(false, Shape::Issue);
    let mut without = with.clone();
    without.tables.retain(|table| table.table_name != "records");
    for backend in [
        DatabaseBackend::Sqlite,
        DatabaseBackend::Postgres,
        DatabaseBackend::Mssql,
    ] {
        let diff = diff_schema_models_for_backend(backend, &with, &without);
        let classified = classify_migration_steps(&diff.steps);
        let drop = classified
            .iter()
            .find(|step| matches!(&step.step, MigrationStep::DropTable { table_name } if table_name == "records"))
            .unwrap_or_else(|| panic!("{backend:?}: {diff:?}"));
        assert_eq!(drop.risk, MigrationRisk::Destructive, "{backend:?}");
        assert_eq!(
            classified.iter().map(|step| step.risk).max(),
            Some(MigrationRisk::Destructive),
            "{backend:?}"
        );
        let detaches_first = matches!(
            classified.first().map(|step| &step.step),
            Some(MigrationStep::DropForeignKey { .. })
        );
        assert_eq!(
            detaches_first,
            backend != DatabaseBackend::Sqlite,
            "{backend:?}"
        );
    }
}
