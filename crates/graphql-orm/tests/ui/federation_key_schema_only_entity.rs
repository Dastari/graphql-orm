use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity)]
#[graphql_entity(table = "zones", plural = "Zones", federation_key)]
struct Zone {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    name: String,
}

fn main() {}
