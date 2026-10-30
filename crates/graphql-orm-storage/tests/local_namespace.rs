use std::{collections::BTreeSet, path::Path, time::Duration};

use bytes::Bytes;
use graphql_orm_storage::{
    AncestorScan, BlobPutOptions, BlobStore, DomainState, LocalStorageBackend, NAMESPACE_MARKER,
    NAMESPACE_MARKER_PUBLISHING, NamespaceFence, NamespaceId, NamespaceInitialisation,
    NamespaceRelation, NamespaceState, NamespaceTarget, NestedScan, ObjectMetadata,
    ScanCompleteness, StorageByteStream, StorageError, StreamingObjectStore, TopologyScan,
    encode_marker, relate_namespaces,
};
use tempfile::TempDir;

fn tree(root: &Path) -> BTreeSet<String> {
    let mut entries = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(&path).expect("read dir") {
            let entry = entry.expect("entry");
            let bytes = std::fs::read(entry.path()).unwrap_or_default();
            entries.insert(format!("{}:{}", entry.path().display(), bytes.len()));
            if entry.file_type().expect("type").is_dir() {
                stack.push(entry.path());
            }
        }
    }
    entries
}

fn nested(max_keys: usize) -> TopologyScan {
    TopologyScan {
        ancestors: None,
        nested: Some(NestedScan {
            max_depth: 8,
            max_pages: 64,
            max_keys,
            max_bytes: 1 << 20,
            max_results: 1000,
            max_duration: Duration::from_secs(10),
        }),
    }
}

#[tokio::test]
async fn local_namespace_lifecycle_reconciles_same_identity_and_never_overwrites() {
    let temp = TempDir::new().expect("temp dir");
    let store = LocalStorageBackend::new(temp.path());
    let root = NamespaceTarget::Root;
    let scan = TopologyScan::default();

    let first = store
        .observe_namespace(&root, &scan)
        .await
        .expect("observe");
    assert_eq!(first.state, NamespaceState::Uninitialised);
    assert_eq!(first.ancestors.completeness, ScanCompleteness::NotRequested);
    assert_eq!(first.nested.completeness, ScanCompleteness::NotRequested);

    let id = NamespaceId::new_random();
    assert_eq!(
        store.initialise_namespace(&root, id).await.expect("init"),
        NamespaceInitialisation::Created(id)
    );
    assert_eq!(
        store.initialise_namespace(&root, id).await.expect("retry"),
        NamespaceInitialisation::AlreadyInitialisedSame(id)
    );
    let other = NamespaceId::new_random();
    assert_eq!(
        store
            .initialise_namespace(&root, other)
            .await
            .expect("other"),
        NamespaceInitialisation::AlreadyInitialisedOther(id)
    );
    assert!(!temp.path().join(NAMESPACE_MARKER_PUBLISHING).exists());

    // A restart (new backend instance, same root) observes the same identity.
    let restarted = LocalStorageBackend::new(temp.path());
    let before = store
        .observe_namespace(&root, &scan)
        .await
        .expect("observe");
    let after = restarted
        .observe_namespace(&root, &scan)
        .await
        .expect("observe");
    assert_eq!(after.state, NamespaceState::Initialised { id });
    assert_ne!(before.epoch.backend_instance, after.epoch.backend_instance);
    assert_eq!(
        relate_namespaces(&before, &after),
        NamespaceRelation::SameNamespace
    );
}

#[tokio::test]
async fn local_observation_never_writes() {
    let temp = TempDir::new().expect("temp dir");
    std::fs::create_dir_all(temp.path().join("a/b")).expect("dirs");
    let store = LocalStorageBackend::new(temp.path().join("a"));
    let before = tree(temp.path());
    let scan = TopologyScan {
        ancestors: Some(AncestorScan {
            max_depth: 1,
            max_duration: Duration::from_secs(10),
        }),
        ..nested(1000)
    };
    store
        .observe_namespace(&NamespaceTarget::Root, &scan)
        .await
        .expect("observe");
    store.observe_capacity_domain().await.expect("domain");
    assert_eq!(tree(temp.path()), before);
}

