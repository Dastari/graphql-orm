use graphql_orm::prelude::*;

// Deleting the parent would make the database (or a dependent-delete plan)
// clear the child's key without its ExpectedVersion or assurance selection.

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "owners", plural = "Owners", default_sort = "id ASC")]
struct Owner {
    #[primary_key]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    name: String,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    table = "claims",
    plural = "Claims",
    default_sort = "id ASC",
    mutation_shapes = "create,update,delete",
    update_expected_version = "required"
)]
struct Claim {
    #[primary_key]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    owner_id: Option<String>,
    #[relation(target = "Owner", from = "owner_id", to = "id", on_delete = "set_null")]
    owner: Option<Owner>,
    #[graphql_orm(version = "managed", default = "0")]
    row_version: i64,
}

fn main() {}
