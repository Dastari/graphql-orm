use graphql_orm::prelude::*;

#[derive(Default)]
struct Mutation;

#[graphql_orm_custom_operations(kind = "mutation")]
#[graphql_orm::async_graphql::Object]
impl Mutation {
    #[graphql(name = "rotateKey")]
    #[graphql(entity)]
    async fn rotate_key(&self) -> bool {
        true
    }
}

fn main() {}