#[tokio::test]
async fn reserved_marker_names_are_hidden_and_protected_on_every_mutation_path() {
    let temp = TempDir::new().expect("temp dir");
    let store = LocalStorageBackend::new(temp.path());
    let id = NamespaceId::new_random();
    store
        .initialise_namespace(&NamespaceTarget::Root, id)
        .await
        .expect("init");
    store
        .put_blob(
            "data/file.bin",
            StorageByteStream::from_bytes(Bytes::from_static(b"x")),
            BlobPutOptions::default(),
        )
        .await
        .expect("ordinary put");
    assert_eq!(
        store.list_blobs("").await.expect("list"),
        vec!["data/file.bin".to_string()]
    );

    for key in [
        NAMESPACE_MARKER,
        NAMESPACE_MARKER_PUBLISHING,
        "data/.graphql-orm-namespace",
        ".GRAPHQL-ORM-NAMESPACE",
        "data/.Graphql-Orm-Namespace.",
    ] {
        let body = || StorageByteStream::from_bytes(Bytes::from_static(b"overwrite"));
        assert!(matches!(
            store.put_blob(key, body(), BlobPutOptions::default()).await,
            Err(StorageError::InvalidStorageKey { .. })
        ));
        assert!(
            store
                .put_blob_if_not_exists(key, body(), BlobPutOptions::default())
                .await
                .is_err()
        );
        assert!(store.delete_blob(key).await.is_err());
        assert!(store.delete_blob_once(key).await.is_err());
        assert!(store.get_blob(key).await.is_err());
        assert!(store.copy_blob("data/file.bin", key).await.is_err());
        assert!(store.copy_blob(key, "data/copy.bin").await.is_err());
        assert!(
            store
                .put_object_stream("bucket", key, None, ObjectMetadata::default(), body())
                .await
                .is_err()
        );
        assert!(
            store
                .create_multipart_object("bucket", key, None, ObjectMetadata::default())
                .await
                .is_err()
        );
        assert!(store.delete_object("bucket", key).await.is_err());
    }
    assert!(
        store
            .put_object_stream(
                NAMESPACE_MARKER,
                "key",
                None,
                ObjectMetadata::default(),
                StorageByteStream::from_bytes(Bytes::from_static(b"x"))
            )
            .await
            .is_err()
    );
    let state = store
        .observe_namespace(&NamespaceTarget::Root, &TopologyScan::default())
        .await
        .expect("observe");
    assert_eq!(state.state, NamespaceState::Initialised { id });
}

#[tokio::test]
async fn foreign_partial_and_oversized_markers_are_conflicts_not_absence() {
    let id = NamespaceId::new_random();
    let canonical = String::from_utf8(encode_marker(id)).expect("utf8");
    for body in [
        String::new(),
        "{\"format\":\"other\"}".to_string(),
        canonical[..canonical.len() / 2].to_string(),
        canonical.replace("\"}", ",\"namespace_id\":\"x\"}"),
        format!("{canonical}trailing"),
        "x".repeat(4096),
    ] {
        let temp = TempDir::new().expect("temp dir");
        std::fs::write(temp.path().join(NAMESPACE_MARKER), &body).expect("write");
        let store = LocalStorageBackend::new(temp.path());
        let observed = store
            .observe_namespace(&NamespaceTarget::Root, &TopologyScan::default())
            .await
            .expect("observe");
        assert!(
            matches!(observed.state, NamespaceState::Conflict { .. }),
            "{body:?} -> {:?}",
            observed.state
        );
        assert!(matches!(
            store
                .initialise_namespace(&NamespaceTarget::Root, id)
                .await
                .expect("init"),
            NamespaceInitialisation::Conflict { .. }
        ));
        assert_eq!(
            std::fs::read_to_string(temp.path().join(NAMESPACE_MARKER)).expect("read"),
            body
        );
    }
}

