use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations)]
#[graphql_entity(
    table = "zones",
    plural = "Zones",
    federation_key,
    federation_key(fields = ["id"])
)]
struct Zone {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    name: String,
}

fn main() {}
