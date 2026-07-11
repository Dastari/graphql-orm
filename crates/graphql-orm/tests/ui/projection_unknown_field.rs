use graphql_orm::prelude::*;

#[derive(GraphQLEntity)]
#[graphql_entity(table = "projection_unknown", plural = "ProjectionUnknown")]
#[graphql_orm(projection(name = "UnknownProjection", fields = [id, other_entity_field]))]
struct Item {
    #[primary_key]
    id: String,
}

fn main() {}
