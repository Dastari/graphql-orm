use graphql_orm::prelude::*;

#[derive(GraphQLEntity)]
#[graphql_entity(backend = "postgres", table = "projection_backend", plural = "ProjectionBackend")]
#[graphql_orm(projection(name = "WrongBackendProjection", fields = [id]))]
struct Item {
    #[primary_key]
    id: String,
}

fn main() {}
