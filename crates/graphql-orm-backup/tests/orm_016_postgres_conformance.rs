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
