use std::collections::BTreeMap;

use bytes::Bytes;
use futures_util::stream;
use graphql_orm_storage::{
    LocalStorageBackend, ObjectMetadata, StorageByteStream, StorageError, StreamingObjectStore,
    collect_storage_stream, sha256_hex,
};
use tempfile::TempDir;

#[tokio::test]
async fn local_streaming_object_large_streamed_write_preserves_metadata() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    let metadata = recording_metadata();
    let chunks = (0..128).map(|index| {
        let byte = u8::try_from(index % 251).expect("byte");
        Ok(Bytes::from(vec![byte; 64 * 1024]))
    });
    let stream = StorageByteStream::new(Box::pin(stream::iter(chunks)));

    let object = backend
        .put_object_stream(
            "recordings",
            "sessions/alpha/video.webm",
            Some("video/webm".to_string()),
            metadata.clone(),
            stream,
        )
        .await
        .expect("put stream");

    assert_eq!(object.bucket, "recordings");
    assert_eq!(object.key, "sessions/alpha/video.webm");
    assert_eq!(object.content_type.as_deref(), Some("video/webm"));
    assert_eq!(object.metadata, metadata);
    assert_eq!(object.size_bytes, 8 * 1024 * 1024);

    let loaded = backend
        .get_object_metadata("recordings", "sessions/alpha/video.webm")
        .await
        .expect("metadata");
    assert_eq!(loaded, object);
}

#[tokio::test]
async fn local_streaming_object_range_reads_support_http_playback() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    backend
        .put_object_stream(
            "recordings",
            "sessions/beta/video.webm",
            Some("video/webm".to_string()),
            ObjectMetadata::new(),
            StorageByteStream::from_bytes(Bytes::from_static(b"0123456789abcdef")),
        )
        .await
        .expect("put stream");

    let range = backend
        .get_object_range("recordings", "sessions/beta/video.webm", 4..10)
        .await
        .expect("range");
    assert_eq!(range.range.start, 4);
    assert_eq!(range.range.end, 10);
    assert_eq!(range.range.total_size, 16);
    assert_eq!(range.content_length, 6);

    let bytes = collect_storage_stream(range.body).await.expect("collect");
    assert_eq!(bytes, Bytes::from_static(b"456789"));
}

#[tokio::test]
async fn local_streaming_object_multipart_abort_cleans_temporary_state() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    let mut writer = backend
        .create_multipart_object(
            "recordings",
            "sessions/gamma/video.webm",
            Some("video/webm".to_string()),
            ObjectMetadata::new(),
        )
        .await
        .expect("create writer");

    writer
        .write_chunk(Bytes::from_static(b"partial"))
        .await
        .expect("write chunk");
    writer.abort().await.expect("abort");

    let err = backend
        .get_object_metadata("recordings", "sessions/gamma/video.webm")
        .await
        .expect_err("aborted object must be missing");
    assert!(matches!(err, StorageError::MissingBlob { .. }));

    let listed = backend
        .list_objects("recordings", "sessions/gamma")
        .await
        .expect("list");
    assert!(listed.is_empty());
}

#[tokio::test]
async fn local_streaming_object_multipart_complete_is_atomic_for_listing() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    let mut writer = backend
        .create_multipart_object(
            "recordings",
            "sessions/delta/video.webm",
            Some("video/webm".to_string()),
            recording_metadata(),
        )
        .await
        .expect("create writer");

    writer
        .write_chunk(Bytes::from_static(b"chunk-1-"))
        .await
        .expect("write chunk");

    assert!(
        backend
            .list_objects("recordings", "sessions/delta")
            .await
            .expect("list before complete")
            .is_empty()
    );

    writer
        .write_chunk(Bytes::from_static(b"chunk-2"))
        .await
        .expect("write chunk");
    let completed = writer.complete().await.expect("complete");

    assert_eq!(completed.size_bytes, 15);
    assert_eq!(completed.sha256_hex, sha256_hex(b"chunk-1-chunk-2"));

    let listed = backend
        .list_objects("recordings", "sessions/delta")
        .await
        .expect("list after complete");
    assert_eq!(listed, vec![completed]);
}

#[tokio::test]
async fn local_streaming_object_delete_removes_completed_object_and_metadata() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    backend
        .put_object_stream(
            "recordings",
            "retention/expired.webm",
            Some("video/webm".to_string()),
            recording_metadata(),
            StorageByteStream::from_bytes(Bytes::from_static(b"expired")),
        )
        .await
        .expect("put stream");

    backend
        .delete_object("recordings", "retention/expired.webm")
        .await
        .expect("delete");
    backend
        .delete_object("recordings", "retention/expired.webm")
        .await
        .expect("delete missing");

    let err = backend
        .get_object_metadata("recordings", "retention/expired.webm")
        .await
        .expect_err("metadata deleted");
    assert!(matches!(err, StorageError::MissingBlob { .. }));
    assert!(
        backend
            .list_objects("recordings", "retention")
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn local_streaming_object_internal_metadata_does_not_leak_into_blob_listing() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    backend
        .put_object_stream(
            "recordings",
            "sessions/epsilon/video.webm",
            Some("video/webm".to_string()),
            ObjectMetadata::new(),
            StorageByteStream::from_bytes(Bytes::from_static(b"video")),
        )
        .await
        .expect("put stream");

    let blobs = graphql_orm_storage::BlobStore::list_blobs(&backend, "")
        .await
        .expect("list blobs");
    assert_eq!(blobs, vec!["recordings/sessions/epsilon/video.webm"]);
}

fn recording_metadata() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("tenant_id".to_string(), "tenant-1".to_string()),
        ("device_id".to_string(), "device-1".to_string()),
        ("auth_scope".to_string(), "recordings.read".to_string()),
    ])
}
