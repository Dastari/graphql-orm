use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "cross_endpoints",
        plural = "Endpoints",
        default_sort = "id ASC",
        read_policy = "endpoint.read"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "cross_endpoints",
        plural = "Endpoints",
        default_sort = "id ASC",
        read_policy = "endpoint.read"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "cross_endpoints",
        plural = "Endpoints",
        default_sort = "id ASC",
        read_policy = "endpoint.read"
    )
)]
pub struct Endpoint {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    #[graphql_orm(read = false, filter = false, order = false, subscribe = false)]
    pub tenant_id: String,
    #[filterable(type = "string")]
    #[sortable]
    #[graphql_orm(read_policy = "endpoint.name.read")]
    pub name: String,
}

impl<B: OrmBackend> graphql_orm::graphql::loaders::BatchLoadEntity<B> for Endpoint
where
    Endpoint: FromSqlRow<B>,
{
    fn batch_column() -> &'static str {
        "id"
    }
    fn batch_key_from_row(row: &B::Row) -> graphql_orm::Result<String> {
        B::try_get_string(row, "id")
    }
}
