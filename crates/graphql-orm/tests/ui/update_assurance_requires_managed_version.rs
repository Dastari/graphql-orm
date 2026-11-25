use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    table = "reviews",
    mutation_shapes = "create,update,delete",
    update_expected_version = "required"
)]
struct Review {
    #[primary_key]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    state: String,
}

fn main() {}
