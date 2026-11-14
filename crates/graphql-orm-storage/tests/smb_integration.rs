use std::{env, sync::Arc, time::Duration};

use bytes::Bytes;
use futures_util::{StreamExt, stream};
use graphql_orm_storage::{
    BlobPutOptions, BlobStore, SmbDialect, SmbProbeOptions, SmbStorageBackend, SmbStorageConfig,
    StorageByteStream, StorageProviderErrorKind, collect_storage_stream,
};
use secrecy::SecretString;
use uuid::Uuid;

#[path = "support/listing_contract.rs"]
mod listing_contract;

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
    // Keys are checked before put_blob derives its longer temporary key: an
    // over-long key is refused by name, and a key at the limit passes the
    // backend's checks. (Samba on Linux refuses a path this long itself.)
    let key_of = |bytes: usize| {
        let mut key = String::new();
        while key.len() + 256 < bytes {
            if !key.is_empty() {
                key.push('/');
            }
            key.push_str(&"k".repeat(250));
        }
        let rest = bytes - key.len() - 1;
        key.push('/');
        key.push_str(&"z".repeat(rest));
        key
    };
    let too_long = key_of(4_097);
    match store
        .put_blob(
            &too_long,
            StorageByteStream::from_bytes(Bytes::from_static(b"x")),
            BlobPutOptions::default(),
        )
        .await
    {
        Err(graphql_orm_storage::StorageError::InvalidStorageKey { key }) => {
            assert_eq!(key, too_long, "the error names the caller's key");
        }
        other => panic!("an over-long key must be refused: {other:?}"),
    }
    let at_limit = key_of(4_096);
    assert_eq!(at_limit.len(), 4_096);
    for result in [
        store
            .put_blob(
                &at_limit,
                StorageByteStream::from_bytes(Bytes::from_static(b"x")),
                BlobPutOptions::default(),
            )
            .await
            .map(|_| ()),
        store
            .put_blob_if_not_exists(
                &at_limit,
                StorageByteStream::from_bytes(Bytes::from_static(b"x")),
                BlobPutOptions::default(),
            )
            .await
            .map(|_| ()),
    ] {
        assert!(
            !matches!(
                result,
                Err(graphql_orm_storage::StorageError::InvalidStorageKey { .. })
            ),
            "a key at the limit must reach the server: {result:?}"
        );
    }
    // A server that accepts the path stores the object; remove it.
    let _ = store.delete_blob(&at_limit).await;

    // Paging returns the same keys through the backend's bounded tokens.
    let first = store
        .list_blobs_page("", None, 1)
        .await
        .expect("first page");
    assert_eq!(first.keys, ["large/stream.bin"]);
    let token = first.next_continuation.expect("a second page");
    let second = store
        .list_blobs_page("", Some(token), 1)
        .await
        .expect("second page");
    assert_eq!(second.keys, ["nested/small.bin"]);
    assert!(second.next_continuation.is_none());
    assert!(
        store
            .list_blobs_page("", Some("large/stream.bin".to_string()), 1)
            .await
            .is_err(),
        "a token this backend did not issue must be rejected"
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
    let fixture = verified_samba_fixture(
        env::var("SMB_TEST_CONTAINER_NAME").ok(),
        env::var("SMB_TEST_FIXTURE_RUN").ok(),
        |container| {
            let ownership = std::process::Command::new("docker")
                .args([
                    "inspect",
                    "--format",
                    "{{index .Config.Labels \"graphql-orm.samba-fixture-run\"}}",
                    container,
                ])
                .output()
                .map_err(|_| "cannot inspect managed fixture".to_string())?;
            if !ownership.status.success() {
                return Err("cannot verify managed fixture ownership".to_string());
            }
            String::from_utf8(ownership.stdout)
                .map_err(|_| "invalid ownership response".to_string())
        },
    )
    .expect("managed reconnect requires verified fixture ownership");
    let container = &fixture.container;
    let root = format!("graphql-orm-storage-reconnect/{}", Uuid::new_v4());
    let config = test_config(root);
    assert_eq!(
        config.server, "127.0.0.1",
        "managed reconnect fixture must be loopback-owned"
    );
    let endpoint = (std::net::Ipv4Addr::LOCALHOST, config.port);
    let expected_mapping = format!("127.0.0.1:{}", config.port);
    let store = SmbStorageBackend::connect(config)
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

    fixture
        .restart_with(|container| {
            let status = std::process::Command::new("docker")
                .args(["restart", container])
                .status()
                .map_err(|_| "cannot restart owned fixture".to_string())?;
            if status.success() {
                Ok(())
            } else {
                Err("owned fixture restart failed".to_string())
            }
        })
        .expect("restart verified owned fixture");
    let published = std::process::Command::new("docker")
        .args(["port", container.as_str(), "445/tcp"])
        .output()
        .expect("inspect owned restarted fixture port");
    assert!(published.status.success(), "inspect restarted port failed");
    assert!(
        String::from_utf8(published.stdout)
            .expect("published port text")
            .lines()
            .any(|line| line == expected_mapping),
        "owned endpoint changed after restart"
    );
    // Docker restart returns before smbd necessarily listens. Wait on the exact
    // original endpoint, then exercise the existing backend's reconnect behavior.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if tokio::net::TcpStream::connect(endpoint).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("owned Samba did not become ready after restart");

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

fn owned_fixture() -> VerifiedSambaFixture {
    verified_samba_fixture(
        env::var("SMB_TEST_CONTAINER_NAME").ok(),
        env::var("SMB_TEST_FIXTURE_RUN").ok(),
        |container| {
            let ownership = std::process::Command::new("docker")
                .args([
                    "inspect",
                    "--format",
                    "{{index .Config.Labels \"graphql-orm.samba-fixture-run\"}}",
                    container,
                ])
                .output()
                .map_err(|_| "cannot inspect managed fixture".to_string())?;
            if !ownership.status.success() {
                return Err("cannot verify managed fixture ownership".to_string());
            }
            String::from_utf8(ownership.stdout)
                .map_err(|_| "invalid ownership response".to_string())
        },
    )
    .expect("this test requires verified fixture ownership")
}

fn assert_refused(error: &graphql_orm_storage::StorageError, expected: StorageProviderErrorKind) {
    match error {
        graphql_orm_storage::StorageError::RemoteProvider {
            kind, retryable, ..
        } => {
            assert_eq!(*kind, expected, "{error}");
            assert!(!retryable, "{error}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the managed Samba container harness"]
async fn samba_self_nesting_listing_terminates() {
    let fixture = owned_fixture();
    let root = format!("graphql-orm-storage-loop/{}", Uuid::new_v4());
    let mut config = test_config(root.clone());
    config.max_listing_depth = 8;
    let store = SmbStorageBackend::connect(config).await.expect("connect");
    store
        .put_blob(
            "seed.bin",
            StorageByteStream::from_bytes(Bytes::from_static(b"seed")),
            BlobPutOptions::default(),
        )
        .await
        .expect("create the listing root");
    // A server-side link to its own directory nests the tree without end.
    let link = format!("/share/{root}/loop");
    let docker_exec = |args: &[&str]| {
        fixture.run_with(|container| {
            let status = std::process::Command::new("docker")
                .args(["exec", container])
                .args(args)
                .status()
                .map_err(|_| "cannot run in owned fixture".to_string())?;
            if status.success() {
                Ok(())
            } else {
                Err("owned fixture command failed".to_string())
            }
        })
    };
    docker_exec(&["ln", "-s", ".", &link]).expect("create a self-nesting directory");

    let result = tokio::time::timeout(Duration::from_secs(60), store.list_blobs("")).await;
    docker_exec(&["rm", "-f", &link]).expect("remove the self-nesting directory");
    let error = result
        .expect("a self-nesting listing must terminate")
        .expect_err("a self-nesting listing must fail instead of returning a partial listing");
    assert_refused(&error, StorageProviderErrorKind::Protocol);
    assert!(
        error.to_string().contains("maximum directory depth of 8"),
        "{error}"
    );

    assert_eq!(store.list_blobs("").await.expect("list"), ["seed.bin"]);
    store.delete_blob("seed.bin").await.expect("delete");
}

/// Forwards SMB connections to a switchable local server, recording the SMB2
/// command of each request the client sends to each server port.
struct SwitchingProxy {
    port: u16,
    target: Arc<std::sync::atomic::AtomicU16>,
    connections: Arc<std::sync::Mutex<Vec<tokio::task::AbortHandle>>>,
    requests: Arc<std::sync::Mutex<Vec<(u16, u16)>>>,
}

impl SwitchingProxy {
    async fn start(target: u16) -> Self {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind proxy");
        let proxy = Self {
            port: listener.local_addr().expect("proxy address").port(),
            target: Arc::new(std::sync::atomic::AtomicU16::new(target)),
            connections: Arc::default(),
            requests: Arc::default(),
        };
        let target = Arc::clone(&proxy.target);
        let connections = Arc::clone(&proxy.connections);
        let requests = Arc::clone(&proxy.requests);
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let port = target.load(std::sync::atomic::Ordering::SeqCst);
                let Ok(server) =
                    tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await
                else {
                    continue;
                };
                let (mut client_read, mut client_write) = client.into_split();
                let (mut server_read, mut server_write) = server.into_split();
                let requests = Arc::clone(&requests);
                let upstream = tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut pending = Vec::new();
                    let mut buffer = vec![0_u8; 64 * 1024];
                    while let Ok(read) = client_read.read(&mut buffer).await {
                        if read == 0 || server_write.write_all(&buffer[..read]).await.is_err() {
                            break;
                        }
                        pending.extend_from_slice(&buffer[..read]);
                        while pending.len() >= 4 {
                            let length =
                                u32::from_be_bytes(pending[..4].try_into().unwrap()) as usize;
                            if pending.len() < 4 + length {
                                break;
                            }
                            let message = &pending[4..4 + length];
                            let command = if message.starts_with(b"\xfeSMB") && length >= 14 {
                                u16::from_le_bytes([message[12], message[13]])
                            } else {
                                u16::MAX
                            };
                            requests.lock().unwrap().push((port, command));
                            pending.drain(..4 + length);
                        }
                    }
                });
                let downstream = tokio::spawn(async move {
                    let _ = tokio::io::copy(&mut server_read, &mut client_write).await;
                });
                let mut connections = connections.lock().unwrap();
                connections.push(upstream.abort_handle());
                connections.push(downstream.abort_handle());
            }
        });
        proxy
    }

    /// Sends new connections to `target` and drops every current connection.
    fn switch_to(&self, target: u16) {
        self.target
            .store(target, std::sync::atomic::Ordering::SeqCst);
        for connection in self.connections.lock().unwrap().drain(..) {
            connection.abort();
        }
    }

    fn commands_sent_to(&self, target: u16) -> Vec<u16> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(port, _)| *port == target)
            .map(|(_, command)| *command)
            .collect()
    }
}

