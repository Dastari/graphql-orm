use graphql_orm::async_graphql::{ComplexObject, SimpleObject};
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "probe_private_keys",
    plural = "ProbePrivateKeys",
    default_sort = "id ASC",
    auth = "none"
)]
struct ProbePrivateKey {
    #[primary_key]
    #[input_only]
    #[graphql(skip)]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    label: String,
}
#[ComplexObject]
impl ProbePrivateKey {}

fn main() {}
