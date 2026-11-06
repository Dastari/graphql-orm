use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "shaped", mutation_shapes = "create,update")]
#[graphql_orm(operation_authorization(
    categories = ["delete"],
    all_scopes = ["records.delete"]
))]
struct Shaped {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[filterable(type = "string")]
    #[sortable]
    code: String,
}

fn main() {}
