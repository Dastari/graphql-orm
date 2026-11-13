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
struct ApplicationQueries;

#[graphql_orm_custom_operations(kind = "query")]
#[graphql_orm::async_graphql::Object]
impl ApplicationQueries {
    /// Returns a handwritten list under the generated list's name.
    async fn items(&self) -> String {
        String::new()
    }
}

schema_roots! {
    backend: "sqlite",
    entities: [Item],
    described_query_types: [ApplicationQueries],
}

fn main() {}
