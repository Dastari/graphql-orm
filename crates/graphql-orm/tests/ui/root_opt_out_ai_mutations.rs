use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "records", mutations = false, ai_mutations(create = "prohibited"))]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
}

fn main() {}
