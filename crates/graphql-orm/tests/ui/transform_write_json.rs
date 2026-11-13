use graphql_orm::prelude::*;

#[derive(GraphQLOperations)]
#[graphql_entity(backend = "sqlite", table = "records")]
struct Record {
    #[primary_key]
    id: String,
    #[json_field]
    #[transform(write = "protect")]
    value: serde_json::Value,
}

fn main() {}
