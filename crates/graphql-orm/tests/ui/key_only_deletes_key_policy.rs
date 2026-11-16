use graphql_orm::prelude::*;

// A key-only DELETED event releases the key with no row to check a field
// policy against, so the opt-in is refused when a key field has one.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "tickets", plural = "Tickets", key_only_deletes)]
struct Ticket {
    #[primary_key]
    #[graphql_orm(auto_generated = false, read_policy = "tickets.key")]
    #[sortable]
    id: String,
    label: String,
}

fn main() {}
