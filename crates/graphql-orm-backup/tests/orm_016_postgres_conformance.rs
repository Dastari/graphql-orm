// The shared conformance suite builds `LocalBackupRepository::new`, which does
// not exist on Windows.
#![cfg(not(windows))]

#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

include!("support/orm_016_conformance.rs");

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn restore_emptiness_refuses_rows_hidden_by_rls() -> Result<(), Box<dyn std::error::Error>> {
    let owner = owned_postgres::OwnedPostgres::pool("backup-empty-rls", 2).await?;
    graphql_orm::sqlx::query("CREATE TABLE conformance_identity_accounts (id uuid PRIMARY KEY, name text, profile jsonb, secret text, ephemeral_token text)").execute(&owner).await?;
    graphql_orm::sqlx::query("INSERT INTO conformance_identity_accounts (id, name) VALUES (pg_catalog.gen_random_uuid(), 'hidden')").execute(&owner).await?;
    graphql_orm::sqlx::query("ALTER TABLE conformance_identity_accounts ENABLE ROW LEVEL SECURITY")
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(
        "CREATE POLICY hidden_rows ON conformance_identity_accounts USING (false)",
    )
    .execute(&owner)
    .await?;
    let role = format!("backup_rls_{}", Uuid::new_v4().simple());
    graphql_orm::sqlx::query(&format!("CREATE ROLE {role} NOSUPERUSER NOBYPASSRLS"))
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(&format!("GRANT USAGE ON SCHEMA public TO {role}"))
        .execute(&owner)
        .await?;
    graphql_orm::sqlx::query(&format!(
        "GRANT SELECT ON conformance_identity_accounts TO {role}"
    ))
    .execute(&owner)
    .await?;
    let runtime = graphql_orm::sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(move |conn, _| {
            let sql = format!("SET ROLE {role}");
            Box::pin(async move {
                graphql_orm::sqlx::query(&sql).execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(owner.connect_options().as_ref().clone())
        .await?;
    let metadata = vec![identity_module::ConformanceAccount::metadata()];
    let owner_adapter = OrmBackupAdapter::new(
        Arc::new(graphql_orm::db::Database::new(owner.clone())),
        metadata.clone(),
    );
    assert!(!owner_adapter.restore_target_is_empty().await?);
    let adapter = OrmBackupAdapter::new(
        Arc::new(graphql_orm::db::Database::new(runtime.clone())),
        metadata,
    );
    let error = adapter
        .restore_target_is_empty()
        .await
        .expect_err("a filtered target count must not look empty");
    assert!(error.to_string().contains("row-level security"), "{error}");
    runtime.close().await;
    owner.close().await;
    Ok(())
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(table = "MixedBackupNotes", plural = "MixedBackupNotes")]
#[allow(dead_code)]
struct MixedBackupNote {
    #[primary_key]
    id: String,
    title: String,
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn mixed_case_backup_restore_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let pool = owned_postgres::OwnedPostgres::pool("backup-table-case", 1).await?;
    let db = Arc::new(graphql_orm::db::Database::new(pool.clone()));
    let plan = db
        .schema()
        .plan_migration_to_entities("mixed-backup", "create", &[MixedBackupNote::metadata()])
        .await?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    graphql_orm::sqlx::query("INSERT INTO MixedBackupNotes VALUES ('one', 'preserved')")
        .execute(&pool)
        .await?;
    let adapter = OrmBackupAdapter::new(db, vec![MixedBackupNote::metadata()]);
    let schema = adapter.schema_snapshot().await?;
    let rows = adapter.export_full().await?;
    assert!(!adapter.restore_target_is_empty().await?);
    adapter.clear_restore_target().await?;
    assert!(adapter.restore_target_is_empty().await?);
    adapter
        .restore_full(schema, rows, RestoreContext::empty_database())
        .await?;
    let title: String =
        graphql_orm::sqlx::query_scalar("SELECT title FROM MixedBackupNotes WHERE id = 'one'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(title, "preserved");
    Ok(())
}
