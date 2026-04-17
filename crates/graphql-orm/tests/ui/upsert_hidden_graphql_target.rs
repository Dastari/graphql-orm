use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
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

    pub title: String,
}

fn main() {}
