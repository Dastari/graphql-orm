use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity)]
#[graphql_entity(
    table = "bad_index_direction_arity",
    plural = "BadIndexDirectionArities",
    index(
        name = "idx_bad_direction_arity",
        columns = ["tenant", "generation"],
        directions = ["desc"]
    )
)]
struct BadIndexDirectionArity {
    #[primary_key]
    tenant: String,
    generation: i64,
}

fn main() {}