#[tokio::test]
async fn interrupted_publication_reconciles_only_the_same_identity() {
    let temp = TempDir::new().expect("temp dir");
    let id = NamespaceId::new_random();
    std::fs::write(
        temp.path().join(NAMESPACE_MARKER_PUBLISHING),
        encode_marker(id),
    )
    .expect("write");
    let store = LocalStorageBackend::new(temp.path());
    let root = NamespaceTarget::Root;
    let observed = store
        .observe_namespace(&root, &TopologyScan::default())
        .await
        .expect("observe");
    assert_eq!(observed.state, NamespaceState::PublicationInProgress);
    assert!(matches!(
        store
            .initialise_namespace(&root, NamespaceId::new_random())
            .await
            .expect("other"),
        NamespaceInitialisation::Conflict { .. }
    ));
    assert_eq!(
        store.initialise_namespace(&root, id).await.expect("same"),
        NamespaceInitialisation::Created(id)
    );
    assert!(!temp.path().join(NAMESPACE_MARKER_PUBLISHING).exists());
}

#[tokio::test]
async fn bounded_topology_scans_report_completeness() {
    let temp = TempDir::new().expect("temp dir");
    std::fs::create_dir_all(temp.path().join("child/sub/deeper")).expect("dirs");
    let parent = LocalStorageBackend::new(temp.path());
    let parent_id = NamespaceId::new_random();
    parent
        .initialise_namespace(&NamespaceTarget::Root, parent_id)
        .await
        .expect("parent");
    let sub_id = NamespaceId::new_random();
    parent
        .initialise_namespace(&NamespaceTarget::SubPrefix("child/sub".to_string()), sub_id)
        .await
        .expect("sub");

    let complete = parent
        .observe_namespace(&NamespaceTarget::Root, &nested(1000))
        .await
        .expect("observe");
    assert_eq!(complete.nested.completeness, ScanCompleteness::Complete);
    assert_eq!(complete.nested.found.len(), 1);
    assert_eq!(complete.nested.found[0].relative_path, "child/sub");
    assert_eq!(
        complete.nested.found[0].state,
        NamespaceState::Initialised { id: sub_id }
    );

    let truncated = parent
        .observe_namespace(&NamespaceTarget::Root, &nested(1))
        .await
        .expect("observe");
    assert!(matches!(
        truncated.nested.completeness,
        ScanCompleteness::Truncated { .. }
    ));

    let child = LocalStorageBackend::new(temp.path().join("child"));
    let scan = TopologyScan {
        ancestors: Some(AncestorScan {
            max_depth: 1,
            max_duration: Duration::from_secs(10),
        }),
        nested: None,
    };
    let observed = child
        .observe_namespace(&NamespaceTarget::Root, &scan)
        .await
        .expect("observe");
    assert_eq!(observed.state, NamespaceState::Uninitialised);
    assert_eq!(observed.ancestors.found.len(), 1);
    assert_eq!(observed.ancestors.found[0].relative_path, "..");
    assert_eq!(
        observed.ancestors.found[0].state,
        NamespaceState::Initialised { id: parent_id }
    );
}

#[tokio::test]
async fn copied_marker_is_duplicated_not_an_alias() {
    let temp = TempDir::new().expect("temp dir");
    std::fs::create_dir_all(temp.path().join("one")).expect("dir");
    std::fs::create_dir_all(temp.path().join("two")).expect("dir");
    let one = LocalStorageBackend::new(temp.path().join("one"));
    let id = NamespaceId::new_random();
    one.initialise_namespace(&NamespaceTarget::Root, id)
        .await
        .expect("init");
    std::fs::copy(
        temp.path().join("one").join(NAMESPACE_MARKER),
        temp.path().join("two").join(NAMESPACE_MARKER),
    )
    .expect("copy");
    let two = LocalStorageBackend::new(temp.path().join("two"));
    let a = one
        .observe_namespace(&NamespaceTarget::Root, &TopologyScan::default())
        .await
        .expect("observe");
    let b = two
        .observe_namespace(&NamespaceTarget::Root, &TopologyScan::default())
        .await
        .expect("observe");
    assert_eq!(b.state, NamespaceState::Initialised { id });
    if cfg!(unix) {
        assert_eq!(relate_namespaces(&a, &b), NamespaceRelation::Duplicated);
    }
}

