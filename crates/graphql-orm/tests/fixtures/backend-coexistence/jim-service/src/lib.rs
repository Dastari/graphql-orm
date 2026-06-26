use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.Jobs",
    plural = "Jobs",
    default_sort = "JobId ASC"
)]
pub struct Job {
    #[primary_key]
    #[graphql_orm(db_column = "JobId")]
    #[filterable(type = "number")]
    #[sortable]
    pub job_id: i32,

    #[graphql_orm(db_column = "JobName")]
    #[filterable(type = "string")]
    #[sortable]
    pub job_name: String,

    #[graphql_orm(db_column = "IsActive")]
    #[filterable(type = "bool")]
    pub is_active: bool,
}

schema_roots! {
    backend: "mssql",
    query_custom_ops: [],
    entities: [Job],
}

pub fn build_schema(
    pool: graphql_orm::db::mssql::MssqlPool,
) -> graphql_orm::async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot> {
    schema_builder(graphql_orm::db::Database::<graphql_orm::MssqlBackend>::new(pool)).finish()
}

