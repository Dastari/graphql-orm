use graphql_orm::prelude::*;

#[derive(Default)]
struct Mutation;

#[graphql_orm_custom_operations(kind = "mutation")]
impl Mutation {
    async fn rotate_key(&self) -> bool {
        true
    }
}

fn main() {}
