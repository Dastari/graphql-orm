#![cfg(feature = "mssql")]
use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "private_records",
    plural = "PrivateRecords",
    schema_policy = "external_writable",
    upsert = "external_key"
)]
struct PrivateRecord {
    #[primary_key]
    id: String,
    #[unique]
    external_key: String,
    value: i64,
}

#[allow(dead_code)]
async fn operations(db: &Database<MssqlBackend>) -> graphql_orm::Result<()> {
    let input = CreatePrivateRecordInput {
        external_key: "a".into(),
        value: 1,
    };
    let _ = PrivateRecord::insert(db, input.clone()).await?;
    let _ = PrivateRecord::upsert(db, input).await?;
    Ok(())
}

#[test]
fn compiles() {}

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "composite_records",
    plural = "CompositeRecords",
    schema_policy = "external_writable",
    repository_mutations = true,
    read_policy = "required",
    write_policy = "required"
)]
struct CompositeRecord {
    #[primary_key]
    first: String,
    #[primary_key]
    second: i64,
    value: String,
}

#[allow(dead_code)]
async fn composite_operations(db: &Database<MssqlBackend>) -> graphql_orm::Result<()> {
    let input = CreateCompositeRecordInput {
        first: "a".into(),
        second: 1,
        value: "value".into(),
    };
    let inserted = CompositeRecord::insert(db, input).await?;
    let key = CompositeRecordKey {
        first: inserted.first,
        second: inserted.second,
    };
    let _ = CompositeRecord::delete_by_key(db, &key).await?;
    Ok(())
}
