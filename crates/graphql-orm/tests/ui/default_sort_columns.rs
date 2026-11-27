use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "sqlite", table = "records", default_sort = r#"  "line, number" desc , "order" ASC, id "#)]
struct Record {
    #[primary_key]
    id: String,
    #[graphql_orm(db_column = "line, number")]
    line: i64,
    #[graphql_orm(db_column = "order")]
    position: i64,
}

fn main() {}
