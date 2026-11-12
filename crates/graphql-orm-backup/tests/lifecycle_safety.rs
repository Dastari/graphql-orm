//! Snapshot lifecycle, locking and capacity regressions (#476, #477, #480, #481).

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use graphql_orm_backup::{
    BackupByteLimit, BackupCancellation, BackupCapacityLimitKind, BackupCapacityLimits,
    BackupChangeAction, BackupChangeExport, BackupError, BackupExecutionOptions, BackupObjectIndex,
    BackupObjectRef, BackupRepository, BackupRow, BackupTableExport, BreakRepositoryLockRequest,
    CompactChainRequest, FullBackupRequest, GraphqlOrmBackupAdapter, GraphqlOrmBackupSchema,
    IncrementalBackupRequest, KeepPolicy, LocalBackupRepository, ManifestAuthenticator,
    ManifestKey, PruneResult, REPOSITORY_LOCK_KEY, RepositoryLock, RepositoryLockMode,
    RepositoryLockOptions, RepositoryLockStatus, RestoreContext, RestoreObjectSink, RestoreOptions,
    VerificationOptions, break_repository_lock, bytes_sha256_hex, compact_chain,
    create_full_backup, create_full_backup_with_options, create_incremental_backup, load_manifest,
    object_content_key, prune, read_repository_lock, restore_objects, restore_objects_with_options,
    restore_snapshot, restore_snapshot_with_options, snapshot_manifest_key,
    verify_manifest_and_objects, verify_manifest_and_objects_with_options, verify_object_checksums,
    write_manifest,
};
use graphql_orm_storage::StorageByteStream;
use serde_json::{Map, Value};
use tempfile::TempDir;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// #481 snapshot lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn prune_refuses_keep_last_zero_without_touching_the_repository() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    full(&repository, id(1), 1, &Database::with_rows("users", 1)).await;
    full(&repository, id(2), 2, &Database::with_rows("users", 1)).await;
    let before = all_blobs(&repository).await;

    let error = prune(
        &repository,
        &KeepPolicy {
            keep_last: 0,
            ..KeepPolicy::default()
        },
    )
    .await
    .expect_err("keep_last 0 would delete every snapshot");

    assert!(
        matches!(error, BackupError::InvalidRetentionPolicy { .. }),
        "unexpected error: {error:?}"
    );
    assert_eq!(all_blobs(&repository).await, before, "nothing may change");
}

#[tokio::test]
async fn full_backup_refuses_an_existing_snapshot_id_before_writing() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    full(&repository, id(1), 1, &Database::with_rows("users", 1)).await;
    let before = all_blobs(&repository).await;

    let error = create_full_backup(
        &repository,
        &Database::with_rows("users", 3),
        &Objects::default(),
        full_request(id(1), 2),
    )
    .await
    .expect_err("snapshot ids are never reused");

    assert!(
        matches!(error, BackupError::SnapshotExists { snapshot_id } if snapshot_id == id(1)),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        all_blobs(&repository).await,
        before,
        "no payload overwritten"
    );
    assert_unlocked(&repository).await;
}

#[tokio::test]
async fn full_backup_refuses_an_id_whose_payloads_exist_without_a_manifest() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let leftover = format!("snapshots/{}/database/tables/users.jsonl.zst", id(1));
    repository
        .put_blob(&leftover, Bytes::from_static(b"leftover"))
        .await
        .expect("leftover payload");

    let error = create_full_backup(
        &repository,
        &Database::with_rows("users", 1),
        &Objects::default(),
        full_request(id(1), 1),
    )
    .await
    .expect_err("an id with stored payloads is in use");

    assert!(matches!(error, BackupError::SnapshotExists { .. }));
    assert_eq!(
        repository.get_blob(&leftover).await.expect("leftover"),
        "leftover"
    );
}

#[tokio::test]
async fn incremental_backup_refuses_an_existing_snapshot_id_before_writing() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    full(&repository, id(1), 1, &Database::with_rows("users", 1)).await;
    let database = Database::with_rows("users", 1).with_changes(changes("users", 2));
    incremental(&repository, id(2), id(1), 2, &database).await;
    let before = all_blobs(&repository).await;

    for existing in [id(1), id(2)] {
        let parent = if existing == id(1) { id(2) } else { id(1) };
        let error = create_incremental_backup(
            &repository,
            &Database::with_rows("users", 1).with_changes(changes("users", 5)),
            &Objects::default(),
            incremental_request(existing, parent, 3),
        )
        .await
        .expect_err("snapshot ids are never reused");
        assert!(
            matches!(error, BackupError::SnapshotExists { snapshot_id } if snapshot_id == existing),
            "unexpected error: {error:?}"
        );
    }
    assert_eq!(
        all_blobs(&repository).await,
        before,
        "no payload overwritten"
    );
    assert_unlocked(&repository).await;
}

#[tokio::test]
async fn incremental_backup_validates_its_parent_before_writing() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    full(&repository, id(1), 1, &Database::with_rows("users", 1)).await;
    let database = Database::with_rows("users", 1).with_changes(changes("users", 1));

    let missing = create_incremental_backup(
        &repository,
        &database,
        &Objects::default(),
        incremental_request(id(2), id(9), 2),
    )
    .await
    .expect_err("missing parent");
    assert!(
        matches!(missing, BackupError::MissingBlob { .. }),
        "unexpected error: {missing:?}"
    );
    assert!(snapshot_blobs(&repository, id(2)).await.is_empty());

    let incompatible = create_incremental_backup(
        &repository,
        &database.clone().with_schema_hash("other-schema"),
        &Objects::default(),
        incremental_request(id(2), id(1), 2),
    )
    .await
    .expect_err("schema differs from the parent chain");
    assert!(
        matches!(incompatible, BackupError::InvalidManifestChain { .. }),
        "unexpected error: {incompatible:?}"
    );
    assert!(snapshot_blobs(&repository, id(2)).await.is_empty());
    assert_unlocked(&repository).await;
}

#[tokio::test]
async fn failed_backup_removes_the_payloads_it_wrote_and_releases_the_lock() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let objects = Objects::with_object(b"object bytes").failing_loads();

    let error = create_full_backup(
        &repository,
        &Database::with_rows("users", 2),
        &objects,
        full_request(id(1), 1),
    )
    .await
    .expect_err("object load fails after the table payload was written");

    assert!(matches!(error, BackupError::Database { .. }), "{error:?}");
    assert!(
        snapshot_blobs(&repository, id(1)).await.is_empty(),
        "no orphaned table payload remains"
    );
    assert_unlocked(&repository).await;
}

#[tokio::test]
async fn prune_removes_payloads_that_no_manifest_owns() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    full(&repository, id(1), 1, &Database::with_rows("users", 1)).await;
    let orphan = format!("snapshots/{}/database/tables/users.jsonl.zst", id(7));
    repository
        .put_blob(&orphan, Bytes::from_static(b"orphan"))
        .await
        .expect("orphan payload");
    let unrelated = "snapshots/not-a-snapshot/notes.txt";
    repository
        .put_blob(unrelated, Bytes::from_static(b"unrelated"))
        .await
        .expect("unrelated blob");

    let result: PruneResult = prune(&repository, &KeepPolicy::default())
        .await
        .expect("prune");

    assert_eq!(result.deleted_orphan_blobs, 1);
    assert!(!repository.blob_exists(&orphan).await.expect("exists"));
    assert!(
        repository.blob_exists(unrelated).await.expect("exists"),
        "keys outside snapshot directories are left alone"
    );
    assert!(
        repository
            .blob_exists(&snapshot_manifest_key(id(1)))
            .await
            .expect("exists")
    );
    restore_snapshot(
        &repository,
        &Database::with_rows("users", 1),
        id(1),
        RestoreContext::dry_run(),
    )
    .await
    .expect("retained snapshot still restores");
}

