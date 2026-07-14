use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "mssql",
    table = "mssql_retained_events",
    plural = "MssqlRetainedEvents",
    append_only = true,
    retention_purge = "retained_event.purge"
)]
struct MssqlRetainedEvent {
    #[primary_key]
    id: String,
    value: String,
}

fn main() {}
