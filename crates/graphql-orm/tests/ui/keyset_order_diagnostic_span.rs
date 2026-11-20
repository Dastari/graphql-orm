use graphql_orm::prelude::GraphQLOperations;

#[derive(GraphQLOperations)]
#[graphql_entity(
    table = "records",
    backend = "sqlite",
    keyset = "id desc nulls",
)]
struct Record {
    #[primary_key]
    id: String,
}

fn main() {}
