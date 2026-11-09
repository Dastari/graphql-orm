#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::{graphql::orm::*, prelude::*};

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "sqlite", table = "sqliteXwidget", schema_policy = "managed")]
struct Widget {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    name: String,
}

async fn populated_widget() -> graphql_orm::Result<Database<SqliteBackend>> {
    let db = Database::<SqliteBackend>::connect_sqlite_with_options(
        "sqlite::memory:",
        graphql_orm::db::ConnectionOptions::default().max_connections(1),
    )
    .await?;
    // Host-created physical schema: planning must discover this existing table.
    sqlx::query("CREATE TABLE sqliteXwidget(id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL)")
        .execute(db.pool())
        .await?;
    sqlx::query("INSERT INTO sqliteXwidget VALUES ('widget', 'retained')")
        .execute(db.pool())
        .await?;
    Ok(db)
}

#[tokio::test]
async fn populated_managed_lookalike_plans_empty_from_entity_metadata() -> graphql_orm::Result<()> {
    let db = populated_widget().await?;
    let plan = db
        .schema()
        .plan_migration_to_entities(
            "existing",
            "existing managed lookalike",
            &[Widget::metadata()],
        )
        .await?;
    assert!(plan.steps.is_empty(), "{plan:?}");
    assert!(plan.statements.is_empty(), "{plan:?}");
    let row: (String, String) = sqlx::query_as("SELECT id, name FROM sqliteXwidget")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(row, ("widget".into(), "retained".into()));
    Ok(())
}

#[tokio::test]
async fn unmanaged_lookalikes_refuse_strict_apply_and_survive_managed_only_replan()
-> graphql_orm::Result<()> {
    let db = populated_widget().await?;
    let names = [
        "sqliteXcustomer",
        "sqlite-customer",
        "SQLITExaccount",
        "SqLiTe-account",
    ];
    for name in names {
        sqlx::query(&format!(
            "CREATE TABLE \"{name}\" (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL)"
        ))
        .execute(db.pool())
        .await?;
        sqlx::query(&format!(
            "INSERT INTO \"{name}\" (name) VALUES ('retained')"
        ))
        .execute(db.pool())
        .await?;
    }
    for name in ["sqlite_customer", "SQLITE_customer", "SqLiTe_customer"] {
        assert!(
            sqlx::query(&format!("CREATE TABLE \"{name}\" (id INTEGER)"))
                .execute(db.pool())
                .await
                .is_err()
        );
    }
    sqlx::query("ANALYZE").execute(db.pool()).await?;
    for name in ["sqlite_sequence", "sqlite_stat1"] {
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)"
            )
            .bind(name)
            .fetch_one(db.pool())
            .await?
        );
    }
    let schema = introspect_sqlite_schema(db.pool()).await?;
    assert_eq!(schema.tables.len(), names.len() + 1);
    for name in names {
        assert!(schema.tables.iter().any(|table| table.table_name == name));
    }
    let manager = db.schema();
    let strict = manager
        .plan_migration_to_entities("strict", "unmanaged lookalikes", &[Widget::metadata()])
        .await?;
    assert_eq!(strict.steps.len(), names.len(), "{strict:?}");
    for name in names {
        assert!(strict.steps.iter().any(|step|
            step.risk == MigrationRisk::Destructive &&
            matches!(&step.step, MigrationStep::DropTable { table_name } if table_name == name)
        ), "{strict:?}");
    }
    let error = manager
        .apply_migration(&strict, ApplyOptions::default())
        .await
        .expect_err("default apply must refuse unmanaged table drops");
    assert!(error.to_string().contains("destructive step"), "{error}");

    for name in names {
        let row: (i64, String) = sqlx::query_as(&format!("SELECT id, name FROM \"{name}\""))
            .fetch_one(db.pool())
            .await?;
        assert_eq!(row, (1, "retained".into()));
    }

    let scoped = manager
        .plan_migration_to_entities_with_options(
            "scoped",
            "preserve unmanaged tables",
            &[Widget::metadata()],
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(scoped.steps.is_empty(), "{scoped:?}");
    manager
        .apply_migration(&scoped, ApplyOptions::default())
        .await?;
    let replan = manager
        .plan_migration_to_entities_with_options(
            "replan",
            "unchanged entity metadata",
            &[Widget::metadata()],
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(replan.steps.is_empty(), "{replan:?}");
    assert!(replan.statements.is_empty(), "{replan:?}");
    for name in names {
        let row: (i64, String) = sqlx::query_as(&format!("SELECT id, name FROM \"{name}\""))
            .fetch_one(db.pool())
            .await?;
        assert_eq!(row, (1, "retained".into()));
    }
    let widget: String = sqlx::query_scalar("SELECT name FROM sqliteXwidget")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(widget, "retained");
    Ok(())
}