#[tokio::test]
async fn prune_keeps_payloads_of_a_stored_manifest_that_was_not_listed() {
    let temp = TempDir::new().expect("temp dir");
    let local = repository(&temp);
    full(&local, id(1), 1, &Database::with_rows("users", 1)).await;
    full(&local, id(2), 2, &Database::with_rows("users", 1)).await;
    let before = all_blobs(&local).await;
    let repository = Hooked::new(local);
    repository.hide_from_listings(snapshot_manifest_key(id(1)));

    let error = prune(&repository, &KeepPolicy::default())
        .await
        .expect_err("a stored but unlisted manifest is not an orphan");

    assert!(
        matches!(&error, BackupError::InvalidManifestChain { reason } if reason.contains("listing is incomplete")),
        "unexpected error: {error:?}"
    );
    assert_eq!(all_blobs(&repository.inner).await, before);
}

#[tokio::test]
async fn compaction_refuses_an_existing_snapshot_id() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    full(&repository, id(1), 1, &Database::with_rows("users", 1)).await;
    full(&repository, id(2), 2, &Database::with_rows("users", 1)).await;
    let before = all_blobs(&repository).await;

    let error = compact_chain(&repository, compact_request(id(2), id(1), 3))
        .await
        .expect_err("compaction never overwrites");

    assert!(
        matches!(error, BackupError::SnapshotExists { snapshot_id } if snapshot_id == id(2)),
        "unexpected error: {error:?}"
    );
    assert_eq!(all_blobs(&repository).await, before);
}

// ---------------------------------------------------------------------------
// #480 restore and verification exclude concurrent prune
// ---------------------------------------------------------------------------

#[tokio::test]
async fn prune_cannot_delete_blobs_that_a_restore_is_reading() {
    let temp = TempDir::new().expect("temp dir");
    let local = repository(&temp);
    full(&local, id(1), 1, &Database::with_rows("users", 2)).await;
    full(&local, id(2), 2, &Database::with_rows("users", 2)).await;
    let repository = Hooked::new(local);
    repository.prune_on_read(format!("snapshots/{}/database/", id(1)));

    restore_snapshot(
        &repository,
        &Database::with_rows("users", 2),
        id(1),
        RestoreContext::dry_run(),
    )
    .await
    .expect("restore reads a consistent snapshot");

    assert_prune_was_locked_out(&repository);
    assert!(
        repository
            .inner
            .blob_exists(&snapshot_manifest_key(id(1)))
            .await
            .expect("exists")
    );
}

#[tokio::test]
async fn prune_cannot_delete_blobs_that_verification_is_reading() {
    let temp = TempDir::new().expect("temp dir");
    let local = repository(&temp);
    let objects = Objects::with_object(b"old object");
    create_full_backup(
        &local,
        &Database::with_rows("users", 1),
        &objects,
        full_request(id(1), 1),
    )
    .await
    .expect("old snapshot");
    full(&local, id(2), 2, &Database::with_rows("users", 1)).await;
    let manifest = load_manifest(&local, id(1)).await.expect("manifest");
    let object_key = manifest.objects[0].content_key.clone();
    let repository = Hooked::new(local);

    repository.prune_on_read(object_key.clone());
    verify_manifest_and_objects(&repository, &manifest)
        .await
        .expect("verification reads a consistent snapshot");
    assert_prune_was_locked_out(&repository);

    repository.prune_on_read(object_key.clone());
    verify_object_checksums(&repository, &manifest)
        .await
        .expect("checksum verification reads a consistent snapshot");
    assert_prune_was_locked_out(&repository);

    repository.prune_on_read(object_key.clone());
    restore_objects(&repository, &manifest, &CollectingSink::default())
        .await
        .expect("object restore reads a consistent snapshot");
    assert_prune_was_locked_out(&repository);
    assert!(
        repository
            .inner
            .blob_exists(&object_key)
            .await
            .expect("exists")
    );
}

#[tokio::test]
async fn restore_refuses_to_start_while_a_writer_holds_the_lock() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    full(&repository, id(1), 1, &Database::with_rows("users", 1)).await;
    let manifest = load_manifest(&repository, id(1)).await.expect("manifest");
    let writer = RepositoryLock::acquire(&repository, &RepositoryLockOptions::default())
        .await
        .expect("writer lock");

    let error = restore_snapshot(
        &repository,
        &Database::with_rows("users", 1),
        id(1),
        RestoreContext::dry_run(),
    )
    .await
    .expect_err("restore waits for the writer");
    let BackupError::RestoreStageFailed { source, .. } = error else {
        panic!("restore reports a validation-stage failure: {error:?}");
    };
    assert!(matches!(*source, BackupError::RepositoryLocked { .. }));
    assert!(matches!(
        verify_manifest_and_objects(&repository, &manifest).await,
        Err(BackupError::RepositoryLocked { .. })
    ));
    assert!(matches!(
        restore_objects(&repository, &manifest, &CollectingSink::default()).await,
        Err(BackupError::RepositoryLocked { .. })
    ));
    writer.release().await.expect("release");
}

// ---------------------------------------------------------------------------
// #477 lock owner record, break-lock and cancellation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lock_records_its_owner_and_reports_it_to_contenders() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let options = RepositoryLockOptions {
        owner_label: Some("nightly-job".to_string()),
        ..RepositoryLockOptions::default()
    };
    let lock = RepositoryLock::acquire(&repository, &options)
        .await
        .expect("lock");
    let owner = lock.owner().clone();
    assert_eq!(owner.process_id, std::process::id());
    assert_eq!(owner.label.as_deref(), Some("nightly-job"));
    assert!(!owner.host.is_empty());
    assert!((owner.acquired_at - unix_seconds()).abs() < 60);

    assert_eq!(
        read_repository_lock(&repository).await.expect("read"),
        RepositoryLockStatus::Owned(owner.clone())
    );
    let error = RepositoryLock::acquire(&repository, &RepositoryLockOptions::default())
        .await
        .expect_err("contended");
    let BackupError::RepositoryLocked {
        owner: Some(found), ..
    } = &error
    else {
        panic!("contention reports the owner: {error:?}");
    };
    assert_eq!(**found, owner);
    assert!(error.to_string().contains(&owner.run_id.to_string()));

    lock.release().await.expect("release");
    assert_unlocked(&repository).await;
}

