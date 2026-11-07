use graphql_orm::prelude::*;
use graphql_orm::async_graphql::SimpleObject;

#[derive(GraphQLEntity, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(complex)]
#[graphql_entity(backend = "sqlite", table = "hidden_fields", plural = "HiddenFields")]
struct HiddenField {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    label: String,
    #[graphql_orm(read = false)]
    secret: String,
}
#[graphql_orm::async_graphql::ComplexObject]
impl HiddenField {}
fn main() {}
