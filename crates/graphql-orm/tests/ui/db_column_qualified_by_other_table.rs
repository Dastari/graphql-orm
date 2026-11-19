use graphql_orm::prelude::*;

// A `db_column` qualified by another table would make generated filters,
// sorts and projections reference that table (#782).
#[derive(GraphQLEntity, Clone, Debug)]
#[graphql_entity(table = "main.children", plural = "Children")]
struct Child {
    #[primary_key]
    pub id: String,
    #[graphql_orm(db_column = "\"parents\".\"owner\"")]
    pub owner: Option<String>,
}

fn main() {}
