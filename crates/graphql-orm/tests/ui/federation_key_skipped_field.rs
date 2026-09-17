use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations)]
#[graphql_entity(
    table = "zones",
    plural = "Zones",
    federation_key(fields = ["cachedLabel"])
)]
struct Zone {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    name: String,
    #[skip_db]
    #[unique]
    cached_label: String,
}

fn main() {}
