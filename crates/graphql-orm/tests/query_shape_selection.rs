#![cfg(feature = "sqlite")]

use graphql_orm::graphql::orm::{GraphqlOperationKind, GraphqlOperationMetadata};
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "shape_records",
    plural = "ShapeRecords",
    query_shapes = "list"
)]
struct ShapeRecord {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    title: String,
}

#[derive(Default)]
struct LegacyQueries;

#[graphql_orm_custom_operations(kind = "query", authorization = true)]
#[graphql_orm::async_graphql::Object]
impl LegacyQueries {
    /// Synthetic legacy shape whose argument differs from the generated singleton.
    async fn shape_record(&self, legacy_value: String) -> String {
        format!("legacy:{legacy_value}")
    }
}

schema_roots! {
    query_custom_ops: [],
    entities: [ShapeRecord],
    described_query_types: [LegacyQueries],
}

#[tokio::test]
async fn selected_generated_list_coexists_with_a_differently_shaped_legacy_singleton() {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE shape_records (id TEXT PRIMARY KEY, title TEXT NOT NULL)")
        .execute(database.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO shape_records (id, title) VALUES ('one', 'First')")
        .execute(database.pool())
        .await
        .unwrap();
    let schema = schema_builder(database)
        .data("test-user".to_string())
        .finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("shapeRecord(legacyValue: String!): String!"));
    assert!(!sdl.contains("shapeRecord(id:"));
    assert!(sdl.contains("shapeRecords("));
    assert!(sdl.contains("shapeRecordChanged("));
    let response = schema
        .execute(
            "{ shapeRecord(legacyValue: \"old\") shapeRecords { edges { node { id title } } } }",
        )
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["shapeRecord"], "legacy:old");
    assert_eq!(data["shapeRecords"]["edges"][0]["node"]["id"], "one");
}

#[test]
fn generated_catalogue_contains_only_selected_query_shapes() {
    let operations = ShapeRecord::generated_graphql_operations();
    let queries = operations
        .iter()
        .filter(|operation| operation.kind() == GraphqlOperationKind::Query)
        .map(|operation| operation.field_name())
        .collect::<Vec<_>>();
    assert_eq!(queries, ["shapeRecords"]);
    assert!(
        operations
            .iter()
            .any(|operation| operation.kind() == GraphqlOperationKind::Subscription)
    );
    assert!(
        operations
            .iter()
            .any(|operation| operation.kind() == GraphqlOperationKind::Mutation)
    );
    assert!(
        graphql_orm_operation_catalog()
            .exposed_operations()
            .any(|operation| operation.field_name() == "shapeRecord")
    );
}
