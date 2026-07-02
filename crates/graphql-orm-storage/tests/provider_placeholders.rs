#[cfg(feature = "azure")]
use graphql_orm_storage::{AzureBlobStorageBackend, AzureBlobStorageConfig};
#[cfg(any(feature = "azure", feature = "s3"))]
use graphql_orm_storage::{
    BlobPutOptions, BlobStore, ObjectStorage, StorageBackend, StorageByteStream, StorageError,
    StorageNamespace, StoredObject, sha256_hex,
};
#[cfg(feature = "s3")]
use graphql_orm_storage::{S3StorageBackend, S3StorageConfig};
#[cfg(any(feature = "azure", feature = "s3"))]
use time::OffsetDateTime;
#[cfg(any(feature = "azure", feature = "s3"))]
use uuid::Uuid;

#[cfg(feature = "s3")]
#[test]
fn s3_debug_output_redacts_secret_access_key() {
    let config = s3_config();
    let debug = format!("{config:?}");
    assert!(!debug.contains("super-secret-s3-key"));
    assert!(debug.contains("<redacted>"));
}

#[cfg(feature = "s3")]
#[tokio::test]
async fn s3_backend_exposes_config_and_backend_without_leaking_secrets() {
    let backend = S3StorageBackend::new(s3_config());

    assert_eq!(backend.backend(), StorageBackend::S3);
    assert_eq!(backend.config().bucket, "objects");
}

#[cfg(feature = "azure")]
#[test]
fn azure_debug_output_redacts_connection_string_and_credential() {
    let config = azure_config();
    let debug = format!("{config:?}");
    assert!(!debug.contains("DefaultEndpointsProtocol=https;AccountKey=azure-secret"));
    assert!(!debug.contains("azure-token"));
    assert!(debug.contains("<redacted>"));
}

#[cfg(feature = "azure")]
#[tokio::test]
async fn azure_placeholder_backend_returns_unsupported_errors() {
    let backend = AzureBlobStorageBackend::new(azure_config());
    let object = test_object(StorageBackend::AzureBlob);

    assert_eq!(backend.backend(), StorageBackend::AzureBlob);
    assert_unsupported(
        backend
            .put_blob(
                "objects/test",
                StorageByteStream::from_bytes(b"bytes".to_vec()),
                BlobPutOptions::default(),
            )
            .await,
        "azure_blob",
    );
    assert_unsupported(
        backend
            .put_blob_if_not_exists(
                "objects/test",
                StorageByteStream::from_bytes(b"bytes".to_vec()),
                BlobPutOptions::default(),
            )
            .await,
        "azure_blob",
    );
    assert_unsupported(backend.get_blob("objects/test").await, "azure_blob");
    assert_unsupported(
        backend.get_blob_range("objects/test", 0..1).await,
        "azure_blob",
    );
    assert_unsupported(backend.blob_exists("objects/test").await, "azure_blob");
    assert_unsupported(backend.head_blob("objects/test").await, "azure_blob");
    assert_unsupported(backend.list_blobs("objects").await, "azure_blob");
    assert_unsupported(
        backend.list_blobs_page("objects", None, 100).await,
        "azure_blob",
    );
    assert_unsupported(
        backend.copy_blob("objects/test", "objects/copy").await,
        "azure_blob",
    );
    assert_unsupported(backend.delete_blob("objects/test").await, "azure_blob");
    assert_unsupported(
        backend.put_object(object.clone(), b"bytes".to_vec()).await,
        "azure_blob",
    );
    assert_unsupported(backend.get_object(&object).await, "azure_blob");
    assert_unsupported(backend.delete_object(&object).await, "azure_blob");
}

#[cfg(feature = "s3")]
fn s3_config() -> S3StorageConfig {
    S3StorageConfig {
        endpoint_url: "https://s3.example.test".to_string(),
        region: "test-region".to_string(),
        bucket: "objects".to_string(),
        key_prefix: Some("prefix".to_string()),
        access_key_id: "access-key".to_string(),
        secret_access_key: "super-secret-s3-key".to_string(),
        path_style: true,
    }
}

#[cfg(feature = "azure")]
fn azure_config() -> AzureBlobStorageConfig {
    AzureBlobStorageConfig {
        account: Some("account".to_string()),
        connection_string: Some(
            "DefaultEndpointsProtocol=https;AccountKey=azure-secret".to_string(),
        ),
        container: "objects".to_string(),
        key_prefix: Some("prefix".to_string()),
        credential: Some("azure-token".to_string()),
    }
}

#[cfg(any(feature = "azure", feature = "s3"))]
fn test_object(backend: StorageBackend) -> StoredObject {
    StoredObject {
        object_id: Uuid::parse_str("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa").expect("valid uuid"),
        namespace: StorageNamespace::Originals,
        backend,
        storage_key: "originals/aa/aa/aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa.txt".to_string(),
        original_file_name: Some("test.txt".to_string()),
        mime_type: Some("text/plain".to_string()),
        size_bytes: 0,
        sha256_hex: sha256_hex(b""),
        created_at: OffsetDateTime::UNIX_EPOCH,
    }
}

#[cfg(any(feature = "azure", feature = "s3"))]
fn assert_unsupported<T>(result: Result<T, StorageError>, expected_backend: &str) {
    assert!(matches!(
        result,
        Err(StorageError::UnsupportedBackend { backend }) if backend == expected_backend
    ));
}
