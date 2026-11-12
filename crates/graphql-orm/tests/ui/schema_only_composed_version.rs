// A schema-only entity has no generated object, so GraphQLRelations keeps
// owning its composed object even without relation fields.
use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLRelations, SimpleObject, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql(complex)]
#[graphql_orm(compose_complex_object)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records", schema_only = true)]
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
