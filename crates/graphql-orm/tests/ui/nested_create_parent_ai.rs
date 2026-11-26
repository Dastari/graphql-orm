use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "orders", ai_mutations(create = "approval_required"))]
struct Order {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql_orm(read = false)]
    #[relation(target = "Line", from = "id", to = "order_id", multiple, nested_create)]
    lines: Vec<Line>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Line;

fn main() {}