const SMB2_NEGOTIATE: u16 = 0;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires two managed Samba servers with the same credentials"]
async fn samba_reconnect_refuses_a_server_with_another_identity() {
    let primary_port: u16 = env::var("SMB_TEST_PORT")
        .expect("SMB_TEST_PORT")
        .parse()
        .expect("primary port");
    let other_port: u16 = env::var("SMB_TEST_ALTERNATE_PORT")
        .expect("SMB_TEST_ALTERNATE_PORT names a second server with the same credentials")
        .parse()
        .expect("alternate port");
    let root = format!("graphql-orm-storage-identity/{}", Uuid::new_v4());

    // An explicit pin refuses a server whose GUID differs from the first
    // connection on, and accepts the server it names.
    let mut pinned = test_config(root.clone());
    pinned.expected_server_guid = Some(Uuid::new_v4());
    let error = SmbStorageBackend::connect(pinned.clone())
        .await
        .expect_err("an unexpected server identity must be refused");
    assert_refused(&error, StorageProviderErrorKind::SecurityNegotiation);
    let probe = SmbStorageBackend::probe(
        test_config(root.clone()),
        SmbProbeOptions {
            create_prefix: true,
        },
    )
    .await
    .expect("probe");
    assert!(!probe.server_guid.is_nil());
    pinned.expected_server_guid = Some(probe.server_guid);
    SmbStorageBackend::connect(pinned)
        .await
        .expect("the pinned server is accepted");

    // Without an explicit pin, the first connection pins the server.
    let proxy = SwitchingProxy::start(primary_port).await;
    let mut config = test_config(root);
    config.port = proxy.port;
    let store = SmbStorageBackend::connect(config).await.expect("connect");
    store
        .put_blob(
            "identity/blob.bin",
            StorageByteStream::from_bytes(Bytes::from_static(b"pinned")),
            BlobPutOptions::default(),
        )
        .await
        .expect("write to the pinned server");

    // The same name now answers from another server that accepts the same
    // credentials. The reconnect must stop before authenticating.
    proxy.switch_to(other_port);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let error = store
        .list_blobs("")
        .await
        .expect_err("a reconnect to another server must be refused");
    assert_refused(&error, StorageProviderErrorKind::SecurityNegotiation);
    let sent = proxy.commands_sent_to(other_port);
    assert!(
        !sent.is_empty(),
        "the reconnect must reach the other server"
    );
    assert!(
        sent.iter().all(|command| *command == SMB2_NEGOTIATE),
        "only NEGOTIATE may reach a server with another identity, sent {sent:?}"
    );
    assert_eq!(store.diagnostics().await.reconnects, 0);

    // The pinned server is accepted again.
    proxy.switch_to(primary_port);
    assert_eq!(
        store
            .list_blobs("")
            .await
            .expect("reconnect to the pinned server"),
        ["identity/blob.bin"]
    );
    assert_eq!(store.diagnostics().await.reconnects, 1);
    store
        .delete_blob("identity/blob.bin")
        .await
        .expect("delete");
}

