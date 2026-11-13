use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records")]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
    #[json_field]
    #[transform(write = "protect")]
    value: serde_json::Value,
}

fn main() {}
