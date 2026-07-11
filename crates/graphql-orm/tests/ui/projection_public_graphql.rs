use graphql_orm::prelude::*;

#[derive(GraphQLEntity)]
#[graphql_entity(table = "projection_public", plural = "ProjectionPublic")]
#[graphql_orm(projection(name = "PublicProjection", fields = [id], private = false))]
struct Item {
    #[primary_key]
    #[sortable]
    id: String,
}

fn main() {}
