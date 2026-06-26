use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "explicit_policy_examples",
    plural = "ExplicitPolicyExamples"
)]
struct ExplicitPolicyExample {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [ExplicitPolicyExample],
}

fn main() {}
