use bytes::Bytes;
use graphql_orm_storage::{
    StorageByteStream, StorageError, collect_storage_stream, validate_blob_key,
    validate_object_bucket,
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
fn validate_blob_key_reserves_the_internal_namespace() {
    for key in [
        ".graphql-orm-storage",
        ".graphql-orm-storage/multipart/upload",
        ".GRAPHQL-ORM-STORAGE/objects",
        ".graphql-orm-storage./x",
        ".graphql-orm-storage /x",
    ] {
        let err = validate_blob_key(key).expect_err("reserved key should be rejected");
        assert!(
            matches!(err, StorageError::InvalidStorageKey { .. }),
            "{key}"
        );
        assert!(validate_object_bucket(key.split('/').next().unwrap()).is_err());
    }
    // Only the first component is reserved; similar names stay available.
    for key in [
        "data/.graphql-orm-storage",
        "data/.graphql-orm-storage/x",
        ".graphql-orm-storage-old/x",
        "graphql-orm-storage/x",
    ] {
        assert!(validate_blob_key(key).is_ok(), "{key}");
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

#[tokio::test]
async fn collect_does_not_preallocate_a_declared_size() {
    use graphql_orm_storage::{StorageByteStream, collect_storage_stream};
    // A provider may report a size the remote side declared.
    let body = futures_util::stream::iter([Ok(bytes::Bytes::from_static(b"small"))]);
    let stream = StorageByteStream::with_size_hint(Box::pin(body), u64::MAX);
    let collected = collect_storage_stream(stream).await.expect("collect");
    assert_eq!(collected.as_ref(), b"small");
}
