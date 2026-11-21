#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "quiet_records",
    auth = "none",
    mutations = false,
    subscriptions = false
)]
pub struct QuietRecord {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: String,
    title: String,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "watched_records",
    auth = "none",
    mutations = false,
    changed_started = true
)]
pub struct WatchedRecord {
    #[primary_key]
    #[sortable]
    id: String,
    title: String,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "write_records", auth = "none", subscriptions = false)]
pub struct WriteRecord {
    #[primary_key]
    #[sortable]
    id: String,
    title: String,
}

mod quiet {
    use super::*;
    schema_roots! { entities: [QuietRecord], client_metadata: true }
}

mod mixed {
    use super::*;
    schema_roots! { entities: [QuietRecord, WatchedRecord, WriteRecord], client_metadata: true }
}

#[tokio::test]
async fn omitting_every_write_and_stream_root_preserves_repository_writes_and_queries() {
    let database = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    graphql_orm::sqlx::query(
        "CREATE TABLE quiet_records (id TEXT PRIMARY KEY, title TEXT NOT NULL)",
    )
    .execute(database.pool())
    .await
    .unwrap();
    QuietRecord::insert(
        &database,
        CreateQuietRecordInput {
            id: "a".into(),
            title: "Available".into(),
        },
    )
    .await
    .unwrap();

    let schema = quiet::schema_builder(database).finish();
    let sdl = schema.sdl();
    assert!(!sdl.contains("mutation:"), "{sdl}");
    assert!(!sdl.contains("subscription:"), "{sdl}");
    assert!(!sdl.contains("CreateQuietRecordInput"), "{sdl}");
    assert!(!sdl.contains("QuietRecordChangedEvent"), "{sdl}");
    let response = schema
        .execute(r#"{ quietRecord(id: "a") { title } }"#)
        .await;
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(
        response.data.into_json().unwrap(),
        serde_json::json!({"quietRecord": {"title": "Available"}}),
    );
    assert!(
        <QuietRecord as GraphqlOperationMetadata>::generated_graphql_operations()
            .iter()
            .all(|operation| operation.kind() == GraphqlOperationKind::Query)
    );
    let _ = quiet::graphql_orm_client_metadata();
}

#[tokio::test]
async fn independently_omitted_roots_are_neutral_in_composition() {
    use graphql_orm::futures::StreamExt;
    let database = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    let schema = mixed::schema_builder(database).finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("createWriteRecord("), "{sdl}");
    assert!(!sdl.contains("createWatchedRecord("), "{sdl}");
    assert!(!sdl.contains("createQuietRecord("), "{sdl}");
    assert!(sdl.contains("watchedRecordChanged("), "{sdl}");
    assert!(!sdl.contains("quietRecordChanged("), "{sdl}");
    assert!(!sdl.contains("writeRecordChanged("), "{sdl}");
    let mut stream =
        schema.execute_stream("subscription { watchedRecordChanged(started: true) { started } }");
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .expect("started frame");
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(
        response.data.into_json().unwrap(),
        serde_json::json!({"watchedRecordChanged": {"started": true}}),
    );
    let _ = mixed::graphql_orm_client_metadata();
}
