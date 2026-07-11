use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity)]
#[graphql_entity(table = "projection_schema_only", plural = "ProjectionSchemaOnly")]
#[graphql_orm(projection(name = "SchemaOnlyProjection", fields = [id]))]
struct Item {
    #[primary_key]
    id: String,
}

fn main() {}
