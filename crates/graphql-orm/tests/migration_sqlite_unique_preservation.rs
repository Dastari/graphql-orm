#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
use graphql_orm::{graphql::orm::*, prelude::*};

async fn setup(sql: &[&str]) -> graphql_orm::Result<Database<SqliteBackend>> {
    let db = Database::<SqliteBackend>::connect_sqlite_with_options(
        "sqlite::memory:",
        graphql_orm::db::ConnectionOptions::default().max_connections(1),
    )
    .await?;
    for sql in sql {
        sqlx::query(sql).execute(db.pool()).await?;
    }
    Ok(db)
}
async fn remove(db: &Database<SqliteBackend>) -> graphql_orm::Result<SchemaModel> {
    let current = introspect_sqlite_schema(db.pool()).await?;
    let mut target = current.clone();
    let table = target
        .tables
        .iter_mut()
        .find(|t| t.table_name == "probe")
        .unwrap();
    table
        .composite_unique_indexes
        .retain(|key| key != &["a", "b"]);
    let plan = db
        .schema()
        .plan_migration("remove", "composite only", &current, &target)?;
    assert!(!plan.steps.is_empty());
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    clean(db, &target).await?;
    Ok(target)
}
async fn clean(db: &Database<SqliteBackend>, target: &SchemaModel) -> graphql_orm::Result<()> {
    let actual = introspect_sqlite_schema(db.pool()).await?;
    assert!(
        diff_schema_models_for_backend(DatabaseBackend::Sqlite, &actual, target)
            .steps
            .is_empty()
    );
    assert!(!db.schema().validate(&actual, target)?.has_errors());
    let plan = db
        .schema()
        .plan_migration("replan", "unchanged", &actual, target)?;
    assert!(plan.steps.is_empty(), "{plan:?}");
    Ok(())
}
#[tokio::test]
async fn stored_generated_columns_and_binary_values_survive() -> graphql_orm::Result<()> {
    let db=setup(&[
        "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,g TEXT GENERATED ALWAYS AS (a || ',' || b) STORED, CONSTRAINT old_uq UNIQUE(a,b))",
        "INSERT INTO probe(rowid,id,a,b) VALUES (99,'one','name','value')",
    ]).await?;
    let target = remove(&db).await?;
    let row: (i64, String, String) = sqlx::query_as("SELECT rowid,id,g FROM probe")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(row, (99, "one".into(), "name,value".into()));
    sqlx::query("INSERT INTO probe(id,a,b) VALUES ('two','name','value')")
        .execute(db.pool())
        .await?;
    clean(&db, &target).await
}
#[tokio::test]
async fn unrelated_collations_and_conflict_policies_survive() -> graphql_orm::Result<()> {
    let db=setup(&[
        "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,c TEXT COLLATE NOCASE,d TEXT, UNIQUE(a,b), CONSTRAINT retained UNIQUE(c,d) ON CONFLICT ABORT)",
        "INSERT INTO probe VALUES ('one','a','b','Name','x')",
    ]).await?;
    let target = remove(&db).await?;
    assert!(
        sqlx::query("INSERT INTO probe VALUES ('two','a','b','name','x')")
            .execute(db.pool())
            .await
            .is_err()
    );
    let sql: String = sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name='probe'")
        .fetch_one(db.pool())
        .await?;
    assert!(sql.contains("c TEXT COLLATE NOCASE"));
    assert!(sql.contains("CONSTRAINT retained UNIQUE(c,d) ON CONFLICT ABORT"));
    clean(&db, &target).await
}
#[tokio::test]
async fn autoincrement_deleted_high_water_mark_survives() -> graphql_orm::Result<()> {
    let db=setup(&[
        "CREATE TABLE probe(id INTEGER PRIMARY KEY AUTOINCREMENT,a TEXT NOT NULL,b TEXT NOT NULL, UNIQUE(a,b))",
        "INSERT INTO probe VALUES (1,'a','b'),(99,'c','d')",
        "DELETE FROM probe WHERE id=99",
    ]).await?;
    let target = remove(&db).await?;
    let seq: i64 = sqlx::query_scalar("SELECT seq FROM sqlite_sequence WHERE name='probe'")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(seq, 99);
    sqlx::query("INSERT INTO probe(a,b) VALUES ('a','b')")
        .execute(db.pool())
        .await?;
    let id: i64 = sqlx::query_scalar("SELECT MAX(id) FROM probe")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(id, 100);
    clean(&db, &target).await
}
#[tokio::test]
async fn text_primary_keys_keep_rowid_and_alias_shadowing_is_handled() -> graphql_orm::Result<()> {
    let db=setup(&[
        "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,rowid TEXT,a TEXT NOT NULL,b TEXT NOT NULL, UNIQUE(a,b))",
        "INSERT INTO probe(_rowid_,id,rowid,a,b) VALUES (99,'one','host-column','a','b')",
    ]).await?;
    let target = remove(&db).await?;
    let row: (i64, String) = sqlx::query_as("SELECT _rowid_,rowid FROM probe")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(row, (99, "host-column".into()));
    clean(&db, &target).await
}
#[tokio::test]
async fn foreign_key_on_update_and_deferrability_survive() -> graphql_orm::Result<()> {
    let db=setup(&[
        "CREATE TABLE parent(id TEXT PRIMARY KEY NOT NULL)",
        "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,p TEXT REFERENCES parent(id) ON UPDATE CASCADE ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED, UNIQUE(a,b))",
        "INSERT INTO parent VALUES ('old')",
        "INSERT INTO probe VALUES ('one','a','b','old')",
    ]).await?;
    let target = remove(&db).await?;
    let action: String =
        sqlx::query_scalar("SELECT on_update FROM pragma_foreign_key_list('probe')")
            .fetch_one(db.pool())
            .await?;
    assert_eq!(action, "CASCADE");
    sqlx::query("UPDATE parent SET id='new'")
        .execute(db.pool())
        .await?;
    let parent: String = sqlx::query_scalar("SELECT p FROM probe")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(parent, "new");
    let sql: String = sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name='probe'")
        .fetch_one(db.pool())
        .await?;
    assert!(sql.contains("DEFERRABLE INITIALLY DEFERRED"));
    clean(&db, &target).await
}
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "sqlite",
    table = "case_probe",
    schema_policy = "managed",
    unique_composite = "A,b"
)]
struct CaseProbe {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    a: String,
    b: String,
}
#[tokio::test]
async fn mixed_case_unique_identifiers_are_idempotent_and_order_is_retained()
-> graphql_orm::Result<()> {
    let db = setup(&[]).await?;
    let initial = db
        .schema()
        .plan_migration_to_entities("initial", "case", &[CaseProbe::metadata()])
        .await?;
    db.schema()
        .apply_migration(&initial, ApplyOptions::default())
        .await?;
    let target = SchemaModel::from_entities(&[CaseProbe::metadata()]);
    clean(&db, &target).await?;
    let actual = introspect_sqlite_schema(db.pool()).await?;
    assert_eq!(
        actual.tables[0].composite_unique_indexes,
        vec![vec!["a", "b"]]
    );
    let mut reversed = target.clone();
    reversed.tables[0].composite_unique_indexes = vec![vec!["b".into(), "A".into()]];
    let plan = db
        .schema()
        .plan_migration("reverse", "order", &actual, &reversed)?;
    assert!(!plan.steps.is_empty());
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    clean(&db, &reversed).await
}
#[tokio::test]
async fn unsupported_shapes_refuse_without_touching_schema_or_rows() -> graphql_orm::Result<()> {
    for ddl in [
        "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,UNIQUE(a,b)) WITHOUT ROWID",
        "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,rowid TEXT,_rowid_ TEXT,oid TEXT,a TEXT NOT NULL,b TEXT NOT NULL,UNIQUE(a,b))",
    ] {
        let db = setup(&[ddl]).await?;
        sqlx::query("INSERT INTO probe(id,a,b) VALUES ('one','a','b')")
            .execute(db.pool())
            .await?;
        let current = introspect_sqlite_schema(db.pool()).await?;
        let mut target = current.clone();
        target.tables[0].composite_unique_indexes.clear();
        let plan = db
            .schema()
            .plan_migration("refuse", "unsafe", &current, &target)?;
        let before: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(db.pool())
        .await?;
        let error = db
            .schema()
            .apply_migration(
                &plan,
                ApplyOptions {
                    record_history: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("UnsupportedCapability"),
            "{error}"
        );
        assert!(!error.to_string().contains("probe"));
        // Refusal precedes even migration-history preparation.
        let after: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(db.pool())
        .await?;
        assert_eq!(after, before);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM probe WHERE id='one'")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(count, 1);
    }
    Ok(())
}

#[tokio::test]
async fn virtual_and_shadow_tables_refuse_before_mutation() -> graphql_orm::Result<()> {
    for table in ["probe", "probe_data"] {
        let db = setup(&[
            "CREATE VIRTUAL TABLE probe USING fts5(id,a,b)",
            "INSERT INTO probe VALUES ('one','a','b')",
        ])
        .await?;
        // Model-only planning cannot identify a virtual/shadow source. Apply
        // must inspect the actual table type before executing any rebuild SQL.
        let mut current = SchemaModel::from_entities(&[CaseProbe::metadata()]);
        current.tables[0].table_name = table.into();
        current.tables[0].composite_unique_indexes.clear();
        let mut target = current.clone();
        target.tables[0].composite_unique_indexes = vec![vec!["a".into(), "b".into()]];
        let plan = db
            .schema()
            .plan_migration("refuse", "virtual", &current, &target)?;
        let before: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(db.pool())
        .await?;
        let error = db
            .schema()
            .apply_migration(
                &plan,
                ApplyOptions {
                    record_history: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("UnsupportedCapability"));
        let after: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(db.pool())
        .await?;
        assert_eq!(before, after);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM probe WHERE id='one'")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(count, 1);
    }
    Ok(())
}

#[tokio::test]
async fn unsupported_ddl_and_temporary_collisions_refuse_before_other_plan_writes()
-> graphql_orm::Result<()> {
    for extra in [
        "CREATE VIEW host_view AS SELECT a FROM probe",
        "CREATE TEMP VIEW host_view AS SELECT a FROM probe",
        "CREATE TEMP TRIGGER host_trigger AFTER INSERT ON probe BEGIN SELECT 1; END",
        "CREATE TABLE __graphql_orm_probe_new (host_data TEXT)",
    ] {
        let db=setup(&["CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,UNIQUE(a,b))", "INSERT INTO probe VALUES ('one','a','b')", extra]).await?;
        let current = introspect_sqlite_schema(db.pool()).await?;
        let mut target = current.clone();
        let index = target
            .tables
            .iter()
            .position(|t| t.table_name == "probe")
            .unwrap();
        target.tables[index].composite_unique_indexes.clear();
        let mut added = target.tables[index].clone();
        added.table_name = "new_table".into();
        target.tables.push(added);
        let plan = db
            .schema()
            .plan_migration("refuse", "preflight all", &current, &target)?;
        let error = db
            .schema()
            .apply_migration(
                &plan,
                ApplyOptions {
                    record_history: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("UnsupportedCapability"),
            "{error}"
        );
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name='new_table'")
                .fetch_one(db.pool())
                .await?;
        assert_eq!(count, 0);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM probe WHERE id='one'")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(count, 1);
    }
    Ok(())
}

#[test]
fn unique_case_folding_respects_backend_quoting_and_column_order() {
    let current = SchemaModel::from_entities(&[CaseProbe::metadata()]);
    let mut target = current.clone();
    target.tables[0].composite_unique_indexes = vec![vec!["a".into(), "b".into()]];
    // The current model represents introspected, physical names.
    let mut current = current;
    current.tables[0].composite_unique_indexes = vec![vec!["a".into(), "b".into()]];
    target.tables[0].composite_unique_indexes = vec![vec!["A".into(), "b".into()]];
    for backend in [DatabaseBackend::Sqlite, DatabaseBackend::Postgres] {
        assert!(
            diff_schema_models_for_backend(backend, &current, &target)
                .steps
                .is_empty()
        );
    }
    target.tables[0].composite_unique_indexes = vec![vec!["\"A\"".into(), "b".into()]];
    assert!(
        diff_schema_models_for_backend(DatabaseBackend::Sqlite, &current, &target)
            .steps
            .is_empty()
    );
    assert!(
        !diff_schema_models_for_backend(DatabaseBackend::Postgres, &current, &target)
            .steps
            .is_empty()
    );
    target.tables[0].composite_unique_indexes = vec![vec!["b".into(), "a".into()]];
    assert!(
        !diff_schema_models_for_backend(DatabaseBackend::Sqlite, &current, &target)
            .steps
            .is_empty()
    );
}

#[tokio::test]
async fn owned_migrations_use_the_same_lossless_directive() -> Result<(), Box<dyn std::error::Error>>
{
    let db=setup(&["CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,g TEXT GENERATED ALWAYS AS (a||b) STORED,UNIQUE(a,b))", "INSERT INTO probe(rowid,id,a,b) VALUES (99,'one','a','b')"]).await?;
    let mut target = introspect_sqlite_schema(db.pool()).await?;
    target.tables[0].composite_unique_indexes.clear();
    let owned = OwnedSchemaModel::from(&target);
    let ownership = ManagedTableSet::new(["probe".into()])?;
    let plan = db
        .schema()
        .plan_owned_migration(
            "owned",
            "preserve",
            &owned,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    db.schema()
        .apply_owned_migration(&plan, ApplyOptions::default())
        .await?;
    let row: (i64, String) = sqlx::query_as("SELECT rowid,g FROM probe")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(row, (99, "ab".into()));
    clean(&db, &target).await?;
    let again = db
        .schema()
        .plan_owned_migration(
            "owned-noop",
            "same",
            &owned,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    assert!(again.steps().is_empty());
    Ok(())
}
#[tokio::test]
async fn isolated_steps_are_safe_and_external_sql_execution_fails_closed() -> graphql_orm::Result<()>
{
    let db=setup(&["CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,UNIQUE(a,b))", "INSERT INTO probe(rowid,id,a,b) VALUES (99,'one','a','b')", "CREATE TABLE child(id TEXT PRIMARY KEY NOT NULL,p TEXT REFERENCES probe(id) ON DELETE CASCADE)", "INSERT INTO child VALUES ('child','one')"]).await?;
    let sql = render_migration_step(
        DatabaseBackend::Sqlite,
        &MigrationStep::SetCompositeUniques {
            table_name: "probe".into(),
            before: vec![vec!["a".into(), "b".into()]],
            after: vec![],
        },
    );
    assert!(sqlx::query(&sql[0]).execute(db.pool()).await.is_err());
    let current = introspect_sqlite_schema(db.pool()).await?;
    let mut target = current.clone();
    target
        .tables
        .iter_mut()
        .find(|t| t.table_name == "probe")
        .unwrap()
        .composite_unique_indexes
        .clear();
    let plan = db
        .schema()
        .plan_migration("isolated", "same", &current, &target)?;
    let mut plan = plan;
    plan.statements = sql;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let rowid: i64 = sqlx::query_scalar("SELECT rowid FROM probe")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(rowid, 99);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM child")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(count, 1);
    clean(&db, &target).await
}

#[tokio::test]
async fn quoted_constraint_members_comments_expressions_and_strict_survive()
-> graphql_orm::Result<()> {
    let db=setup(&[r#"CREATE TABLE "probe" ([id] TEXT PRIMARY KEY NOT NULL, [a] TEXT NOT NULL /* , ) UNIQUE(a,b) */, "b" TEXT NOT NULL, n INTEGER DEFAULT (3+4), g TEXT GENERATED ALWAYS AS (printf('(%s,%s)',a,b)) STORED, CONSTRAINT [legacy] UNIQUE ([a] COLLATE BINARY DESC, "b") ON CONFLICT FAIL) STRICT"#,
        "INSERT INTO probe(rowid,id,a,b) VALUES (99,'one','a','b')"]).await?;
    let target = remove(&db).await?;
    let row: (i64, i64, String) = sqlx::query_as("SELECT rowid,n,g FROM probe")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(row, (99, 7, "(a,b)".into()));
    let strict: i64 = sqlx::query_scalar("SELECT strict FROM pragma_table_list WHERE name='probe'")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(strict, 1);
    clean(&db, &target).await
}
#[tokio::test]
async fn migration_restores_an_initially_disabled_fk_pragma() -> graphql_orm::Result<()> {
    let db=setup(&["CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,UNIQUE(a,b))","INSERT INTO probe VALUES ('one','a','b')", "PRAGMA foreign_keys=OFF"]).await?;
    let target = remove(&db).await?;
    let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(enabled, 0);
    clean(&db, &target).await
}

#[tokio::test]
async fn unowned_incoming_fks_refuse_key_removal_but_allow_retained_keys() -> graphql_orm::Result<()>
{
    let db=setup(&["CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,UNIQUE(a,b))", "INSERT INTO probe VALUES ('one','a','b')", "CREATE TABLE child(id TEXT PRIMARY KEY NOT NULL,a TEXT,b TEXT,FOREIGN KEY(a,b) REFERENCES probe(a,b) ON UPDATE CASCADE)", "INSERT INTO child VALUES ('child','a','b')"]).await?;
    let current = introspect_sqlite_schema(db.pool()).await?;
    let mut target = current.clone();
    target.tables.retain(|t| t.table_name == "probe");
    target.tables[0].composite_unique_indexes.clear();
    let plan = db.schema().plan_migration_with_options(
        "refuse",
        "incoming FK",
        &current,
        &target,
        PlanOptions::managed_tables_only(),
    )?;
    let before: Vec<(String, String)> =
        sqlx::query_as("SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
            .fetch_all(db.pool())
            .await?;
    let error = db
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("UnsupportedCapability"));
    let after: Vec<(String, String)> =
        sqlx::query_as("SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
            .fetch_all(db.pool())
            .await?;
    assert_eq!(before, after);
    target.tables[0].composite_unique_indexes =
        vec![vec!["a".into(), "b".into()], vec!["b".into(), "a".into()]];
    let plan = db.schema().plan_migration_with_options(
        "reverse",
        "incoming FK intact",
        &current,
        &target,
        PlanOptions::managed_tables_only(),
    )?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let mut expected = current;
    expected
        .tables
        .iter_mut()
        .find(|t| t.table_name == "probe")
        .unwrap()
        .composite_unique_indexes = target.tables[0].composite_unique_indexes.clone();
    clean(&db, &expected).await?;
    sqlx::query("UPDATE probe SET a='new'")
        .execute(db.pool())
        .await?;
    let child: String = sqlx::query_scalar("SELECT a FROM child")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(child, "new");
    clean(&db, &expected).await
}

#[tokio::test]
async fn legacy_cleanup_is_not_introduced_into_owned_column_rebuilds()
-> Result<(), Box<dyn std::error::Error>> {
    let db=setup(&["CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,UNIQUE(a,b))","INSERT INTO probe VALUES ('one','a','b')"]).await?;
    let current = introspect_sqlite_schema(db.pool()).await?;
    let mut target = current.clone();
    target.tables[0]
        .columns
        .iter_mut()
        .find(|c| c.name == "a")
        .unwrap()
        .nullable = true;
    let owned = OwnedSchemaModel::from(&target);
    let ownership = ManagedTableSet::new(["probe".into()])?;
    let plan = db
        .schema()
        .plan_owned_migration(
            "owned-column",
            "existing safeguard",
            &owned,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    // A host object appears after planning. The original owned path must
    // continue to refuse it, rather than run legacy stale-name cleanup.
    sqlx::query("CREATE TABLE __graphql_orm_probe_new(host_data TEXT)")
        .execute(db.pool())
        .await?;
    sqlx::query("INSERT INTO __graphql_orm_probe_new VALUES ('precious')")
        .execute(db.pool())
        .await?;
    assert!(
        db.schema()
            .apply_owned_migration(&plan, ApplyOptions::default())
            .await
            .is_err()
    );
    let value: String = sqlx::query_scalar("SELECT host_data FROM __graphql_orm_probe_new")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(value, "precious");
    clean(&db, &current).await?;
    Ok(())
}

#[tokio::test]
async fn delimiter_shaped_catalog_names_refuse_without_copying_wrong_columns()
-> graphql_orm::Result<()> {
    let db=setup(&[r#"CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,g TEXT,"""g""" TEXT,UNIQUE(a,b))"#, "INSERT INTO probe VALUES ('one','a','b','normal','literal')"]).await?;
    // Model-only callers may not know about extra live fields. The directive
    // must not conflate raw column name `"g"` with SQL spelling for column g.
    let mut current = SchemaModel::from_entities(&[CaseProbe::metadata()]);
    current.tables[0].table_name = "probe".into();
    let mut target = current.clone();
    target.tables[0].composite_unique_indexes.clear();
    let plan = db
        .schema()
        .plan_migration("refuse", "ambiguous identifier", &current, &target)?;
    let before: Vec<(String, String)> =
        sqlx::query_as("SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
            .fetch_all(db.pool())
            .await?;
    let error = db
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("UnsupportedCapability"));
    let after: Vec<(String, String)> =
        sqlx::query_as("SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
            .fetch_all(db.pool())
            .await?;
    assert_eq!(before, after);
    let values: (String, String) = sqlx::query_as(r#"SELECT g,"""g""" FROM probe"#)
        .fetch_one(db.pool())
        .await?;
    assert_eq!(values, ("normal".into(), "literal".into()));
    Ok(())
}

#[tokio::test]
async fn case_variant_temp_rebuild_names_cannot_shadow_main_data() -> graphql_orm::Result<()> {
    for temporary in ["__GRAPHQL_ORM_PROBE_NEW", "probe", "PrObE"] {
        let ddl = format!(
            "CREATE TEMP TABLE {temporary}(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL)"
        );
        let db=setup(&["CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,UNIQUE(a,b))", "INSERT INTO probe(rowid,id,a,b) VALUES (99,'one','a','b')", &ddl]).await?;
        let current = introspect_sqlite_schema(db.pool()).await?;
        assert_eq!(
            current.tables[0].composite_unique_indexes,
            vec![vec!["a", "b"]],
            "introspection must read main, not TEMP"
        );
        let mut target = current.clone();
        target.tables[0].composite_unique_indexes.clear();
        let plan = db
            .schema()
            .plan_migration("refuse", "temp case shadow", &current, &target)?;
        let before: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM main.sqlite_master WHERE sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(db.pool())
        .await?;
        let error = db
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("UnsupportedCapability"));
        let after: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM main.sqlite_master WHERE sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(db.pool())
        .await?;
        assert_eq!(before, after);
        let value: (i64, String) = sqlx::query_as("SELECT rowid,id FROM main.probe")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(value, (99, "one".into()));
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM temp.{temporary}"))
            .fetch_one(db.pool())
            .await?;
        assert_eq!(count, 0);
    }
    Ok(())
}

#[tokio::test]
async fn diagnostic_preparation_preserves_host_tables_and_live_rebuild_refuses_collision()
-> graphql_orm::Result<()> {
    for reserved in ["__graphql_orm_probe_new", "__GRAPHQL_ORM_PROBE_NEW"] {
        let db = setup(&[
            "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,UNIQUE(a,b))",
            "INSERT INTO probe VALUES ('one','a','b')",
            "CREATE TABLE abgraphqlXorm_probe_new(host_data TEXT)",
            "INSERT INTO abgraphqlXorm_probe_new VALUES ('lookalike')",
        ]).await?;
        sqlx::query(&format!("CREATE TABLE {reserved}(host_data TEXT)"))
            .execute(db.pool())
            .await?;
        sqlx::query(&format!(
            "INSERT INTO {reserved} VALUES ('unproven host data')"
        ))
        .execute(db.pool())
        .await?;
        // #417 preparation diagnoses literal candidates and never deletes them.
        db.apply_migrations(&[]).await?;
        let error = remove(&db).await.unwrap_err();
        assert!(
            error.to_string().contains("UnsupportedCapability"),
            "{error}"
        );
        let host: String = sqlx::query_scalar(&format!("SELECT host_data FROM {reserved}"))
            .fetch_one(db.pool())
            .await?;
        assert_eq!(host, "unproven host data");
        let lookalike: String = sqlx::query_scalar("SELECT host_data FROM abgraphqlXorm_probe_new")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(lookalike, "lookalike");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM probe WHERE id='one'")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(count, 1);
    }
    Ok(())
}
