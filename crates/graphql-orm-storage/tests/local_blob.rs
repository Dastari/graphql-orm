use bytes::Bytes;
use graphql_orm_storage::{
    BlobPutOptions, BlobStore, LocalStorageBackend, StorageByteStream, StorageError,
    collect_storage_stream, sha256_hex,
};
use std::time::Duration;
use tempfile::TempDir;

#[tokio::test]
async fn local_blob_put_get_delete_round_trip() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    let outcome = backend
        .put_blob(
            "snapshots/a/manifest.json",
            StorageByteStream::from_bytes(Bytes::from_static(b"manifest")),
            BlobPutOptions::default(),
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
            BlobPutOptions::default(),
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
            BlobPutOptions::default(),
        )
        .await
        .expect("put manifest");
    backend
        .put_blob(
            "objects/sha256/aa/bb/hash",
            StorageByteStream::from_bytes(Bytes::from_static(b"object")),
            BlobPutOptions::default(),
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
async fn local_blob_paged_listing_returns_continuation_tokens() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    for key in ["prefix/a", "prefix/b", "prefix/c"] {
        backend
            .put_blob(
                key,
                StorageByteStream::from_bytes(Bytes::from_static(b"x")),
                BlobPutOptions::default(),
            )
            .await
            .expect("put blob");
    }

    let first_page = backend
        .list_blobs_page("prefix", None, 2)
        .await
        .expect("first page");
    assert_eq!(
        first_page.keys,
        vec!["prefix/a".to_string(), "prefix/b".to_string()]
    );
    assert_eq!(first_page.next_continuation, Some("prefix/b".to_string()));

    let second_page = backend
        .list_blobs_page("prefix", first_page.next_continuation, 2)
        .await
        .expect("second page");
    assert_eq!(second_page.keys, vec!["prefix/c".to_string()]);
    assert_eq!(second_page.next_continuation, None);
}

#[tokio::test]
async fn local_blob_range_reads_return_requested_bytes() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    backend
        .put_blob(
            "objects/ranged",
            StorageByteStream::from_bytes(Bytes::from_static(b"0123456789")),
            BlobPutOptions::default(),
        )
        .await
        .expect("put blob");

    let body = backend
        .get_blob_range("objects/ranged", 2..6)
        .await
        .expect("range read");
    let bytes = collect_storage_stream(body.body).await.expect("collect");
    assert_eq!(bytes, Bytes::from_static(b"2345"));
}

#[tokio::test]
async fn local_blob_conditional_write_returns_none_when_key_exists() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    let first = backend
        .put_blob_if_not_exists(
            "objects/dedup",
            StorageByteStream::from_bytes(Bytes::from_static(b"first")),
            BlobPutOptions::default(),
        )
        .await
        .expect("first conditional write");
    assert!(first.is_some());

    let second = backend
        .put_blob_if_not_exists(
            "objects/dedup",
            StorageByteStream::from_bytes(Bytes::from_static(b"second")),
            BlobPutOptions::default(),
        )
        .await
        .expect("second conditional write");
    assert_eq!(second, None);

    let body = backend.get_blob("objects/dedup").await.expect("get blob");
    let bytes = collect_storage_stream(body.body).await.expect("collect");
    assert_eq!(bytes, Bytes::from_static(b"first"));
}

#[tokio::test]
async fn local_blob_copy_promotes_without_download_api() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());

    backend
        .put_blob(
            "temp/object",
            StorageByteStream::from_bytes(Bytes::from_static(b"copy me")),
            BlobPutOptions::default(),
        )
        .await
        .expect("put source");

    backend
        .copy_blob("temp/object", "originals/object")
        .await
        .expect("copy blob");

    let body = backend
        .get_blob("originals/object")
        .await
        .expect("get copied");
    let bytes = collect_storage_stream(body.body).await.expect("collect");
    assert_eq!(bytes, Bytes::from_static(b"copy me"));
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
            .put_blob(
                "../escape",
                StorageByteStream::from_bytes(Bytes::new()),
                BlobPutOptions::default(),
            )
            .await,
    );
    assert_invalid(
        backend
            .put_blob_if_not_exists(
                "../escape",
                StorageByteStream::from_bytes(Bytes::new()),
                BlobPutOptions::default(),
            )
            .await,
    );
    assert_invalid(backend.get_blob("../escape").await);
    assert_invalid(backend.get_blob_range("../escape", 0..1).await);
    assert_invalid(backend.blob_exists("../escape").await);
    assert_invalid(backend.head_blob("../escape").await);
    assert_invalid(backend.list_blobs("../escape").await);
    assert_invalid(backend.list_blobs_page("../escape", None, 100).await);
    assert_invalid(backend.copy_blob("../escape", "objects/safe").await);
    assert_invalid(backend.copy_blob("objects/safe", "../escape").await);
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

#[tokio::test]
async fn local_blob_sweep_temp_files_removes_stale_uploads() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    let temp_path = temp.path().join("snapshots/a/manifest.json.temp.uploading");
    tokio::fs::create_dir_all(temp_path.parent().expect("temp parent"))
        .await
        .expect("create parent");
    tokio::fs::write(&temp_path, b"partial")
        .await
        .expect("write temp");

    let removed = backend
        .sweep_temp_files(Duration::ZERO)
        .await
        .expect("sweep temp files");

    assert_eq!(removed, 1);
    assert!(!temp_path.exists());
}

#[tokio::test]
async fn local_blob_sweep_temp_files_keeps_recent_uploads() {
    let temp = TempDir::new().expect("temp dir");
    let backend = LocalStorageBackend::new(temp.path());
    let temp_path = temp.path().join("snapshots/a/manifest.json.temp.uploading");
    tokio::fs::create_dir_all(temp_path.parent().expect("temp parent"))
        .await
        .expect("create parent");
    tokio::fs::write(&temp_path, b"partial")
        .await
        .expect("write temp");

    let removed = backend
        .sweep_temp_files(Duration::from_secs(86_400))
        .await
        .expect("sweep temp files");

    assert_eq!(removed, 0);
    assert!(temp_path.exists());
}

fn assert_invalid<T>(result: Result<T, StorageError>) {
    assert!(matches!(
        result,
        Err(StorageError::InvalidStorageKey { .. })
    ));
}
