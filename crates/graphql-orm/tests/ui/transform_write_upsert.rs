use graphql_orm::prelude::*;

#[derive(GraphQLOperations)]
#[graphql_entity(backend = "sqlite", table = "records", upsert = "id")]
struct Record {
    #[primary_key]
    id: String,
    
    #[transform(write = "protect")]
    value: String,
}

fn main() {}
