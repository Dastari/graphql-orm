use graphql_orm::prelude::*;
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "public_clock", plural = "PublicClocks")]
struct PublicClock {
    #[primary_key]
    id: String,
    #[graphql_orm(timestamp = "host")]
    created_at: i64,
}
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(table = "text_clock", plural = "TextClocks")]
struct TextClock {
    #[primary_key]
    id: String,
    #[graphql_orm(timestamp = "host")]
    updated_at: String,
}
fn main() {}
