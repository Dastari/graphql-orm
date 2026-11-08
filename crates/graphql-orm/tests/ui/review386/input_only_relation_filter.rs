use graphql_orm::prelude::*;
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Record {
    #[primary_key]
    id: String,
    label: String,

    owner_id: String,
    #[input_only]
    #[relation(target = "Record", from = "owner_id", to = "id", emit_fk = false, multiple, filter)]
    owner: Vec<Record>,
}
fn main() {}
