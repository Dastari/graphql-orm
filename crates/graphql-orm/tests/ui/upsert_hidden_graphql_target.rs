use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity,
    GraphQLOperations,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql_entity(
    table = "upsert_hidden_target_examples",
    plural = "UpsertHiddenTargetExamples",
    upsert = "mac"
)]
struct UpsertHiddenTargetExample {
    #[primary_key]
    pub id: String,

    #[unique]
    #[graphql_orm(skip_input)]
    pub mac: String,

    #[filterable(type = "string")]
    #[sortable]
    pub title: String,
}

fn main() {}
