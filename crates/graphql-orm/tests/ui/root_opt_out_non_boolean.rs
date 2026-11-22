use graphql_orm::prelude::*;

#[derive(GraphQLEntity)]
#[graphql_entity(table = "records", subscriptions = "false")]
struct Record {
    #[primary_key]
    id: String,
}

fn main() {}
