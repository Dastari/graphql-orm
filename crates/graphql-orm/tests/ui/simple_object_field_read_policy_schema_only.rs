use graphql_orm::prelude::*;
use graphql_orm::async_graphql::SimpleObject;

#[derive(GraphQLEntity, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "policy_fields",
    plural = "PolicyFields",
    schema_only = true
)]
struct PolicyField {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[graphql_orm(read_policy = "secret.read")]
    secret: String,
}
fn main() {}
