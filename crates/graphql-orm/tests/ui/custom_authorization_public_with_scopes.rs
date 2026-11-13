use graphql_orm::prelude::*;

#[derive(Default)]
struct Queries;

#[graphql_orm_custom_operations(kind = "query", authorization = false, all_scopes = ["reports.read"])]
#[graphql_orm::async_graphql::Object]
impl Queries {
    /// Returns a status.
    async fn status(&self) -> String {
        String::new()
    }
}

fn main() {}
