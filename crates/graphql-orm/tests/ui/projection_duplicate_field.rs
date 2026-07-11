use graphql_orm::prelude::*;

#[derive(GraphQLEntity)]
#[graphql_entity(table = "projection_duplicate", plural = "ProjectionDuplicates")]
#[graphql_orm(projection(name = "DuplicateProjection", fields = [id, id]))]
struct Item {
    #[primary_key]
    id: String,
}

fn main() {}
