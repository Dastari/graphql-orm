use graphql_orm::prelude::*;

#[derive(Default)]
struct Query;

#[graphql_orm_custom_operations(kind = "command")]
#[graphql_orm::async_graphql::Object]
impl Query {
    async fn value(&self) -> String {
        String::new()
    }
}

fn main() {}
