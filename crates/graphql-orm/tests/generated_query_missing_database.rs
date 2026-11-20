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

#[tokio::test]
async fn keyset_and_scan_return_missing_database_errors() {
    assert_missing_database("missingDatabaseRecordsKeyset(page: {limit: 1}) { edges { cursor } }")
        .await;
    assert_missing_database("missingDatabaseRecordsScan(page: {limit: 1}) { pageInfo { status } }")
        .await;
}

#[tokio::test]
async fn search_returns_a_missing_database_error() {
    assert_missing_database(
        r#"missingDatabaseRecordsSearch(search: {query: "a"}) { edges { cursor } }"#,
    )
    .await;
}

#[tokio::test]
async fn aggregate_returns_a_missing_database_error() {
    assert_missing_database("missingDatabaseRecordsAggregate(groupBy: [], metrics: [{operator: COUNT}], groupLimit: 1) { metrics { value } }").await;
}

#[tokio::test]
async fn federation_reads_report_missing_database_and_loader() {
    let id = graphql_orm::graphql::federation::encode_node_id(
        "MissingDatabaseRecord",
        vec![serde_json::json!("a")],
    );
    let queries = [
        r#"{ _entities(representations: [{__typename: "MissingDatabaseRecord", id: "a"}]) { ... on MissingDatabaseRecord { id } } }"#.to_owned(),
        format!(r#"{{ _entities(representations: [{{__typename: "MissingDatabaseRecord", NodeId: "{}"}}]) {{ ... on MissingDatabaseRecord {{ id }} }} }}"#, id.0),
    ];
    for register_database in [false, true] {
        let mut builder = async_graphql::Schema::build(
            MissingDatabaseRecordQueries,
            async_graphql::EmptyMutation,
            async_graphql::EmptySubscription,
        )
        .enable_federation();
        if register_database {
            builder = builder.data(Database::connect_sqlite("sqlite::memory:").await.unwrap());
        }
        let schema = builder.finish();
        let expected = if register_database {
            "FederationKeyLoader"
        } else {
            "Database"
        };
        for query in &queries {
            let response = schema.execute(query).await;
            assert_eq!(response.errors.len(), 1, "{query}: {response:?}");
            assert!(
                response.errors[0].message.contains(expected),
                "{query}: {response:?}"
            );
        }
    }
}
