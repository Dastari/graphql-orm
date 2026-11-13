use graphql_orm::prelude::*;

#[derive(GraphQLOperations)]
#[graphql_entity(backend = "sqlite", table = "records", mutation_shapes = "create")]
struct Record {
    #[primary_key]
    id: String,
    
    #[transform(write = "protect")]
    value: String,
}

fn main() {}
