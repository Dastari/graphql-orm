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

// No `#[graphql_orm_root_fields]`: its field names are unknown to schema_roots!.
#[graphql_orm::async_graphql::Object]
impl LegacyQueries {
    async fn status(&self) -> i32 {
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
