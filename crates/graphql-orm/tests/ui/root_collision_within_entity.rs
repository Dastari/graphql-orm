use graphql_orm::prelude::*;

// The plural equals the entity name: the list and get-by-key roots would
// both be `item`.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "collision_items", plural = "Item")]
struct Item {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [Item],
}

fn main() {}
