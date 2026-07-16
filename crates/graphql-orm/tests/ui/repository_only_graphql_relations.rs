use graphql_orm::prelude::*;

#[derive(
    RepositoryEntity,
    GraphQLRelations,
    Clone,
    serde::Serialize,
    serde::Deserialize,
)]
#[repository_entity(backend = "sqlite", table = "private_records", plural = "PrivateRecords")]
struct PrivateRecord {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    value: String,
}

fn main() {}
