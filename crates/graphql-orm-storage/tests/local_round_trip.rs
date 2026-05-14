use std::sync::Arc;

use graphql_orm_storage::{
    LocalStorageBackend, StorageBackend, StorageByteStream, StorageError, StorageNamespace,
    StoragePutRequest, StoragePutStreamRequest, StorageService, StoredObject,
    collect_storage_stream, sha256_hex,
};
use tempfile::TempDir;
use time::OffsetDateTime;
use uuid::Uuid;

#[tokio::test]
async fn local_put_get_delete_round_trip_preserves_bytes_and_metadata() {
    let temp = TempDir::new().expect("temp dir");
    let service = StorageService::new(Arc::new(LocalStorageBackend::new(temp.path())));

    let stored = service
        .put_object(StoragePutRequest {
            namespace: StorageNamespace::Originals,
            file_name: Some("artifact.JPEG".to_string()),
            mime_type: Some("image/jpeg".to_string()),
            bytes: b"hello storage".to_vec(),
        })
        .await
        .expect("put object");

    assert_eq!(stored.backend, StorageBackend::Local);
    assert_eq!(stored.namespace, StorageNamespace::Originals);
    assert_eq!(stored.original_file_name.as_deref(), Some("artifact.JPEG"));
    assert_eq!(stored.mime_type.as_deref(), Some("image/jpeg"));
    assert_eq!(stored.size_bytes, 13);
    assert_eq!(stored.sha256_hex, sha256_hex(b"hello storage"));
    assert!(stored.storage_key.ends_with(".jpeg"));

    let loaded = service.get_object(&stored).await.expect("get object");
    assert_eq!(loaded.bytes, b"hello storage");
    assert_eq!(loaded.object, stored);
    assert!(
        tokio::fs::metadata(temp.path().join(&stored.storage_key))
            .await
            .is_ok()
    );

    service.delete_object(&stored).await.expect("delete object");
    assert!(service.get_object(&stored).await.is_err());
}

#[tokio::test]
async fn local_put_get_stream_round_trip_preserves_bytes_and_metadata() {
    let temp = TempDir::new().expect("temp dir");
    let service = StorageService::new(Arc::new(LocalStorageBackend::new(temp.path())));

    let stored = service
        .put_object_stream(StoragePutStreamRequest {
            namespace: StorageNamespace::Originals,
            file_name: Some("artifact.TXT".to_string()),
            mime_type: Some("text/plain".to_string()),
            body: StorageByteStream::from_bytes(b"streamed object".to_vec()),
        })
        .await
        .expect("put stream");

    assert_eq!(stored.backend, StorageBackend::Local);
    assert_eq!(stored.namespace, StorageNamespace::Originals);
    assert_eq!(stored.original_file_name.as_deref(), Some("artifact.TXT"));
    assert_eq!(stored.mime_type.as_deref(), Some("text/plain"));
    assert_eq!(stored.size_bytes, 15);
    assert_eq!(stored.sha256_hex, sha256_hex(b"streamed object"));
    assert!(stored.storage_key.ends_with(".txt"));
    assert!(service.object_exists(&stored).await.expect("object exists"));

    let metadata = service
        .object_backend_metadata(&stored)
        .await
        .expect("metadata")
        .expect("metadata exists");
    assert_eq!(metadata.key, stored.storage_key);
    assert_eq!(metadata.size_bytes, Some(15));

    let loaded = service
        .get_object_stream(&stored)
        .await
        .expect("get stream");
    assert_eq!(loaded.object, stored);
    assert_eq!(
        collect_storage_stream(loaded.body)
            .await
            .expect("collect stream"),
        b"streamed object".as_slice()
    );

    service.delete_object(&stored).await.expect("delete object");
    assert!(!service.object_exists(&stored).await.expect("object exists"));
    let err = service
        .get_object_stream(&stored)
        .await
        .expect_err("deleted object should be missing");
    assert!(matches!(err, StorageError::MissingBlob { .. }));
}

#[tokio::test]
async fn delete_missing_object_succeeds() {
    let temp = TempDir::new().expect("temp dir");
    let service = StorageService::new(Arc::new(LocalStorageBackend::new(temp.path())));
    let object = test_object("originals/aa/bb/aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa.txt");

    service
        .delete_object(&object)
        .await
        .expect("delete missing object");
}

