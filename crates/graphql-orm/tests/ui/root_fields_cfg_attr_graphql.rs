use graphql_orm::prelude::*;

#[derive(Default)]
struct LegacyQueries;

#[graphql_orm_root_fields]
#[graphql_orm::async_graphql::Object]
impl LegacyQueries {
    // The name would be applied after the macro has read the resolver.
    #[cfg_attr(feature = "sqlite", graphql(name = "items"))]
    async fn status(&self) -> i32 {
        0
    }
}

fn main() {}
