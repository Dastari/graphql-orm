use graphql_orm::prelude::*;

#[derive(Default)]
struct Mutations;

#[graphql_orm_custom_operations(kind = "mutation", all_scopes = ["notes.write"])]
#[graphql_orm::async_graphql::Object]
impl Mutations {
    /// States a different requirement without saying it replaces the root's.
    #[graphql_orm(authorization(all_scopes = ["notes.read"]))]
    async fn peek_note(&self) -> bool {
        true
    }
}

fn main() {}
