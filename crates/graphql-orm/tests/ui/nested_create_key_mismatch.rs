use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "orders")]
struct Order {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[graphql_orm(read = false)]
    #[relation(target = "Line", from = "id", to = "other_order_id", multiple, nested_create)]
    lines: Vec<Line>,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "lines", nested_create_keys = ["order_id"])]
struct Line {
    #[primary_key]
    #[sortable]
    id: String,
    order_id: String,
    other_order_id: String,
}

fn main() {}
