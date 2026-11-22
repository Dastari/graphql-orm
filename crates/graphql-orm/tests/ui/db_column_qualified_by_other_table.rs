use graphql_orm::prelude::*;

// A qualified declaration has no unambiguous physical column name.
#[derive(GraphQLEntity, Clone, Debug)]
#[graphql_entity(table = "main.children", plural = "Children")]
struct Child {
    #[primary_key]
    pub id: String,
    #[graphql_orm(db_column = "\"children\".\"owner\"")]
    pub owner: Option<String>,
}

fn main() {}
