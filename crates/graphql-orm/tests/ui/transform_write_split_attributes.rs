use graphql_orm::prelude::*;

#[derive(GraphQLOperations)]
#[graphql_entity(backend = "sqlite", table = "records")]
struct Record {
    #[primary_key]
    id: String,
    
    #[transform(read = "reveal")]
    #[transform(write = "protect")]
    value: String,
}

fn main() {}
