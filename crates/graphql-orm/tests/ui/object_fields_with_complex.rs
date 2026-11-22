// `object_fields` joins the object `GraphQLEntity` generates. A `SimpleObject`
// entity with `#[graphql(complex)]` owns its object and composes handwritten
// fields with `compose_complex_object` instead.
use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(complex)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
#[graphql_orm(object_fields)]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
}

fn main() {}
