#![cfg(feature = "s3")]

use bytes::Bytes;
use graphql_orm_storage::{
    BlobPutOptions, BlobStore, S3StorageBackend, S3StorageConfig, StorageByteStream,
    collect_storage_stream, sha256_hex,
};
use uuid::Uuid;

#[tokio::test]
async fn s3_blob_store_round_trip_when_env_is_configured() {
    let Some(backend) = backend_from_env() else {
        return;
    };

    let key = "objects/test.txt";
    let copy_key = "objects/test-copy.txt";

    let outcome = backend
        .put_blob(
            key,
            StorageByteStream::from_bytes(Bytes::from_static(b"hello s3 storage")),
            BlobPutOptions {
                content_type: Some("text/plain".to_string()),
            },
        )
        .await
        .expect("put object");
    assert_eq!(outcome.size_bytes, 16);
    assert_eq!(outcome.sha256_hex, sha256_hex(b"hello s3 storage"));

    assert!(backend.blob_exists(key).await.expect("exists"));

    let metadata = backend
        .head_blob(key)
        .await
        .expect("head")
        .expect("metadata");
    assert_eq!(metadata.key, key);
    assert_eq!(metadata.size_bytes, Some(16));

    let body = backend.get_blob(key).await.expect("get object");
    let bytes = collect_storage_stream(body.body).await.expect("collect");
    assert_eq!(bytes, Bytes::from_static(b"hello s3 storage"));

    let ranged = backend
        .get_blob_range(key, 6..8)
        .await
        .expect("range object");
    let ranged_bytes = collect_storage_stream(ranged.body)
        .await
        .expect("collect range");
    assert_eq!(ranged_bytes, Bytes::from_static(b"s3"));

    let conditional = backend
        .put_blob_if_not_exists(
            key,
            StorageByteStream::from_bytes(Bytes::from_static(b"replacement")),
            BlobPutOptions::default(),
        )
        .await
        .expect("conditional write");
    assert_eq!(conditional, None);

    backend.copy_blob(key, copy_key).await.expect("copy object");
    let page = backend
        .list_blobs_page("objects", None, 1)
        .await
        .expect("list page");
    assert_eq!(page.keys.len(), 1);
    assert!(page.next_continuation.is_some());

    backend.delete_blob(key).await.expect("delete object");
    backend
        .delete_blob(copy_key)
        .await
        .expect("delete copied object");
}

fn backend_from_env() -> Option<S3StorageBackend> {
    let endpoint_url = std::env::var("S3_TEST_ENDPOINT").ok()?;
    let bucket = std::env::var("S3_TEST_BUCKET").ok()?;
    let region = std::env::var("S3_TEST_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    let access_key_id =
        std::env::var("S3_TEST_ACCESS_KEY").unwrap_or_else(|_| "minioadmin".to_string());
    let secret_access_key =
        std::env::var("S3_TEST_SECRET_KEY").unwrap_or_else(|_| "minioadmin".to_string());
    let path_style = std::env::var("S3_TEST_PATH_STYLE")
        .map(|value| value != "false")
        .unwrap_or(true);

    Some(S3StorageBackend::new(S3StorageConfig {
        endpoint_url,
        region,
        bucket,
        key_prefix: Some(format!("graphql-orm-storage-tests/{}", Uuid::new_v4())),
        access_key_id,
        secret_access_key,
        path_style,
    }))
}
