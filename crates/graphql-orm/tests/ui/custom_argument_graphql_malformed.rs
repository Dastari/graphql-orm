use graphql_orm::prelude::*;

#[derive(Default)]
struct InventoryQueries;

#[graphql_orm_custom_operations(kind = "query", authorization = true)]
#[graphql_orm::async_graphql::Object]
impl InventoryQueries {
    /// Counts volumes on one shelf.
    async fn count_volumes(&self, #[graphql(name = "shelfKey" default = 1)] shelf: i32) -> i32 {
        shelf
    }
}

fn main() {}
