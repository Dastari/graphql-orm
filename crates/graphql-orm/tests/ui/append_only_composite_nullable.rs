use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(table = "audit", append_only = true)]
struct Audit {
    #[primary_key]
    
    tenant: Option<i32>,
    #[primary_key]
    #[sortable]
    sequence: i32,
    payload: String,
}

fn main() {}
