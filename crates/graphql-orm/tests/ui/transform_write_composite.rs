use graphql_orm::prelude::*;

#[derive(RepositoryEntity)]
#[repository_entity(backend = "sqlite", table = "records", repository_mutations = true)]
struct Record {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    region: i64,
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    slot: i64,
    #[transform(write = "protect")]
    value: i64,
}

fn main() {}
