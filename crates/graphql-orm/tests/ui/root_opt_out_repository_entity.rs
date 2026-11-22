use graphql_orm::prelude::*;

#[derive(RepositoryEntity)]
#[repository_entity(table = "records", mutations = false)]
struct Record {
    #[primary_key]
    id: String,
}

fn main() {}