#[tokio::test]
async fn fenced_writes_fail_after_marker_or_instance_changes() {
    let temp = TempDir::new().expect("temp dir");
    let store = LocalStorageBackend::new(temp.path());
    let id = NamespaceId::new_random();
    store
        .initialise_namespace(&NamespaceTarget::Root, id)
        .await
        .expect("init");
    let observed = store
        .observe_namespace(&NamespaceTarget::Root, &TopologyScan::default())
        .await
        .expect("observe");
    let fence = NamespaceFence {
        target: NamespaceTarget::Root,
        namespace: id,
        epoch: observed.epoch.clone(),
    };
    let body = || StorageByteStream::from_bytes(Bytes::from_static(b"fenced"));
    store
        .put_blob_fenced("data/a", body(), BlobPutOptions::default(), &fence)
        .await
        .expect("fenced put");

    let other_instance = LocalStorageBackend::new(temp.path());
    assert!(matches!(
        other_instance
            .put_blob_fenced("data/b", body(), BlobPutOptions::default(), &fence)
            .await,
        Err(StorageError::PreconditionFailed { .. })
    ));

    std::fs::remove_file(temp.path().join(NAMESPACE_MARKER)).expect("remove");
    std::fs::write(
        temp.path().join(NAMESPACE_MARKER),
        encode_marker(NamespaceId::new_random()),
    )
    .expect("replace");
    assert!(matches!(
        store
            .put_blob_fenced("data/c", body(), BlobPutOptions::default(), &fence)
            .await,
        Err(StorageError::PreconditionFailed { .. })
    ));
    assert!(!temp.path().join("data/b").exists());
    assert!(!temp.path().join("data/c").exists());
}

#[tokio::test]
async fn capacity_domain_facts_are_partial_and_scoped() {
    let temp = TempDir::new().expect("temp dir");
    let store = LocalStorageBackend::new(temp.path());
    let observed = store.observe_capacity_domain().await.expect("domain");
    if cfg!(unix) {
        assert!(matches!(observed.state, DomainState::Partial { .. }));
        assert_eq!(observed.facts.len(), 2);
    } else {
        assert_eq!(observed.state, DomainState::Unsupported);
    }
}

fn scan_with(nested: NestedScan) -> TopologyScan {
    TopologyScan {
        ancestors: None,
        nested: Some(nested),
    }
}

fn wide_scan() -> NestedScan {
    NestedScan {
        max_depth: 8,
        max_pages: 1000,
        max_keys: 100_000,
        max_bytes: 1 << 20,
        max_results: 1000,
        max_duration: Duration::from_secs(10),
    }
}

