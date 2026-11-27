use graphql_orm::prelude::GraphQLOperations;

#[derive(GraphQLOperations)]
#[graphql_entity(table = "records", plural = "Records-ByDay")]
struct Record {
    #[primary_key]
    id: String,
}

#[derive(GraphQLOperations)]
#[graphql_entity(table = "entries", plural = "EntriesResult//")]
struct Entry {
    #[primary_key]
    id: String,
}

fn main() {}
