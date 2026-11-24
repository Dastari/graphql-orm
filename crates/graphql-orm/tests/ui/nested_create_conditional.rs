use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "orders")]
struct Order {
    kind: String,
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql_orm(read = false)]
    #[relation(
        target = "Line",
        from = "id",
        to = "order_id",
        multiple,
        nested_create,
        emit_fk = false,
        source_condition(field = "kind", equals = "enabled")
    )]
    lines: Vec<Line>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Line;

fn main() {}
