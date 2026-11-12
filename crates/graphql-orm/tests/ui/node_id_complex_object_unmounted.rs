// A handwritten `ComplexObject` is the whole complex object, so nothing can add
// `NodeId` to it. The witness names the composed declaration instead.
use graphql_orm::async_graphql::{ComplexObject, SimpleObject};
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(complex)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records", node_id)]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
    scope_json: String,
}

#[ComplexObject]
impl Record {
    async fn scope_length(&self) -> usize {
        self.scope_json.len()
    }
}

fn main() {}
