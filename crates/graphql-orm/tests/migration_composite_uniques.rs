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
async fn sqlite_rebuild_uses_target_append_only_policy() -> Result<(), Box<dyn std::error::Error>> {
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
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    sqlx::query("UPDATE unique_delta SET name='changed'")
        .execute(db.pool())
        .await?;
    Ok(())
}
