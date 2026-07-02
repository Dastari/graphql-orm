use graphql_orm::prelude::*;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
struct Content {
    summary: String,
}

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(table = "bad_search_json_paths", backend = "sqlite")]
#[graphql_orm(search(index = true))]
struct BadSearchJsonPath {
    #[primary_key]
    pub id: String,

    #[graphql_orm(json)]
    #[graphql_orm(search_json(path = "$.keywords[0].label", weight = "C"))]
    pub content: Content,
}

fn main() {}
