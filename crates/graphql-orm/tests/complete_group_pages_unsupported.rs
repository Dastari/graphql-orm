#![cfg(all(any(feature = "postgres", feature = "mssql"), not(feature = "sqlite")))]
use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "postgres",
    repository_entity(
        backend = "postgres",
        table = "unsupported_group_pages",
        plural = "UnsupportedGroupPages"
    )
)]
#[cfg_attr(
    all(feature = "mssql", not(feature = "postgres")),
    repository_entity(
        backend = "mssql",
        table = "dbo.unsupported_group_pages",
        plural = "UnsupportedGroupPages",
        schema_policy = "external_read_only"
    )
)]
struct GroupSource {
    #[primary_key]
    id: String,
    category: String,
}
#[tokio::test]
async fn unsupported_group_pages_fail_before_pool_io() -> graphql_orm::Result<()> {
    #[cfg(feature = "postgres")]
    let db = Database::<PostgresBackend>::new(
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/invalid")?,
    );
    #[cfg(all(feature = "mssql", not(feature = "postgres")))]
    let db = {
        let mut config = graphql_orm::tiberius::Config::new();
        config.host("127.0.0.1");
        config.port(1);
        Database::<MssqlBackend>::new(graphql_orm::db::mssql::MssqlPool::new(config))
    };
    let result = GroupSource::aggregate(&db)
        .group_by(GroupSourceAggregateField::Category)?
        .fetch_group_page(
            AggregateGroupPageOptions {
                order: AggregateGroupOrder::Binary,
                exclude_blank: false,
                context: "trusted".into(),
            },
            None,
        )
        .await;
    assert!(
        matches!(result,Err(sqlx::Error::Protocol(ref message)) if message=="unsupported aggregate group pagination capability")
    );
    Ok(())
}
