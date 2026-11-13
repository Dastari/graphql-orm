use graphql_orm::prelude::*;
use graphql_orm::async_graphql::SimpleObject;

// The inert marker alone, without the guard `graphql_policy_object` installs.
#[derive(GraphQLEntity, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(complex)]
#[graphql_orm_policy_object]
#[graphql_entity(backend = "sqlite", table = "policy_fields", plural = "PolicyFields")]
struct PolicyField {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[graphql_orm(read_policy = "secret.read")]
    secret: String,
}
#[graphql_orm::async_graphql::ComplexObject]
impl PolicyField {}
fn main() {}
