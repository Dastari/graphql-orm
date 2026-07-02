#[test]
fn storage_streaming_range_boundary_records_external_api_contract() {
    let backup_boundary = include_str!("../../../docs/backup.md");
    let range_boundary = include_str!("../../../docs/storage-streaming-range-boundary.md");

    assert!(
        backup_boundary.contains("graphql-orm-storage"),
        "backup docs should keep object storage outside graphql-orm"
    );
    assert!(
        range_boundary.contains("StorageService::get_object_stream"),
        "Digitise needs the existing full-object streaming API documented"
    );
    assert!(
        range_boundary.contains("BlobStore::get_blob_range"),
        "Digitise needs the existing byte-range API documented"
    );
    assert!(
        range_boundary.contains("StorageService::get_object_range"),
        "the optional high-level range helper must be recorded for graphql-orm-storage"
    );
    assert!(
        range_boundary.contains("206 Partial Content")
            && range_boundary.contains("416 Range Not Satisfiable"),
        "HTTP range serving responsibilities should stay with the consumer"
    );
}
