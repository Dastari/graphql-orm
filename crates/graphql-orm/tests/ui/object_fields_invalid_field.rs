// A handwritten field must be able to report a refused read, and the
// attribute takes no object options.
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
#[graphql_orm(object_fields)]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
}

#[graphql_object_fields]
impl Record {
    async fn infallible(&self) -> i32 {
        1
    }
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "others", plural = "Others")]
#[graphql_orm(object_fields)]
struct Other {
    #[primary_key]
    #[sortable]
    id: String,
}

#[graphql_object_fields(rename_fields = "PascalCase")]
impl Other {
    async fn one(&self) -> graphql_orm::async_graphql::Result<i32> {
        Ok(1)
    }
}

fn main() {}
