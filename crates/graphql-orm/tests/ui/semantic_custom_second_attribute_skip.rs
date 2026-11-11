use graphql_orm::prelude::*;

#[derive(Default)]
struct Mutation;

struct HostGuard;

impl graphql_orm::async_graphql::Guard for HostGuard {
    async fn check(
        &self,
        _ctx: &graphql_orm::async_graphql::Context<'_>,
    ) -> graphql_orm::async_graphql::Result<()> {
        Ok(())
    }
}

#[graphql_orm_custom_operations(kind = "mutation")]
#[graphql_orm::async_graphql::Object]
impl Mutation {
    #[graphql(guard = "HostGuard")]
    #[graphql(skip)]
    async fn rotate_key(&self) -> bool {
        true
    }
}

fn main() {}
