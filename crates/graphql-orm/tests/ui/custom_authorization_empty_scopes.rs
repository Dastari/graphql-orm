use graphql_orm::prelude::*;

#[derive(Default)]
struct Queries;

#[graphql_orm_custom_operations(kind = "query")]
#[graphql_orm::async_graphql::Object]
impl Queries {
    /// Returns a status.
    #[graphql_orm(any_scopes = [["reports.read"], []])]
    async fn status(&self) -> String {
        String::new()
    }
}

fn main() {}
