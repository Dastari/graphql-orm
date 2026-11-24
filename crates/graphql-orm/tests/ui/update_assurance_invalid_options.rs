use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    table = "reviews",
    mutation_shapes = "create,update",
    update_assurance(select = "select", levels = "write,with space")
)]
struct Review {
    #[primary_key]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    state: String,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    table = "drafts",
    mutation_shapes = "create,update",
    update_assurance(levels = "write")
)]
struct Draft {
    #[primary_key]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    state: String,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "notes", update_expected_version = "optional")]
struct Note {
    #[primary_key]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    state: String,
}

fn main() {}
