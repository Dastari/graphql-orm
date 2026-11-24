use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "targets", plural = "Targets")]
struct Target {
    #[primary_key]
    #[sortable]
    id: i32,
    #[graphql_orm(private)]
    partition: i32,
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "sources", plural = "Sources")]
struct Source {
    #[primary_key]
    #[sortable]
    id: i32,
    target_id: i32,
    #[graphql_orm(private)]
    partition: i32,
    #[relation(
        target = "Target",
        from = ["target_id", "partition"],
        to = ["id", "partition"],
        server_only_key(from = "partition", to = "partition"),
        filter,
        emit_fk = false
    )]
    target: Option<Target>,
}

fn main() {}
