use graphql_orm::prelude::*;

// A key-only DELETED event would release a key field that GraphQL skips, so
// the opt-in is refused when a key field is `graphql(skip)`.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "tickets", plural = "Tickets", key_only_deletes)]
struct Ticket {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[graphql(skip)]
    #[sortable]
    id: String,
    label: String,
}

fn main() {}
