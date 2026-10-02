#[path = "../examples/host_timestamps.rs"]
#[allow(dead_code)]
mod example;
#[cfg(feature = "postgres")]
#[path = "../../../support/owned_postgres.rs"]
mod owned_postgres;

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn standalone_host_timestamp_usage_executes_on_owned_sqlite()
-> Result<(), Box<dyn std::error::Error>> {
    let db = graphql_orm::db::Database::connect_sqlite("sqlite::memory:").await?;
    example::run(&db).await
}
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "owns a disposable Docker PostgreSQL container"]
async fn standalone_host_timestamp_usage_executes_on_owned_postgres()
-> Result<(), Box<dyn std::error::Error>> {
    let mut owned = owned_postgres::OwnedPostgres::start("host-timestamp-consumer")?;
    let db = graphql_orm::db::Database::connect_postgres(&owned.url).await?;
    example::run(&db).await?;
    owned.cleanup()
}
