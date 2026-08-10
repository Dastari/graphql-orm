use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity)]
#[graphql_entity(
    table = "bad_index_direction",
    plural = "BadIndexDirections",
    index(
        name = "idx_bad_direction",
        columns = ["generation"],
        directions = ["sideways"]
    )
)]
struct BadIndexDirection {
    #[primary_key]
    generation: i64,
}

fn main() {}
