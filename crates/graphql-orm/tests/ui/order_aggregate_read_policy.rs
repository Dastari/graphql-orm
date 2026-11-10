use graphql_orm::prelude::*;
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "parents", plural = "Parents")]
struct Parent {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql_orm(read_policy = "parent.children.read")]
    #[relation(target = "Child", from = "id", to = "parent_id", emit_fk = false, multiple, order_aggregate(name = "ChildCount", aggregate = "count"))]
    children: Vec<Child>,
}
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "children", plural = "Children")]
struct Child {
    #[primary_key]
    #[sortable]
    id: String,
    parent_id: String,
}
fn main() {}
