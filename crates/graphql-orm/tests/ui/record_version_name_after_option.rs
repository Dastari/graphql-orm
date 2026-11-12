// The explicit name is read wherever it appears among the field's options.
use graphql_orm::prelude::*;

#[derive(GraphQLEntity)]
#[graphql_entity(table = "tickets", plural = "Tickets", auth = "none")]
struct Ticket {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql(deprecation = "old", name = "RecordVersion")]
    label: String,
    #[graphql_orm(version = "managed", default = "0")]
    row_version: i64,
}

fn main() {}
