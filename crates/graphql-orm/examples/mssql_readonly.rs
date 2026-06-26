#[cfg(feature = "mssql")]
use graphql_orm::prelude::*;

#[cfg(feature = "mssql")]
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(table = "dbo.Jobs", plural = "Jobs", default_sort = "[JobId] ASC")]
pub struct LegacyJob {
    #[primary_key]
    #[graphql_orm(db_column = "JobId", write = false)]
    #[filterable(type = "number")]
    #[sortable]
    pub id: i64,

    #[graphql_orm(db_column = "JobName", write = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub job_name: String,

    #[graphql_orm(db_column = "IsClosed", write = false)]
    #[filterable(type = "boolean")]
    pub closed: bool,

    #[graphql_orm(db_column = "StartedAt", write = false)]
    #[filterable(type = "date")]
    #[sortable]
    pub started_at: Option<String>,
}

#[cfg(feature = "mssql")]
schema_roots! {
    query_custom_ops: [],
    entities: [LegacyJob],
}

#[cfg(feature = "mssql")]
pub fn build_schema(pool: graphql_orm::DbPool) -> AppSchema {
    let database = graphql_orm::db::Database::new(pool);
    schema_builder(database)
        .data("mssql-readonly-example".to_string())
        .finish()
}

fn main() {}
