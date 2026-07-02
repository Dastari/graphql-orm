use graphql_orm::prelude::*;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
struct Content {
    summary: String,
}

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(table = "private_search_json_records", backend = "sqlite")]
#[graphql_orm(search(index = true))]
struct PrivateSearchJsonRecord {
    #[primary_key]
    pub id: String,

    #[graphql_orm(private)]
    #[graphql_orm(json)]
    #[graphql_orm(search_json(path = "$.summary", weight = "C"))]
    pub content: Content,
}

fn main() {}