#[tokio::test]
async fn break_lock_requires_the_named_owner_and_a_minimum_age() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let owner = {
        // A crashed run: the handle is dropped without release.
        let lock = RepositoryLock::acquire(&repository, &RepositoryLockOptions::default())
            .await
            .expect("lock");
        lock.owner().clone()
    };
    for request in [
        BreakRepositoryLockRequest {
            expected_run_id: Uuid::new_v4(),
            min_age_seconds: 0,
        },
        BreakRepositoryLockRequest {
            expected_run_id: owner.run_id,
            min_age_seconds: 3_600,
        },
        BreakRepositoryLockRequest {
            expected_run_id: owner.run_id,
            min_age_seconds: -1,
        },
    ] {
        let error = break_repository_lock(&repository, &request)
            .await
            .expect_err("refused");
        assert!(
            matches!(error, BackupError::BreakLockRefused { .. }),
            "unexpected error for {request:?}: {error:?}"
        );
        assert_eq!(
            read_repository_lock(&repository).await.expect("read"),
            RepositoryLockStatus::Owned(owner.clone())
        );
    }

    let broken = break_repository_lock(
        &repository,
        &BreakRepositoryLockRequest {
            expected_run_id: owner.run_id,
            min_age_seconds: 0,
        },
    )
    .await
    .expect("break the confirmed lock");
    assert_eq!(broken, owner);
    assert_unlocked(&repository).await;

    let error = break_repository_lock(
        &repository,
        &BreakRepositoryLockRequest {
            expected_run_id: owner.run_id,
            min_age_seconds: 0,
        },
    )
    .await
    .expect_err("nothing to break");
    assert!(matches!(error, BackupError::BreakLockRefused { .. }));
}

#[tokio::test]
async fn break_lock_refuses_a_lock_without_an_owner_record() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    repository
        .put_blob(REPOSITORY_LOCK_KEY, Bytes::from_static(b"0"))
        .await
        .expect("legacy lock");
    assert_eq!(
        read_repository_lock(&repository).await.expect("read"),
        RepositoryLockStatus::Unidentified
    );
    let error = break_repository_lock(
        &repository,
        &BreakRepositoryLockRequest {
            expected_run_id: Uuid::new_v4(),
            min_age_seconds: 0,
        },
    )
    .await
    .expect_err("legacy locks need offline recovery");
    assert!(matches!(error, BackupError::BreakLockRefused { .. }));
    assert!(
        repository
            .blob_exists(REPOSITORY_LOCK_KEY)
            .await
            .expect("exists")
    );
}

#[tokio::test]
async fn release_leaves_a_lock_that_another_run_now_holds() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let stale = RepositoryLock::acquire(&repository, &RepositoryLockOptions::default())
        .await
        .expect("first run");
    let stale_run = stale.owner().run_id;
    break_repository_lock(
        &repository,
        &BreakRepositoryLockRequest {
            expected_run_id: stale_run,
            min_age_seconds: 0,
        },
    )
    .await
    .expect("operator breaks the lock");
    let current = RepositoryLock::acquire(&repository, &RepositoryLockOptions::default())
        .await
        .expect("second run");
    let current_owner = current.owner().clone();

    let error = stale.release().await.expect_err("stale holder");
    assert!(
        matches!(&error, BackupError::RepositoryLockNotHeld { run_id, found: Some(found), .. }
            if *run_id == stale_run && **found == current_owner),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        read_repository_lock(&repository).await.expect("read"),
        RepositoryLockStatus::Owned(current_owner)
    );
    current.release().await.expect("current holder releases");
}

#[tokio::test]
async fn cancelled_backup_releases_the_lock_and_removes_its_payloads() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let cancellation = BackupCancellation::new();
    let objects = Objects::with_object(b"first object")
        .and_object(b"second object")
        .cancel_on_load(cancellation.clone(), 1);
    let options = BackupExecutionOptions {
        object_concurrency: 1,
        cancellation,
        ..BackupExecutionOptions::default()
    };

    let error = create_full_backup_with_options(
        &repository,
        &Database::with_rows("users", 2),
        &objects,
        full_request(id(1), 1),
        &options,
    )
    .await
    .expect_err("cancelled after the first object");

    assert!(matches!(error, BackupError::Cancelled), "{error:?}");
    assert_unlocked(&repository).await;
    assert!(snapshot_blobs(&repository, id(1)).await.is_empty());
    assert_eq!(objects.loads(), 1, "no object starts after cancellation");

    let precancelled = BackupExecutionOptions::default();
    precancelled.cancellation.cancel();
    let error = create_full_backup_with_options(
        &repository,
        &Database::with_rows("users", 2),
        &Objects::default(),
        full_request(id(2), 2),
        &precancelled,
    )
    .await
    .expect_err("cancelled before any write");
    assert!(matches!(error, BackupError::Cancelled));
    assert_unlocked(&repository).await;
    assert!(snapshot_blobs(&repository, id(2)).await.is_empty());
}

#[tokio::test]
async fn failed_release_after_a_committed_backup_names_the_snapshot() {
    let temp = TempDir::new().expect("temp dir");
    let repository = Hooked::new(repository(&temp));
    repository.fail_lock_release.store(true, Ordering::SeqCst);

    let error = create_full_backup(
        &repository,
        &Database::with_rows("users", 1),
        &Objects::default(),
        full_request(id(1), 1),
    )
    .await
    .expect_err("release failed");

    let BackupError::LockReleaseFailed {
        committed_snapshot_id,
        run_id,
        ..
    } = &error
    else {
        panic!("unexpected error: {error:?}");
    };
    assert_eq!(*committed_snapshot_id, Some(id(1)));
    let RepositoryLockStatus::Owned(owner) =
        read_repository_lock(&repository.inner).await.expect("read")
    else {
        panic!("the lock is still held");
    };
    assert_eq!(owner.run_id, *run_id);
    assert_eq!(owner.operation, "full_backup");
    verify_manifest_and_objects(
        &repository.inner,
        &load_manifest(&repository.inner, id(1))
            .await
            .expect("committed manifest"),
    )
    .await
    .expect_err("verification waits for the stuck lock");
}

#[tokio::test]
async fn cancelled_backup_lets_in_flight_uploads_finish() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let cancellation = BackupCancellation::new();
    let objects = Objects::with_object(b"object zero")
        .and_object(b"object one")
        .and_object(b"object two")
        .and_object(b"object three")
        .slow_streams()
        .cancel_on_load(cancellation.clone(), 2);
    let options = BackupExecutionOptions {
        object_concurrency: 2,
        cancellation,
        ..BackupExecutionOptions::default()
    };

    let error = create_full_backup_with_options(
        &repository,
        &Database::with_rows("users", 1),
        &objects,
        full_request(id(1), 1),
        &options,
    )
    .await
    .expect_err("cancelled while two uploads are in flight");

    assert!(matches!(error, BackupError::Cancelled), "{error:?}");
    assert_eq!(objects.loads(), 2, "no upload starts after cancellation");
    for (index, (object, bytes)) in objects.objects.iter().enumerate() {
        let key = object_content_key(&object.sha256_hex);
        if index < 2 {
            assert_eq!(
                repository
                    .get_blob(&key)
                    .await
                    .expect("in-flight upload finished"),
                *bytes,
                "upload {index} ran to completion"
            );
        } else {
            assert!(!repository.blob_exists(&key).await.expect("exists"));
        }
    }
    assert!(snapshot_blobs(&repository, id(1)).await.is_empty());
    assert_unlocked(&repository).await;
}

