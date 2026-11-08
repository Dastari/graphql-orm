// Metadata-only derives leave object visibility to the host.
use graphql_orm::prelude::*;
use graphql_orm::async_graphql::SimpleObject;

#[derive(GraphQLSchemaEntity, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "hidden_fields", plural = "HiddenFields")]
struct HiddenField {
    #[primary_key]
    id: String,
    #[graphql_orm(private)]
    secret: String,
}
struct Query;
#[graphql_orm::async_graphql::Object]
impl Query {
    async fn row(&self) -> HiddenField { HiddenField { id: "id".into(), secret: "hidden".into() } }
}
fn main() {
    let schema = graphql_orm::async_graphql::Schema::build(
        Query, graphql_orm::async_graphql::EmptyMutation,
        graphql_orm::async_graphql::EmptySubscription,
    ).finish();
    assert!(schema.sdl().contains("secret:"), "metadata-only derive must preserve the host-owned object");
}
