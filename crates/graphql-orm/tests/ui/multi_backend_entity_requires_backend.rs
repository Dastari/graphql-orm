use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug)]
#[graphql_entity(table = "missing_backend_examples", plural = "MissingBackendExamples")]
struct MissingBackendExample {
    #[primary_key]
    pub id: String,
}

fn main() {}
