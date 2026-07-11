use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "generated_composite",
    plural = "GeneratedComposites",
    repository_mutations = true,
    default_sort = "id ASC, local_id ASC"
)]
struct GeneratedComposite {
    #[primary_key]
    #[sortable]
    id: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    local_id: String,
    name: String,
}

fn main() {}
