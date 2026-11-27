use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "sqlite", table = "records", default_sort = "id; DROP TABLE x")]
struct Statement {
    #[primary_key]
    id: String,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "sqlite", table = "records", default_sort = "id--")]
struct LineComment {
    #[primary_key]
    id: String,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "sqlite", table = "records", default_sort = "id/**/ASC")]
struct BlockComment {
    #[primary_key]
    id: String,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "sqlite", table = "records", default_sort = "")]
struct Empty {
    #[primary_key]
    id: String,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "sqlite", table = "records", default_sort = "id,")]
struct TrailingComma {
    #[primary_key]
    id: String,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "sqlite", table = "records", default_sort = "other_table.id")]
struct ForeignTable {
    #[primary_key]
    id: String,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "sqlite", table = "records", default_sort = r#""a"""#)]
struct UnclosedQuote {
    #[primary_key]
    id: String,
}

fn main() {}
