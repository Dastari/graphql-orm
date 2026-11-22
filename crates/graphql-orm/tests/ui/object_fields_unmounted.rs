// An entity that opts into `object_fields` has no GraphQL object until a
// `#[graphql_object_fields]` impl composes it.
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
#[graphql_orm(object_fields)]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
}

fn main() {}
