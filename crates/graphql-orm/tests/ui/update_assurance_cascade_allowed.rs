use graphql_orm::prelude::*;

// A cascade deletes the child row. Deletes are not guarded, so a protected
// entity may declare one.

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
    update_expected_version = "required",
    update_assurance(select = "select_claim", levels = "write")
)]
struct Claim {
    #[primary_key]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    owner_id: String,
    #[relation(target = "Owner", from = "owner_id", to = "id", on_delete = "cascade")]
    owner: Option<Owner>,
    #[graphql_orm(version = "managed", default = "0")]
    row_version: i64,
}

fn select_claim(
    _: graphql_orm::graphql::assurance::UpdateAssuranceInput<'_, Claim, UpdateClaimInput>,
) -> async_graphql::Result<&'static str> {
    Ok("write")
}

fn main() {}
