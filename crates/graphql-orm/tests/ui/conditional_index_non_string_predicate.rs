use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity)]
#[graphql_entity(table = "bad_conditional_type", plural = "BadConditionalTypes")]
#[graphql_orm(conditional_index(
    columns = ["digest"],
    unique = true,
    predicate_field = "status",
    predicate_values = ["1"]
))]
struct BadConditionalType {
    #[primary_key]
    id: String,
    digest: Vec<u8>,
    status: i64,
}

fn main() {}
