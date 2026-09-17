use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations)]
#[graphql_entity(
    table = "zones",
    plural = "Zones",
    federation_key(fields = ["slug"])
)]
struct Zone {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    name: String,
    #[unique]
    slug: Option<String>,
}

fn main() {}
