use graphql_orm::prelude::*;

// Each opt-in requires the generated single-row update and refuses every
// other root that rewrites an existing row.

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    table = "disabled",
    mutations = false,
    update_assurance(select = "select_disabled", levels = "write")
)]
struct Disabled {
    #[primary_key]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    state: String,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    table = "no_update",
    mutation_shapes = "create,delete",
    update_expected_version = "required"
)]
struct NoUpdate {
    #[primary_key]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    state: String,
    #[graphql_orm(version = "managed", default = "0")]
    row_version: i64,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    table = "upserted",
    upsert = "code",
    mutation_shapes = "create,upsert,update",
    update_expected_version = "required"
)]
struct Upserted {
    #[primary_key]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[filterable(type = "string")]
    #[sortable]
    code: String,
    #[filterable(type = "string")]
    state: String,
    #[graphql_orm(version = "managed", default = "0")]
    row_version: i64,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    table = "bulk",
    update_assurance(select = "select_bulk", levels = "write")
)]
struct Bulk {
    #[primary_key]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    state: String,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    dependent_deletes,
    table = "dependent",
    mutation_shapes = "create,update,delete",
    update_assurance(select = "select_dependent", levels = "write")
)]
struct Dependent {
    #[primary_key]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    state: String,
}

fn select_disabled(
    _: graphql_orm::graphql::assurance::UpdateAssuranceInput<'_, Disabled, UpdateDisabledInput>,
) -> async_graphql::Result<&'static str> {
    Ok("write")
}

fn select_bulk(
    _: graphql_orm::graphql::assurance::UpdateAssuranceInput<'_, Bulk, UpdateBulkInput>,
) -> async_graphql::Result<&'static str> {
    Ok("write")
}

fn select_dependent(
    _: graphql_orm::graphql::assurance::UpdateAssuranceInput<'_, Dependent, UpdateDependentInput>,
) -> async_graphql::Result<&'static str> {
    Ok("write")
}

fn main() {}
