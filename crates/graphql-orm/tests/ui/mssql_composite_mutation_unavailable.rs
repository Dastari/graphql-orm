use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.PrivateGrant",
    plural = "PrivateGrants",
    repository_mutations = true,
    default_sort = "subject_id ASC, grant ASC"
)]
struct PrivateGrant {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    subject_id: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    grant: String,
    value: String,
}

fn main() {}
