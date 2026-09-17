use graphql_orm::prelude::*;

#[derive(RepositoryEntity)]
#[repository_entity(table = "zones", federation_key)]
struct Zone {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    name: String,
}

fn main() {}
