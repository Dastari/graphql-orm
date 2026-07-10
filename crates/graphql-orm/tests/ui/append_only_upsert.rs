use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(table = "events", plural = "Events", append_only = true, upsert = "kind")]
struct Event {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[filterable(type = "string")]
    #[sortable]
    kind: String,
}

fn main() {}
