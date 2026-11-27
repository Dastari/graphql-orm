use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "mssql", table = "records", default_sort = "[a]]")]
struct UnclosedBracket {
    #[primary_key]
    id: String,
}

fn main() {}