#[tokio::test]
async fn oversized_owner_label_is_refused_before_locking() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let lock = RepositoryLockOptions {
        owner_label: Some("x".repeat(100 * 1024)),
        ..RepositoryLockOptions::default()
    };

    let error = RepositoryLock::acquire(&repository, &lock)
        .await
        .expect_err("label too long for a readable owner record");
    assert!(
        matches!(error, BackupError::InvalidRepositoryLockOptions { .. }),
        "{error:?}"
    );
    assert_unlocked(&repository).await;

    let error = create_full_backup_with_options(
        &repository,
        &Database::with_rows("users", 1),
        &Objects::default(),
        full_request(id(1), 1),
        &BackupExecutionOptions {
            lock,
            ..BackupExecutionOptions::default()
        },
    )
    .await
    .expect_err("refused before the lock write");
    assert!(matches!(
        error,
        BackupError::InvalidRepositoryLockOptions { .. }
    ));
    assert!(all_blobs(&repository).await.is_empty());
}

#[tokio::test]
async fn owner_records_with_unknown_fields_stay_identified() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let run_id = Uuid::new_v4();
    let record = serde_json::json!({
        "run_id": run_id,
        "host": "builder",
        "process_id": 7,
        "operation": "full_backup",
        "label": null,
        "acquired_at": 0,
        "fencing_token": 3,
    });
    repository
        .put_blob(
            REPOSITORY_LOCK_KEY,
            Bytes::from(serde_json::to_vec(&record).expect("json")),
        )
        .await
        .expect("lock from a later release");

    let RepositoryLockStatus::Owned(owner) = read_repository_lock(&repository).await.expect("read")
    else {
        panic!("a later release's record is still an owner record");
    };
    assert_eq!(owner.run_id, run_id);
    break_repository_lock(
        &repository,
        &BreakRepositoryLockRequest {
            expected_run_id: run_id,
            min_age_seconds: 0,
        },
    )
    .await
    .expect("break the confirmed lock");
    assert_unlocked(&repository).await;
}

#[tokio::test]
async fn read_only_repositories_need_the_explicit_unlocked_mode() {
    let temp = TempDir::new().expect("temp dir");
    let local = repository(&temp);
    create_full_backup(
        &local,
        &Database::with_rows("users", 2),
        &Objects::with_object(b"read only object"),
        full_request(id(1), 1),
    )
    .await
    .expect("snapshot");
    let manifest = load_manifest(&local, id(1)).await.expect("manifest");
    let read_only = ReadOnly(local);
    let database = Database::with_rows("users", 2);

    let error = restore_snapshot(&read_only, &database, id(1), RestoreContext::dry_run())
        .await
        .expect_err("the default takes the lock");
    let BackupError::RestoreStageFailed { source, .. } = error else {
        panic!("validation-stage failure: {error:?}");
    };
    assert!(matches!(*source, BackupError::UnsupportedOperation { .. }));
    assert!(matches!(
        verify_manifest_and_objects(&read_only, &manifest).await,
        Err(BackupError::UnsupportedOperation { .. })
    ));
    assert!(matches!(
        restore_objects(&read_only, &manifest, &CollectingSink::default()).await,
        Err(BackupError::UnsupportedOperation { .. })
    ));

    let unlocked = RestoreOptions {
        lock_mode: RepositoryLockMode::AssumeNoConcurrentWriters,
        ..RestoreOptions::default()
    };
    restore_snapshot_with_options(
        &read_only,
        &database,
        id(1),
        RestoreContext::dry_run(),
        &unlocked,
    )
    .await
    .expect("explicit unlocked restore");
    let sink = CollectingSink::default();
    restore_objects_with_options(&read_only, &manifest, &sink, &unlocked)
        .await
        .expect("explicit unlocked object restore");
    assert_eq!(sink.restored.lock().expect("sink").len(), 1);
    verify_manifest_and_objects_with_options(
        &read_only,
        &manifest,
        &VerificationOptions {
            lock_mode: RepositoryLockMode::AssumeNoConcurrentWriters,
            ..VerificationOptions::default()
        },
    )
    .await
    .expect("explicit unlocked verification");
}

#[tokio::test]
async fn cancelled_restore_and_verification_release_the_lock() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    create_full_backup(
        &repository,
        &Database::with_rows("users", 1),
        &Objects::with_object(b"cancelled restore object"),
        full_request(id(1), 1),
    )
    .await
    .expect("full");
    let changes = Database::with_rows("users", 1).with_changes(changes("users", 2));
    incremental(&repository, id(2), id(1), 2, &changes).await;
    let manifest = load_manifest(&repository, id(1)).await.expect("manifest");

    let cancellation = BackupCancellation::new();
    let target = Database::with_rows("users", 1).cancel_on_restore(cancellation.clone());
    let error = restore_snapshot_with_options(
        &repository,
        &target,
        id(2),
        RestoreContext::empty_database(),
        &RestoreOptions {
            cancellation,
            ..RestoreOptions::default()
        },
    )
    .await
    .expect_err("cancelled after the full stage");
    let BackupError::RestoreStageFailed { progress, source } = error else {
        panic!("typed progress: {error:?}");
    };
    assert!(matches!(*source, BackupError::Cancelled), "{source:?}");
    assert_eq!(progress.completed_manifest_ids, vec![id(1)]);
    assert_eq!(
        target.incremental_applies(),
        0,
        "no stage starts after cancel"
    );
    assert_unlocked(&repository).await;

    let cancelled = BackupCancellation::new();
    cancelled.cancel();
    let error = verify_manifest_and_objects_with_options(
        &repository,
        &manifest,
        &VerificationOptions {
            cancellation: cancelled.clone(),
            ..VerificationOptions::default()
        },
    )
    .await
    .expect_err("cancelled verification");
    assert!(matches!(error, BackupError::Cancelled), "{error:?}");
    assert_unlocked(&repository).await;

    let sink = CollectingSink::default();
    let error = restore_objects_with_options(
        &repository,
        &manifest,
        &sink,
        &RestoreOptions {
            cancellation: cancelled,
            ..RestoreOptions::default()
        },
    )
    .await
    .expect_err("cancelled object restore");
    assert!(matches!(error, BackupError::Cancelled), "{error:?}");
    assert!(sink.restored.lock().expect("sink").is_empty());
    assert_unlocked(&repository).await;
}

#[tokio::test]
async fn object_restore_caps_the_streamed_read_at_the_object_limit() {
    let temp = TempDir::new().expect("temp dir");
    let local = repository(&temp).with_capacity_limits(BackupCapacityLimits {
        object_bytes: BackupByteLimit::Bytes(8),
        ..BackupCapacityLimits::default()
    });
    create_full_backup(
        &local,
        &Database::with_rows("users", 1),
        &Objects::with_object(b"12345"),
        full_request(id(1), 1),
    )
    .await
    .expect("snapshot");
    let manifest = load_manifest(&local, id(1)).await.expect("manifest");
    let repository = Hooked::new(local);
    repository.grow_on_second_read(
        manifest.objects[0].content_key.clone(),
        Bytes::from(vec![b'x'; 4096]),
    );
    let sink = CollectingSink::default();

    let error = restore_objects(&repository, &manifest, &sink)
        .await
        .expect_err("the second read grew past the limit");

    assert!(
        matches!(error, BackupError::CapacityExceeded { limit_bytes: 8 }),
        "{error:?}"
    );
    assert!(sink.restored.lock().expect("sink").is_empty());
}

