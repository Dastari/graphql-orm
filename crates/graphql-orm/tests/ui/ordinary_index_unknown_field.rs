use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity)]
#[graphql_entity(table = "bad_index_unknown", plural = "BadIndexUnknowns", index = "missing")]
struct BadIndexUnknown {
    #[primary_key]
    id: String,
}

fn main() {}
