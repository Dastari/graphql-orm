use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "records", mutations = false, dependent_deletes)]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
}

fn main() {}
