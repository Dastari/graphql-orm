use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(table = "badges", mutation_shapes = "create,update")]
struct Badge {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    code: String,
    #[graphql_orm(skip_input)]
    issued_at: String,
}

fn main() {}
