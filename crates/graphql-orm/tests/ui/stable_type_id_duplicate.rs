use graphql_orm::prelude::*;
#[derive(GraphQLEntity)]
#[graphql_entity(table = "records", plural = "Records", stable_type_id = "one", stable_type_id = "two")]
struct Record {
    #[primary_key]
    id: String,
}
fn main() {}
