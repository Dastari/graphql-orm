use graphql_orm::prelude::*;
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Record {
    #[primary_key]
    id: String,
    label: String,
    #[graphql_orm(private)]
    owner_id: String,
    #[relation(target = "Record", from = "id", to = "missing_key", emit_fk = false)]
    owner: Vec<Record>,
}
fn main() {}
