use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "collision_items", plural = "Items")]
struct Item {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    label: String,
}

#[derive(Default)]
struct ApplicationEvents;

#[graphql_orm_custom_operations(kind = "subscription")]
#[graphql_orm::async_graphql::Subscription]
impl ApplicationEvents {
    /// Observes handwritten item events under the generated name.
    async fn item_changed(&self) -> impl graphql_orm::futures::Stream<Item = i32> {
        graphql_orm::futures::stream::empty()
    }
}

schema_roots! {
    backend: "sqlite",
    entities: [Item],
    described_subscription_types: [ApplicationEvents],
}

fn main() {}
