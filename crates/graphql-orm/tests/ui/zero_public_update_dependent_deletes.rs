use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(dependent_deletes, table = "holders", plural = "Holders", default_sort = "id ASC")]
struct Holder {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql_orm(skip_input)]
    stamped_at: String,
}

fn main() {}
