use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "shaped", append_only = true, mutation_shapes = "create,delete")]
struct Shaped {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[filterable(type = "string")]
    #[sortable]
    code: String,
}

fn main() {}
