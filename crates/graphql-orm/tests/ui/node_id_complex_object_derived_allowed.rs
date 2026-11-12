// Derived names that async-graphql publishes in camelCase are not identity
// fields, so they compile beside a generated `NodeId` and `RecordVersion`.
use graphql_orm::async_graphql::{Context, SimpleObject};
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(complex)]
#[graphql_orm(compose_complex_object)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records", node_id)]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql(derived(name = "RecordVersion", into = "String"))]
    name: String,
    #[graphql_orm(version = "managed", default = "0")]
    version: i64,
}

#[graphql_complex_object]
impl Record {
    #[graphql(derived(name = "NodeId", into = "String"))]
    async fn label(&self) -> String {
        self.name.clone()
    }
}

fn main() {}
