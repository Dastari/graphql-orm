use graphql_orm::prelude::*;

#[derive(Default)]
struct Mutations;

#[graphql_orm_custom_operations(kind = "mutation", all_scopes = ["notes.write"])]
#[graphql_orm::async_graphql::Object]
impl Mutations {
    /// A replacement states the whole requirement; nothing is added to it.
    #[graphql_orm(authorization(replace), all_scopes = ["notes.read"])]
    async fn peek_note(&self) -> bool {
        true
    }
}

fn main() {}
