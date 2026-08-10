use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity)]
#[graphql_entity(
    table = "bad_index_empty_name",
    plural = "BadIndexEmptyNames",
    index(name = " ", columns = ["generation"], directions = ["desc"])
)]
struct BadIndexEmptyName {
    #[primary_key]
    generation: i64,
}

fn main() {}
