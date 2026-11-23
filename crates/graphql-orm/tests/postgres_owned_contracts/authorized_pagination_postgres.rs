#![cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]
use super::owned_postgres;
#[path = "../support/authorized_pagination_parity.rs"]
mod parity;
use graphql_orm::prelude::*;

#[tokio::test]
#[ignore = "starts a test-owned loopback-only PostgreSQL container"]
async fn postgres_authorized_pagination() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = owned_postgres::OwnedPostgres::start_ready(
        "authorized-pagination",
        owned_postgres::POSTGRES_IMAGE,
    )
    .await?;
    let db = Database::<PostgresBackend>::connect_postgres(&server.url).await?;
    sqlx::raw_sql(&parity::seed_sql())
        .execute(db.pool())
        .await?;
    parity::verify(db).await;
    server.cleanup()?;
    Ok(())
}
