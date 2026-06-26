use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(table = "dbo.LegacyJobs", plural = "Jobs")]
struct LegacyJob {
    #[primary_key]
    #[filterable(type = "string")]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub job_name: String,
}

fn main() {
    let _ = LegacyJob::create;
}
