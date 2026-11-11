use graphql_orm::prelude::*;

#[derive(Default)]
struct Mutation;

#[graphql_orm_custom_operations(kind = "mutation")]
#[graphql_orm::async_graphql::Object(rename_fields = "snake_case")]
impl Mutation {
    async fn rotate_key(&self) -> bool {
        true
    }
}

fn main() {}
