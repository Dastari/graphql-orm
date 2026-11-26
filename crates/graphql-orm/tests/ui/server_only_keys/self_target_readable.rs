use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "nodes", plural = "Nodes")]
struct Node {
    #[primary_key]
    #[sortable]
    id: i32,
    parent_id: Option<i32>,
    partition: i32,
    #[relation(
        target = "Node",
        from = ["parent_id", "partition"],
        to = ["id", "partition"],
        server_only_key(from = "partition", to = "partition"),
        emit_fk = false
    )]
    parent: Option<Box<Node>>,
}

fn main() {}
