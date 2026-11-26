use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "lines", notify_with = "notify", nested_create_keys = ["order_id"])]
struct Line {
    #[primary_key]
    #[sortable]
    id: String,
    order_id: String,
}

fn main() {}
