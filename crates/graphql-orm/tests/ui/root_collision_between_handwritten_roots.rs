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
struct StatusQueries;

#[graphql_orm_custom_operations(kind = "query")]
#[graphql_orm::async_graphql::Object]
impl StatusQueries {
    /// Returns the described status.
    async fn status(&self) -> String {
        String::new()
    }
}

#[derive(Default)]
struct LegacyQueries;

#[graphql_orm_root_fields]
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
    described_query_types: [StatusQueries],
}

fn main() {}