#[tokio::test]
async fn fenced_keys_must_stay_inside_the_target_and_its_witnessed_root() {
    let temp = TempDir::new().expect("temp dir");
    std::fs::create_dir_all(temp.path().join("a")).expect("dir");
    let store = LocalStorageBackend::new(temp.path());
    let target = NamespaceTarget::SubPrefix("a".to_string());
    let id = NamespaceId::new_random();
    store.initialise_namespace(&target, id).await.expect("init");
    let observed = store
        .observe_namespace(&target, &TopologyScan::default())
        .await
        .expect("observe");
    let fence = NamespaceFence {
        target: target.clone(),
        namespace: id,
        epoch: observed.epoch.clone(),
    };
    let body = || StorageByteStream::from_bytes(Bytes::from_static(b"x"));
    for outside in ["b/object", "a-sibling/object", "a", "object"] {
        assert!(
            matches!(
                store
                    .put_blob_fenced(outside, body(), BlobPutOptions::default(), &fence)
                    .await,
                Err(StorageError::PreconditionFailed { .. })
            ),
            "{outside}"
        );
        assert!(
            store
                .put_blob_if_not_exists_fenced(outside, body(), BlobPutOptions::default(), &fence)
                .await
                .is_err()
        );
    }
    store
        .put_blob_fenced("a/object", body(), BlobPutOptions::default(), &fence)
        .await
        .expect("inside");

    // Replacing the target directory with one carrying a copied marker
    // changes the target-root witness, so the fence is invalidated.
    std::fs::rename(temp.path().join("a"), temp.path().join("a-old")).expect("rename");
    std::fs::create_dir_all(temp.path().join("a")).expect("dir");
    std::fs::copy(
        temp.path().join("a-old").join(NAMESPACE_MARKER),
        temp.path().join("a").join(NAMESPACE_MARKER),
    )
    .expect("copy");
    assert!(matches!(
        store
            .put_blob_fenced("a/after", body(), BlobPutOptions::default(), &fence)
            .await,
        Err(StorageError::PreconditionFailed { .. })
    ));
    assert!(!temp.path().join("a/after").exists());
}

#[tokio::test]
async fn conditional_fenced_publication_never_overwrites() {
    let temp = TempDir::new().expect("temp dir");
    let store = LocalStorageBackend::new(temp.path());
    let id = NamespaceId::new_random();
    store
        .initialise_namespace(&NamespaceTarget::Root, id)
        .await
        .expect("init");
    let observed = store
        .observe_namespace(&NamespaceTarget::Root, &TopologyScan::default())
        .await
        .expect("observe");
    let fence = NamespaceFence {
        target: NamespaceTarget::Root,
        namespace: id,
        epoch: observed.epoch.clone(),
    };
    let first = store
        .put_blob_if_not_exists_fenced(
            "data/immutable",
            StorageByteStream::from_bytes(Bytes::from_static(b"one")),
            BlobPutOptions::default(),
            &fence,
        )
        .await
        .expect("first");
    assert!(first.is_some());
    let second = store
        .put_blob_if_not_exists_fenced(
            "data/immutable",
            StorageByteStream::from_bytes(Bytes::from_static(b"two")),
            BlobPutOptions::default(),
            &fence,
        )
        .await
        .expect("second");
    assert!(second.is_none());
    assert_eq!(
        std::fs::read(temp.path().join("data/immutable")).expect("read"),
        b"one"
    );
}

#[tokio::test]
async fn scans_enforce_deadline_byte_result_and_limit_bounds() {
    let temp = TempDir::new().expect("temp dir");
    for index in 0..200 {
        std::fs::create_dir_all(temp.path().join(format!("dir-{index:03}"))).expect("dir");
    }
    let store = LocalStorageBackend::new(temp.path());
    for index in 0..3 {
        store
            .initialise_namespace(
                &NamespaceTarget::SubPrefix(format!("dir-{index:03}")),
                NamespaceId::new_random(),
            )
            .await
            .expect("sub");
    }
    let root = NamespaceTarget::Root;

    // A single wide directory still stops at the original deadline.
    let expired = NestedScan {
        max_duration: Duration::ZERO,
        ..wide_scan()
    };
    let observed = store
        .observe_namespace(&root, &scan_with(expired))
        .await
        .expect("observe");
    assert!(matches!(
        observed.nested.completeness,
        ScanCompleteness::Truncated { .. }
    ));

    let bytes = NestedScan {
        max_bytes: 64,
        ..wide_scan()
    };
    let observed = store
        .observe_namespace(&root, &scan_with(bytes))
        .await
        .expect("observe");
    assert!(
        matches!(&observed.nested.completeness, ScanCompleteness::Truncated { limit } if limit == "max_bytes")
    );

    let results = NestedScan {
        max_results: 1,
        ..wide_scan()
    };
    let observed = store
        .observe_namespace(&root, &scan_with(results))
        .await
        .expect("observe");
    assert!(
        matches!(&observed.nested.completeness, ScanCompleteness::Truncated { limit } if limit == "max_results")
    );
    assert_eq!(observed.nested.found.len(), 1);

    let complete = store
        .observe_namespace(&root, &scan_with(wide_scan()))
        .await
        .expect("observe");
    assert_eq!(complete.nested.completeness, ScanCompleteness::Complete);
    assert_eq!(complete.nested.found.len(), 3);

    // Overflowing or unreasonable limits are rejected, never clamped.
    let overflowing = TopologyScan {
        ancestors: Some(AncestorScan {
            max_depth: usize::MAX,
            max_duration: Duration::from_secs(1),
        }),
        nested: None,
    };
    assert!(store.observe_namespace(&root, &overflowing).await.is_err());
    let too_long = scan_with(NestedScan {
        max_duration: Duration::from_secs(86_400),
        ..wide_scan()
    });
    assert!(store.observe_namespace(&root, &too_long).await.is_err());
}

