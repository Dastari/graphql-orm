use std::{env, sync::Arc, time::Duration};

use bytes::Bytes;
use futures_util::stream;
use graphql_orm_storage::{
    BlobPutOptions, BlobStore, SmbDialect, SmbProbeOptions, SmbStorageBackend, SmbStorageConfig,
    StorageByteStream, collect_storage_stream,
};
use secrecy::SecretString;
use uuid::Uuid;

fn test_config(root_prefix: String) -> SmbStorageConfig {
    let mut config = SmbStorageConfig::new(
        env::var("SMB_TEST_SERVER").unwrap_or_else(|_| "127.0.0.1".to_string()),
        env::var("SMB_TEST_SHARE").unwrap_or_else(|_| "backups".to_string()),
        env::var("SMB_TEST_USERNAME").unwrap_or_else(|_| "backup".to_string()),
        SecretString::from(
            env::var("SMB_TEST_PASSWORD").unwrap_or_else(|_| "BackupTest-42!".to_string()),
        ),
    );
    config.port = env::var("SMB_TEST_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1445);
    config.domain = env::var("SMB_TEST_DOMAIN").ok();
    config.require_encryption = env::var("SMB_TEST_REQUIRE_ENCRYPTION")
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
    config.root_prefix = Some(root_prefix);
    config.min_dialect = if config.require_encryption {
        SmbDialect::Smb3_0
    } else {
        SmbDialect::Smb2_1
    };
    config.connect_timeout = Duration::from_secs(5);
    config.operation_timeout = Duration::from_secs(20);
    config
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires SMB_TEST_* or the documented Samba container"]
async fn samba_round_trip_streaming_listing_and_atomic_create() {
    let root = format!("graphql-orm-storage-tests/{}", Uuid::new_v4());
    let config = test_config(root.clone());
    let encryption_required = config.require_encryption;
    let probe = SmbStorageBackend::probe(
        config.clone(),
        SmbProbeOptions {
            create_prefix: true,
        },
    )
    .await
    .expect("probe");
    assert!(probe.server_reachable);
    assert!(probe.share_reachable);
    assert!(probe.signing_active);
    assert_eq!(probe.encryption_active, encryption_required);
    assert!(probe.prefix_readable);
    assert!(probe.prefix_writable);

    let store = Arc::new(SmbStorageBackend::connect(config).await.expect("connect"));
    store
        .put_blob(
            "nested/small.bin",
            StorageByteStream::from_bytes(Bytes::from_static(b"small payload")),
            BlobPutOptions::default(),
        )
        .await
        .expect("put small");
    let small = store.get_blob("nested/small.bin").await.expect("get small");
    assert_eq!(
        collect_storage_stream(small.body)
            .await
            .expect("collect small"),
        Bytes::from_static(b"small payload")
    );

    const CHUNK_SIZE: usize = 256 * 1024;
    const CHUNK_COUNT: usize = 32;
    let chunks = (0..CHUNK_COUNT).map(|index| {
        let byte = u8::try_from(index % 251).expect("bounded byte");
        Ok(Bytes::from(vec![byte; CHUNK_SIZE]))
    });
    let large = StorageByteStream::with_size_hint(
        Box::pin(stream::iter(chunks)),
        u64::try_from(CHUNK_SIZE * CHUNK_COUNT).expect("bounded size"),
    );
    let outcome = store
        .put_blob("large/stream.bin", large, BlobPutOptions::default())
        .await
        .expect("put large stream");
    assert_eq!(outcome.size_bytes as usize, CHUNK_SIZE * CHUNK_COUNT);
    let loaded = store
        .get_blob("large/stream.bin")
        .await
        .expect("get large stream");
    let loaded = collect_storage_stream(loaded.body)
        .await
        .expect("collect large stream");
    assert_eq!(loaded.len(), CHUNK_SIZE * CHUNK_COUNT);

    let interrupted = StorageByteStream::new(Box::pin(stream::iter(vec![
        Ok(Bytes::from_static(b"partial")),
        Err(graphql_orm_storage::StorageError::Provider {
            backend: "test-source".to_string(),
            message: "injected interruption".to_string(),
            retryable: true,
        }),
    ])));
    store
        .put_blob(
            "interrupted/upload.bin",
            interrupted,
            BlobPutOptions::default(),
        )
        .await
        .expect_err("interrupted stream must fail");
    assert!(
        !store
            .blob_exists("interrupted/upload.bin")
            .await
            .expect("interrupted final exists")
    );

    let all = store.list_blobs("").await.expect("list all");
    assert_eq!(
        all,
        vec![
            "large/stream.bin".to_string(),
            "nested/small.bin".to_string()
        ]
    );
    assert!(all.iter().all(|key| !key.ends_with(".uploading")));
    assert_eq!(
        store.list_blobs("nested").await.expect("list prefix"),
        vec!["nested/small.bin".to_string()]
    );

    let left = Arc::clone(&store);
    let right = Arc::clone(&store);
    let (left_result, right_result) = tokio::join!(
        left.put_blob_if_not_exists(
            "locks/repository.lock",
            StorageByteStream::from_bytes(Bytes::from_static(b"left")),
            BlobPutOptions::default(),
        ),
        right.put_blob_if_not_exists(
            "locks/repository.lock",
            StorageByteStream::from_bytes(Bytes::from_static(b"right")),
            BlobPutOptions::default(),
        )
    );
    let created = [
        left_result.expect("left create"),
        right_result.expect("right create"),
    ]
    .into_iter()
    .filter(Option::is_some)
    .count();
    assert_eq!(created, 1, "exactly one FILE_CREATE must succeed");

    for key in [
        "nested/small.bin",
        "large/stream.bin",
        "locks/repository.lock",
    ] {
        store.delete_blob(key).await.expect("delete");
        assert!(!store.blob_exists(key).await.expect("exists after delete"));
    }
    assert!(store.list_blobs("").await.expect("empty list").is_empty());
}

#[tokio::test]
#[ignore = "requires the documented Samba container"]
async fn samba_rejects_invalid_password() {
    let mut config = test_config(format!("graphql-orm-storage-tests/{}", Uuid::new_v4()));
    config.password = SecretString::from("definitely-wrong-password");
    let error = SmbStorageBackend::connect(config)
        .await
        .expect_err("authentication must fail");
    assert!(!error.is_retryable());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the managed Samba container harness"]
async fn samba_reconnects_after_server_restart() {
    let Ok(container) = env::var("SMB_TEST_CONTAINER_NAME") else {
        return;
    };
    let root = format!("graphql-orm-storage-reconnect/{}", Uuid::new_v4());
    let store = SmbStorageBackend::connect(test_config(root))
        .await
        .expect("connect before restart");
    store
        .put_blob(
            "reconnect/blob.bin",
            StorageByteStream::from_bytes(Bytes::from_static(b"survives restart")),
            BlobPutOptions::default(),
        )
        .await
        .expect("write before restart");

    let status = std::process::Command::new("docker")
        .args(["restart", &container])
        .status()
        .expect("run docker restart");
    assert!(status.success(), "docker restart failed");

    assert!(
        store
            .blob_exists("reconnect/blob.bin")
            .await
            .expect("reconnect and inspect remote state")
    );
    store
        .delete_blob("reconnect/blob.bin")
        .await
        .expect("delete after reconnect");
}
