use graphql_orm::prelude::*;

#[derive(GraphQLOperations)]
#[graphql_entity(backend = "sqlite", table = "records")]
struct Record {
    #[primary_key]
    id: String,
    
    #[transform(write = "protect")]
    value: i64,
}

fn main() {}
