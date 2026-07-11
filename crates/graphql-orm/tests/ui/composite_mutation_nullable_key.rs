use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "nullable_composite",
    plural = "NullableComposites",
    repository_mutations = true,
    default_sort = "tenant_id ASC, local_id ASC"
)]
struct NullableComposite {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    tenant_id: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    local_id: Option<String>,
    name: String,
}

fn main() {}