#[tokio::test]
async fn write_manifest_never_overwrites_a_snapshot() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    full(&repository, id(1), 1, &Database::with_rows("users", 1)).await;
    let key = snapshot_manifest_key(id(1));
    let before = repository.get_blob(&key).await.expect("manifest");
    let mut manifest = load_manifest(&repository, id(1)).await.expect("manifest");
    manifest.app_version = "rewritten".to_string();

    let error = write_manifest(&repository, &mut manifest)
        .await
        .expect_err("existing manifest");

    assert!(
        matches!(error, BackupError::SnapshotExists { snapshot_id } if snapshot_id == id(1)),
        "{error:?}"
    );
    assert_eq!(repository.get_blob(&key).await.expect("manifest"), before);
}

#[tokio::test]
async fn failed_release_after_cancellation_is_reported_with_the_run() {
    let temp = TempDir::new().expect("temp dir");
    let repository = Hooked::new(repository(&temp));
    full(
        &repository.inner,
        id(1),
        1,
        &Database::with_rows("users", 1),
    )
    .await;
    repository.fail_lock_release.store(true, Ordering::SeqCst);
    let cancelled = BackupCancellation::new();
    cancelled.cancel();

    let error = create_full_backup_with_options(
        &repository,
        &Database::with_rows("users", 1),
        &Objects::default(),
        full_request(id(2), 2),
        &BackupExecutionOptions {
            cancellation: cancelled.clone(),
            ..BackupExecutionOptions::default()
        },
    )
    .await
    .expect_err("cancelled backup");
    let run_id = assert_lock_not_released(&repository, &error, |operation| {
        matches!(operation, BackupError::Cancelled)
    })
    .await;
    assert!(error.to_string().contains(&run_id.to_string()));
    break_repository_lock(
        &repository.inner,
        &BreakRepositoryLockRequest {
            expected_run_id: run_id,
            min_age_seconds: 0,
        },
    )
    .await
    .expect("the reported run's lock can be broken");

    let error = restore_snapshot_with_options(
        &repository,
        &Database::with_rows("users", 1),
        id(1),
        RestoreContext::dry_run(),
        &RestoreOptions {
            cancellation: cancelled,
            ..RestoreOptions::default()
        },
    )
    .await
    .expect_err("cancelled restore");
    assert_lock_not_released(&repository, &error, |operation| {
        matches!(operation, BackupError::RestoreStageFailed { source, .. }
            if matches!(**source, BackupError::Cancelled))
    })
    .await;
}

#[tokio::test]
async fn object_restore_cancelled_partway_keeps_earlier_objects_and_unlocks() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    create_full_backup(
        &repository,
        &Database::with_rows("users", 1),
        &Objects::with_object(b"first restored object")
            .and_object(b"second restored object")
            .and_object(b"third restored object"),
        full_request(id(1), 1),
    )
    .await
    .expect("snapshot");
    let manifest = load_manifest(&repository, id(1)).await.expect("manifest");
    let cancellation = BackupCancellation::new();
    let sink = CollectingSink {
        cancel_after: Some((cancellation.clone(), 1)),
        ..CollectingSink::default()
    };

    let error = restore_objects_with_options(
        &repository,
        &manifest,
        &sink,
        &RestoreOptions {
            cancellation,
            ..RestoreOptions::default()
        },
    )
    .await
    .expect_err("cancelled after the first object");

    assert!(matches!(error, BackupError::Cancelled), "{error:?}");
    assert_eq!(
        sink.restored.lock().expect("sink").as_slice(),
        &[object_ref(&manifest.objects[0])],
        "only the object restored before cancellation reached the sink"
    );
    assert_unlocked(&repository).await;
}

// ---------------------------------------------------------------------------
// #476 backup writes enforce restore limits
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_backup_enforces_the_decoded_payload_limit_at_the_restore_boundary() {
    let database = Database::with_rows("users", 3);
    let exact = decoded_len(&database.tables[0].rows);

    let temp = TempDir::new().expect("temp dir");
    let limited = repository(&temp).with_capacity_limits(BackupCapacityLimits {
        decompressed_bytes: BackupByteLimit::Bytes(exact - 1),
        ..BackupCapacityLimits::default()
    });
    let error = create_full_backup(
        &limited,
        &database,
        &Objects::default(),
        full_request(id(1), 1),
    )
    .await
    .expect_err("one byte over");
    assert_capacity(&error, BackupCapacityLimitKind::DecodedPayload, exact - 1);
    assert!(all_blobs(&limited).await.is_empty(), "nothing written");

    let fits = repository(&temp).with_capacity_limits(BackupCapacityLimits {
        decompressed_bytes: BackupByteLimit::Bytes(exact),
        ..BackupCapacityLimits::default()
    });
    full(&fits, id(1), 1, &database).await;
    restore_snapshot(&fits, &database, id(1), RestoreContext::dry_run())
        .await
        .expect("the same limits restore what they backed up");
}

#[tokio::test]
async fn full_backup_enforces_the_stage_limit_across_tables() {
    let database = Database::with_rows("users", 2).and_table("teams", 2);
    let total = decoded_len(&database.tables[0].rows) + decoded_len(&database.tables[1].rows);

    let temp = TempDir::new().expect("temp dir");
    let limited = repository(&temp).with_capacity_limits(BackupCapacityLimits {
        stage_bytes: BackupByteLimit::Bytes(total - 1),
        ..BackupCapacityLimits::default()
    });
    let error = create_full_backup(
        &limited,
        &database,
        &Objects::default(),
        full_request(id(1), 1),
    )
    .await
    .expect_err("aggregate over");
    assert_capacity(&error, BackupCapacityLimitKind::Stage, total - 1);
    assert!(all_blobs(&limited).await.is_empty(), "nothing written");

    let fits = repository(&temp).with_capacity_limits(BackupCapacityLimits {
        stage_bytes: BackupByteLimit::Bytes(total),
        ..BackupCapacityLimits::default()
    });
    full(&fits, id(1), 1, &database).await;
    restore_snapshot(&fits, &database, id(1), RestoreContext::dry_run())
        .await
        .expect("exact stage boundary restores");
}

#[tokio::test]
async fn full_backup_enforces_the_stored_payload_limit() {
    let database = Database::with_incompressible_rows("users", 64);
    let temp = TempDir::new().expect("temp dir");
    let limited = repository(&temp).with_capacity_limits(BackupCapacityLimits {
        buffered_bytes: BackupByteLimit::Bytes(1024),
        ..BackupCapacityLimits::default()
    });

    let error = create_full_backup(
        &limited,
        &database,
        &Objects::default(),
        full_request(id(1), 1),
    )
    .await
    .expect_err("stored payload over");

    assert_capacity(&error, BackupCapacityLimitKind::StoredPayload, 1024);
    assert!(snapshot_blobs(&limited, id(1)).await.is_empty());
    assert_unlocked(&limited).await;
}

