use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

#[derive(SimpleObject)]
struct Inner {
    status: i32,
}

#[derive(Default)]
struct LegacyQueries;

#[graphql_orm_root_fields]
#[graphql_orm::async_graphql::Object]
impl LegacyQueries {
    #[graphql(flatten)]
    async fn inner(&self) -> Inner {
        Inner { status: 0 }
    }
}

fn main() {}
