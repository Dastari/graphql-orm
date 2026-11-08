use graphql_orm::prelude::*;
#[derive(GraphQLEntity)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
struct Record {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[input_only]
    #[graphql_orm(searchable(weight = "A"))]
    secret: String,
}
fn main() {}
