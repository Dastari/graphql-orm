use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "sqlite", table = "records", default_sort = "id ASC NULLS LAST")]
struct Record {
    #[primary_key]
    id: String,
}

fn main() {}
