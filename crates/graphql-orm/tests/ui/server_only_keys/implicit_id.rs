use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "targets", plural = "Targets")]
struct Target {
    #[graphql(skip)]
    #[graphql_orm(private)]
    id: i32,
    #[sortable]
    code: i32,
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "sources", plural = "Sources")]
struct Source {
    #[primary_key]
    #[sortable]
    id: i32,
    code: i32,
    #[graphql_orm(private)]
    target_key: i32,
    #[relation(
        target = "Target",
        from = ["code", "target_key"],
        to = ["code", "id"],
        server_only_key(from = "target_key", to = "id"),
        emit_fk = false
    )]
    target: Option<Target>,
}

fn main() {}
