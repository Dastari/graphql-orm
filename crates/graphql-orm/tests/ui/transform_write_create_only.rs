use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", mutation_shapes = "create")]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,

    #[transform(write = "protect")]
    value: String,
}

fn main() {}
