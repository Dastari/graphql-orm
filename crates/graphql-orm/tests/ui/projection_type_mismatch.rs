use graphql_orm::prelude::*;

#[derive(GraphQLEntity)]
#[graphql_entity(table = "projection_type", plural = "ProjectionTypes")]
#[graphql_orm(projection(name = "TypedProjection", fields = [id, issued_at]))]
struct Item {
    #[primary_key]
    #[sortable]
    id: String,
    #[sortable]
    issued_at: i64,
}

fn mismatch(value: TypedProjection) {
    let _: String = value.issued_at;
}

fn main() {}
