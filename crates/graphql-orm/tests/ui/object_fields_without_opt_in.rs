// `#[graphql_object_fields]` joins the object of an entity that opted in with
// `#[graphql_orm(object_fields)]`; without it the object is already closed.
use graphql_orm::async_graphql::Result;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
    name: String,
}

#[graphql_object_fields]
impl Record {
    async fn name_length(&self) -> Result<i32> {
        Ok(self.name.len() as i32)
    }
}

fn main() {}
