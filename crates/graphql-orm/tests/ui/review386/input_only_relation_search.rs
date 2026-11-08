use graphql_orm::prelude::*;
#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Record {
    #[primary_key]
    id: String,
    label: String,

    owner_id: String,
    #[input_only]
    #[graphql_orm(search_relation(fields = ["label"]))]
    #[relation(target = "Record", from = "owner_id", to = "id", emit_fk = false, multiple)]
    owner: Vec<Record>,
}
fn main() {}