#[tokio::test]
async fn local_backend_rejects_path_traversal_storage_keys() {
    let temp = TempDir::new().expect("temp dir");
    let service = StorageService::new(Arc::new(LocalStorageBackend::new(temp.path())));
    let object = test_object("../escape.txt");

    let err = service
        .get_object(&object)
        .await
        .expect_err("path traversal should be rejected");

    assert!(matches!(err, StorageError::InvalidStorageKey { .. }));
}

#[tokio::test]
async fn local_backend_rejects_absolute_storage_keys() {
    let temp = TempDir::new().expect("temp dir");
    let service = StorageService::new(Arc::new(LocalStorageBackend::new(temp.path())));
    let object = test_object("/tmp/escape.txt");

    let err = service
        .get_object(&object)
        .await
        .expect_err("absolute path should be rejected");

    assert!(matches!(err, StorageError::InvalidStorageKey { .. }));
}

#[tokio::test]
async fn local_backend_rejects_dot_storage_key_components() {
    let temp = TempDir::new().expect("temp dir");
    let service = StorageService::new(Arc::new(LocalStorageBackend::new(temp.path())));
    let object = test_object("originals/./escape.txt");

    let err = service
        .get_object(&object)
        .await
        .expect_err("dot path component should be rejected");

    assert!(matches!(err, StorageError::InvalidStorageKey { .. }));
}

#[tokio::test]
async fn local_backend_rejects_parent_storage_key_components() {
    let temp = TempDir::new().expect("temp dir");
    let service = StorageService::new(Arc::new(LocalStorageBackend::new(temp.path())));
    let object = test_object("originals/aa/../escape.txt");

    let err = service
        .get_object(&object)
        .await
        .expect_err("parent path component should be rejected");

    assert!(matches!(err, StorageError::InvalidStorageKey { .. }));
}

#[tokio::test]
async fn local_backend_rejects_backslash_storage_keys() {
    let temp = TempDir::new().expect("temp dir");
    let service = StorageService::new(Arc::new(LocalStorageBackend::new(temp.path())));
    let object = test_object("originals\\aa\\escape.txt");

    let err = service
        .get_object(&object)
        .await
        .expect_err("backslash path should be rejected");

    assert!(matches!(err, StorageError::InvalidStorageKey { .. }));
}

#[tokio::test]
async fn local_backend_rejects_nul_storage_keys() {
    let temp = TempDir::new().expect("temp dir");
    let service = StorageService::new(Arc::new(LocalStorageBackend::new(temp.path())));
    let object = test_object("originals/aa/escape\0.txt");

    let err = service
        .get_object(&object)
        .await
        .expect_err("nul path should be rejected");

    assert!(matches!(err, StorageError::InvalidStorageKey { .. }));
}

#[tokio::test]
async fn local_backend_creates_parent_directories() {
    let temp = TempDir::new().expect("temp dir");
    let service = StorageService::new(Arc::new(LocalStorageBackend::new(temp.path())));

    let stored = service
        .put_object(StoragePutRequest {
            namespace: StorageNamespace::Derivatives,
            file_name: Some("preview.txt".to_string()),
            mime_type: Some("text/plain".to_string()),
            bytes: b"derived bytes".to_vec(),
        })
        .await
        .expect("put object");

    let stored_path = temp.path().join(&stored.storage_key);
    let parent = stored_path.parent().expect("stored path has parent");
    assert!(
        tokio::fs::metadata(parent)
            .await
            .expect("parent metadata")
            .is_dir()
    );
    assert_eq!(
        tokio::fs::read(stored_path).await.expect("stored bytes"),
        b"derived bytes"
    );
}

fn test_object(storage_key: &str) -> StoredObject {
    StoredObject {
        object_id: Uuid::parse_str("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa").expect("valid uuid"),
        namespace: StorageNamespace::Originals,
        backend: StorageBackend::Local,
        storage_key: storage_key.to_string(),
        original_file_name: Some("test.txt".to_string()),
        mime_type: Some("text/plain".to_string()),
        size_bytes: 0,
        sha256_hex: sha256_hex(b""),
        created_at: OffsetDateTime::UNIX_EPOCH,
    }
}
