#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "source_stamped_records", plural = "SourceStampedRecords")]
struct SourceStampedRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[graphql_orm(timestamp = "host", db_column = "updated_at")]
    updated_timestamp: String,
    #[graphql_orm(timestamp = "host", db_column = "created_at", default = false)]
    created_source: Option<String>,
}

schema_roots! {
    query_custom_ops: [],
    entities: [SourceStampedRecord],
}

async fn setup() -> graphql_orm::Result<Database<SqliteBackend>> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    sqlx::query("CREATE TABLE source_stamped_records (id TEXT PRIMARY KEY, updated_at TEXT NOT NULL, created_at TEXT)")
        .execute(database.pool()).await?;
    Ok(database)
}

#[tokio::test]
async fn mutation_context_keeps_supplied_source_strings_and_nullable_omission() -> graphql_orm::Result<()> {
    let database = setup().await?;
    database.transaction(TransactionMode::Default, |tx| Box::pin(async move {
        tx.insert::<SourceStampedRecord>(CreateSourceStampedRecordInput {
            id: "source-1".into(),
            updated_timestamp: "2026-01-02T03:04:05+10:00".into(),
            created_source: Some("2025-06-07T08:09:10Z".into()),
        }).await?;
        tx.update_by_id::<SourceStampedRecord>(&"source-1".to_string(), UpdateSourceStampedRecordInput {
            updated_timestamp: Some("2026-02-03T04:05:06.123Z".into()),
            ..Default::default()
        }).await?;
        Ok::<_, OrmPublicError>(())
    })).await.expect("source timestamp transaction");
    let row = SourceStampedRecord::find_by_id(&database, &"source-1".to_string()).await?.unwrap();
    assert_eq!(row.updated_timestamp, "2026-02-03T04:05:06.123Z");
    assert_eq!(row.created_source.as_deref(), Some("2025-06-07T08:09:10Z"));
    database.transaction(TransactionMode::Default, |tx| Box::pin(async move {
        tx.update_by_id::<SourceStampedRecord>(&"source-1".to_string(), UpdateSourceStampedRecordInput {
            created_source: Some(None),
            ..Default::default()
        }).await?;
        Ok::<_, OrmPublicError>(())
    })).await.expect("explicit nullable source clear");
    let row = SourceStampedRecord::find_by_id(&database, &"source-1".to_string()).await?.unwrap();
    assert_eq!(row.updated_timestamp, "2026-02-03T04:05:06.123Z");
    assert_eq!(row.created_source, None);
    Ok(())
}

#[tokio::test]
async fn generated_graphql_writes_preserve_source_values_without_clock_conversion() -> graphql_orm::Result<()> {
    let database = setup().await?;
    let schema = schema_builder(database.clone()).data("test-user".to_string()).finish();
    let response = schema.execute("mutation { createSourceStampedRecord(input: {id: \"source-2\", updatedTimestamp: \"2026-03-04T05:06:07-03:00\"}) { success } }").await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(response.data.into_json().unwrap()["createSourceStampedRecord"]["success"], true);
    let response = schema.execute("mutation { updateSourceStampedRecord(id: \"source-2\", input: {updatedTimestamp: \"upstream-opaque-time\"}) { success } }").await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(response.data.into_json().unwrap()["updateSourceStampedRecord"]["success"], true);
    let row = SourceStampedRecord::find_by_id(&database, &"source-2".to_string()).await?.unwrap();
    // A host-owned String is an ordinary caller value, not a parsed date contract.
    assert_eq!(row.updated_timestamp, "upstream-opaque-time");
    Ok(())
}
