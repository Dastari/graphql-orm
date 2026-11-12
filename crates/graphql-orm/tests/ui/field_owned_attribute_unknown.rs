use graphql_orm::prelude::*;

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(backend = "sqlite", table = "shelves", plural = "Shelves")]
struct Shelf {
    #[primary_key]
    pub shelf_id: String,
    #[backup(encrypt)]
    pub title: String,
}

fn main() {}
