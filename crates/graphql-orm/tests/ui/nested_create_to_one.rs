use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "orders")]
struct Order {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql_orm(read = false)]
    #[relation(target = "Line", from = "id", to = "order_id", nested_create)]
    line: Option<Line>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Line;

fn main() {}