#[tokio::test]
async fn temp_sweep_never_enters_reserved_names() {
    let temp = TempDir::new().expect("temp dir");
    let reserved = temp.path().join(NAMESPACE_MARKER);
    std::fs::create_dir_all(&reserved).expect("reserved dir");
    std::fs::write(reserved.join("child.uploading"), b"x").expect("write");
    std::fs::create_dir_all(temp.path().join("data")).expect("dir");
    std::fs::write(temp.path().join("data/old.uploading"), b"x").expect("write");
    let store = LocalStorageBackend::new(temp.path());
    let removed = store.sweep_temp_files(Duration::ZERO).await.expect("sweep");
    assert_eq!(removed, 1);
    assert!(reserved.join("child.uploading").exists());
    assert!(!temp.path().join("data/old.uploading").exists());
}

#[tokio::test]
async fn nested_byte_budget_is_charged_before_the_last_marker_read() {
    // One child, last entry: path "c" (1 byte) plus the bounded marker read,
    // which is the 1 KiB body plus the one-byte overflow probe.
    let charge = 1 + graphql_orm_storage::MAX_NAMESPACE_MARKER_BYTES + 1;
    let oversized = vec![b'x'; 2 * graphql_orm_storage::MAX_NAMESPACE_MARKER_BYTES];
    for marker in [None, Some(oversized)] {
        let temp = TempDir::new().expect("temp dir");
        std::fs::create_dir_all(temp.path().join("c")).expect("only child");
        let store = LocalStorageBackend::new(temp.path());
        match &marker {
            None => {
                store
                    .initialise_namespace(
                        &NamespaceTarget::SubPrefix("c".to_string()),
                        NamespaceId::new_random(),
                    )
                    .await
                    .expect("child marker");
            }
            Some(bytes) => {
                std::fs::write(temp.path().join("c").join(NAMESPACE_MARKER), bytes)
                    .expect("oversized marker");
            }
        }
        let root = NamespaceTarget::Root;
        let short = NestedScan {
            max_bytes: charge - 1,
            ..wide_scan()
        };
        let observed = store
            .observe_namespace(&root, &scan_with(short))
            .await
            .expect("observe");
        assert!(
            matches!(&observed.nested.completeness, ScanCompleteness::Truncated { limit } if limit == "max_bytes"),
            "oversized={}",
            marker.is_some()
        );
        assert!(observed.nested.found.is_empty());
        let exact = NestedScan {
            max_bytes: charge,
            ..wide_scan()
        };
        let observed = store
            .observe_namespace(&root, &scan_with(exact))
            .await
            .expect("observe");
        assert_eq!(observed.nested.completeness, ScanCompleteness::Complete);
        assert_eq!(observed.nested.found.len(), 1);
        // The overflow probe is charged even though parsing reports Conflict.
        assert_eq!(
            matches!(
                observed.nested.found[0].state,
                NamespaceState::Conflict { .. }
            ),
            marker.is_some()
        );
    }
}
