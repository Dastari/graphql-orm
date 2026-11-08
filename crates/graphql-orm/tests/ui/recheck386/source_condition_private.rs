use graphql_orm::prelude::*;
#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Target {
    #[primary_key]
    #[sortable]
    id: i32,
}
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Source {
    #[primary_key]
    #[sortable]
    id: i32,
    #[graphql_orm(private)]
    tier: i32,
    #[relation(
        target = "Target",
        from = "id",
        to = "id",
        source_condition(field = "tier", equals = 7),
        emit_fk = false
    )]
    vip: Option<Target>,
}
fn main() {}
