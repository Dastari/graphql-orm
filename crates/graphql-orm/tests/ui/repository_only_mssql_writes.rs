use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "private_records",
    plural = "PrivateRecords",
    upsert = "external_key"
)]
struct PrivateRecord {
    #[primary_key]
    id: String,
    #[unique]
    external_key: String,
}

fn main() {}
