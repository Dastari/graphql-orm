use graphql_orm::prelude::*;

#[derive(Default)]
struct Query;

#[graphql_orm_custom_operations(kind = "query")]
#[graphql_orm::async_graphql::Object]
impl Query {
    #[graphql(derived(name = "secret_text", into = "String"))]
    async fn secret(&self) -> String {
        String::new()
    }
}

fn main() {}
