use graphql_orm::prelude::*;

#[derive(graphql_orm::async_graphql::SimpleObject, GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "hidden_fields", plural = "HiddenFields", schema_only = true)]
struct HiddenField {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    label: String,
    #[graphql_orm(private)]
    secret: String,
}
fn main() {}