// A restart capability is constructed only after a successful ownership read.
struct VerifiedSambaFixture {
    container: String,
}
impl VerifiedSambaFixture {
    fn restart_with(&self, restart: impl FnOnce(&str) -> Result<(), String>) -> Result<(), String> {
        restart(&self.container)
    }
    fn run_with(&self, command: impl FnOnce(&str) -> Result<(), String>) -> Result<(), String> {
        command(&self.container)
    }
}
fn verified_samba_fixture(
    container: Option<String>,
    run: Option<String>,
    inspect: impl FnOnce(&str) -> Result<String, String>,
) -> Result<VerifiedSambaFixture, String> {
    let container = container.ok_or("missing immutable fixture ID")?;
    let run = run.ok_or("missing fixture run ID")?;
    if container.len() != 64 || !container.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid immutable fixture ID".to_string());
    }
    if run.len() != 32 || !run.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid fixture run ID".to_string());
    }
    if inspect(&container)?.trim() != run {
        return Err("fixture ownership mismatch".to_string());
    }
    Ok(VerifiedSambaFixture { container })
}

#[test]
fn restart_guard_denies_missing_failed_and_mismatched_ownership_without_mutation() {
    use std::cell::Cell;
    let id = "a".repeat(64);
    let run = "b".repeat(32);
    for (container, expected_run, inspected_owner) in [
        (None, Some(run.clone()), Ok(run.clone())),
        (Some(id.clone()), None, Ok(run.clone())),
        (
            Some("ambient-container-name".to_string()),
            Some(run.clone()),
            Ok(run.clone()),
        ),
        (
            Some(id.clone()),
            Some(run.clone()),
            Err("failed inspect".to_string()),
        ),
        (Some(id.clone()), Some(run.clone()), Ok(String::new())),
        (
            Some(id.clone()),
            Some(run.clone()),
            Ok("foreign-run".to_string()),
        ),
    ] {
        let mutations = Cell::new(0);
        let outcome = verified_samba_fixture(container, expected_run, |_| inspected_owner)
            .and_then(|fixture| {
                fixture.restart_with(|_| {
                    mutations.set(mutations.get() + 1);
                    Ok(())
                })
            });
        assert!(outcome.is_err());
        assert_eq!(
            mutations.get(),
            0,
            "an unverified fixture must never reach mutation"
        );
    }
    let mutations = Cell::new(0);
    verified_samba_fixture(Some(id.clone()), Some(run.clone()), |inspected| {
        assert_eq!(inspected, id);
        Ok(format!("{run}\n"))
    })
    .expect("matching inspect grants capability")
    .restart_with(|restarted| {
        assert_eq!(restarted, id);
        mutations.set(1);
        Ok(())
    })
    .expect("verified restart callback");
    assert_eq!(mutations.get(), 1);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the managed Samba container harness"]
async fn samba_listing_follows_the_provider_prefix_contract() {
    let root = format!("graphql-orm-storage-tests/{}", Uuid::new_v4());
    let mut config = test_config(root);
    // The default minimum dialect, SMB 3.1.1, is what this server negotiates.
    config.min_dialect = SmbDialect::default();
    assert_eq!(config.min_dialect, SmbDialect::Smb3_1_1);
    let probe = SmbStorageBackend::probe(
        config.clone(),
        SmbProbeOptions {
            create_prefix: true,
        },
    )
    .await
    .expect("probe");
    assert_eq!(probe.negotiated_dialect, SmbDialect::Smb3_1_1);
    let store = SmbStorageBackend::connect(config).await.expect("connect");
    listing_contract::prefixes_are_string_prefixes(&store, "contract").await;
    listing_contract::the_internal_namespace_is_reserved(&store).await;
    // `:` separates an alternate data stream on SMB servers.
    for key in ["contract/a:b", "contract/x::$DATA"] {
        assert!(
            matches!(
                store
                    .put_blob(
                        key,
                        StorageByteStream::from_bytes(Bytes::from_static(b"x")),
                        BlobPutOptions::default(),
                    )
                    .await,
                Err(graphql_orm_storage::StorageError::InvalidStorageKey { .. })
            ),
            "{key}"
        );
        assert!(matches!(
            store.list_blobs(key).await,
            Err(graphql_orm_storage::StorageError::InvalidStorageKey { .. })
        ));
    }
}

/// With `portable_keys`, a new write of a key a Windows target cannot store
/// as written is refused before any request, while objects stored earlier
/// under such names stay listed, readable and deletable (#482).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the managed Samba container harness"]
async fn samba_portable_keys_refuse_windows_unsafe_names() {
    use graphql_orm_storage::StorageError;

    let root = format!("graphql-orm-storage-portable/{}", Uuid::new_v4());
    let posix = SmbStorageBackend::connect(test_config(root.clone()))
        .await
        .expect("connect");
    let mut config = test_config(root);
    config.portable_keys = true;
    let portable = SmbStorageBackend::connect(config).await.expect("connect");
    let body = || StorageByteStream::from_bytes(Bytes::from_static(b"portable"));

    // Names a POSIX share stores, written before the mode was enabled.
    let existing = ["names/CON.txt", "names/kept.bin", "names/report."];
    for key in existing {
        posix
            .put_blob(key, body(), BlobPutOptions::default())
            .await
            .unwrap_or_else(|error| panic!("{key}: {error}"));
    }

    // Existing objects stay listed: the portable listing is the ordinary
    // one. Samba's default `mangled names = yes` presents names Windows
    // cannot store under 8.3 aliases, such as `CSHOFG~F.TXT` for `CON.txt`,
    // in every listing; the mode does not change what the server presents.
    let listed = portable.list_blobs("names/").await.expect("portable list");
    assert_eq!(listed, posix.list_blobs("names/").await.expect("list"));
    assert_eq!(listed.len(), existing.len(), "{listed:?}");
    assert!(listed.contains(&"names/kept.bin".to_string()), "{listed:?}");
    for key in ["names/report.", "names/CON.txt"] {
        let read = portable
            .get_blob(key)
            .await
            .expect("an existing object reads");
        assert_eq!(
            collect_storage_stream(read.body).await.expect("body"),
            Bytes::from_static(b"portable")
        );
        assert!(portable.blob_exists(key).await.expect("exists"));
    }

    // New writes of non-portable keys are refused, and the refusal does not
    // reveal the key or the root prefix.
    for key in [
        "names/report.",
        "names/CON.txt",
        "names/a?b",
        "LPT1/x",
        "new.",
    ] {
        let refused = |result: Result<(), StorageError>| match result {
            Err(error @ StorageError::InvalidStorageKey { .. }) => {
                let message = error.to_string();
                assert!(!message.contains(key), "{message}");
                assert!(
                    !message.contains("graphql-orm-storage-portable"),
                    "{message}"
                );
            }
            other => panic!("{key}: {other:?}"),
        };
        refused(
            portable
                .put_blob(key, body(), BlobPutOptions::default())
                .await
                .map(drop),
        );
        refused(
            portable
                .put_blob_if_not_exists(key, body(), BlobPutOptions::default())
                .await
                .map(drop),
        );
        refused(portable.copy_blob("names/kept.bin", key).await);
    }

    portable
        .put_blob("names/portable.bin", body(), BlobPutOptions::default())
        .await
        .expect("a portable key is written");

    // Existing non-portable objects can still be deleted.
    portable
        .delete_blob("names/report.")
        .await
        .expect("an existing object deletes");
    assert!(!posix.blob_exists("names/report.").await.expect("exists"));
    for key in ["names/CON.txt", "names/kept.bin", "names/portable.bin"] {
        portable.delete_blob(key).await.expect("delete");
    }
    assert!(posix.list_blobs("").await.expect("list").is_empty());
}

/// The sweep removes only this backend's stale temporary upload names, by
/// the server's clock, through a validated exclusive handle (#593):
///
/// - fresh, future-dated and callers' look-alike names are kept;
/// - a stale temporary file another handle still has open is skipped, and
///   an upload in progress keeps its temporary file and still publishes;
/// - a stale temporary file behind a symbolic link survives, and nothing
///   outside the root prefix is touched.
///
/// Samba 4.12, the managed server, resolves symbolic links itself and never
/// reports them to SMB2 clients as reparse points. It refuses links that
/// leave the share (`wide links = no`), which this test uses; a link that
/// stays inside the share is presented as an ordinary directory and cannot
/// be detected by any client, so it is not modelled here. Reparse-point
/// entries and handles, as Windows servers report junctions and symbolic
/// links, are covered by the handle and entry checks, not by this server.
/// A file that changes between enumeration and removal is covered by a unit
/// test of the handle re-check, because the server offers no hook between
/// the two steps.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the managed Samba container harness"]
async fn samba_sweep_removes_only_stale_temporary_files() {
    use graphql_orm_storage::SmbUploadsQuiesced;

    let fixture = owned_fixture();
    let root = format!("graphql-orm-storage-sweep/{}", Uuid::new_v4());
    let store = SmbStorageBackend::connect(test_config(root.clone()))
        .await
        .expect("connect");
    let quiesced = SmbUploadsQuiesced::confirmed_by_host();
    let hour = Duration::from_secs(60 * 60);
    let caller_lookalike = format!("sweep/x.{}.uploading", Uuid::new_v4());
    let callers = [
        "sweep/kept.bin".to_string(),
        "sweep/report.uploading".to_string(),
        caller_lookalike.clone(),
    ];
    for key in &callers {
        store
            .put_blob(
                key,
                StorageByteStream::from_bytes(Bytes::from_static(b"caller")),
                BlobPutOptions::default(),
            )
            .await
            .unwrap_or_else(|error| panic!("{key}: {error}"));
    }
    let stale = [
        format!("sweep/.{}.uploading", Uuid::new_v4()),
        format!(".{}.uploading", Uuid::new_v4()),
        format!("sweep/.blob.bin.{}.uploading", Uuid::new_v4()),
    ];
    let fresh = format!("sweep/.{}.uploading", Uuid::new_v4());
    let open_elsewhere = format!("sweep/.{}.uploading", Uuid::new_v4());
    let future = format!("sweep/.{}.uploading", Uuid::new_v4());
    let outside_temp = format!(".{}.uploading", Uuid::new_v4());
    let outside = format!("/outside-{}", Uuid::new_v4());
    let share = format!("/share/{root}");
    let shell = |script: String| {
        fixture.run_with(|container| {
            let status = std::process::Command::new("docker")
                .args(["exec", container, "sh", "-c", &script])
                .status()
                .map_err(|_| "cannot run in owned fixture".to_string())?;
            if status.success() {
                Ok(())
            } else {
                Err("owned fixture command failed".to_string())
            }
        })
    };
    // Interrupted uploads, owned like the share's files. The stale ones and
    // the callers' keys were last written in 2020, one in 2100. A link to a
    // directory outside the share holds another stale temporary file. The
    // fixture has BusyBox.
    let mut script = format!("set -e\nowner=$(stat -c %u:%g '{share}/sweep')\n");
    for name in stale.iter().chain([&fresh, &future, &open_elsewhere]) {
        script.push_str(&format!(
            "printf partial > '{share}/{name}'\nchown \"$owner\" '{share}/{name}'\n"
        ));
    }
    for name in stale.iter().chain(&callers).chain([&open_elsewhere]) {
        script.push_str(&format!("touch -t 202001010000 '{share}/{name}'\n"));
    }
    script.push_str(&format!("touch -t 210001010000 '{share}/{future}'\n"));
    script.push_str(&format!(
        "mkdir -p '{outside}'\nprintf partial > '{outside}/{outside_temp}'\n\
         chown -R \"$owner\" '{outside}'\ntouch -t 202001010000 '{outside}/{outside_temp}'\n\
         ln -s '{outside}' '{share}/sweep/linked'\n"
    ));
    shell(script).expect("seed temporary files");

    // Another connection holds a stale temporary file open, as an upload
    // between writes would; the read stream is never consumed.
    let other = SmbStorageBackend::connect(test_config(root.clone()))
        .await
        .expect("connect");
    let holder = other
        .get_blob(&open_elsewhere)
        .await
        .expect("open the stale temporary file elsewhere");

    // An upload in progress: its temporary file is open, then dated 2020.
    // Samba reports an open file's write time from its own state, so this
    // file need not look stale; it must survive either way.
    let (release, released) = tokio::sync::oneshot::channel();
    let writer = store.clone();
    let upload = tokio::spawn(async move {
        writer
            .put_blob(
                "held/in-progress.bin",
                held_body(
                    Bytes::from_static(b"head-"),
                    Bytes::from_static(b"tail"),
                    released,
                ),
                BlobPutOptions::default(),
            )
            .await
    });
    let mut held_temp = None;
    for _ in 0..200 {
        let mut found = String::new();
        fixture
            .run_with(|container| {
                let output = std::process::Command::new("docker")
                    .args(["exec", container, "sh", "-c"])
                    .arg(format!(
                        "ls -a '{share}/held' 2>/dev/null | grep uploading || true"
                    ))
                    .output()
                    .map_err(|_| "cannot run in owned fixture".to_string())?;
                found = String::from_utf8_lossy(&output.stdout).trim().to_string();
                Ok(())
            })
            .expect("inspect the upload");
        if !found.is_empty() {
            held_temp = Some(found);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let held_temp = held_temp.expect("the upload created its temporary file");
    // Let any delayed write-time update land before dating the file.
    tokio::time::sleep(Duration::from_secs(3)).await;
    shell(format!("touch -t 202001010000 '{share}/held/{held_temp}'"))
        .expect("date the held upload");

    let too_short = store
        .sweep_temp_files(quiesced, Duration::from_secs(30 * 60))
        .await
        .expect_err("an age below one hour is refused");
    assert_refused(&too_short, StorageProviderErrorKind::Protocol);

    let report = store.sweep_temp_files(quiesced, hour).await.expect("sweep");
    assert_eq!(report.removed, stale.len(), "{report:?}");
    assert_eq!(report.skipped_in_use, 1, "{report:?}");
    // The link out of the share is refused by the server and skipped.
    assert_eq!(report.skipped_directories, 1, "{report:?}");
    // An ordinary listing fails closed on the refused link, so remove it.
    shell(format!("rm '{share}/sweep/linked'")).expect("remove the link");
    let mut expected = callers.to_vec();
    expected.sort();
    assert_eq!(store.list_blobs("sweep/").await.expect("list"), expected);
    let mut check = String::from("set -e\n");
    for name in &stale {
        check.push_str(&format!("test ! -e '{share}/{name}'\n"));
    }
    for name in [&fresh, &future, &open_elsewhere] {
        check.push_str(&format!("test -f '{share}/{name}'\n"));
    }
    check.push_str(&format!("test -f '{share}/held/{held_temp}'\n"));
    check.push_str(&format!("test -f '{outside}/{outside_temp}'\n"));
    shell(check).expect("only the stale temporary files inside the root were removed");

    drop(holder);
    // The upload that was in progress still publishes.
    release.send(()).expect("release the upload");
    upload
        .await
        .expect("upload task")
        .expect("the held upload publishes");
    let published = store.get_blob("held/in-progress.bin").await.expect("read");
    assert_eq!(
        collect_storage_stream(published.body).await.expect("body"),
        Bytes::from_static(b"head-tail")
    );

    // Nothing stale is left to remove.
    let again = store
        .sweep_temp_files(quiesced, hour)
        .await
        .expect("second sweep");
    // Once released, the stale file is removed by the next sweep.
    assert_eq!(again.removed, 1, "{again:?}");
    assert_eq!(again.skipped_in_use, 0, "{again:?}");
    shell(format!("test ! -e '{share}/{open_elsewhere}'")).expect("removed once released");

    shell(format!(
        "rm -f '{share}/{fresh}' '{share}/{future}' '{share}/sweep/linked'\nrm -rf '{outside}'"
    ))
    .expect("remove the seeded files");
    for key in store.list_blobs("").await.expect("list") {
        store.delete_blob(&key).await.expect("delete");
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a managed Samba server limited to SMB 3.0"]
async fn samba_refuses_a_dialect_below_the_minimum() {
    let root = format!("graphql-orm-storage-tests/{}", Uuid::new_v4());
    let mut config = test_config(root);
    config.min_dialect = SmbDialect::default();
    // Offered only SMB 3.1.1, this server refuses the negotiation itself; a
    // server that selected SMB 3.0 anyway would be refused by the client.
    let error = SmbStorageBackend::connect(config.clone())
        .await
        .expect_err("SMB 3.0 is below the default minimum");
    match &error {
        graphql_orm_storage::StorageError::RemoteProvider {
            kind, retryable, ..
        } => {
            assert!(
                matches!(
                    kind,
                    StorageProviderErrorKind::Protocol
                        | StorageProviderErrorKind::SecurityNegotiation
                ),
                "{error}"
            );
            assert!(!retryable, "{error}");
        }
        other => panic!("unexpected error: {other:?}"),
    }

    // SMB 3.0 is an explicit opt-in.
    config.min_dialect = SmbDialect::Smb3_0;
    let probe = SmbStorageBackend::probe(
        config,
        SmbProbeOptions {
            create_prefix: true,
        },
    )
    .await
    .expect("probe with SMB 3.0 allowed");
    assert_eq!(probe.negotiated_dialect, SmbDialect::Smb3_0);
}

/// Waits until the backend's SMB WRITE responses acknowledge more than
/// `before` bytes, so a stalled upload has reached the server.
async fn wait_for_uploaded_bytes(store: &SmbStorageBackend, before: u64) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while store.diagnostics().await.bytes_uploaded <= before {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the stalled upload's first write was acknowledged");
}

/// A body that yields `head`, then waits for `release` before yielding `tail`.
fn held_body(
    head: Bytes,
    tail: Bytes,
    release: tokio::sync::oneshot::Receiver<()>,
) -> StorageByteStream {
    let body = stream::once(async move { Ok(head) }).chain(stream::once(async move {
        let _ = release.await;
        Ok(tail)
    }));
    StorageByteStream::new(Box::pin(body))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the managed Samba container harness"]
async fn samba_conditional_create_is_invisible_until_published() {
    let root = format!("graphql-orm-storage-tests/{}", Uuid::new_v4());
    let store = Arc::new(
        SmbStorageBackend::connect(test_config(root))
            .await
            .expect("connect"),
    );
    let key = "content/identical.bin";
    let content = Bytes::from_static(b"identical content that two writers store at once");
    let before = store.diagnostics().await.bytes_uploaded;
    let (release, released) = tokio::sync::oneshot::channel();
    let first_store = Arc::clone(&store);
    let first_body = held_body(content.slice(..8), content.slice(8..), released);
    let first = tokio::spawn(async move {
        first_store
            .put_blob_if_not_exists(key, first_body, BlobPutOptions::default())
            .await
    });
    wait_for_uploaded_bytes(&store, before).await;

    // Part of the first object is on the server, but the key is not taken.
    assert!(!store.blob_exists(key).await.expect("exists"));
    assert!(store.head_blob(key).await.expect("head").is_none());
    assert!(store.list_blobs("content/").await.expect("list").is_empty());
    // A second writer of the same content creates the key.
    let second = store
        .put_blob_if_not_exists(
            key,
            StorageByteStream::from_bytes(content.clone()),
            BlobPutOptions::default(),
        )
        .await
        .expect("second conditional create")
        .expect("the second writer creates the key");
    assert_eq!(second.size_bytes, content.len() as u64);

    // The first writer completes its object and finds the key taken.
    release.send(()).expect("release the first writer");
    let first = first
        .await
        .expect("first writer task")
        .expect("first conditional create");
    assert!(first.is_none(), "the second writer already holds the key");
    let stored = store.get_blob(key).await.expect("get");
    assert_eq!(
        collect_storage_stream(stored.body).await.expect("collect"),
        content
    );
    assert_eq!(store.list_blobs("content/").await.expect("list"), [key]);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the managed Samba container harness"]
async fn samba_stalled_transfers_release_their_permits() {
    let root = format!("graphql-orm-storage-tests/{}", Uuid::new_v4());
    let mut config = test_config(root);
    config.max_transfer_concurrency = 1;
    let store = Arc::new(SmbStorageBackend::connect(config).await.expect("connect"));
    let large = Bytes::from(
        (0..(2 * 1024 * 1024 + 11))
            .map(|index| u8::try_from(index % 241).expect("bounded byte"))
            .collect::<Vec<_>>(),
    );
    for (key, bytes) in [
        ("stalled/read.bin", large.clone()),
        ("other/read.bin", Bytes::from_static(b"other object")),
    ] {
        store
            .put_blob(
                key,
                StorageByteStream::from_bytes(bytes),
                BlobPutOptions::default(),
            )
            .await
            .expect("seed");
    }

    // A download whose consumer stops after its first chunk.
    let mut stalled_read = store
        .get_blob("stalled/read.bin")
        .await
        .expect("open stalled download")
        .body
        .into_inner();
    let mut downloaded = stalled_read
        .next()
        .await
        .expect("first chunk")
        .expect("first chunk")
        .to_vec();
    assert!(downloaded.len() < large.len());

    // An upload whose body stops after its first chunk.
    let before = store.diagnostics().await.bytes_uploaded;
    let (release, released) = tokio::sync::oneshot::channel();
    let upload_store = Arc::clone(&store);
    let upload = tokio::spawn(async move {
        upload_store
            .put_blob(
                "stalled/write.bin",
                held_body(
                    Bytes::from_static(b"stalled upload, "),
                    Bytes::from_static(b"resumed"),
                    released,
                ),
                BlobPutOptions::default(),
            )
            .await
    });
    wait_for_uploaded_bytes(&store, before).await;

    // With one transfer permit, other transfers still run while both stall.
    tokio::time::timeout(Duration::from_secs(30), async {
        let other = store.get_blob("other/read.bin").await.expect("get other");
        assert_eq!(
            collect_storage_stream(other.body).await.expect("collect"),
            Bytes::from_static(b"other object")
        );
        store
            .put_blob(
                "other/write.bin",
                StorageByteStream::from_bytes(Bytes::from_static(b"written meanwhile")),
                BlobPutOptions::default(),
            )
            .await
            .expect("put other");
    })
    .await
    .expect("a stalled transfer must not hold the transfer permit");

    // The stalled transfers complete once they resume.
    while let Some(chunk) = stalled_read.next().await {
        downloaded.extend_from_slice(&chunk.expect("resumed chunk"));
    }
    assert_eq!(downloaded, large);
    release.send(()).expect("release the stalled upload");
    upload
        .await
        .expect("upload task")
        .expect("resumed upload completes");
    let written = store.get_blob("stalled/write.bin").await.expect("get");
    assert_eq!(
        collect_storage_stream(written.body).await.expect("collect"),
        Bytes::from_static(b"stalled upload, resumed")
    );
}

/// Writers on separate connections, as separate hosts would be, race to
/// publish the same key. Their bodies finish together, so the publishes
/// meet at the server. Exactly one may create the key, and it must hold that
/// writer's complete content.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the managed Samba container harness"]
async fn samba_racing_conditional_creates_have_exactly_one_winner() {
    const WRITERS: usize = 4;
    const ROUNDS: usize = 25;
    let root = format!("graphql-orm-storage-tests/{}", Uuid::new_v4());
    let mut stores = Vec::new();
    for _ in 0..WRITERS {
        stores.push(Arc::new(
            SmbStorageBackend::connect(test_config(root.clone()))
                .await
                .expect("connect writer"),
        ));
    }
    for round in 0..ROUNDS {
        let key = format!("race/lock-{round}.bin");
        let ready = Arc::new(tokio::sync::Barrier::new(WRITERS));
        let mut writers = Vec::new();
        for (writer, store) in stores.iter().enumerate() {
            let store = Arc::clone(store);
            let ready = Arc::clone(&ready);
            let key = key.clone();
            let content = Bytes::from(
                format!("round {round} writer {writer} ")
                    .repeat(2_048)
                    .into_bytes(),
            );
            writers.push(tokio::spawn(async move {
                let head = content.slice(..content.len() / 2);
                let tail = content.slice(content.len() / 2..);
                let body = stream::once(async move { Ok(head) }).chain(stream::once(async move {
                    // Every writer has started before any finishes its body.
                    ready.wait().await;
                    Ok(tail)
                }));
                let created = store
                    .put_blob_if_not_exists(
                        &key,
                        StorageByteStream::new(Box::pin(body)),
                        BlobPutOptions::default(),
                    )
                    .await
                    .expect("conditional create");
                (created.is_some(), content)
            }));
        }
        let mut winners = Vec::new();
        for writer in writers {
            let (created, content) = writer.await.expect("writer task");
            if created {
                winners.push(content);
            }
        }
        assert_eq!(
            winners.len(),
            1,
            "round {round}: exactly one writer creates the key"
        );
        let stored = stores[0].get_blob(&key).await.expect("get");
        assert_eq!(
            collect_storage_stream(stored.body).await.expect("collect"),
            winners[0],
            "round {round}: the key holds the winner's complete content"
        );
    }
    assert_eq!(
        stores[0].list_blobs("race/").await.expect("list").len(),
        ROUNDS
    );
}
