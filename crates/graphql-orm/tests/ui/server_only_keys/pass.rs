use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "targets", plural = "Targets")]
struct Target {
    #[primary_key]
    #[sortable]
    id: i32,
    source_id: i32,
    #[graphql_orm(private)]
    partition: i32,
    #[graphql_orm(db_column = "Region", private)]
    region: i32,
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "sources", plural = "Sources")]
struct Source {
    #[primary_key]
    #[sortable]
    id: i32,
    target_id: Option<i32>,
    #[graphql_orm(private)]
    partition: i32,
    region: i32,
    #[relation(
        target = "Target",
        from = ["target_id", "partition", "region"],
        to = ["id", "partition", "Region"],
        server_only_key(from = "partition", to = "partition"),
        server_only_key(from = "region", to = "Region"),
        emit_fk = false
    )]
    target: Option<Target>,
    #[relation(
        target = "Target",
        from = ["id", "partition"],
        to = ["source_id", "partition"],
        server_only_key(from = "partition", to = "partition"),
        multiple,
        emit_fk = false
    )]
    targets: Vec<Target>,
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "nodes", plural = "Nodes")]
struct Node {
    #[primary_key]
    #[sortable]
    id: i32,
    parent_id: Option<i32>,
    #[graphql_orm(private)]
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

const _: () = {
    assert!(graphql_orm::graphql::orm::generated_relation_column_is_server_only(
        <Target as graphql_orm::graphql::orm::GeneratedRelationReadability>::SERVER_ONLY_COLUMNS,
        "partition",
        false,
    ));
    assert!(!graphql_orm::graphql::orm::generated_relation_column_is_server_only(
        <Target as graphql_orm::graphql::orm::GeneratedRelationReadability>::SERVER_ONLY_COLUMNS,
        "source_id",
        false,
    ));
};

fn main() {}
