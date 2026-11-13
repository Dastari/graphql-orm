use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records")]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,

    #[transform(write = "protect", read = "reveal")]
    value: String,
}

fn main() {}