#[tokio::test]
async fn backup_and_restore_enforce_the_same_object_limit() {
    let temp = TempDir::new().expect("temp dir");
    let limited = repository(&temp).with_capacity_limits(BackupCapacityLimits {
        object_bytes: BackupByteLimit::Bytes(4),
        ..BackupCapacityLimits::default()
    });

    let declared = Objects::with_object(b"12345");
    let error = create_full_backup(
        &limited,
        &Database::with_rows("users", 1),
        &declared,
        full_request(id(1), 1),
    )
    .await
    .expect_err("declared size over");
    assert_capacity(&error, BackupCapacityLimitKind::Object, 4);
    assert!(
        all_blobs(&limited).await.is_empty(),
        "refused before writing"
    );
    assert_eq!(declared.loads(), 0);

    let understated = Objects::with_object(b"12345").declaring_size(1);
    let error = create_full_backup(
        &limited,
        &Database::with_rows("users", 1),
        &understated,
        full_request(id(1), 1),
    )
    .await
    .expect_err("streamed size over");
    assert_capacity(&error, BackupCapacityLimitKind::Object, 4);
    assert!(
        !limited
            .blob_exists(&object_content_key(&bytes_sha256_hex(b"12345")))
            .await
            .expect("exists"),
        "the conditional create was aborted"
    );
    assert!(snapshot_blobs(&limited, id(1)).await.is_empty());
    assert_unlocked(&limited).await;

    let unlimited = repository(&temp);
    create_full_backup(
        &unlimited,
        &Database::with_rows("users", 1),
        &Objects::with_object(b"12345"),
        full_request(id(1), 1),
    )
    .await
    .expect("unbounded handle");
    let manifest = load_manifest(&limited, id(1)).await.expect("manifest");
    assert!(matches!(
        restore_objects(&limited, &manifest, &CollectingSink::default()).await,
        Err(BackupError::CapacityExceeded { limit_bytes: 4 })
    ));
    assert!(matches!(
        verify_manifest_and_objects(&limited, &manifest).await,
        Err(BackupError::CapacityExceeded { limit_bytes: 4 })
    ));
}

#[tokio::test]
async fn incremental_backup_enforces_restore_limits_before_writing() {
    let temp = TempDir::new().expect("temp dir");
    let unlimited = repository(&temp);
    full(&unlimited, id(1), 1, &Database::with_rows("users", 1)).await;
    let database = Database::with_rows("users", 1).with_changes(changes("users", 4));
    let exact = decoded_len(&database.changes);

    let limited = repository(&temp).with_capacity_limits(BackupCapacityLimits {
        decompressed_bytes: BackupByteLimit::Bytes(exact - 1),
        ..BackupCapacityLimits::default()
    });
    let error = create_incremental_backup(
        &limited,
        &database,
        &Objects::default(),
        incremental_request(id(2), id(1), 2),
    )
    .await
    .expect_err("change payload over");
    assert_capacity(&error, BackupCapacityLimitKind::DecodedPayload, exact - 1);
    assert!(snapshot_blobs(&limited, id(2)).await.is_empty());
    assert_unlocked(&limited).await;
}

