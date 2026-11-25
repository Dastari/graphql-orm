use graphql_orm::prelude::*;

#[derive(GraphQLEntity)]
#[graphql_entity(table = "records", mutations = false, mutations = true)]
struct Record {
    #[primary_key]
    id: String,
}

fn main() {}
