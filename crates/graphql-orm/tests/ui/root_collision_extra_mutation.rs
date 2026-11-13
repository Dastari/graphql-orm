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
struct LegacyMutations;

#[graphql_orm_root_fields]
#[graphql_orm::async_graphql::Object]
impl LegacyMutations {
    async fn delete_item(&self, id: String) -> bool {
        !id.is_empty()
    }
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [Item],
    extra_mutation_types: [LegacyMutations],
}

fn main() {}
