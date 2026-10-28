use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(feature = "sqlite", graphql_entity(backend = "sqlite", table = "up_roots", plural = "UpRoots", default_sort = "id ASC"))]
#[cfg_attr(feature = "postgres", graphql_entity(backend = "postgres", table = "up_roots", plural = "UpRoots", default_sort = "id ASC"))]
#[cfg_attr(feature = "mssql", graphql_entity(backend = "mssql", table = "up_roots", plural = "UpRoots", default_sort = "id ASC"))]
pub struct UpRoot {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    pub tenant: String,
}
