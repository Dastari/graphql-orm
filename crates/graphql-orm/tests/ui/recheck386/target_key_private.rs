use graphql_orm::prelude::*;
#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Target {
    #[primary_key]
    #[sortable]
    id: i32,
    #[graphql_orm(private)]
    badge: i32,
}
type AliasTarget = Target;
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Source {
    #[primary_key]
    #[sortable]
    id: i32,
    guess: i32,
    #[relation(target = "AliasTarget", from = "guess", to = "badge", emit_fk = false)]
    owner: Option<Target>,
}
fn main() {}