#[tokio::test]
async fn compaction_enforces_the_stage_limit_of_its_synthetic_full() {
    let temp = TempDir::new().expect("temp dir");
    let base = Database::with_rows("users", 8);
    let full_stage = decoded_len(&base.tables[0].rows);
    let database = base.clone().with_changes(changes("teams", 1));
    assert!(decoded_len(&database.changes) <= full_stage);
    let limited = repository(&temp).with_capacity_limits(BackupCapacityLimits {
        stage_bytes: BackupByteLimit::Bytes(full_stage),
        ..BackupCapacityLimits::default()
    });
    full(&limited, id(1), 1, &base).await;
    incremental(&limited, id(2), id(1), 2, &database).await;

    let error = compact_chain(&limited, compact_request(id(3), id(2), 3))
        .await
        .expect_err("merged stage over");

    assert_capacity(&error, BackupCapacityLimitKind::Stage, full_stage);
    assert!(snapshot_blobs(&limited, id(3)).await.is_empty());
    assert_unlocked(&limited).await;
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn repository(temp: &TempDir) -> LocalBackupRepository {
    LocalBackupRepository::new(temp.path()).with_manifest_authenticator(ManifestAuthenticator::new(
        ManifestKey::from_bytes(&[7; 32]).expect("test manifest key"),
    ))
}

fn id(n: u8) -> Uuid {
    Uuid::from_bytes([n; 16])
}

fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn full_request(snapshot_id: Uuid, created_at: i64) -> FullBackupRequest {
    FullBackupRequest {
        snapshot_id,
        created_at,
        app_id: "example-app".to_string(),
        app_version: "0.1.0".to_string(),
    }
}

fn incremental_request(
    snapshot_id: Uuid,
    parent_snapshot_id: Uuid,
    created_at: i64,
) -> IncrementalBackupRequest {
    IncrementalBackupRequest {
        snapshot_id,
        parent_snapshot_id,
        created_at,
        app_id: "example-app".to_string(),
        app_version: "0.1.0".to_string(),
    }
}

fn compact_request(
    snapshot_id: Uuid,
    source_snapshot_id: Uuid,
    created_at: i64,
) -> CompactChainRequest {
    CompactChainRequest {
        snapshot_id,
        source_snapshot_id,
        created_at,
        app_id: "example-app".to_string(),
        app_version: "0.1.0".to_string(),
    }
}

async fn full(
    repository: &dyn BackupRepository,
    snapshot_id: Uuid,
    created_at: i64,
    database: &Database,
) {
    create_full_backup(
        repository,
        database,
        &Objects::default(),
        full_request(snapshot_id, created_at),
    )
    .await
    .expect("full backup");
}

async fn incremental(
    repository: &dyn BackupRepository,
    snapshot_id: Uuid,
    parent: Uuid,
    created_at: i64,
    database: &Database,
) {
    create_incremental_backup(
        repository,
        database,
        &Objects::default(),
        incremental_request(snapshot_id, parent, created_at),
    )
    .await
    .expect("incremental backup");
}

async fn all_blobs(repository: &dyn BackupRepository) -> Vec<(String, Bytes)> {
    let mut keys = repository.list_blobs("").await.expect("list");
    keys.sort();
    let mut blobs = Vec::new();
    for key in keys {
        let body = repository.get_blob(&key).await.expect("read");
        blobs.push((key, body));
    }
    blobs
}

async fn snapshot_blobs(repository: &dyn BackupRepository, snapshot_id: Uuid) -> Vec<String> {
    repository
        .list_blobs(&format!("snapshots/{snapshot_id}"))
        .await
        .expect("list")
}

async fn assert_unlocked(repository: &dyn BackupRepository) {
    assert!(
        !repository
            .blob_exists(REPOSITORY_LOCK_KEY)
            .await
            .expect("exists"),
        "the repository lock was released"
    );
}

fn assert_capacity(error: &BackupError, kind: BackupCapacityLimitKind, limit: u64) {
    assert!(
        matches!(error, BackupError::BackupCapacityExceeded { limit: found, limit_bytes, .. }
            if *found == kind && *limit_bytes == limit),
        "expected {kind:?} limit {limit}, got {error:?}"
    );
}

/// Asserts that the operation failed with `operation` and its lock release
/// failed, and that the reported run still holds the lock. Returns the run.
async fn assert_lock_not_released(
    repository: &Hooked,
    error: &BackupError,
    operation: impl Fn(&BackupError) -> bool,
) -> Uuid {
    let BackupError::LockNotReleased {
        run_id,
        operation: source,
        ..
    } = error
    else {
        panic!("a failed release is reported: {error:?}");
    };
    assert!(operation(source), "unexpected operation error: {source:?}");
    let RepositoryLockStatus::Owned(owner) =
        read_repository_lock(&repository.inner).await.expect("read")
    else {
        panic!("the lock is still held");
    };
    assert_eq!(owner.run_id, *run_id);
    *run_id
}

fn object_ref(entry: &graphql_orm_backup::ObjectBackupEntry) -> BackupObjectRef {
    BackupObjectRef {
        object_id: entry.object_id,
        storage_key: entry.storage_key.clone(),
        sha256_hex: entry.sha256_hex.clone(),
        size_bytes: entry.size_bytes,
        mime_type: entry.mime_type.clone(),
    }
}

fn assert_prune_was_locked_out(repository: &Hooked) {
    let outcome = repository
        .prune_outcome
        .lock()
        .expect("outcome")
        .take()
        .expect("the hook ran a prune");
    assert!(
        matches!(outcome, Err(BackupError::RepositoryLocked { .. })),
        "a concurrent prune must be locked out: {outcome:?}"
    );
}

/// Size of the JSONL payload restore decodes for these entries.
fn decoded_len<T: serde::Serialize>(entries: &[T]) -> u64 {
    entries
        .iter()
        .map(|entry| serde_json::to_vec(entry).expect("json").len() as u64 + 1)
        .sum()
}

fn row(table: &str, key: usize, value: String) -> BackupRow {
    let mut values = Map::new();
    values.insert("id".to_string(), Value::String(key.to_string()));
    values.insert("name".to_string(), Value::String(value));
    BackupRow {
        table_name: table.to_string(),
        primary_key: key.to_string(),
        row_hash: format!("hash-{key}"),
        values,
    }
}

fn changes(table: &str, count: usize) -> Vec<BackupChangeExport> {
    (0..count)
        .map(|key| BackupChangeExport {
            table_name: table.to_string(),
            primary_key: format!("c{key}"),
            action: BackupChangeAction::Create,
            row: Some(row(table, key + 1000, format!("changed {key}"))),
            changed_at: 10,
        })
        .collect()
}

#[derive(Clone)]
struct Database {
    tables: Vec<BackupTableExport>,
    changes: Vec<BackupChangeExport>,
    schema_hash: String,
    cancel_on_restore: Option<BackupCancellation>,
    incremental_applies: Arc<std::sync::atomic::AtomicUsize>,
}

impl Database {
    fn with_rows(table: &str, count: usize) -> Self {
        Self {
            tables: vec![BackupTableExport {
                table_name: table.to_string(),
                rows: (0..count)
                    .map(|key| row(table, key, format!("row {key} of {table}")))
                    .collect(),
            }],
            changes: Vec::new(),
            schema_hash: "schema-hash".to_string(),
            cancel_on_restore: None,
            incremental_applies: Arc::default(),
        }
    }

    fn cancel_on_restore(mut self, cancellation: BackupCancellation) -> Self {
        self.cancel_on_restore = Some(cancellation);
        self
    }

    fn incremental_applies(&self) -> usize {
        self.incremental_applies.load(Ordering::SeqCst)
    }

    fn with_incompressible_rows(table: &str, count: usize) -> Self {
        let mut database = Self::with_rows(table, 0);
        database.tables[0].rows = (0..count)
            .map(|key| {
                let noise = (0..4)
                    .map(|part| bytes_sha256_hex(format!("{key}-{part}").as_bytes()))
                    .collect::<String>();
                row(table, key, noise)
            })
            .collect();
        database
    }

    fn and_table(mut self, table: &str, count: usize) -> Self {
        self.tables.extend(Self::with_rows(table, count).tables);
        self
    }

    fn with_changes(mut self, changes: Vec<BackupChangeExport>) -> Self {
        self.changes = changes;
        self
    }

    fn with_schema_hash(mut self, schema_hash: &str) -> Self {
        self.schema_hash = schema_hash.to_string();
        self
    }
}

#[async_trait]
impl GraphqlOrmBackupAdapter for Database {
    async fn schema_snapshot(&self) -> Result<GraphqlOrmBackupSchema, BackupError> {
        Ok(GraphqlOrmBackupSchema {
            backend: "sqlite".to_string(),
            migration_version: "1".to_string(),
            schema_hash: self.schema_hash.clone(),
        })
    }

    async fn restore_target_is_empty(&self) -> Result<bool, BackupError> {
        Ok(true)
    }

    async fn export_full(&self) -> Result<Vec<BackupTableExport>, BackupError> {
        Ok(self.tables.clone())
    }

    async fn export_incremental(
        &self,
        _parent_snapshot_id: Uuid,
    ) -> Result<Vec<BackupChangeExport>, BackupError> {
        Ok(self.changes.clone())
    }

    async fn restore_full(
        &self,
        _backup_schema: GraphqlOrmBackupSchema,
        _export: Vec<BackupTableExport>,
        _context: RestoreContext,
    ) -> Result<(), BackupError> {
        if let Some(cancellation) = &self.cancel_on_restore {
            cancellation.cancel();
        }
        Ok(())
    }

    async fn restore_incremental(
        &self,
        _changes: Vec<BackupChangeExport>,
        _context: RestoreContext,
    ) -> Result<(), BackupError> {
        self.incremental_applies.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
struct Objects {
    objects: Vec<(BackupObjectRef, Bytes)>,
    fail_loads: bool,
    /// Cancels when this many loads have started.
    cancel_on_load: Option<(BackupCancellation, usize)>,
    slow_streams: bool,
    loads: std::sync::atomic::AtomicUsize,
}

impl Objects {
    fn with_object(bytes: &'static [u8]) -> Self {
        Self::default().and_object(bytes)
    }

    fn and_object(mut self, bytes: &'static [u8]) -> Self {
        let sha256_hex = bytes_sha256_hex(bytes);
        self.objects.push((
            BackupObjectRef {
                object_id: Uuid::new_v4(),
                storage_key: format!("media/{sha256_hex}"),
                sha256_hex,
                size_bytes: bytes.len() as u64,
                mime_type: None,
            },
            Bytes::from_static(bytes),
        ));
        self
    }

    fn declaring_size(mut self, size_bytes: u64) -> Self {
        for (object, _) in &mut self.objects {
            object.size_bytes = size_bytes;
        }
        self
    }

    fn failing_loads(mut self) -> Self {
        self.fail_loads = true;
        self
    }

    fn cancel_on_load(mut self, cancellation: BackupCancellation, on_load: usize) -> Self {
        self.cancel_on_load = Some((cancellation, on_load));
        self
    }

    /// Streams that stay pending for a while, so uploads overlap.
    fn slow_streams(mut self) -> Self {
        self.slow_streams = true;
        self
    }

    fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl BackupObjectIndex for Objects {
    async fn list_objects_for_full_backup(&self) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(self
            .objects
            .iter()
            .map(|(object, _)| object.clone())
            .collect())
    }

    async fn list_objects_for_incremental_backup(
        &self,
        _since_snapshot_id: Uuid,
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        self.list_objects_for_full_backup().await
    }

    async fn load_object(&self, object: &BackupObjectRef) -> Result<Bytes, BackupError> {
        let started = self.loads.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some((cancellation, on_load)) = &self.cancel_on_load
            && started >= *on_load
        {
            cancellation.cancel();
        }
        if self.fail_loads {
            return Err(BackupError::Database {
                message: "object store unavailable".to_string(),
            });
        }
        self.objects
            .iter()
            .find(|(candidate, _)| candidate.object_id == object.object_id)
            .map(|(_, bytes)| bytes.clone())
            .ok_or_else(|| BackupError::MissingBlob {
                key: object.storage_key.clone(),
            })
    }

    async fn load_object_stream(
        &self,
        object: &BackupObjectRef,
    ) -> Result<StorageByteStream, BackupError> {
        let bytes = self.load_object(object).await?;
        if !self.slow_streams {
            return Ok(StorageByteStream::from_bytes(bytes));
        }
        // Later objects stream far more slowly, so an earlier upload finishes
        // and frees a slot while a later one is still in flight.
        let position = self
            .objects
            .iter()
            .position(|(candidate, _)| candidate.object_id == object.object_id)
            .unwrap_or_default();
        let yields = 20 + position * 2_000;
        let chunks = bytes
            .chunks(2)
            .map(Bytes::copy_from_slice)
            .collect::<Vec<_>>();
        let stream = futures::stream::iter(chunks).then(move |chunk| async move {
            for _ in 0..yields {
                tokio::task::yield_now().await;
            }
            Ok(chunk)
        });
        Ok(StorageByteStream::new(Box::pin(stream)))
    }
}

#[derive(Default)]
struct CollectingSink {
    restored: Mutex<Vec<BackupObjectRef>>,
    /// Cancels once this many objects were restored.
    cancel_after: Option<(BackupCancellation, usize)>,
}

#[async_trait]
impl RestoreObjectSink for CollectingSink {
    async fn restore_object(
        &self,
        object: BackupObjectRef,
        _bytes: Bytes,
    ) -> Result<(), BackupError> {
        let mut restored = self.restored.lock().expect("sink");
        restored.push(object);
        if let Some((cancellation, after)) = &self.cancel_after
            && restored.len() >= *after
        {
            cancellation.cancel();
        }
        Ok(())
    }
}

/// Forwards to a local repository, with hooks for concurrent prune, hidden
/// listings and failed lock release.
struct Hooked {
    inner: LocalBackupRepository,
    prune_on_read: Mutex<Option<String>>,
    prune_outcome: Mutex<Option<Result<PruneResult, BackupError>>>,
    hidden: Mutex<Option<String>>,
    fail_lock_release: AtomicBool,
    /// Serves these bytes for a key from its second read on.
    grown: Mutex<Option<(String, Bytes, usize)>>,
}

impl Hooked {
    fn new(inner: LocalBackupRepository) -> Self {
        Self {
            inner,
            prune_on_read: Mutex::new(None),
            prune_outcome: Mutex::new(None),
            hidden: Mutex::new(None),
            fail_lock_release: AtomicBool::new(false),
            grown: Mutex::new(None),
        }
    }

    fn grow_on_second_read(&self, key: String, bytes: Bytes) {
        *self.grown.lock().expect("hook") = Some((key, bytes, 0));
    }

    fn prune_on_read(&self, prefix: String) {
        *self.prune_on_read.lock().expect("hook") = Some(prefix);
    }

    fn hide_from_listings(&self, key: String) {
        *self.hidden.lock().expect("hook") = Some(key);
    }

    /// Runs a prune that would expire the snapshot being read, once.
    async fn before_read(&self, key: &str) {
        let armed = {
            let mut prefix = self.prune_on_read.lock().expect("hook");
            match prefix.as_deref() {
                Some(value) if key.starts_with(value) => prefix.take().is_some(),
                _ => false,
            }
        };
        if armed {
            let outcome = prune(&self.inner, &KeepPolicy::default()).await;
            *self.prune_outcome.lock().expect("hook") = Some(outcome);
        }
    }
}

#[async_trait]
impl BackupRepository for Hooked {
    fn capacity_limits(&self) -> BackupCapacityLimits {
        self.inner.capacity_limits()
    }

    fn manifest_authenticator(&self) -> Option<&ManifestAuthenticator> {
        self.inner.manifest_authenticator()
    }

    async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError> {
        self.inner.put_blob(key, body).await
    }

    async fn put_blob_stream(&self, key: &str, body: StorageByteStream) -> Result<(), BackupError> {
        self.inner.put_blob_stream(key, body).await
    }

    async fn put_blob_if_absent(&self, key: &str, body: Bytes) -> Result<bool, BackupError> {
        self.inner.put_blob_if_absent(key, body).await
    }

    async fn put_blob_stream_if_absent(
        &self,
        key: &str,
        body: StorageByteStream,
    ) -> Result<bool, BackupError> {
        self.inner.put_blob_stream_if_absent(key, body).await
    }

    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
        self.before_read(key).await;
        self.inner.get_blob(key).await
    }

    async fn get_blob_stream(&self, key: &str) -> Result<StorageByteStream, BackupError> {
        self.before_read(key).await;
        if let Some((grown_key, bytes, reads)) = self.grown.lock().expect("hook").as_mut()
            && grown_key == key
        {
            *reads += 1;
            if *reads > 1 {
                return Ok(StorageByteStream::from_bytes(bytes.clone()));
            }
        }
        self.inner.get_blob_stream(key).await
    }

    async fn blob_exists(&self, key: &str) -> Result<bool, BackupError> {
        self.inner.blob_exists(key).await
    }

    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError> {
        let hidden = self.hidden.lock().expect("hook").clone();
        let mut keys = self.inner.list_blobs(prefix).await?;
        keys.retain(|key| Some(key) != hidden.as_ref());
        Ok(keys)
    }

    async fn delete_blob_once(&self, key: &str) -> Result<(), BackupError> {
        if key == REPOSITORY_LOCK_KEY && self.fail_lock_release.load(Ordering::SeqCst) {
            return Err(BackupError::Database {
                message: "lost response".to_string(),
            });
        }
        self.inner.delete_blob_once(key).await
    }

    async fn delete_blob(&self, key: &str) -> Result<(), BackupError> {
        self.inner.delete_blob(key).await
    }
}

/// A repository the caller may only read, as with read-only credentials.
struct ReadOnly(LocalBackupRepository);

fn read_only_error() -> BackupError {
    BackupError::UnsupportedOperation {
        operation: "write to a read-only repository".to_string(),
    }
}

#[async_trait]
impl BackupRepository for ReadOnly {
    fn capacity_limits(&self) -> BackupCapacityLimits {
        self.0.capacity_limits()
    }

    fn manifest_authenticator(&self) -> Option<&ManifestAuthenticator> {
        self.0.manifest_authenticator()
    }

    async fn put_blob(&self, _key: &str, _body: Bytes) -> Result<(), BackupError> {
        Err(read_only_error())
    }

    async fn put_blob_if_absent(&self, _key: &str, _body: Bytes) -> Result<bool, BackupError> {
        Err(read_only_error())
    }

    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
        self.0.get_blob(key).await
    }

    async fn get_blob_stream(&self, key: &str) -> Result<StorageByteStream, BackupError> {
        self.0.get_blob_stream(key).await
    }

    async fn blob_exists(&self, key: &str) -> Result<bool, BackupError> {
        self.0.blob_exists(key).await
    }

    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError> {
        self.0.list_blobs(prefix).await
    }

    async fn delete_blob_once(&self, _key: &str) -> Result<(), BackupError> {
        Err(read_only_error())
    }

    async fn delete_blob(&self, _key: &str) -> Result<(), BackupError> {
        Err(read_only_error())
    }
}
