#![cfg(feature = "mssql")]

use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "PrivateRecords",
    plural = "PrivateRecords",
    schema_policy = "external_read_only",
    default_sort = "RecordId ASC"
)]
struct PrivateRecord {
    #[primary_key]
    #[graphql_orm(db_column = "RecordId", write = false)]
    #[filterable(type = "string")]
    #[sortable]
    record_id: String,
    #[graphql_orm(db_column = "PrivateValue", private, sensitive, write = false)]
    private_value: Vec<u8>,
}

fn assert_entity<T: Entity + FromSqlRow<MssqlBackend>>() {}

#[test]
fn repository_only_mssql_emits_read_metadata_and_no_write_contract() {
    assert_entity::<PrivateRecord>();
    assert_eq!(PrivateRecord::TABLE_NAME, "[PrivateRecords]");
    assert_eq!(
        PrivateRecord::column_names(),
        &["[RecordId]", "[PrivateValue]"]
    );
}

#[allow(dead_code)]
fn database_bound_query_compiles(database: &Database<MssqlBackend>) {
    let _query = PrivateRecord::query(database)
        .filter(PrivateRecordWhereInput {
            record_id: Some(StringFilter {
                eq: Some("record-1".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        })
        .default_order();
}
