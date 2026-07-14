use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "mutable_events",
    plural = "MutableEvents",
    retention_purge = "events.purge"
)]
struct MutableEvent {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
}

fn main() {}
