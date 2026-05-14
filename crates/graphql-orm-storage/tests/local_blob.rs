use bytes::Bytes;
use graphql_orm_storage::{
    BlobStore, LocalStorageBackend, StorageByteStream, StorageError, collect_storage_stream,
    sha256_hex,
};
use tempfile::TempDir;

#[tokio::test]
async fn local_blob_put_get_delete_round_trip() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    let outcome = backend
        .put_blob(
            "snapshots/a/manifest.json",
            StorageByteStream::from_bytes(Bytes::from_static(b"manifest")),
        )
        .await
        .expect("put blob");

    assert_eq!(outcome.size_bytes, 8);
    assert_eq!(outcome.sha256_hex, sha256_hex(b"manifest"));
    assert!(
        backend
            .blob_exists("snapshots/a/manifest.json")
            .await
            .expect("exists")
    );

    let body = backend
        .get_blob("snapshots/a/manifest.json")
        .await
        .expect("get blob");
    assert_eq!(body.key, "snapshots/a/manifest.json");
    assert_eq!(
        collect_storage_stream(body.body)
            .await
            .expect("collect body"),
        Bytes::from_static(b"manifest")
    );

    backend
        .delete_blob("snapshots/a/manifest.json")
        .await
        .expect("delete blob");
    assert!(
        !backend
            .blob_exists("snapshots/a/manifest.json")
            .await
            .expect("exists")
    );
}

#[tokio::test]
async fn local_blob_head_and_exists_handle_present_and_missing_blobs() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    assert!(
        !backend
            .blob_exists("objects/sha256/aa/bb/hash")
            .await
            .expect("exists")
    );
    assert_eq!(
        backend
            .head_blob("objects/sha256/aa/bb/hash")
            .await
            .expect("head missing"),
        None
    );

    backend
        .put_blob(
            "objects/sha256/aa/bb/hash",
            StorageByteStream::from_bytes(Bytes::from_static(b"object")),
        )
        .await
        .expect("put blob");

    let metadata = backend
        .head_blob("objects/sha256/aa/bb/hash")
        .await
        .expect("head blob")
        .expect("metadata");
    assert_eq!(metadata.key, "objects/sha256/aa/bb/hash");
    assert_eq!(metadata.size_bytes, Some(6));
    assert_eq!(metadata.sha256_hex, None);
}

#[tokio::test]
async fn local_blob_list_blobs_supports_empty_and_non_empty_prefixes() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    backend
        .put_blob(
            "snapshots/a/manifest.json",
            StorageByteStream::from_bytes(Bytes::from_static(b"manifest")),
        )
        .await
        .expect("put manifest");
    backend
        .put_blob(
            "objects/sha256/aa/bb/hash",
            StorageByteStream::from_bytes(Bytes::from_static(b"object")),
        )
        .await
        .expect("put object");

    assert_eq!(
        backend.list_blobs("").await.expect("list all"),
        vec![
            "objects/sha256/aa/bb/hash".to_string(),
            "snapshots/a/manifest.json".to_string()
        ]
    );
    assert_eq!(
        backend.list_blobs("snapshots").await.expect("list prefix"),
        vec!["snapshots/a/manifest.json".to_string()]
    );
    assert!(
        backend
            .list_blobs("missing")
            .await
            .expect("list missing")
            .is_empty()
    );
}

#[tokio::test]
async fn local_blob_delete_missing_succeeds() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    backend
        .delete_blob("objects/sha256/aa/bb/hash")
        .await
        .expect("delete missing");
}

#[tokio::test]
async fn local_blob_get_missing_returns_missing_blob() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    let err = backend
        .get_blob("objects/sha256/aa/bb/hash")
        .await
        .expect_err("missing blob");

    assert!(matches!(err, StorageError::MissingBlob { .. }));
}

#[tokio::test]
async fn local_blob_rejects_invalid_keys_for_all_operations() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    assert_invalid(
        backend
            .put_blob("../escape", StorageByteStream::from_bytes(Bytes::new()))
            .await,
    );
    assert_invalid(backend.get_blob("../escape").await);
    assert_invalid(backend.blob_exists("../escape").await);
    assert_invalid(backend.head_blob("../escape").await);
    assert_invalid(backend.list_blobs("../escape").await);
    assert_invalid(backend.delete_blob("../escape").await);
}

#[tokio::test]
async fn local_blob_list_blobs_ignores_uploading_temp_files() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    let temp_path = temp.path().join("snapshots/a/manifest.json.temp.uploading");
    tokio::fs::create_dir_all(temp_path.parent().expect("temp parent"))
        .await
        .expect("create parent");
    tokio::fs::write(&temp_path, b"partial")
        .await
        .expect("write temp");

    assert!(backend.list_blobs("").await.expect("list all").is_empty());
}

fn assert_invalid<T>(result: Result<T, StorageError>) {
    assert!(matches!(
        result,
        Err(StorageError::InvalidStorageKey { .. })
    ));
}
