#![cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]
#[allow(dead_code)]
#[path = "../examples/complete_events.rs"]
mod example;
#[path = "../../../support/owned_postgres.rs"]
mod owned_postgres;
#[tokio::test]
#[ignore = "owns verified disposable PostgreSQL 17 container"]
async fn postgres_private_consumer_enumerates_every_event_without_query_sql()
-> Result<(), Box<dyn std::error::Error>> {
    let mut postgres = owned_postgres::OwnedPostgres::start("consumer-portable-groups")?;
    let db =
        graphql_orm::db::Database::<graphql_orm::graphql::orm::PostgresBackend>::connect_postgres(
            &postgres.url,
        )
        .await?;
    example::run(&db).await?;
    db.pool().close().await;
    drop(db);
    postgres.cleanup()?;
    Ok(())
}
