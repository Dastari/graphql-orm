use graphql_orm::prelude::*;

#[derive(RepositoryEntity)]
#[repository_entity(backend = "sqlite", table = "records")]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,

    #[transform(write = "protect")]
    value: String,
}

fn main() {}
