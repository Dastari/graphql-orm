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

pub mod propagation {
use graphql_orm::prelude::*;
pub use cross_crate_ancestor_models::*;
#[derive(GraphQLEntity, GraphQLOperations, GraphQLRelations, async_graphql::SimpleObject, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql(complex)]
#[cfg_attr(feature = "sqlite", graphql_entity(backend = "sqlite", table = "up_middles", plural = "UpMiddles", default_sort = "id ASC"))]
#[cfg_attr(feature = "postgres", graphql_entity(backend = "postgres", table = "up_middles", plural = "UpMiddles", default_sort = "id ASC"))]
#[cfg_attr(feature = "mssql", graphql_entity(backend = "mssql", table = "up_middles", plural = "UpMiddles", default_sort = "id ASC"))]
#[graphql_orm(operation_authorization(categories = ["subscription"], all_scopes = ["up.middle"]))]
pub struct UpMiddle {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    pub tenant: String,
    pub root_id: String,
    #[graphql(skip)]
    #[relation(target = "UpRoot", from = ["root_id", "tenant"], to = ["id", "tenant"], propagate_change = "up", emit_fk = false)]
    pub root: Option<UpRoot>,
}
#[derive(GraphQLEntity, GraphQLOperations, GraphQLRelations, async_graphql::SimpleObject, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql(complex)]
#[cfg_attr(feature = "sqlite", graphql_entity(backend = "sqlite", table = "required_middles", plural = "RequiredMiddleRows", auth = "required", default_sort = "id ASC"))]
#[cfg_attr(feature = "postgres", graphql_entity(backend = "postgres", table = "required_middles", plural = "RequiredMiddleRows", auth = "required", default_sort = "id ASC"))]
#[cfg_attr(feature = "mssql", graphql_entity(backend = "mssql", table = "required_middles", plural = "RequiredMiddleRows", auth = "required", default_sort = "id ASC"))]
pub struct RequiredMiddle {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    pub tenant: String,
    pub parent_id: String,
    #[graphql(skip)]
    #[relation(target = "AuthRoot", from = ["parent_id", "tenant"], to = ["id", "tenant"], propagate_change = "up", emit_fk = false)]
    pub parent: Option<AuthRoot>,
}

#[derive(GraphQLEntity, GraphQLOperations, GraphQLRelations, async_graphql::SimpleObject, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql(complex)]
#[cfg_attr(feature = "sqlite", graphql_entity(backend = "sqlite", table = "optional_middles", plural = "OptionalMiddleRows", auth = "optional", default_sort = "id ASC"))]
#[cfg_attr(feature = "postgres", graphql_entity(backend = "postgres", table = "optional_middles", plural = "OptionalMiddleRows", auth = "optional", default_sort = "id ASC"))]
#[cfg_attr(feature = "mssql", graphql_entity(backend = "mssql", table = "optional_middles", plural = "OptionalMiddleRows", auth = "optional", default_sort = "id ASC"))]
pub struct OptionalMiddle {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    pub tenant: String,
    pub parent_id: String,
    #[graphql(skip)]
    #[relation(target = "AuthRoot", from = ["parent_id", "tenant"], to = ["id", "tenant"], propagate_change = "up", emit_fk = false)]
    pub parent: Option<AuthRoot>,
}

}
