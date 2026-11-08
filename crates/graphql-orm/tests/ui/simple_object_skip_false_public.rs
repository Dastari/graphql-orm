use graphql_orm::prelude::*;
use graphql_orm::async_graphql::SimpleObject;

#[derive(GraphQLEntity, GraphQLOperations, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(complex)]
#[graphql_entity(backend = "sqlite", table = "hidden_fields", plural = "HiddenFields")]
struct HiddenField {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    label: String,
    #[graphql(skip = false)]
    secret: String,
}
#[graphql_orm::async_graphql::ComplexObject]
impl HiddenField {}
struct Query;
#[graphql_orm::async_graphql::Object]
impl Query {
    async fn row(&self, _input: Option<GraphQLCreateHiddenFieldInput>) -> HiddenField {
        HiddenField { id: "id".into(), label: "label".into(), secret: "hidden".into() }
    }
}
fn main() {
    let schema = graphql_orm::async_graphql::Schema::build(
        Query, graphql_orm::async_graphql::EmptyMutation,
        graphql_orm::async_graphql::EmptySubscription,
    ).finish();
    let sdl = schema.sdl();
    let start = sdl.find("input CreateHiddenFieldInput {").unwrap();
    let input = &sdl[start..start + sdl[start..].find('}').unwrap()];
    assert!(!input.contains("secret:"), "legacy input exclusion changed: {input}");
    assert!(sdl.contains("secret:"), "skip = false must keep the public getter");
}
