use graphql_orm::async_graphql::Object as GqlObject;
use graphql_orm::prelude::*;

#[derive(Default)]
struct Mutation;

struct AdminGuard;

impl graphql_orm::async_graphql::Guard for AdminGuard {
    async fn check(
        &self,
        _ctx: &graphql_orm::async_graphql::Context<'_>,
    ) -> graphql_orm::async_graphql::Result<()> {
        Ok(())
    }
}

#[graphql_orm_custom_operations(kind = "mutation")]
#[GqlObject(guard = "AdminGuard")]
impl Mutation {
    async fn rotate_key(&self) -> bool {
        true
    }
}

fn main() {}
