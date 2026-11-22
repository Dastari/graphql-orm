use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    table = "reviews",
    upsert = "code",
    mutation_shapes = "create,upsert,update",
    update_assurance(select = "select", levels = "write,elevated")
)]
struct Review {
    #[primary_key]
    #[sortable]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[filterable(type = "string")]
    #[sortable]
    code: String,
    #[filterable(type = "string")]
    state: String,
}

fn select(
    _: graphql_orm::graphql::assurance::UpdateAssuranceInput<'_, Review, UpdateReviewInput>,
) -> async_graphql::Result<&'static str> {
    Ok("write")
}

fn main() {}
