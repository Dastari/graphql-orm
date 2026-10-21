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

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the managed Samba container harness"]
async fn samba_namespace_marker_lifecycle_and_reserved_aliases() {
    use graphql_orm_storage::{
        DomainState, NamespaceFence, NamespaceId, NamespaceInitialisation, NamespaceRelation,
        NamespaceState, NamespaceTarget, StorageError, TopologyScan, relate_namespaces,
    };

    let root = format!("graphql-orm-storage-namespace/{}", Uuid::new_v4());
    let config = test_config(root);
    SmbStorageBackend::probe(
        config.clone(),
        SmbProbeOptions {
            create_prefix: true,
        },
    )
    .await
    .expect("probe creates the owned prefix");
    let store = SmbStorageBackend::connect(config.clone())
        .await
        .expect("connect");
    let target = NamespaceTarget::Root;
    let scan = TopologyScan::default();

    let first = store
        .observe_namespace(&target, &scan)
        .await
        .expect("observe");
    assert_eq!(first.state, NamespaceState::Uninitialised);
    let id = NamespaceId::new_random();
    assert_eq!(
        store.initialise_namespace(&target, id).await.expect("init"),
        NamespaceInitialisation::Created(id)
    );
    assert_eq!(
        store
            .initialise_namespace(&target, id)
            .await
            .expect("retry"),
        NamespaceInitialisation::AlreadyInitialisedSame(id)
    );
    assert_eq!(
        store
            .initialise_namespace(&target, NamespaceId::new_random())
            .await
            .expect("other"),
        NamespaceInitialisation::AlreadyInitialisedOther(id)
    );

    // A new connection (restart) observes the same identity; never "distinct".
    let restarted = SmbStorageBackend::connect(config).await.expect("reconnect");
    let before = store
        .observe_namespace(&target, &scan)
        .await
        .expect("observe");
    let after = restarted
        .observe_namespace(&target, &scan)
        .await
        .expect("observe");
    assert_eq!(after.state, NamespaceState::Initialised { id });
    let relation = relate_namespaces(&before, &after);
    assert!(
        matches!(
            relation,
            NamespaceRelation::SameNamespace | NamespaceRelation::Unknown
        ),
        "{relation:?}"
    );

    // Case-insensitive, trailing-dot/space, stream and short-name aliases never reach the server.
    for key in [
        ".GRAPHQL-ORM-NAMESPACE",
        "a/.Graphql-Orm-Namespace.",
        ".graphql-orm-namespace ",
        ".graphql-orm-namespace:stream",
        "a/file:stream",
        "GRAPHQ~1",
    ] {
        let body = StorageByteStream::from_bytes(Bytes::from_static(b"overwrite"));
        assert!(
            matches!(
                store.put_blob(key, body, BlobPutOptions::default()).await,
                Err(StorageError::InvalidStorageKey { .. })
            ),
            "{key}"
        );
    }
    store
        .put_blob(
            "data/x",
            StorageByteStream::from_bytes(Bytes::from_static(b"x")),
            BlobPutOptions::default(),
        )
        .await
        .expect("ordinary put");
    assert_eq!(
        store.list_blobs("").await.expect("list"),
        vec!["data/x".to_string()]
    );

    let domain = store.observe_capacity_domain().await.expect("domain");
    assert!(matches!(domain.state, DomainState::Partial { .. }));
    assert!(!domain.facts.is_empty());

    let fence = NamespaceFence {
        target: NamespaceTarget::Root,
        namespace: id,
        epoch: before.epoch.clone(),
    };
    let payload = |bytes: &'static [u8]| StorageByteStream::from_bytes(Bytes::from_static(bytes));
    // Overwriting fenced writes are honestly Unsupported on SMB.
    assert!(matches!(
        store
            .put_blob_fenced(
                "data/overwrite",
                payload(b"o"),
                BlobPutOptions::default(),
                &fence
            )
            .await,
        Err(StorageError::UnsupportedOperation { .. })
    ));
    // Session-bound conditional publication: create once, never overwrite.
    assert!(
        store
            .put_blob_if_not_exists_fenced(
                "data/fenced",
                payload(b"f"),
                BlobPutOptions::default(),
                &fence
            )
            .await
            .expect("fenced create")
            .is_some()
    );
    assert!(
        store
            .put_blob_if_not_exists_fenced(
                "data/fenced",
                payload(b"g"),
                BlobPutOptions::default(),
                &fence
            )
            .await
            .expect("fenced existing")
            .is_none()
    );
    // Keys outside the target and other backend instances are refused.
    let sub_fence = NamespaceFence {
        target: NamespaceTarget::SubPrefix("data".to_string()),
        ..fence.clone()
    };
    assert!(
        store
            .put_blob_if_not_exists_fenced(
                "other/x",
                payload(b"x"),
                BlobPutOptions::default(),
                &sub_fence
            )
            .await
            .is_err()
    );
    assert!(
        restarted
            .put_blob_if_not_exists_fenced(
                "data/stale",
                payload(b"s"),
                BlobPutOptions::default(),
                &fence
            )
            .await
            .is_err()
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
    let stale_id = graphql_orm_storage::NamespaceId::new_random();
    store
        .initialise_namespace(&graphql_orm_storage::NamespaceTarget::Root, stale_id)
        .await
        .expect("initialise before restart");
    let observed = store
        .observe_namespace(
            &graphql_orm_storage::NamespaceTarget::Root,
            &graphql_orm_storage::TopologyScan::default(),
        )
        .await
        .expect("observe before restart");
    let stale_fence = graphql_orm_storage::NamespaceFence {
        target: graphql_orm_storage::NamespaceTarget::Root,
        namespace: stale_id,
        epoch: observed.epoch,
    };

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
    // A fence observed before the reconnect must not publish through the new session.
    let fence_after_restart = stale_fence.clone();
    assert!(
        store
            .put_blob_if_not_exists_fenced(
                "reconnect/fenced.bin",
                StorageByteStream::from_bytes(Bytes::from_static(b"stale")),
                BlobPutOptions::default(),
                &fence_after_restart,
            )
            .await
            .is_err()
    );
    assert!(
        !store
            .blob_exists("reconnect/fenced.bin")
            .await
            .expect("inspect")
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

// A restart capability is constructed only after a successful ownership read.
struct VerifiedSambaFixture {
    container: String,
}
impl VerifiedSambaFixture {
    fn restart_with(&self, restart: impl FnOnce(&str) -> Result<(), String>) -> Result<(), String> {
        restart(&self.container)
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

/// Owned loopback SMB2 fault proxy. It forwards whole NetBIOS frames and, once
/// armed, closes both sockets instead of forwarding the selected request, so a
/// fault lands at an exact protocol step of the operation under test.
mod fault_proxy {
    use std::{
        io::{Read, Write},
        net::{Shutdown, TcpListener, TcpStream},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
    };

    pub const CREATE: u16 = 0x0005;
    pub const CLOSE: u16 = 0x0006;
    pub const WRITE: u16 = 0x0009;
    pub const SET_INFO: u16 = 0x0011;

    #[derive(Default)]
    struct Rule {
        active: bool,
        name: Vec<u8>,
        command: u16,
        skip: usize,
        seen: bool,
        count: usize,
    }

    pub struct FaultProxy {
        pub port: u16,
        rule: Arc<Mutex<Rule>>,
        fired: Arc<AtomicBool>,
    }

    impl FaultProxy {
        pub fn start(upstream: (String, u16)) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("owned proxy listener");
            let port = listener.local_addr().expect("proxy address").port();
            let rule = Arc::new(Mutex::new(Rule::default()));
            let fired = Arc::new(AtomicBool::new(false));
            let (accept_rule, accept_fired) = (Arc::clone(&rule), Arc::clone(&fired));
            std::thread::spawn(move || {
                for client in listener.incoming() {
                    let Ok(client) = client else { return };
                    let Ok(server) = TcpStream::connect((upstream.0.as_str(), upstream.1)) else {
                        let _ = client.shutdown(Shutdown::Both);
                        continue;
                    };
                    let _ = client.set_nodelay(true);
                    let _ = server.set_nodelay(true);
                    let (mut down_from, mut down_to) = (
                        server.try_clone().expect("server clone"),
                        client.try_clone().expect("client clone"),
                    );
                    std::thread::spawn(move || {
                        let _ = std::io::copy(&mut down_from, &mut down_to);
                        let _ = down_to.shutdown(Shutdown::Both);
                    });
                    let (rule, fired) = (Arc::clone(&accept_rule), Arc::clone(&accept_fired));
                    std::thread::spawn(move || forward_requests(client, server, &rule, &fired));
                }
            });
            Self { port, rule, fired }
        }

        /// Drops the `skip`-th request with `command` issued after the first
        /// CREATE whose path contains `name` (that CREATE itself counts).
        pub fn arm(&self, name: &str, command: u16, skip: usize) {
            let name = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
            *self.rule.lock().expect("rule") = Rule {
                active: true,
                name,
                command,
                skip,
                ..Rule::default()
            };
            self.fired.store(false, Ordering::SeqCst);
        }

        pub fn fired(&self) -> bool {
            self.fired.load(Ordering::SeqCst)
        }
    }

    fn forward_requests(
        mut client: TcpStream,
        mut server: TcpStream,
        rule: &Mutex<Rule>,
        fired: &AtomicBool,
    ) {
        loop {
            let mut header = [0_u8; 4];
            if client.read_exact(&mut header).is_err() {
                break;
            }
            let length = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
            let mut frame = vec![0_u8; length];
            if client.read_exact(&mut frame).is_err() {
                break;
            }
            if should_drop(&mut rule.lock().expect("rule"), &frame) {
                fired.store(true, Ordering::SeqCst);
                break;
            }
            if server.write_all(&header).is_err() || server.write_all(&frame).is_err() {
                break;
            }
        }
        let _ = client.shutdown(Shutdown::Both);
        let _ = server.shutdown(Shutdown::Both);
    }

    fn should_drop(rule: &mut Rule, frame: &[u8]) -> bool {
        if !rule.active {
            return false;
        }
        let mut offset = 0;
        while let Some(message) = frame.get(offset..) {
            if message.len() < 64 || &message[..4] != b"\xfeSMB" {
                return false;
            }
            let command = u16::from_le_bytes([message[12], message[13]]);
            let next = u32::from_le_bytes([message[20], message[21], message[22], message[23]])
                as usize;
            let body = if next == 0 {
                message
            } else {
                &message[..next.min(message.len())]
            };
            if command == CREATE
                && !rule.name.is_empty()
                && body.windows(rule.name.len()).any(|window| window == rule.name)
            {
                rule.seen = true;
            }
            if rule.seen && command == rule.command {
                if rule.count == rule.skip {
                    rule.active = false;
                    return true;
                }
                rule.count += 1;
            }
            if next == 0 {
                return false;
            }
            offset += next;
        }
        false
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the documented Samba container"]
async fn samba_fenced_create_never_reconnects_or_replays_mid_operation() {
    use fault_proxy::{CLOSE, CREATE, FaultProxy, SET_INFO, WRITE};
    use graphql_orm_storage::{
        NamespaceFence, NamespaceId, NamespaceTarget, StorageError, TopologyScan,
    };

    let root = format!("graphql-orm-storage-fault/{}", Uuid::new_v4());
    let mut direct_config = test_config(root);
    direct_config.operation_timeout = Duration::from_secs(5);
    SmbStorageBackend::probe(
        direct_config.clone(),
        SmbProbeOptions {
            create_prefix: true,
        },
    )
    .await
    .expect("probe");
    let direct = SmbStorageBackend::connect(direct_config.clone())
        .await
        .expect("direct connection");
    let id = NamespaceId::new_random();
    direct
        .initialise_namespace(&NamespaceTarget::Root, id)
        .await
        .expect("initialise");
    let proxy = FaultProxy::start((direct_config.server.clone(), direct_config.port));
    let mut proxied_config = direct_config.clone();
    proxied_config.server = "127.0.0.1".to_string();
    proxied_config.port = proxy.port;

    // (key, CREATE path fragment, dropped command, skipped matches, step)
    let cases = [
        ("fp-a/deep/object.bin", "fp-a", CREATE, 0, "parent directory create"),
        ("fp-b/object.bin", "fp-b", CLOSE, 0, "parent directory close"),
        ("fp-c/fp-c-final.bin", "fp-c-final.bin", CREATE, 0, "final exclusive create"),
        ("fp-d/fp-d-final.bin", "fp-d-final.bin", WRITE, 0, "body write"),
        ("fp-e/fp-e-final.bin", "fp-e-final.bin", SET_INFO, 1, "set-info before publication"),
    ];
    for (key, name, command, skip, step) in cases {
        let store = SmbStorageBackend::connect(proxied_config.clone())
            .await
            .expect("proxied connection");
        let observed = store
            .observe_namespace(&NamespaceTarget::Root, &TopologyScan::default())
            .await
            .expect("observe");
        let fence = NamespaceFence {
            target: NamespaceTarget::Root,
            namespace: id,
            epoch: observed.epoch,
        };
        let reconnects = store.diagnostics().await.reconnects;
        proxy.arm(name, command, skip);
        let result = store
            .put_blob_if_not_exists_fenced(
                key,
                StorageByteStream::from_bytes(Bytes::from_static(b"fenced")),
                BlobPutOptions::default(),
                &fence,
            )
            .await;
        assert!(proxy.fired(), "{step}: the fault point was not reached");
        assert!(result.is_err(), "{step}: {result:?}");
        // Spawned drop cleanup must not reconnect either.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            store.diagnostics().await.reconnects,
            reconnects,
            "{step}: the fenced path reconnected"
        );
        // Nothing was published: the server discards the delete-pending file
        // with the lost session.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while direct.blob_exists(key).await.expect("inspect") {
            assert!(std::time::Instant::now() < deadline, "{step}: object published");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // Only an ordinary call repairs the session; the old fence then fails
        // closed and still writes nothing.
        let _ = store.blob_exists("unrelated.bin").await;
        assert!(
            matches!(
                store
                    .put_blob_if_not_exists_fenced(
                        key,
                        StorageByteStream::from_bytes(Bytes::from_static(b"stale")),
                        BlobPutOptions::default(),
                        &fence,
                    )
                    .await,
                Err(StorageError::PreconditionFailed { .. })
            ),
            "{step}: a stale fence was accepted after reconnect"
        );
        assert!(!direct.blob_exists(key).await.expect("inspect"), "{step}");
    }
}
