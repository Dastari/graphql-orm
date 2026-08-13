use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug)]
#[graphql_entity(table = "invalid_semantics", plural = "InvalidSemantics")]
struct InvalidSemantics {
    #[primary_key]
    id: String,
    #[graphql_orm(sensitive, classification = "internal")]
    secret: String,
}

fn main() {}
