use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLRelations, SimpleObject, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(backend = "sqlite", table = "shelves", plural = "Shelves")]
#[graphql(rename_fields = "PascalCase" complex)]
struct Shelf {
    #[primary_key]
    pub shelf_id: String,
}

fn main() {}
