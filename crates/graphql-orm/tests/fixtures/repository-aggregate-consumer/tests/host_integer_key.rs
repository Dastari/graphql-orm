#[path = "../examples/host_integer_key.rs"]
#[allow(dead_code)]
mod example;
use graphql_orm::prelude::*;
use repository_aggregate_consumer::{
    Backend,
    host_primary_keys::{CreateHostI32Input, HostI32, HostKeyFixturePolicy},
};

#[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
#[path = "../../../support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[path = "../../../support/owned_postgres.rs"]
mod owned_postgres;

#[tokio::test]
#[cfg_attr(
    not(feature = "sqlite"),
    ignore = "owns a disposable backend container"
)]
async fn minimal_external_consumer_preserves_allocated_integer_keys()
-> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let mut database = Database::<Backend>::connect_sqlite("sqlite::memory:").await?;
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    let mut owned = owned_postgres::OwnedPostgres::start("integer-key-consumer")?;
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    let mut database = Database::<Backend>::connect_postgres(&owned.url).await?;
    #[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
    let mut owned = owned_mssql::OwnedSqlServer::start().await?;
    #[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
    owned_mssql::execute_batch(
        &owned.connection_string,
        "CREATE TABLE dbo.HostI32 (id INT NOT NULL PRIMARY KEY, label NVARCHAR(64) NOT NULL)",
    )
    .await?;
    #[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
    let mut database =
        Database::<Backend>::connect_ado_external_writable(&owned.connection_string).await?;
    database.set_entity_policy(HostKeyFixturePolicy);
    #[cfg(feature = "sqlite")]
    {
        let plan = database
            .schema()
            .plan_migration_to_entities(
                "host-key",
                "owned consumer fixture",
                &[HostI32::metadata()],
            )
            .await?;
        database
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await?;
    }
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    graphql_orm::sqlx::query("CREATE TABLE host_i32 (id INT PRIMARY KEY, label TEXT NOT NULL)")
        .execute(database.pool())
        .await?;
    let direct = HostI32::insert(
        &database,
        CreateHostI32Input {
            id: 123_456,
            label: "ordinary".into(),
        },
    )
    .await?;
    assert_eq!(direct.id, 123_456);
    for supplied in [0, -7, i32::MAX] {
        let row = example::insert_allocated(&database, supplied).await?;
        assert_eq!(row.id, supplied);
    }
    #[cfg(not(feature = "mssql"))]
    database.pool().close().await;
    #[cfg(not(feature = "sqlite"))]
    owned.cleanup()?;
    Ok(())
}
