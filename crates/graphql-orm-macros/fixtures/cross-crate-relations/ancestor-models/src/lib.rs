use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "up_roots",
        plural = "UpRoots",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "up_roots",
        plural = "UpRoots",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "up_roots",
        plural = "UpRoots",
        default_sort = "id ASC"
    )
)]
#[graphql_orm(operation_authorization(categories = ["subscription"], all_scopes = ["up.root"]))]
pub struct UpRoot {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    pub tenant: String,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "auth_roots",
        plural = "AuthRootRows",
        auth = "none",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "auth_roots",
        plural = "AuthRootRows",
        auth = "none",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "auth_roots",
        plural = "AuthRootRows",
        auth = "none",
        default_sort = "id ASC"
    )
)]
pub struct AuthRoot {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    pub tenant: String,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "required_auth_roots",
        plural = "RequiredAuthRootRows",
        auth = "required",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "required_auth_roots",
        plural = "RequiredAuthRootRows",
        auth = "required",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "required_auth_roots",
        plural = "RequiredAuthRootRows",
        auth = "required",
        default_sort = "id ASC"
    )
)]
pub struct RequiredAuthRoot {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    pub id: String,
    pub tenant: String,
}

// RelationLoader uses this row identity; current relation and policy checks remain separate.
impl<B: OrmBackend> graphql_orm::graphql::loaders::BatchLoadEntity<B> for UpRoot
where
    UpRoot: FromSqlRow<B>,
{
    fn batch_column() -> &'static str {
        "id"
    }

    fn batch_key_from_row(row: &B::Row) -> graphql_orm::Result<String> {
        B::try_get_string(row, "id")
    }
}

// RelationLoader uses this row identity; current relation and policy checks remain separate.
impl<B: OrmBackend> graphql_orm::graphql::loaders::BatchLoadEntity<B> for AuthRoot
where
    AuthRoot: FromSqlRow<B>,
{
    fn batch_column() -> &'static str {
        "id"
    }

    fn batch_key_from_row(row: &B::Row) -> graphql_orm::Result<String> {
        B::try_get_string(row, "id")
    }
}
