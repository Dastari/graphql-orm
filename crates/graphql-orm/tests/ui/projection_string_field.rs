use graphql_orm::prelude::*;

#[derive(GraphQLEntity)]
#[graphql_entity(table = "projection_string", plural = "ProjectionString")]
#[graphql_orm(projection(name = "StringProjection", fields = ["id"]))]
struct Item {
    #[primary_key]
    #[sortable]
    id: String,
}

fn main() {}
