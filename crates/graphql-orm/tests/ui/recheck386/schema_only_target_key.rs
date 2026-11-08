use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "targets", plural = "Targets")]
struct Target {
    #[primary_key]
    id: i32,
    #[graphql_orm(private)]
    secret: i32,
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "sources",
    plural = "Sources",
    schema_only = true
)]
struct Source {
    #[primary_key]
    id: i32,
    #[relation(target = "Target", from = "id", to = "secret", emit_fk = false)]
    target: Option<Target>,
}

fn main() {}
