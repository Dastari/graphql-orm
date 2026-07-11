use graphql_orm::prelude::*;

#[derive(GraphQLEntity)]
#[graphql_entity(table = "projection_absent", plural = "ProjectionAbsent")]
struct Item {
    #[primary_key]
    #[sortable]
    id: String,
}

fn main() {
    let _ = std::mem::size_of::<ItemProjection>();
}
