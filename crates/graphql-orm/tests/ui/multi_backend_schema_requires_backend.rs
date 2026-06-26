use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "explicit_backend_examples",
    plural = "ExplicitBackendExamples"
)]
struct ExplicitBackendExample {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

schema_roots! {
    query_custom_ops: [],
    entities: [ExplicitBackendExample],
}

fn main() {}
