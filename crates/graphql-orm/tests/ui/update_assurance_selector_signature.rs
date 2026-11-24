use graphql_orm::prelude::*;

// The selector receives the typed patch and locked row; a mismatched
// signature is a type error at the generated call.
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    table = "reviews",
    mutation_shapes = "create,update",
    update_assurance(select = "select", levels = "write")
)]
struct Review {
    #[primary_key]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    state: String,
}

fn select(_: &Review) -> &'static str {
    "write"
}

fn main() {}
