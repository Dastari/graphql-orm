use graphql_orm::prelude::*;
#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Target {
    #[primary_key]
    #[sortable]
    id: i32,
    tenant_id: i32,
    #[graphql_orm(private)]
    owner_id: i32,
}
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Source {
    #[primary_key]
    #[sortable]
    id: i32,
    tenant_id: i32,
    #[relation(target = "Target", from = [tenant_id, id], to = [tenant_id, owner_id], multiple, emit_fk = false)]
    items: Vec<Target>,
}
fn main() {}
