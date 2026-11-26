use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "postgres", table = "lines", nested_create_keys = ["order_id"])]
#[graphql_rls(insert(tenant = "tenant_id"))]
struct Line {
    #[primary_key]
    #[sortable]
    id: String,
    order_id: String,
    tenant_id: String,
}

fn main() {}
