// `GraphQLSchemaEntity` has no generated object; a relation-less composed
// entity still receives its composed object, as it did from GraphQLRelations.
use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity, GraphQLRelations, SimpleObject, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql(complex)]
#[graphql_orm(compose_complex_object)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Record {
    #[primary_key]
    id: String,
    #[graphql_orm(version = "managed", default = "0")]
    version: i64,
}

#[graphql_complex_object]
impl Record {
    async fn label(&self) -> String {
        self.id.clone()
    }
}

fn main() {}
