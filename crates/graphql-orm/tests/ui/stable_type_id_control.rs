use graphql_orm::prelude::*;
#[derive(GraphQLEntity)]
#[graphql_entity(table = "records", plural = "Records", stable_type_id = "bad\nvalue")]
struct Record {
    #[primary_key]
    id: String,
}
fn main() {}
