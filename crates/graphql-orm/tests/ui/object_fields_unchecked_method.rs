// The hidden wrapper behind `#[graphql_object_fields]` takes one handwritten
// impl and wraps every resolver in it. It has no position for a method that
// would join the object without the field check, so a block of methods beside
// the impl is refused.
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
#[graphql_orm(object_fields)]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
}

graphql_orm::__graphql_object_fields_wrap! {
    "Record" { ::graphql_orm::SqliteBackend } {} {} ["id"]
    { __gom_object_fields_Record }
    {
        async fn unchecked(&self) -> graphql_orm::async_graphql::Result<i32> {
            Ok(1)
        }
    }
    impl Record {}
}

fn main() {}
