use graphql_orm::prelude::*;

#[derive(Default)]
struct Subscription;

#[graphql_orm_custom_operations(kind = "subscription")]
#[graphql_orm::async_graphql::Object]
impl Subscription {
    async fn ticks(&self) -> bool {
        true
    }
}

fn main() {}
