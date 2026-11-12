// A handwritten composed resolver resolves before the flattened generated
// object, so one named `NodeId` or `RecordVersion` would replace the generated,
// policy-checked field. Explicit names and renamed idents are both refused.
use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(complex)]
#[graphql_orm(compose_complex_object)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records", node_id)]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
    name: String,
    #[graphql_orm(version = "managed", default = "0")]
    version: i64,
}

#[graphql_complex_object(rename_fields = "PascalCase")]
impl Record {
    #[graphql(name = "RecordVersion")]
    async fn handwritten_version(&self) -> i64 {
        self.version
    }

    async fn node_id(&self) -> String {
        self.id.clone()
    }

    async fn label(&self) -> String {
        self.id.clone()
    }
}

fn main() {}
