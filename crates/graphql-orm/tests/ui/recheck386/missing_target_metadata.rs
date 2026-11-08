use graphql_orm::prelude::*;
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Target;
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Source {
    #[primary_key]
    #[sortable]
    id: i32,
    #[relation(target = "Target", from = "id", to = "id", emit_fk = false)]
    owner: Option<Target>,
}
fn main() {}
