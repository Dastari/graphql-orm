use graphql_orm::prelude::*;

#[derive(GraphQLEntity)]
#[graphql_entity(table = "tickets", plural = "Tickets", auth = "none")]
struct Ticket {
    #[primary_key]
    id: String,
    #[graphql(name = "RecordVersion")]
    label: String,
    #[graphql_orm(version = "managed", default = "0")]
    row_version: i64,
}

fn main() {}
