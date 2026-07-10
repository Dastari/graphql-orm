use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity)]
#[graphql_entity(table = "bad_conditional_unknown", plural = "BadConditionalUnknowns")]
#[graphql_orm(conditional_index(
    columns = ["missing"],
    unique = true,
    predicate_field = "status",
    predicate_values = ["active"]
))]
struct BadConditionalUnknown {
    #[primary_key]
    id: String,
    status: String,
}

fn main() {}
