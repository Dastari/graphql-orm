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
struct ApplicationMutations;

#[graphql_orm_custom_operations(kind = "mutation")]
#[graphql_orm::async_graphql::Object]
impl ApplicationMutations {
    /// Deletes an item with handwritten validation.
    #[graphql(name = "deleteItem")]
    async fn remove(&self, id: String) -> bool {
        !id.is_empty()
    }
}

schema_roots! {
    backend: "sqlite",
    entities: [Item],
    described_mutation_types: [ApplicationMutations],
}

fn main() {}
