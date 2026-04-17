use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(
    table = "upsert_non_unique_examples",
    plural = "UpsertNonUniqueExamples",
    upsert = "slug"
)]
struct UpsertNonUniqueExample {
    #[primary_key]
    pub id: String,

    pub slug: String,

    pub title: String,
}

fn main() {}
