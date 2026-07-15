use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "sqlite", table = "private_records", plural = "PrivateRecords")]
struct PrivateRecord {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    value: String,
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [PrivateRecord],
}

fn main() {}
