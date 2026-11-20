#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::graphql::input_validation::GeneratedInputValidation;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "missing_database_records",
    plural = "MissingDatabaseRecords",
    auth = "none",
    aggregate = true,
    keyset = "id asc",
    federation_key(fields = ["id"]),
    node_id
)]
struct MissingDatabaseRecord {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql_orm(searchable(weight = "A"))]
    title: String,
}

async fn assert_missing_database(field: &str) {
    let schema = async_graphql::Schema::build(
        MissingDatabaseRecordQueries,
        async_graphql::EmptyMutation,
        async_graphql::EmptySubscription,
    )
    .extension(GeneratedInputValidation)
    .finish();

    let response = schema.execute(format!("{{ {field} }}")).await;
    assert_eq!(response.errors.len(), 1, "{field}: {response:?}");
    let error = &response.errors[0];
    assert!(error.message.contains("Database"), "{field}: {error:?}");
    assert_eq!(error.path.len(), 1, "{field}: {error:?}");
}

#[tokio::test]
async fn point_read_returns_a_missing_database_error() {
    assert_missing_database(r#"missingDatabaseRecord(id: "a") { id }"#).await;
}

#[tokio::test]
async fn list_returns_a_missing_database_error() {
    assert_missing_database("missingDatabaseRecords { edges { cursor } }").await;
}
