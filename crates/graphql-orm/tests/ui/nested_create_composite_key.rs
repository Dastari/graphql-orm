use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "lines", default_sort = "id ASC, local_id ASC", nested_create_keys = ["order_id"])]
struct Line {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    local_id: String,
    order_id: String,
}

fn main() {}
