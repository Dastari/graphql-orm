use graphql_orm::prelude::*;
#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Target {
    #[primary_key]
    #[sortable]
    id: i32,
    #[graphql_orm(private, db_column = "tier_code")]
    tier: i32,
}
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Source {
    #[primary_key]
    #[sortable]
    id: i32,
    #[relation(
        target = "Target",
        from = "id",
        to = "id",
        target_condition(column = "TIER_CODE", equals = 7),
        multiple,
        emit_fk = false
    )]
    peers: Vec<Target>,
}
fn main() {}
