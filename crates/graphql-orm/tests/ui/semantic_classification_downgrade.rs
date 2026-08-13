use graphql_orm::prelude::*;

#[derive(GraphQLEntity)]
#[graphql_entity(table = "classification_downgrade", classification = "confidential")]
struct ClassificationDowngrade {
    #[primary_key]
    id: String,
    #[graphql_orm(classification = "public")]
    label: String,
}

fn main() {}
