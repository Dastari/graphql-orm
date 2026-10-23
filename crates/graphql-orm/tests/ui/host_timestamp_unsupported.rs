use graphql_orm::prelude::*;
#[derive(GraphQLSchemaEntity)]
#[graphql_entity(table = "schema_clock", plural = "SchemaClocks")]
struct SchemaClock {
    #[primary_key]
    id: String,
    #[graphql_orm(timestamp = "host")]
    created_at: i64,
}
#[derive(GraphQLOperations)]
#[graphql_entity(table = "operations_clock", schema_only = true)]
struct OperationsClock {
    #[primary_key]
    id: String,
    #[graphql_orm(timestamp = "host")]
    updated_at: i64,
}
#[derive(RepositoryEntity)]
#[repository_entity(table = "inferred_key_clock", plural = "InferredKeyClocks")]
struct InferredKeyClock {
    #[graphql_orm(timestamp = "host", db_column = "updated_at", auto_generated = false)]
    id: i64,
}
fn main() {}
