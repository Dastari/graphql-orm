use graphql_orm::prelude::*;

#[derive(Default)]
struct Query;

#[graphql_orm_custom_operations(kind = "query")]
#[graphql_orm::async_graphql::Object]
impl Query {
    #[graphql_orm(result_classification = "secret", result_export = "exportable")]
    async fn value(&self) -> String {
        String::new()
    }
}

fn main() {}
