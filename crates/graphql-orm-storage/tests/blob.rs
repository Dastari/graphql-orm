use bytes::Bytes;
use graphql_orm_storage::{
    StorageByteStream, StorageError, collect_storage_stream, validate_blob_key,
};

#[test]
fn validate_blob_key_accepts_safe_relative_keys() {
    assert!(validate_blob_key("snapshots/a/manifest.json").is_ok());
    assert!(validate_blob_key("objects/sha256/aa/bb/hash").is_ok());
    assert!(validate_blob_key("originals/aa/bb/object.jpg").is_ok());
}

#[test]
fn validate_blob_key_rejects_unsafe_keys() {
    for key in [
        "",
        "/absolute/path",
        "../escape",
        "a/../escape",
        "a/./b",
        "a//b",
        "a\\b",
        "a\0b",
    ] {
        let err = validate_blob_key(key).expect_err("unsafe key should be rejected");
        assert!(matches!(err, StorageError::InvalidStorageKey { .. }));
    }
}

#[test]
fn byte_stream_from_bytes_preserves_size_hint() {
    let stream = StorageByteStream::from_bytes(Bytes::from_static(b"hello storage"));

    assert_eq!(stream.size_hint(), Some(13));
}

#[tokio::test]
async fn collect_storage_stream_returns_original_bytes() {
    let bytes = collect_storage_stream(StorageByteStream::from_bytes(Bytes::from_static(
        b"hello storage",
    )))
    .await
    .expect("collect stream");

    assert_eq!(bytes, Bytes::from_static(b"hello storage"));
}
