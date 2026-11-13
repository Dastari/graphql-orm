use graphql_orm::prelude::*;

#[derive(Default)]
struct Mutation;

#[graphql_orm_custom_operations(kind = "mutation")]
#[graphql_orm::async_graphql::Object(rename_args = "snake_case")]
impl Mutation {
    /// Rotates one key.
    async fn rotate_key(&self, key_name: String) -> bool {
        !key_name.is_empty()
    }
}

fn main() {}
