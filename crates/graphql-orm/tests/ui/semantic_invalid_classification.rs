use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug)]
#[graphql_entity(table = "invalid_semantics", plural = "InvalidSemantics", classification = "private")]
struct InvalidSemantics {
    #[primary_key]
    id: String,
}

fn main() {}
