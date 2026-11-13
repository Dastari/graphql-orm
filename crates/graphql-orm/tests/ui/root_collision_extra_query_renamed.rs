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
struct LegacyQueries;

// The root's rename rule decides the exposed name, here `items`.
#[graphql_orm_root_fields]
#[graphql_orm::async_graphql::Object(rename_fields = "lowercase")]
impl LegacyQueries {
    #[allow(non_snake_case)]
    async fn ITEMS(&self) -> i32 {
        0
    }
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [Item],
    extra_query_types: [LegacyQueries],
}

fn main() {}
