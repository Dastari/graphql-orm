use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "records", subscriptions = false, changed_started = true)]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
}

fn main() {}
