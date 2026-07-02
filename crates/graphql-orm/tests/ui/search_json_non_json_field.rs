use graphql_orm::prelude::*;

#[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(table = "bad_search_json_records", backend = "sqlite")]
#[graphql_orm(search(index = true))]
struct BadSearchJsonRecord {
    #[primary_key]
    pub id: String,

    #[graphql_orm(search_json(path = "$.summary", weight = "C"))]
    pub title: String,
}

fn main() {}
