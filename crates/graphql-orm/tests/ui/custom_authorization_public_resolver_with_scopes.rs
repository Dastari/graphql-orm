use graphql_orm::prelude::*;

#[derive(Default)]
struct Queries;

#[graphql_orm_custom_operations(kind = "query")]
#[graphql_orm::async_graphql::Object]
impl Queries {
    /// Declares itself public and scoped at once.
    #[graphql_orm(authorization = false, all_scopes = ["reports.read"])]
    async fn status(&self) -> String {
        String::new()
    }
}

fn main() {}
