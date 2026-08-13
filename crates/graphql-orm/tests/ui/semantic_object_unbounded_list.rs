use graphql_orm::prelude::*;

#[derive(GraphQLSemanticObject)]
struct UnboundedResult {
    values: Vec<String>,
}

fn main() {}
