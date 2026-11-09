use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "records")]
struct Record {
    #[primary_key]
    id: String,
    name: String,
}

fn main() {}
