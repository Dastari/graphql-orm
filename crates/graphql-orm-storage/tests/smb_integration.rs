use std::{env, sync::Arc, time::Duration};

use bytes::Bytes;
use futures_util::{StreamExt, stream};
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
    assert!(probe.max_read_size > 0);
    assert!(probe.max_write_size > 0);
    assert!(probe.max_transact_size > 0);
    assert!(probe.write_request_payload_limit > 0);
    assert!(probe.write_request_payload_limit <= probe.max_write_size as usize);
    assert!(probe.write_request_payload_limit <= 1024 * 1024);

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

    store
        .put_blob(
            "nested/small.bin",
            StorageByteStream::from_bytes(Bytes::from_static(b"overwritten payload")),
            BlobPutOptions::default(),
        )
        .await
        .expect("overwrite small");
    let overwritten = store
        .get_blob("nested/small.bin")
        .await
        .expect("get overwritten small");
    assert_eq!(
        collect_storage_stream(overwritten.body)
            .await
            .expect("collect overwritten small"),
        Bytes::from_static(b"overwritten payload")
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

    let collision = store
        .put_blob_if_not_exists(
            "locks/repository.lock",
            StorageByteStream::from_bytes(Bytes::from_static(b"must not replace")),
            BlobPutOptions::default(),
        )
        .await
        .expect("conditional collision");
    assert!(collision.is_none());
    let lock = store
        .get_blob("locks/repository.lock")
        .await
        .expect("load lock after collision");
    let lock = collect_storage_stream(lock.body)
        .await
        .expect("collect lock");
    assert!(
        lock == Bytes::from_static(b"left") || lock == Bytes::from_static(b"right"),
        "collision must not delete or replace the winner"
    );

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

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the managed Samba container harness"]
async fn samba_oversized_chunk_cancellation_and_concurrency_matrix() {
    let root = format!("graphql-orm-storage-hardening/{}", Uuid::new_v4());
    let mut oversized_config = test_config(format!("{root}/oversized"));
    oversized_config.max_transfer_concurrency = 4;
    let oversized_store = Arc::new(
        SmbStorageBackend::connect(oversized_config)
            .await
            .expect("connect oversized store"),
    );

    const OVERSIZED_LEN: usize = 12 * 1024 * 1024 + 17;
    let oversized = Bytes::from(
        (0..OVERSIZED_LEN)
            .map(|index| u8::try_from(index % 251).expect("bounded byte"))
            .collect::<Vec<_>>(),
    );
    let oversized_outcome = oversized_store
        .put_blob(
            "large/single-chunk.bin",
            StorageByteStream::from_bytes(oversized.clone()),
            BlobPutOptions::default(),
        )
        .await
        .expect("put oversized single chunk");
    assert_eq!(oversized_outcome.size_bytes, OVERSIZED_LEN as u64);
    let loaded = oversized_store
        .get_blob("large/single-chunk.bin")
        .await
        .expect("get oversized single chunk");
    assert_eq!(
        collect_storage_stream(loaded.body)
            .await
            .expect("collect oversized single chunk"),
        oversized
    );

    let bytes_before_cancellation = oversized_store.diagnostics().await.bytes_uploaded;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let cancellation_body = stream::once(async move {
        let _ = started_tx.send(());
        Ok(Bytes::from_static(b"cancel after this write"))
    })
    .chain(stream::pending());
    let cancellation_store = Arc::clone(&oversized_store);
    let cancelled = tokio::spawn(async move {
        cancellation_store
            .put_blob_if_not_exists(
                "cancel/direct.bin",
                StorageByteStream::new(Box::pin(cancellation_body)),
                BlobPutOptions::default(),
            )
            .await
    });
    started_rx.await.expect("cancellation stream started");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if oversized_store.diagnostics().await.bytes_uploaded > bytes_before_cancellation {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first cancellation write acknowledged");
    cancelled.abort();
    let _ = cancelled.await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if !oversized_store
                .blob_exists("cancel/direct.bin")
                .await
                .expect("inspect cancelled direct write")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("cancelled direct write cleanup");

    for concurrency in [1_usize, 2, 4, 8] {
        let prefix = format!("{root}/concurrency-{concurrency}");
        let expected_directory_depth = prefix.split('/').count() + 1;
        let mut config = test_config(prefix);
        config.max_transfer_concurrency = concurrency;
        let store = Arc::new(
            SmbStorageBackend::connect(config)
                .await
                .expect("connect matrix"),
        );
        let mut tasks = Vec::new();
        for index in 0..8 {
            let store = Arc::clone(&store);
            tasks.push(tokio::spawn(async move {
                let key = format!("shared-parent/object-{index}.bin");
                store
                    .put_blob(
                        &key,
                        StorageByteStream::from_bytes(Bytes::from(vec![index as u8; 32 * 1024])),
                        BlobPutOptions::default(),
                    )
                    .await
                    .expect("matrix put");
            }));
        }
        for task in tasks {
            task.await.expect("matrix task");
        }
        let diagnostics = store.diagnostics().await;
        assert_eq!(
            diagnostics.uploads_completed, 8,
            "concurrency {concurrency}"
        );
        assert_eq!(
            diagnostics.directory_create_requests, expected_directory_depth as u64,
            "shared parents must be created once at concurrency {concurrency}"
        );

        let before = diagnostics.directory_create_requests;
        for index in 8..24 {
            let key = format!("shared-parent/object-{index}.bin");
            store
                .put_blob(
                    &key,
                    StorageByteStream::from_bytes(Bytes::from_static(b"cached parent")),
                    BlobPutOptions::default(),
                )
                .await
                .expect("cached-parent put");
        }
        assert_eq!(
            store.diagnostics().await.directory_create_requests,
            before,
            "shared parent was reopened for later objects at concurrency {concurrency}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Samba configured with a constrained maximum write size"]
async fn samba_constrained_max_write_handles_oversized_input_chunk() {
    let root = format!("graphql-orm-storage-constrained/{}", Uuid::new_v4());
    let config = test_config(root);
    let probe = SmbStorageBackend::probe(
        config.clone(),
        SmbProbeOptions {
            create_prefix: true,
        },
    )
    .await
    .expect("constrained probe");
    let expected_max = env::var("SMB_TEST_EXPECT_MAX_WRITE")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(65_536);
    assert!(probe.max_write_size <= expected_max);
    assert!(probe.write_request_payload_limit <= probe.max_write_size as usize);

    let store = SmbStorageBackend::connect(config)
        .await
        .expect("connect constrained store");
    let input = Bytes::from(vec![0x5a; 2 * 1024 * 1024 + 9]);
    let outcome = store
        .put_blob(
            "constrained/single-chunk.bin",
            StorageByteStream::from_bytes(input.clone()),
            BlobPutOptions::default(),
        )
        .await
        .expect("constrained write");
    assert_eq!(outcome.size_bytes, input.len() as u64);
    let loaded = store
        .get_blob("constrained/single-chunk.bin")
        .await
        .expect("constrained read");
    assert_eq!(
        collect_storage_stream(loaded.body)
            .await
            .expect("collect constrained object"),
        input
    );
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
    let before = store.diagnostics().await.directory_create_requests;
    store
        .put_blob(
            "reconnect/after-restart.bin",
            StorageByteStream::from_bytes(Bytes::from_static(b"cache was invalidated")),
            BlobPutOptions::default(),
        )
        .await
        .expect("write after reconnect");
    let after = store.diagnostics().await;
    assert!(after.reconnects >= 1);
    assert!(after.directory_create_requests > before);
    store
        .delete_blob("reconnect/blob.bin")
        .await
        .expect("delete after reconnect");
}
