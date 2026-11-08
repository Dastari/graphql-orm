use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "sqlite", table = "private_records", plural = "PrivateRecords")]
struct PrivateRecord {
    #[primary_key]
    #[graphql_orm(private, auto_generated = false)]
    id: String,
    #[graphql_orm(private)]
    owner_id: String,
    label: String,
    #[relation(target = "PrivateRecord", from = "owner_id", to = "id", emit_fk = false)]
    owner: Vec<PrivateRecord>,
}

fn main() {}
