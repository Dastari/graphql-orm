use graphql_orm::prelude::*;

#[derive(Default)]
struct Admin;

#[graphql_orm::async_graphql::Object]
impl Admin {
    async fn secret(&self) -> String {
        String::new()
    }
}

#[derive(Default)]
struct Query;

#[graphql_orm_custom_operations(kind = "query")]
#[graphql_orm::async_graphql::Object]
impl Query {
    #[graphql(flatten)]
    async fn admin(&self) -> Admin {
        Admin
    }
}

fn main() {}
