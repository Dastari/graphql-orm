#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[path = "../examples/runtime_strings.rs"]
#[allow(dead_code)]
mod example;
#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[path = "../../../support/owned_postgres.rs"]
mod owned_postgres;

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[tokio::test]
#[ignore = "owns a disposable PostgreSQL Docker container; run explicitly"]
async fn standalone_runtime_string_consumer_executes_on_owned_postgres()
-> Result<(), Box<dyn std::error::Error>> {
    for variable in [
        "DATABASE_URL",
        "TEST_DATABASE_URL",
        "MSSQL_TEST_DATABASE_URL",
    ] {
        assert!(
            std::env::var_os(variable).is_none(),
            "refusing ambient database configuration"
        );
    }
    let mut owned = owned_postgres::OwnedPostgres::start("runtime-string-consumer")?;
    let db = graphql_orm::db::Database::<graphql_orm::prelude::PostgresBackend>::connect_postgres(
        &owned.url,
    )
    .await?;
    example::run(&db).await?;
    let (database, user): (String, String) =
        graphql_orm::sqlx::query_as("SELECT current_database(), current_user")
            .fetch_one(db.pool())
            .await?;
    assert_eq!(Some(database.as_str()), owned.url.rsplit('/').next());
    assert_eq!(user, "graphql_orm_owner");
    db.pool().close().await;
    owned.cleanup()?;
    Ok(())
}
