#![cfg(any(feature = "sqlite", feature = "postgres"))]

use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(table = "legacy_history_items", plural = "LegacyHistoryItems")]
#[allow(dead_code)]
struct LegacyHistoryItem {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    label: String,
}

#[cfg(feature = "sqlite")]
fn sqlite_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "graphql-orm-{label}-{}.sqlite",
        graphql_orm::uuid::Uuid::new_v4()
    ))
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_legacy_history_is_upgraded_preserved_reopened_and_can_migrate()
-> Result<(), Box<dyn std::error::Error>> {
    let path = sqlite_path("legacy-history");
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let database = Database::<SqliteBackend>::connect_sqlite(&url).await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE __graphql_orm_migrations (
            version TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL
        )",
    )
    .execute(database.pool())
    .await?;
    for (version, applied_at) in [
        ("001-bootstrap", "2022-01-02 03:04:05"),
        ("002-indexes", "2023-06-07T08:09:10.123Z"),
    ] {
        graphql_orm::sqlx::query(
            "INSERT INTO __graphql_orm_migrations (version, applied_at) VALUES (?, ?)",
        )
        .bind(version)
        .bind(applied_at)
        .execute(database.pool())
        .await?;
    }

    assert_eq!(
        database.schema().current_version().await?.as_deref(),
        Some("002-indexes")
    );
    let rows: Vec<(String, String, String, Option<String>)> = graphql_orm::sqlx::query_as(
        "SELECT version, description, applied_at, backend
         FROM __graphql_orm_migrations ORDER BY version",
    )
    .fetch_all(database.pool())
    .await?;
    assert_eq!(
        rows,
        vec![
            (
                "001-bootstrap".to_string(),
                "Legacy migration 001-bootstrap".to_string(),
                "2022-01-02 03:04:05".to_string(),
                None,
            ),
            (
                "002-indexes".to_string(),
                "Legacy migration 002-indexes".to_string(),
                "2023-06-07T08:09:10.123Z".to_string(),
                None,
            ),
        ]
    );

    let entities = [LegacyHistoryItem::metadata()];
    let plan = database
        .schema()
        .plan_migration_to_entities("003-managed", "managed adoption", &entities)
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    drop(database);

    let reopened = Database::<SqliteBackend>::connect_sqlite(&url).await?;
    assert_eq!(
        reopened.schema().current_version().await?.as_deref(),
        Some("003-managed")
    );
    let legacy_rows: Vec<(String, String, String)> = graphql_orm::sqlx::query_as(
        "SELECT version, description, applied_at FROM __graphql_orm_migrations
         WHERE version < '003' ORDER BY version",
    )
    .fetch_all(reopened.pool())
    .await?;
    assert_eq!(legacy_rows[0].2, "2022-01-02 03:04:05");
    assert_eq!(legacy_rows[1].2, "2023-06-07T08:09:10.123Z");
    assert_eq!(legacy_rows.len(), 2);
    let column_count: i64 = graphql_orm::sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('__graphql_orm_migrations')",
    )
    .fetch_one(reopened.pool())
    .await?;
    assert_eq!(column_count, 9);
    drop(reopened);
    std::fs::remove_file(path)?;
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_current_history_is_unchanged_and_malformed_identity_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let current = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE __graphql_orm_migrations (
            version TEXT PRIMARY KEY, description TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            backend TEXT, graphql_orm_version TEXT, source_schema_hash TEXT,
            target_schema_hash TEXT, plan_hash TEXT, policy TEXT
        )",
    )
    .execute(current.pool())
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO __graphql_orm_migrations
         (version, description, applied_at, backend)
         VALUES ('current', 'kept exactly', '2001-02-03 04:05:06', 'sqlite')",
    )
    .execute(current.pool())
    .await?;
    current.schema().current_version().await?;
    let row: (String, String, String, String) = graphql_orm::sqlx::query_as(
        "SELECT version, description, applied_at, backend FROM __graphql_orm_migrations",
    )
    .fetch_one(current.pool())
    .await?;
    assert_eq!(
        row,
        (
            "current".to_string(),
            "kept exactly".to_string(),
            "2001-02-03 04:05:06".to_string(),
            "sqlite".to_string(),
        )
    );

    let malformed = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE __graphql_orm_migrations (
            version TEXT NOT NULL,
            applied_at TEXT NOT NULL
        )",
    )
    .execute(malformed.pool())
    .await?;
    let error = malformed
        .schema()
        .current_version()
        .await
        .expect_err("history without version primary-key identity must fail closed");
    assert!(
        error
            .to_string()
            .contains("unsafe __graphql_orm_migrations schema")
    );
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_legacy_adoption_preserves_recorded_version_drift_protection()
-> Result<(), Box<dyn std::error::Error>> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE __graphql_orm_migrations (
            version TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL
        )",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO __graphql_orm_migrations VALUES ('legacy-reused', '2020-01-01')",
    )
    .execute(database.pool())
    .await?;
    database.schema().current_version().await?;
    let target = SchemaModel::from_entities(&[LegacyHistoryItem::metadata()]);
    let live = introspect_sqlite_schema(&database).await?;
    let plan = database.schema().plan_migration(
        "legacy-reused",
        "must retain fail closed",
        &live,
        &target,
    )?;
    assert!(!plan.steps.is_empty());
    let error = database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect_err("recorded legacy version with remaining work must fail closed");
    assert!(error.to_string().contains("already recorded"));
    Ok(())
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_legacy_history_adopts_idempotently_and_malformed_identity_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        return Ok(());
    };
    let database = Database::<PostgresBackend>::connect_postgres(url).await?;
    graphql_orm::sqlx::query("DROP TABLE IF EXISTS legacy_history_items CASCADE")
        .execute(database.pool())
        .await?;
    graphql_orm::sqlx::query("DROP TABLE IF EXISTS __graphql_orm_migrations CASCADE")
        .execute(database.pool())
        .await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE __graphql_orm_migrations (
            version TEXT PRIMARY KEY,
            applied_at TIMESTAMPTZ NOT NULL
        )",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO __graphql_orm_migrations VALUES
         ('pg-001', '2020-01-02T03:04:05Z'),
         ('pg-002', '2021-06-07T08:09:10Z')",
    )
    .execute(database.pool())
    .await?;
    database.schema().current_version().await?;
    let rows: Vec<(String, String, String, Option<String>)> = graphql_orm::sqlx::query_as(
        "SELECT version, description, applied_at::TEXT, backend
         FROM __graphql_orm_migrations ORDER BY version",
    )
    .fetch_all(database.pool())
    .await?;
    assert_eq!(rows[0].0, "pg-001");
    assert_eq!(rows[0].1, "Legacy migration pg-001");
    assert_eq!(rows[0].2, "2020-01-02 03:04:05+00");
    assert!(rows.iter().all(|row| row.3.is_none()));
    database.schema().current_version().await?;
    let count: i64 =
        graphql_orm::sqlx::query_scalar("SELECT COUNT(*) FROM __graphql_orm_migrations")
            .fetch_one(database.pool())
            .await?;
    assert_eq!(count, 2);

    let entities = [LegacyHistoryItem::metadata()];
    let plan = database
        .schema()
        .plan_migration_to_entities_with_options(
            "pg-003-managed",
            "postgres managed adoption",
            &entities,
            PlanOptions::managed_tables_only(),
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;

    graphql_orm::sqlx::query("DROP TABLE legacy_history_items CASCADE")
        .execute(database.pool())
        .await?;
    graphql_orm::sqlx::query("DROP TABLE __graphql_orm_migrations CASCADE")
        .execute(database.pool())
        .await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE __graphql_orm_migrations (
            version TEXT NOT NULL,
            applied_at TIMESTAMPTZ NOT NULL
        )",
    )
    .execute(database.pool())
    .await?;
    let error = database
        .schema()
        .current_version()
        .await
        .expect_err("PostgreSQL history without primary key fails closed");
    assert!(
        error
            .to_string()
            .contains("unsafe __graphql_orm_migrations schema")
    );
    graphql_orm::sqlx::query("DROP TABLE __graphql_orm_migrations CASCADE")
        .execute(database.pool())
        .await?;
    Ok(())
}
