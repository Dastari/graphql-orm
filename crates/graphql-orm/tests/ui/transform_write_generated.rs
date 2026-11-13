use graphql_orm::prelude::*;

#[derive(GraphQLOperations)]
#[graphql_entity(backend = "sqlite", table = "records")]
struct Record {
    #[primary_key]
    id: String,
    #[graphql_orm(auto_generated = true)]
    #[transform(write = "protect")]
    value: String,
}

fn main() {}
