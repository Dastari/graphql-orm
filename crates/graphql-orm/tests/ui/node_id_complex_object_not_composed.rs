// `compose_complex_object` without `graphql_complex_object` never mounts the
// composed object, so the key would have no field. The witness names the fix.
use graphql_orm::async_graphql::{ComplexObject, SimpleObject};
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(complex)]
#[graphql_orm(compose_complex_object)]
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
