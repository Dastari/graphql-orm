use graphql_orm::prelude::*;

#[derive(GraphQLEntity)]
#[graphql_entity(table = "projection_empty", plural = "ProjectionEmpty")]
#[graphql_orm(projection(name = "EmptyProjection", fields = []))]
struct Item {
    #[primary_key]
    id: String,
}

fn main() {}
