//! Manifest authentication, snapshot identity and restore-target constraints.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm_backup::{
    BackupByteLimit, BackupChangeExport, BackupError, BackupObjectIndex, BackupObjectRef,
    BackupRepository, BackupRow, BackupSnapshotManifest, BackupTableExport,
    BlobStoreRestoreObjectSink, FullBackupRequest, GraphqlOrmBackupAdapter, GraphqlOrmBackupSchema,
    KeepPolicy, LEGACY_BACKUP_FORMAT_VERSION, LegacyManifestPolicy, LocalBackupRepository,
    ManifestAuthenticator, ManifestKey, RestoreContext, RestoreKeyScope, authenticate_manifest,
    bytes_sha256_hex, collect_backup_stream, compress_payload, create_full_backup, load_manifest,
    prune, restore_objects, restore_snapshot, set_manifest_checksum, snapshot_manifest_key,
    verify_manifest_and_objects,
};
use graphql_orm_backup::{
    CompactChainRequest, IncrementalBackupRequest, RepositoryLockOptions, RestoreFailurePhase,
    compact_chain, create_incremental_backup, delete_snapshot,
};
use graphql_orm_storage::{BlobPutOptions, BlobStore, LocalStorageBackend, StorageByteStream};
use serde_json::{Map, Value};
use tempfile::TempDir;
use uuid::Uuid;

fn key(byte: u8) -> ManifestKey {
    ManifestKey::from_bytes(&[byte; 32]).expect("test key")
}

fn authenticator() -> ManifestAuthenticator {
    ManifestAuthenticator::new(key(7))
}

#[tokio::test]
async fn sealed_legacy_restore_accepts_copied_content_when_the_key_is_shared() {
    let source_dir = TempDir::new().unwrap();
    let target_dir = TempDir::new().unwrap();
    let source = repository(&source_dir);
    let target = repository(&target_dir);
    let mut old = backup_with_object(&source, "media/source.txt").await;
    seal_format_two(&mut old);
    write_raw_manifest(&source, &mut old).await;
    backup_with_object(&target, "media/target.txt").await;
    for prefix in ["snapshots", "objects"] {
        for key in source.list_blobs(prefix).await.unwrap() {
            target
                .put_blob(&key, source.get_blob(&key).await.unwrap())
                .await
                .unwrap();
        }
    }
    // This is an unavoidable legacy limitation, not repository-bound trust.
    // See docs/architecture.md#trust-model and MIGRATION.md step 4: use
    // TrustPinned when the sealing key has been shared across repositories.
    let accepting = legacy_repository(&target_dir, LegacyManifestPolicy::AcceptSealedLegacy);
    load_manifest(&accepting, old.snapshot_id).await.unwrap();
    verify_manifest_and_objects(&accepting, &old).await.unwrap();
    let database = MockDatabase::default();
    restore_snapshot(
        &accepting,
        &database,
        old.snapshot_id,
        RestoreContext::empty_database(),
    )
    .await
    .unwrap();
    assert_eq!(database.restored()[0].rows[0].values["name"], "Ada");
}

#[tokio::test]
async fn sealed_legacy_compaction_requires_every_legacy_input_to_be_pinned() {
    let dir = TempDir::new().unwrap();
    let repo = repository(&dir);
    let mut old = backup_with_object(&repo, "media/legacy.txt").await;
    seal_format_two(&mut old);
    write_raw_manifest(&repo, &mut old).await;
    let accepting = legacy_repository(&dir, LegacyManifestPolicy::AcceptSealedLegacy);
    let mut child = create_incremental_backup(
        &accepting,
        &MockDatabase::default(),
        &MockObjectIndex {
            object: entry_ref(&old.objects[0]),
            bytes: Bytes::from_static(b"object bytes"),
        },
        IncrementalBackupRequest {
            snapshot_id: Uuid::new_v4(),
            parent_snapshot_id: old.snapshot_id,
            created_at: old.created_at + 1,
            app_id: old.app_id.clone(),
            app_version: old.app_version.clone(),
        },
    )
    .await
    .unwrap()
    .manifest;
    seal_format_two(&mut child);
    write_raw_manifest(&repo, &mut child).await;
    for policy in [
        LegacyManifestPolicy::AcceptSealedLegacy,
        LegacyManifestPolicy::TrustUnauthenticated,
        LegacyManifestPolicy::TrustPinned(BTreeMap::from([(
            child.snapshot_id,
            child.checksum.clone(),
        )])),
    ] {
        let accepting = legacy_repository(&dir, policy);
        let before = accepting.list_blobs("").await.unwrap();
        let error = compact_chain(&accepting, compact_request(child.snapshot_id))
            .await
            .expect_err("unbound legacy data must not be resealed without a host checksum pin");
        assert!(
            matches!(error, BackupError::LegacyManifestRefused { snapshot_id } if snapshot_id == old.snapshot_id)
        );
        assert_eq!(
            accepting.list_blobs("").await.unwrap(),
            before,
            "no compaction writes and lock released"
        );
    }
    let pinned = legacy_repository(
        &dir,
        LegacyManifestPolicy::TrustPinned(BTreeMap::from([
            (old.snapshot_id, old.checksum.clone()),
            (child.snapshot_id, child.checksum.clone()),
        ])),
    );
    let compacted = compact_chain(&pinned, compact_request(child.snapshot_id))
        .await
        .unwrap();
    assert_eq!(compacted.manifest.format_version, 3);
    load_manifest(&repo, compacted.manifest.snapshot_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn incremental_and_compaction_identity_mismatch_write_nothing_and_release_the_lock() {
    let dir = TempDir::new().unwrap();
    let repo = repository(&dir);
    let parent = backup_with_object(&repo, "media/parent.txt").await;
    let foreign_pin = WriteCountingRepository::new(
        LocalBackupRepository::new(dir.path())
            .with_manifest_authenticator(authenticator().with_repository_identity(Uuid::new_v4())),
    );
    let before = repo.list_blobs("").await.unwrap();
    let error = create_incremental_backup(
        &foreign_pin,
        &MockDatabase::default(),
        &MockObjectIndex {
            object: entry_ref(&parent.objects[0]),
            bytes: Bytes::from_static(b"object bytes"),
        },
        IncrementalBackupRequest {
            snapshot_id: Uuid::new_v4(),
            parent_snapshot_id: parent.snapshot_id,
            created_at: parent.created_at + 1,
            app_id: parent.app_id.clone(),
            app_version: parent.app_version.clone(),
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        BackupError::ManifestAuthenticationFailed { .. }
    ));
    assert!(
        !repo
            .blob_exists(graphql_orm_backup::REPOSITORY_LOCK_KEY)
            .await
            .unwrap()
    );
    assert_eq!(
        repo.list_blobs("").await.unwrap(),
        before,
        "incremental wrote nothing"
    );
    assert_eq!(
        foreign_pin
            .payload_writes
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no payload writes, including cleaned-up ones"
    );
    let error = compact_chain(&foreign_pin, compact_request(parent.snapshot_id))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        BackupError::ManifestAuthenticationFailed { .. }
    ));
    assert!(
        !repo
            .blob_exists(graphql_orm_backup::REPOSITORY_LOCK_KEY)
            .await
            .unwrap()
    );
    assert_eq!(
        repo.list_blobs("").await.unwrap(),
        before,
        "compaction wrote nothing"
    );
    assert_eq!(
        foreign_pin
            .payload_writes
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no payload writes, including cleaned-up ones"
    );
}

#[tokio::test]
async fn sealed_legacy_policy_refuses_unsealed_bad_and_downgraded_manifests() {
    let dir = TempDir::new().unwrap();
    let repo = repository(&dir);
    let modern = backup_with_object(&repo, "media/legacy.txt").await;
    let accepting = legacy_repository(&dir, LegacyManifestPolicy::AcceptSealedLegacy);
    let mut unsealed = legacy(&modern);
    write_raw_manifest(&repo, &mut unsealed).await;
    assert!(matches!(
        load_manifest(&accepting, modern.snapshot_id).await,
        Err(BackupError::LegacyManifestRefused { .. })
    ));
    let mut sealed = modern.clone();
    seal_format_two(&mut sealed);
    write_raw_manifest(&repo, &mut sealed).await;
    load_manifest(&accepting, modern.snapshot_id).await.unwrap();
    sealed.authentication.as_mut().unwrap().mac = "00".repeat(32);
    write_raw_manifest(&repo, &mut sealed).await;
    assert!(load_manifest(&accepting, modern.snapshot_id).await.is_err());
    for (format, remove_identity) in [(2, false), (2, true), (3, true)] {
        let mut downgraded = modern.clone();
        downgraded.format_version = format;
        if remove_identity {
            downgraded.authentication.as_mut().unwrap().repository_id = None;
        }
        write_raw_manifest(&repo, &mut downgraded).await;
        assert!(
            load_manifest(&accepting, modern.snapshot_id).await.is_err(),
            "relabelling a bound seal cannot turn it into an old seal"
        );
    }
}

#[tokio::test]
async fn sealed_legacy_policy_supports_restore_incremental_compaction_and_retention() {
    let dir = TempDir::new().unwrap();
    let repo = repository(&dir);
    let mut old = backup_with_object(&repo, "media/legacy.txt").await;
    seal_format_two(&mut old);
    write_raw_manifest(&repo, &mut old).await;
    repo.delete_blob(graphql_orm_backup::REPOSITORY_ID_KEY)
        .await
        .unwrap();
    let objects = MockObjectIndex {
        object: entry_ref(&old.objects[0]),
        bytes: Bytes::from_static(b"object bytes"),
    };
    let incremental = |parent_snapshot_id| IncrementalBackupRequest {
        snapshot_id: Uuid::new_v4(),
        parent_snapshot_id,
        created_at: old.created_at + 1,
        app_id: old.app_id.clone(),
        app_version: old.app_version.clone(),
    };
    assert!(prune(&repo, &KeepPolicy::default()).await.is_err());
    assert!(
        delete_snapshot(&repo, old.snapshot_id, &RepositoryLockOptions::default())
            .await
            .is_err()
    );
    assert!(
        create_incremental_backup(
            &repo,
            &MockDatabase::default(),
            &objects,
            incremental(old.snapshot_id)
        )
        .await
        .is_err()
    );
    let accepting = legacy_repository(&dir, LegacyManifestPolicy::AcceptSealedLegacy);
    verify_manifest_and_objects(&accepting, &old).await.unwrap();
    restore_snapshot(
        &accepting,
        &MockDatabase::default(),
        old.snapshot_id,
        RestoreContext::dry_run(),
    )
    .await
    .unwrap();
    let child = create_incremental_backup(
        &accepting,
        &MockDatabase::default(),
        &objects,
        incremental(old.snapshot_id),
    )
    .await
    .unwrap();
    restore_snapshot(
        &accepting,
        &MockDatabase::default(),
        child.manifest.snapshot_id,
        RestoreContext::dry_run(),
    )
    .await
    .unwrap();
    let pinned = legacy_repository(
        &dir,
        LegacyManifestPolicy::TrustPinned(BTreeMap::from([(
            old.snapshot_id,
            old.checksum.clone(),
        )])),
    );
    let compacted = compact_chain(&pinned, compact_request(child.manifest.snapshot_id))
        .await
        .unwrap();
    assert_eq!(compacted.manifest.format_version, 3);
    prune(
        &accepting,
        &KeepPolicy {
            keep_last: 10,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    delete_snapshot(
        &accepting,
        child.manifest.snapshot_id,
        &RepositoryLockOptions::default(),
    )
    .await
    .unwrap();
    delete_snapshot(
        &accepting,
        old.snapshot_id,
        &RepositoryLockOptions::default(),
    )
    .await
    .unwrap();
    let strict = repository(&dir);
    load_manifest(&strict, compacted.manifest.snapshot_id)
        .await
        .unwrap();
    prune(&strict, &KeepPolicy::default()).await.unwrap();
}

#[tokio::test]
async fn incremental_chain_refuses_a_parent_copied_from_another_repository() {
    let source_dir = TempDir::new().unwrap();
    let source = repository(&source_dir);
    let parent = backup_with_object(&source, "media/parent.txt").await;
    let target_dir = TempDir::new().unwrap();
    let target = repository(&target_dir);
    graphql_orm_backup::initialize_repository_identity(&target)
        .await
        .unwrap();
    target
        .put_blob(
            &snapshot_manifest_key(parent.snapshot_id),
            source
                .get_blob(&snapshot_manifest_key(parent.snapshot_id))
                .await
                .unwrap(),
        )
        .await
        .unwrap();
    let before = without_locks(target.list_blobs("").await.unwrap());
    let error = create_incremental_backup(
        &target,
        &MockDatabase::default(),
        &MockObjectIndex {
            object: entry_ref(&parent.objects[0]),
            bytes: Bytes::from_static(b"object bytes"),
        },
        IncrementalBackupRequest {
            snapshot_id: Uuid::new_v4(),
            parent_snapshot_id: parent.snapshot_id,
            created_at: parent.created_at + 1,
            app_id: parent.app_id,
            app_version: parent.app_version,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        BackupError::ManifestAuthenticationFailed { .. }
    ));
    assert_eq!(without_locks(target.list_blobs("").await.unwrap()), before);
}

fn repository(temp: &TempDir) -> LocalBackupRepository {
    repository_with(temp, authenticator())
}

fn repository_with(temp: &TempDir, authenticator: ManifestAuthenticator) -> LocalBackupRepository {
    // The host fixture retains its pin independently of repository contents.
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    temp.path().hash(&mut hasher);
    let identity = Uuid::from_u128(u128::from(hasher.finish()));
    LocalBackupRepository::new(temp.path())
        .with_manifest_authenticator(authenticator.with_repository_identity(identity))
}

fn legacy_repository(temp: &TempDir, policy: LegacyManifestPolicy) -> LocalBackupRepository {
    repository_with(temp, authenticator().with_legacy_manifest_policy(policy))
}

fn restore_source(error: BackupError) -> BackupError {
    match error {
        BackupError::RestoreStageFailed { progress, source } => {
            assert!(!progress.target_may_have_changed());
            *source
        }
        other => other,
    }
}

#[tokio::test]
async fn tampered_manifest_with_recomputed_checksum_is_refused() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let manifest = backup_with_object(&repository, "media/a.txt").await;

    // Replace the table payload and recompute every plain digest, as a writer
    // without the host key can.
    let forged_rows = serialize_rows(&[row("users", "99", "forged")]);
    let mut forged = manifest.clone();
    forged.database.tables[0].sha256_hex = bytes_sha256_hex(&forged_rows);
    forged.database.tables[0].row_count = 1;
    repository
        .put_blob(&forged.database.tables[0].content_key, forged_rows)
        .await
        .expect("overwrite payload");
    write_raw_manifest(&repository, &mut forged).await;

    let target = MockDatabase::default();
    let error = restore_snapshot(
        &repository,
        &target,
        manifest.snapshot_id,
        RestoreContext::dry_run(),
    )
    .await
    .expect_err("forged manifest must be refused");
    assert!(
        matches!(
            restore_source(error),
            BackupError::ManifestAuthenticationFailed { .. }
        ),
        "tampered contents fail the seal"
    );
    let error = restore_snapshot(
        &repository,
        &target,
        manifest.snapshot_id,
        RestoreContext::empty_database(),
    )
    .await
    .expect_err("forged manifest must be refused");
    assert!(matches!(
        restore_source(error),
        BackupError::ManifestAuthenticationFailed { .. }
    ));
    assert!(target.restored().is_empty(), "nothing may be applied");

    // The caller-supplied manifest path re-authenticates too.
    let sink = RecordingSink::default();
    let error = restore_objects(&repository, &forged, &sink)
        .await
        .expect_err("forged manifest refused for object restore");
    assert!(matches!(
        error,
        BackupError::ManifestAuthenticationFailed { .. }
    ));
    assert!(sink.restored().is_empty());
    verify_manifest_and_objects(&repository, &forged)
        .await
        .expect_err("verification refuses the forged manifest");
}

#[tokio::test]
async fn manifest_sealed_with_another_key_is_refused_and_rotation_is_explicit() {
    let temp = TempDir::new().expect("temp dir");
    let old = repository_with(&temp, ManifestAuthenticator::new(key(1)));
    let manifest = backup_with_object(&old, "media/a.txt").await;

    let rotated = repository_with(&temp, ManifestAuthenticator::new(key(2)));
    let error = load_manifest(&rotated, manifest.snapshot_id)
        .await
        .expect_err("unknown key refused");
    assert!(matches!(
        error,
        BackupError::ManifestAuthenticationFailed { .. }
    ));

    let rotated = repository_with(
        &temp,
        ManifestAuthenticator::new(key(2)).with_previous_key(key(1)),
    );
    load_manifest(&rotated, manifest.snapshot_id)
        .await
        .expect("previous key still verifies");
    let resealed = backup_with_object(&rotated, "media/b.txt").await;
    assert_eq!(
        resealed.authentication.expect("sealed").key_id,
        key(2).key_id(),
        "new manifests use the current key"
    );
}

#[tokio::test]
async fn repository_without_authenticator_fails_closed_before_writing() {
    let temp = TempDir::new().expect("temp dir");
    let sealed = repository(&temp);
    let manifest = backup_with_object(&sealed, "media/a.txt").await;
    let before = sealed.list_blobs("").await.expect("list");

    let unkeyed = WriteCountingRepository::new(LocalBackupRepository::new(temp.path()));
    let error = load_manifest(&unkeyed, manifest.snapshot_id)
        .await
        .expect_err("no authenticator");
    assert!(matches!(error, BackupError::ManifestAuthenticationRequired));
    let error = restore_snapshot(
        &unkeyed,
        &MockDatabase::default(),
        manifest.snapshot_id,
        RestoreContext::dry_run(),
    )
    .await
    .expect_err("no authenticator");
    assert!(matches!(
        restore_source(error),
        BackupError::ManifestAuthenticationRequired
    ));
    let error = try_backup(&unkeyed, "media/c.txt")
        .await
        .expect_err("backup refused without a key");
    assert!(matches!(error, BackupError::ManifestAuthenticationRequired));
    let error = prune(&unkeyed, &KeepPolicy::default())
        .await
        .expect_err("prune refused without a key");
    assert!(matches!(error, BackupError::ManifestAuthenticationRequired));
    let error = delete_snapshot(
        &unkeyed,
        manifest.snapshot_id,
        &RepositoryLockOptions::default(),
    )
    .await
    .expect_err("delete refused without a key");
    assert!(matches!(error, BackupError::ManifestAuthenticationRequired));
    let error = compact_chain(&unkeyed, compact_request(manifest.snapshot_id))
        .await
        .expect_err("compaction refused without a key");
    assert!(matches!(error, BackupError::ManifestAuthenticationRequired));
    let error = verify_manifest_and_objects(&unkeyed, &manifest)
        .await
        .expect_err("verification refused without a key");
    assert!(matches!(error, BackupError::ManifestAuthenticationRequired));
    assert_eq!(
        unkeyed.list_blobs("").await.expect("list"),
        before,
        "nothing written"
    );
    assert_eq!(
        unkeyed.writes.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no write, not even a lock blob"
    );
}

/// Counts mutation attempts and actual non-lock writes, including failed-run cleanup.
struct WriteCountingRepository {
    inner: LocalBackupRepository,
    writes: Arc<std::sync::atomic::AtomicUsize>,
    payload_writes: Arc<std::sync::atomic::AtomicUsize>,
}

impl WriteCountingRepository {
    fn new(inner: LocalBackupRepository) -> Self {
        Self {
            inner,
            writes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            payload_writes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }
    fn wrote(&self, key: &str) {
        if key != graphql_orm_backup::REPOSITORY_LOCK_KEY {
            self.payload_writes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

#[async_trait]
impl BackupRepository for WriteCountingRepository {
    fn manifest_authenticator(&self) -> Option<&ManifestAuthenticator> {
        self.inner.manifest_authenticator()
    }
    async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError> {
        self.writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.wrote(key);
        self.inner.put_blob(key, body).await
    }
    async fn put_blob_if_absent(&self, key: &str, body: Bytes) -> Result<bool, BackupError> {
        self.writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let created = self.inner.put_blob_if_absent(key, body).await?;
        if created {
            self.wrote(key);
        }
        Ok(created)
    }
    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
        self.inner.get_blob(key).await
    }
    async fn blob_exists(&self, key: &str) -> Result<bool, BackupError> {
        self.inner.blob_exists(key).await
    }
    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError> {
        self.inner.list_blobs(prefix).await
    }
    async fn delete_blob_once(&self, key: &str) -> Result<(), BackupError> {
        self.writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.wrote(key);
        self.inner.delete_blob_once(key).await
    }
    async fn delete_blob(&self, key: &str) -> Result<(), BackupError> {
        self.writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.wrote(key);
        self.inner.delete_blob(key).await
    }
}

#[tokio::test]
async fn manifest_moved_to_another_snapshot_id_is_refused() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let manifest = backup_with_object(&repository, "media/a.txt").await;
    let other = Uuid::new_v4();
    let bytes = repository
        .get_blob(&snapshot_manifest_key(manifest.snapshot_id))
        .await
        .expect("manifest bytes");
    repository
        .put_blob(&snapshot_manifest_key(other), bytes)
        .await
        .expect("copy manifest");

    let error = load_manifest(&repository, other)
        .await
        .expect_err("manifest must match its path");
    assert!(matches!(
        error,
        BackupError::ManifestSnapshotMismatch { expected, actual, .. }
            if expected == other && actual == manifest.snapshot_id
    ));
    let error = restore_snapshot(
        &repository,
        &MockDatabase::default(),
        other,
        RestoreContext::dry_run(),
    )
    .await
    .expect_err("restore refuses the moved manifest");
    assert!(matches!(
        restore_source(error),
        BackupError::ManifestSnapshotMismatch { .. }
    ));
}

#[tokio::test]
async fn unknown_fields_and_unsupported_formats_are_refused() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let manifest = backup_with_object(&repository, "media/a.txt").await;
    let key = snapshot_manifest_key(manifest.snapshot_id);
    let original: Value =
        serde_json::from_slice(&repository.get_blob(&key).await.expect("manifest")).expect("json");

    let mut extra = original.clone();
    extra["unsealed_extension"] = Value::Bool(true);
    repository
        .put_blob(&key, Bytes::from(serde_json::to_vec(&extra).expect("json")))
        .await
        .expect("write");
    let error = load_manifest(&repository, manifest.snapshot_id)
        .await
        .expect_err("unknown field refused");
    assert!(matches!(error, BackupError::Serialization(_)));

    let mut future = manifest.clone();
    let future_version = graphql_orm_backup::BACKUP_FORMAT_VERSION + 1;
    future.format_version = future_version;
    future.authentication = None;
    write_raw_manifest(&repository, &mut future).await;
    let error = load_manifest(&repository, manifest.snapshot_id)
        .await
        .expect_err("unknown format refused");
    assert!(matches!(
        error,
        BackupError::UnsupportedManifestFormat { format_version, .. } if format_version == future_version
    ));

    // A current-format manifest with its seal stripped is refused even when
    // legacy manifests are trusted.
    let mut stripped = manifest.clone();
    stripped.authentication = None;
    write_raw_manifest(&repository, &mut stripped).await;
    let trusting = legacy_repository(&temp, LegacyManifestPolicy::TrustUnauthenticated);
    let error = load_manifest(&trusting, manifest.snapshot_id)
        .await
        .expect_err("unsealed current format refused");
    assert!(matches!(
        error,
        BackupError::ManifestAuthenticationFailed { .. }
    ));
}

#[tokio::test]
async fn object_content_key_outside_its_namespace_is_refused() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let manifest = backup_with_object(&repository, "media/a.txt").await;

    // Point the object at the table payload, with a matching digest, in a
    // legacy manifest that the host has chosen to trust.
    let mut forged = legacy(&manifest);
    forged.objects[0].content_key = forged.database.tables[0].content_key.clone();
    forged.objects[0].sha256_hex = forged.database.tables[0].sha256_hex.clone();
    write_raw_manifest(&repository, &mut forged).await;

    let trusting = legacy_repository(&temp, LegacyManifestPolicy::TrustUnauthenticated);
    let error = load_manifest(&trusting, manifest.snapshot_id)
        .await
        .expect_err("content key must stay in its namespace");
    assert!(matches!(error, BackupError::InvalidManifestEntry { .. }));

    let sink = RecordingSink::default();
    let error = restore_objects(&trusting, &forged, &sink)
        .await
        .expect_err("restore must refuse an out-of-namespace content key");
    assert!(matches!(error, BackupError::InvalidManifestEntry { .. }));
    assert!(sink.restored().is_empty());

    // The host never seals such a manifest either.
    let mut current = manifest.clone();
    current.objects[0].content_key = manifest.database.tables[0].content_key.clone();
    let error = authenticate_manifest(&authenticator(), &mut current)
        .expect_err("invalid entries are not sealed");
    assert!(matches!(error, BackupError::InvalidManifestEntry { .. }));
}

#[tokio::test]
async fn table_content_key_outside_its_snapshot_is_refused() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let manifest = backup_with_object(&repository, "media/a.txt").await;
    let other = backup_with_object(&repository, "media/b.txt").await;
    let trusting = legacy_repository(&temp, LegacyManifestPolicy::TrustUnauthenticated);

    for content_key in [
        "locks/../outside.jsonl.zst".to_string(),
        other.database.tables[0].content_key.clone(),
    ] {
        let mut forged = legacy(&manifest);
        forged.database.tables[0].content_key = content_key;
        write_raw_manifest(&repository, &mut forged).await;
        let error = load_manifest(&trusting, manifest.snapshot_id)
            .await
            .expect_err("table payload key must belong to its snapshot");
        assert!(matches!(error, BackupError::InvalidManifestEntry { .. }));
    }

    let mut forged = legacy(&manifest);
    forged.database.tables[0].table_name = "../users".to_string();
    write_raw_manifest(&repository, &mut forged).await;
    let error = load_manifest(&trusting, manifest.snapshot_id)
        .await
        .expect_err("table name must be one key segment");
    assert!(matches!(error, BackupError::InvalidManifestEntry { .. }));
}

#[tokio::test]
async fn restore_target_outside_the_allowed_keys_is_refused() {
    let temp = TempDir::new().expect("temp dir");
    let live_root = TempDir::new().expect("live store");
    let repository = repository(&temp);
    let live: Arc<dyn BlobStore> = Arc::new(LocalStorageBackend::new(live_root.path()));
    seed(&live, "config/settings.json", b"live").await;

    let manifest = backup_with_object(&repository, "config/settings.json").await;
    let sink = BlobStoreRestoreObjectSink::new(
        Arc::clone(&live),
        RestoreKeyScope::prefixes(["media"]).expect("scope"),
    );
    let error = restore_objects(&repository, &manifest, &sink)
        .await
        .expect_err("restore must not write outside the allowed namespace");
    assert!(matches!(
        error,
        BackupError::RestoreTargetRejected { storage_key, .. } if storage_key == "config/settings.json"
    ));
    assert_eq!(read(&live, "config/settings.json").await, "live");

    // A prefix matches whole segments only.
    let manifest = backup_with_object(&repository, "media-other/x").await;
    restore_objects(&repository, &manifest, &sink)
        .await
        .expect_err("sibling prefix is outside the scope");
    assert!(!live.blob_exists("media-other/x").await.expect("exists"));

    // Traversal is refused for every sink, before any object is restored.
    let mut forged = legacy(&backup_with_object(&repository, "media/a.txt").await);
    forged.objects[0].storage_key = "media/../config/settings.json".to_string();
    write_raw_manifest(&repository, &mut forged).await;
    let trusting = legacy_repository(&temp, LegacyManifestPolicy::TrustUnauthenticated);
    let recording = RecordingSink::default();
    let error = restore_objects(&trusting, &forged, &recording)
        .await
        .expect_err("traversal refused");
    assert!(matches!(error, BackupError::RestoreTargetRejected { .. }));
    assert!(recording.restored().is_empty());

    // Inside the scope, restore creates objects and never overwrites them.
    let manifest = backup_with_object(&repository, "media/a.txt").await;
    restore_objects(&repository, &manifest, &sink)
        .await
        .expect("restore into scope");
    assert_eq!(read(&live, "media/a.txt").await, "object bytes");
    restore_objects(&repository, &manifest, &sink)
        .await
        .expect("identical content is left in place");
    seed(&live, "media/b.txt", b"different live bytes").await;
    let manifest = backup_with_object(&repository, "media/b.txt").await;
    let error = restore_objects(&repository, &manifest, &sink)
        .await
        .expect_err("different content is never overwritten");
    assert!(matches!(
        error,
        BackupError::RestoreObjectConflict { storage_key } if storage_key == "media/b.txt"
    ));
    assert_eq!(read(&live, "media/b.txt").await, "different live bytes");
}

#[tokio::test]
async fn backup_refuses_object_keys_restore_would_reject() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let error = try_backup(&repository, "../outside")
        .await
        .expect_err("invalid storage key refused at backup time");
    assert!(matches!(error, BackupError::RestoreTargetRejected { .. }));
    assert!(
        repository
            .list_blobs("snapshots")
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test]
async fn legacy_manifests_are_refused_unless_the_host_opts_in() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let sealed = backup_with_object(&repository, "media/a.txt").await;
    let mut legacy_manifest = legacy(&sealed);
    write_raw_manifest(&repository, &mut legacy_manifest).await;
    let snapshot_id = sealed.snapshot_id;
    let restore = |repository: LocalBackupRepository| async move {
        restore_snapshot(
            &repository,
            &MockDatabase::default(),
            snapshot_id,
            RestoreContext::dry_run(),
        )
        .await
        .map_err(restore_source)
    };

    // Default: refused.
    let error = restore(repository.clone())
        .await
        .expect_err("legacy refused by default");
    assert!(matches!(
        error,
        BackupError::LegacyManifestRefused { snapshot_id: id } if id == snapshot_id
    ));

    // An independently retained checksum authenticates historical format 1.
    let pinned = BTreeMap::from([(snapshot_id, legacy_manifest.checksum.clone())]);
    restore(legacy_repository(
        &temp,
        LegacyManifestPolicy::TrustPinned(pinned.clone()),
    ))
    .await
    .expect("host checksum pin authenticates format 1");
    for policy in [
        LegacyManifestPolicy::TrustPinned(BTreeMap::new()),
        LegacyManifestPolicy::AcceptSealedLegacy,
    ] {
        let error = restore(legacy_repository(&temp, policy))
            .await
            .expect_err("format 1 needs its own host-held checksum pin");
        assert!(matches!(error, BackupError::LegacyManifestRefused { .. }));
    }
    let mut altered = legacy_manifest.clone();
    altered.app_version = "altered".to_string();
    write_raw_manifest(&repository, &mut altered).await;
    let error = restore(legacy_repository(
        &temp,
        LegacyManifestPolicy::TrustPinned(pinned),
    ))
    .await
    .expect_err("altered legacy manifest no longer matches its pin");
    assert!(matches!(error, BackupError::LegacyManifestRefused { .. }));

    // Explicit trust: accepted, still subject to entry checks.
    restore(legacy_repository(
        &temp,
        LegacyManifestPolicy::TrustUnauthenticated,
    ))
    .await
    .expect("trusted legacy manifest restores");
}

#[tokio::test]
async fn legacy_manifest_written_by_earlier_releases_is_readable_when_trusted() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let sealed = backup_with_object(&repository, "media/a.txt").await;
    // Earlier releases serialized exactly these fields, with format version 1
    // and no seal.
    let mut fixture: Value = serde_json::to_value(&sealed).expect("json");
    let object = fixture.as_object_mut().expect("object");
    object.remove("authentication");
    object.insert("format_version".to_string(), Value::from(1));
    let mut parsed: BackupSnapshotManifest =
        serde_json::from_value(fixture.clone()).expect("legacy shape parses");
    set_manifest_checksum(&mut parsed).expect("checksum");
    fixture["checksum"] = Value::String(parsed.checksum.clone());
    repository
        .put_blob(
            &snapshot_manifest_key(sealed.snapshot_id),
            Bytes::from(serde_json::to_vec_pretty(&fixture).expect("json")),
        )
        .await
        .expect("write fixture");

    let trusting = legacy_repository(&temp, LegacyManifestPolicy::TrustUnauthenticated);
    let loaded = load_manifest(&trusting, sealed.snapshot_id)
        .await
        .expect("legacy manifest loads");
    assert_eq!(loaded.format_version, LEGACY_BACKUP_FORMAT_VERSION);
    assert!(loaded.authentication.is_none());
}

#[tokio::test]
async fn prune_refuses_a_forged_manifest_before_deleting_anything() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let manifest = backup_with_object(&repository, "media/a.txt").await;

    // A forged "newest" snapshot would otherwise expire the real one.
    let mut forged = manifest.clone();
    forged.snapshot_id = Uuid::new_v4();
    forged.created_at += 1_000;
    forged.database.tables.clear();
    forged.objects.clear();
    write_raw_manifest(&repository, &mut forged).await;
    let before = repository.list_blobs("").await.expect("list");

    let error = prune(&repository, &KeepPolicy::default())
        .await
        .expect_err("forged manifest blocks pruning");
    assert!(matches!(
        error,
        BackupError::ManifestAuthenticationFailed { .. }
    ));
    let mut after = repository.list_blobs("").await.expect("list");
    after.retain(|key| !key.starts_with("locks/"));
    let mut before = before;
    before.retain(|key| !key.starts_with("locks/"));
    assert_eq!(after, before, "nothing deleted");
}

#[tokio::test]
async fn payload_swapped_after_verification_is_never_applied_or_compacted() {
    let temp = TempDir::new().expect("temp dir");
    let inner = repository(&temp);
    let manifest = backup_with_object(&inner, "media/a.txt").await;
    let table_key = manifest.database.tables[0].content_key.clone();
    let forged = serialize_rows(&[row("users", "99", "forged")]);

    // Genuine bytes on the first read (verification), forged bytes after.
    let swapping = SwappingRepository::new(inner.clone(), &table_key, forged.clone());
    let target = MockDatabase::default();
    let error = restore_snapshot(
        &swapping,
        &target,
        manifest.snapshot_id,
        RestoreContext::empty_database(),
    )
    .await
    .expect_err("swapped payload refused");
    let BackupError::RestoreStageFailed { progress, source } = error else {
        panic!("restore errors identify their stage");
    };
    assert_eq!(progress.phase, RestoreFailurePhase::Preparation);
    assert!(!progress.target_may_have_changed());
    assert!(matches!(
        *source,
        BackupError::ChecksumMismatch { ref key, .. } if *key == table_key
    ));
    assert!(target.restored().is_empty(), "forged rows never applied");
    assert!(swapping.reads() >= 2, "verification and use both read");

    let swapping = SwappingRepository::new(inner.clone(), &table_key, forged);
    let request = compact_request(manifest.snapshot_id);
    let new_id = request.snapshot_id;
    let error = compact_chain(&swapping, request)
        .await
        .expect_err("swapped payload not compacted");
    assert!(matches!(
        error,
        BackupError::ChecksumMismatch { ref key, .. } if *key == table_key
    ));
    assert!(
        !inner
            .blob_exists(&snapshot_manifest_key(new_id))
            .await
            .expect("exists"),
        "no synthetic full sealed over forged rows"
    );
}

/// Change payloads take the same digest-checked path as table payloads: an
/// incremental snapshot whose change payload is swapped after verification
/// is never applied or compacted (#612).
#[tokio::test]
async fn change_payload_swapped_after_verification_is_never_applied_or_compacted() {
    let temp = TempDir::new().expect("temp dir");
    let inner = repository(&temp);
    let full = backup_with_object(&inner, "media/a.txt").await;
    let change = |name: &str| BackupChangeExport {
        table_name: "users".to_string(),
        primary_key: "2".to_string(),
        action: graphql_orm_backup::BackupChangeAction::Create,
        row: Some(row("users", "2", name)),
        changed_at: 1_775_174_450,
    };
    let source = MockDatabase {
        changes: vec![change("Grace")],
        ..MockDatabase::default()
    };
    let incremental = create_incremental_backup(
        &inner,
        &source,
        &MockObjectIndex {
            object: entry_ref(&full.objects[0]),
            bytes: Bytes::from_static(b"object bytes"),
        },
        IncrementalBackupRequest {
            snapshot_id: Uuid::new_v4(),
            parent_snapshot_id: full.snapshot_id,
            created_at: full.created_at + 1,
            app_id: full.app_id.clone(),
            app_version: full.app_version.clone(),
        },
    )
    .await
    .expect("incremental backup")
    .manifest;
    let change_key = incremental
        .database
        .changes
        .first()
        .expect("the incremental snapshot stores a change payload")
        .content_key
        .clone();

    // The genuine chain restores the change.
    let target = MockDatabase::default();
    restore_snapshot(
        &inner,
        &target,
        incremental.snapshot_id,
        RestoreContext::empty_database(),
    )
    .await
    .expect("genuine chain restores");
    assert_eq!(target.restored_changes().len(), 1);

    let mut forged = Vec::new();
    serde_json::to_writer(&mut forged, &change("forged")).expect("change json");
    forged.push(b'\n');
    let forged = Bytes::from(compress_payload(&forged).expect("compress"));

    // Genuine bytes on the first read (verification), forged bytes after.
    let swapping = SwappingRepository::new(inner.clone(), &change_key, forged.clone());
    let target = MockDatabase::default();
    let error = restore_snapshot(
        &swapping,
        &target,
        incremental.snapshot_id,
        RestoreContext::empty_database(),
    )
    .await
    .expect_err("swapped change payload refused");
    let BackupError::RestoreStageFailed { progress, source } = error else {
        panic!("restore errors identify their stage");
    };
    // Chain restore applies stage by stage: the genuine full stage was
    // applied, and the change stage failed while it was prepared.
    assert_eq!(progress.phase, RestoreFailurePhase::Preparation);
    assert_eq!(progress.failed_manifest_id, Some(incremental.snapshot_id));
    assert_eq!(progress.completed_manifest_ids, [full.snapshot_id]);
    assert!(matches!(
        *source,
        BackupError::ChecksumMismatch { ref key, .. } if *key == change_key
    ));
    assert_eq!(target.restored()[0].rows[0].values["name"], "Ada");
    assert!(
        target.restored_changes().is_empty(),
        "forged change never applied"
    );
    assert!(swapping.reads() >= 2, "verification and use both read");

    let swapping = SwappingRepository::new(inner.clone(), &change_key, forged);
    let request = compact_request(incremental.snapshot_id);
    let new_id = request.snapshot_id;
    let error = compact_chain(&swapping, request)
        .await
        .expect_err("swapped change payload not compacted");
    assert!(matches!(
        error,
        BackupError::ChecksumMismatch { ref key, .. } if *key == change_key
    ));
    assert!(
        !inner
            .blob_exists(&snapshot_manifest_key(new_id))
            .await
            .expect("exists"),
        "no synthetic full sealed over a forged change"
    );
}

/// Reads at most `chunks` chunks of each object stream, then reports success
/// without reaching the end of the stream.
struct EarlyStopSink {
    chunks: usize,
    restored: Mutex<Vec<BackupObjectRef>>,
}

#[async_trait]
impl graphql_orm_backup::RestoreObjectSink for EarlyStopSink {
    async fn restore_object(
        &self,
        _object: BackupObjectRef,
        _bytes: Bytes,
    ) -> Result<(), BackupError> {
        unreachable!("the streaming method is overridden")
    }

    async fn restore_object_stream(
        &self,
        object: BackupObjectRef,
        stream: StorageByteStream,
    ) -> Result<(), BackupError> {
        use futures::StreamExt;
        let mut stream = stream.into_inner();
        for _ in 0..self.chunks {
            if stream.next().await.is_none() {
                break;
            }
        }
        self.restored.lock().expect("restored").push(object);
        Ok(())
    }
}

/// A sink that returns success before reading an object's stream to its end
/// never saw the digest check, so restore fails instead of reporting the
/// object restored (#612).
#[tokio::test]
async fn a_sink_that_stops_reading_early_fails_the_restore() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let manifest = backup_with_object(&repository, "media/a.txt").await;
    let object_key = manifest.objects[0].content_key.clone();
    for chunks in [0, 1] {
        let sink = EarlyStopSink {
            chunks,
            restored: Mutex::default(),
        };
        let error = restore_objects(&repository, &manifest, &sink)
            .await
            .expect_err("an unverified object must fail the restore");
        assert!(
            matches!(
                error,
                BackupError::ChecksumMismatch { ref key, ref actual, .. }
                    if *key == object_key && actual.starts_with("unverified")
            ),
            "{chunks} chunks: {error:?}"
        );
        assert_eq!(sink.restored.lock().expect("restored").len(), 1);
    }
}

#[tokio::test]
async fn object_swapped_after_verification_never_reaches_the_live_store() {
    let temp = TempDir::new().expect("temp dir");
    let live_root = TempDir::new().expect("live store");
    let inner = repository(&temp);
    let manifest = backup_with_object(&inner, "media/a.txt").await;
    let object_key = manifest.objects[0].content_key.clone();
    let swapping = SwappingRepository::new(
        inner.clone(),
        &object_key,
        Bytes::from_static(b"forged object"),
    );

    let live: Arc<dyn BlobStore> = Arc::new(LocalStorageBackend::new(live_root.path()));
    let sink = BlobStoreRestoreObjectSink::new(
        Arc::clone(&live),
        RestoreKeyScope::prefixes(["media"]).expect("scope"),
    );
    let error = restore_objects(&swapping, &manifest, &sink)
        .await
        .expect_err("swapped object refused");
    assert!(matches!(
        error,
        BackupError::ChecksumMismatch { ref key, .. } if *key == object_key
    ));
    assert!(
        !live.blob_exists("media/a.txt").await.expect("exists"),
        "conditional create aborted before commit"
    );
    assert!(
        walk(live_root.path()).is_empty(),
        "no temporary or partial file remains"
    );

    // A buffering sink never receives the forged bytes either.
    let swapping = SwappingRepository::new(inner, &object_key, Bytes::from_static(b"forged"));
    let recording = RecordingSink::default();
    restore_objects(&swapping, &manifest, &recording)
        .await
        .expect_err("swapped object refused");
    assert!(recording.restored().is_empty());
}

#[tokio::test]
async fn forged_manifests_are_refused_by_compaction_and_deletion() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let manifest = backup_with_object(&repository, "media/a.txt").await;
    let mut forged = manifest.clone();
    forged.app_version = "forged".to_string();
    write_raw_manifest(&repository, &mut forged).await;
    let before = without_locks(repository.list_blobs("").await.expect("list"));

    verify_manifest_and_objects(&repository, &forged)
        .await
        .expect_err("verification refuses a forged manifest");
    let request = compact_request(manifest.snapshot_id);
    let error = compact_chain(&repository, request)
        .await
        .expect_err("compaction refuses a forged source");
    assert!(matches!(
        error,
        BackupError::ManifestAuthenticationFailed { .. }
    ));
    let error = delete_snapshot(
        &repository,
        manifest.snapshot_id,
        &RepositoryLockOptions::default(),
    )
    .await
    .expect_err("deletion refuses a forged manifest");
    assert!(matches!(
        error,
        BackupError::ManifestAuthenticationFailed { .. }
    ));
    assert_eq!(
        without_locks(repository.list_blobs("").await.expect("list")),
        before,
        "nothing written or deleted"
    );
}

#[tokio::test]
async fn forged_or_unsealed_parent_breaks_an_incremental_chain() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let full = backup_with_object(&repository, "media/a.txt").await;
    let incremental_id = Uuid::new_v4();
    create_incremental_backup(
        &repository,
        &MockDatabase::default(),
        &MockObjectIndex {
            object: full.objects.first().map(entry_ref).expect("object"),
            bytes: Bytes::from_static(b"object bytes"),
        },
        IncrementalBackupRequest {
            snapshot_id: incremental_id,
            parent_snapshot_id: full.snapshot_id,
            created_at: 1_775_174_500,
            app_id: "example-app".to_string(),
            app_version: "0.1.0".to_string(),
        },
    )
    .await
    .expect("incremental backup");
    let restore = || {
        let repository = repository.clone();
        async move {
            restore_snapshot(
                &repository,
                &MockDatabase::default(),
                incremental_id,
                RestoreContext::dry_run(),
            )
            .await
            .map_err(restore_source)
        }
    };
    restore().await.expect("sealed chain restores");

    let mut forged = full.clone();
    forged.created_at += 1;
    write_raw_manifest(&repository, &mut forged).await;
    let error = restore().await.expect_err("forged parent refused");
    assert!(matches!(
        error,
        BackupError::ManifestAuthenticationFailed { ref key, .. }
            if *key == snapshot_manifest_key(full.snapshot_id)
    ));

    let mut unsealed = legacy(&full);
    write_raw_manifest(&repository, &mut unsealed).await;
    let error = restore().await.expect_err("unsealed parent refused");
    assert!(matches!(
        error,
        BackupError::LegacyManifestRefused { snapshot_id } if snapshot_id == full.snapshot_id
    ));
}

#[tokio::test]
async fn compaction_refuses_an_existing_snapshot_id() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let first = backup_with_object(&repository, "media/a.txt").await;
    let second = backup_with_object(&repository, "media/b.txt").await;
    let manifest_key = snapshot_manifest_key(second.snapshot_id);
    let before = repository.get_blob(&manifest_key).await.expect("manifest");

    let mut request = compact_request(first.snapshot_id);
    request.snapshot_id = second.snapshot_id;
    let error = compact_chain(&repository, request)
        .await
        .expect_err("existing snapshot id refused");
    assert!(matches!(
        error,
        BackupError::SnapshotExists { snapshot_id } if snapshot_id == second.snapshot_id
    ));
    assert_eq!(
        repository.get_blob(&manifest_key).await.expect("manifest"),
        before,
        "existing snapshot unchanged"
    );
}

fn compact_request(source_snapshot_id: Uuid) -> CompactChainRequest {
    CompactChainRequest {
        snapshot_id: Uuid::new_v4(),
        source_snapshot_id,
        created_at: 1_775_174_900,
        app_id: "example-app".to_string(),
        app_version: "0.1.0".to_string(),
    }
}

fn entry_ref(entry: &graphql_orm_backup::ObjectBackupEntry) -> BackupObjectRef {
    BackupObjectRef {
        object_id: entry.object_id,
        storage_key: entry.storage_key.clone(),
        sha256_hex: entry.sha256_hex.clone(),
        size_bytes: entry.size_bytes,
        mime_type: entry.mime_type.clone(),
    }
}

fn without_locks(mut keys: Vec<String>) -> Vec<String> {
    keys.retain(|key| !key.starts_with("locks/"));
    keys
}

fn walk(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("read dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}

/// Serves genuine bytes for one key on its first read and `forged` bytes on
/// every later read, as a repository writer or endpoint could.
#[derive(Clone)]
struct SwappingRepository {
    inner: LocalBackupRepository,
    key: String,
    forged: Bytes,
    reads: Arc<std::sync::atomic::AtomicUsize>,
}

impl SwappingRepository {
    fn new(inner: LocalBackupRepository, key: &str, forged: Bytes) -> Self {
        Self {
            inner,
            key: key.to_string(),
            forged,
            reads: Arc::default(),
        }
    }

    fn reads(&self) -> usize {
        self.reads.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl BackupRepository for SwappingRepository {
    fn manifest_authenticator(&self) -> Option<&ManifestAuthenticator> {
        self.inner.manifest_authenticator()
    }
    async fn get_blob_stream(&self, key: &str) -> Result<StorageByteStream, BackupError> {
        if key == self.key && self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
            return Ok(StorageByteStream::from_bytes(self.forged.clone()));
        }
        self.inner.get_blob_stream(key).await
    }
    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
        collect_backup_stream(self.get_blob_stream(key).await?, BackupByteLimit::Unbounded).await
    }
    async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError> {
        self.inner.put_blob(key, body).await
    }
    async fn put_blob_if_absent(&self, key: &str, body: Bytes) -> Result<bool, BackupError> {
        self.inner.put_blob_if_absent(key, body).await
    }
    async fn blob_exists(&self, key: &str) -> Result<bool, BackupError> {
        self.inner.blob_exists(key).await
    }
    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError> {
        self.inner.list_blobs(prefix).await
    }
    async fn delete_blob_once(&self, key: &str) -> Result<(), BackupError> {
        self.inner.delete_blob_once(key).await
    }
    async fn delete_blob(&self, key: &str) -> Result<(), BackupError> {
        self.inner.delete_blob(key).await
    }
}

/// Downgrades a sealed manifest to the legacy unauthenticated format.
fn legacy(manifest: &BackupSnapshotManifest) -> BackupSnapshotManifest {
    let mut legacy = manifest.clone();
    legacy.format_version = LEGACY_BACKUP_FORMAT_VERSION;
    legacy.authentication = None;
    legacy
}

async fn seed(store: &Arc<dyn BlobStore>, key: &str, bytes: &'static [u8]) {
    store
        .put_blob(
            key,
            StorageByteStream::from_bytes(Bytes::from_static(bytes)),
            BlobPutOptions::default(),
        )
        .await
        .expect("seed live key");
}

async fn read(store: &Arc<dyn BlobStore>, key: &str) -> String {
    let body = store.get_blob(key).await.expect("live key").body;
    let bytes = collect_backup_stream(body, BackupByteLimit::Unbounded)
        .await
        .expect("read live key");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

async fn backup_with_object(
    repository: &LocalBackupRepository,
    storage_key: &str,
) -> BackupSnapshotManifest {
    try_backup(repository, storage_key)
        .await
        .expect("create full backup")
}

async fn try_backup(
    repository: &dyn BackupRepository,
    storage_key: &str,
) -> Result<BackupSnapshotManifest, BackupError> {
    let bytes = Bytes::from_static(b"object bytes");
    let objects = MockObjectIndex {
        object: BackupObjectRef {
            object_id: Uuid::new_v4(),
            storage_key: storage_key.to_string(),
            sha256_hex: bytes_sha256_hex(&bytes),
            size_bytes: bytes.len() as u64,
            mime_type: None,
        },
        bytes,
    };
    let database = MockDatabase {
        tables: vec![BackupTableExport {
            table_name: "users".to_string(),
            rows: vec![row("users", "1", "Ada")],
        }],
        ..MockDatabase::default()
    };
    create_full_backup(
        repository,
        &database,
        &objects,
        FullBackupRequest {
            snapshot_id: Uuid::new_v4(),
            created_at: 1_775_174_400,
            app_id: "example-app".to_string(),
            app_version: "0.1.0".to_string(),
        },
    )
    .await
    .map(|result| result.manifest)
}

/// Writes a manifest exactly as a repository writer without the host key can.
async fn write_raw_manifest(
    repository: &LocalBackupRepository,
    manifest: &mut BackupSnapshotManifest,
) {
    set_manifest_checksum(manifest).expect("checksum");
    repository
        .put_blob(
            &snapshot_manifest_key(manifest.snapshot_id),
            Bytes::from(serde_json::to_vec(manifest).expect("json")),
        )
        .await
        .expect("write manifest");
}

fn serialize_rows(rows: &[BackupRow]) -> Bytes {
    let mut jsonl = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut jsonl, row).expect("row json");
        jsonl.push(b'\n');
    }
    Bytes::from(compress_payload(&jsonl).expect("compress"))
}

fn row(table_name: &str, primary_key: &str, name: &str) -> BackupRow {
    let mut values = Map::new();
    values.insert("name".to_string(), Value::String(name.to_string()));
    BackupRow {
        table_name: table_name.to_string(),
        primary_key: primary_key.to_string(),
        row_hash: bytes_sha256_hex(primary_key.as_bytes()),
        values,
    }
}

struct MockObjectIndex {
    object: BackupObjectRef,
    bytes: Bytes,
}

#[async_trait]
impl BackupObjectIndex for MockObjectIndex {
    async fn objects_from_incremental_export(
        &self,
        parent_snapshot_id: Uuid,
        _changes: &[graphql_orm_backup::BackupChangeExport],
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        self.list_objects_for_incremental_backup(parent_snapshot_id)
            .await
    }

    async fn objects_from_full_export(
        &self,
        _: &[graphql_orm_backup::BackupTableExport],
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        self.list_objects_for_full_backup().await
    }

    async fn list_objects_for_full_backup(&self) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(vec![self.object.clone()])
    }

    async fn list_objects_for_incremental_backup(
        &self,
        _since_snapshot_id: Uuid,
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        Ok(Vec::new())
    }

    async fn load_object(&self, _object: &BackupObjectRef) -> Result<Bytes, BackupError> {
        Ok(self.bytes.clone())
    }
}

#[derive(Clone, Default)]
struct MockDatabase {
    tables: Vec<BackupTableExport>,
    changes: Vec<BackupChangeExport>,
    restored: Arc<Mutex<Vec<BackupTableExport>>>,
    restored_changes: Arc<Mutex<Vec<BackupChangeExport>>>,
}

impl MockDatabase {
    fn restored(&self) -> Vec<BackupTableExport> {
        self.restored.lock().expect("restored").clone()
    }

    fn restored_changes(&self) -> Vec<BackupChangeExport> {
        self.restored_changes.lock().expect("restored").clone()
    }
}

#[async_trait]
impl GraphqlOrmBackupAdapter for MockDatabase {
    async fn schema_snapshot(&self) -> Result<GraphqlOrmBackupSchema, BackupError> {
        Ok(GraphqlOrmBackupSchema {
            backend: "sqlite".to_string(),
            migration_version: "1".to_string(),
            schema_hash: "schema-hash".to_string(),
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
        export: Vec<BackupTableExport>,
        _context: RestoreContext,
    ) -> Result<(), BackupError> {
        self.restored.lock().expect("restored").extend(export);
        Ok(())
    }

    async fn restore_incremental(
        &self,
        changes: Vec<BackupChangeExport>,
        _context: RestoreContext,
    ) -> Result<(), BackupError> {
        self.restored_changes
            .lock()
            .expect("restored")
            .extend(changes);
        Ok(())
    }
}

#[derive(Default)]
struct RecordingSink {
    restored: Mutex<Vec<BackupObjectRef>>,
}

impl RecordingSink {
    fn restored(&self) -> Vec<BackupObjectRef> {
        self.restored.lock().expect("restored").clone()
    }
}

#[async_trait]
impl graphql_orm_backup::RestoreObjectSink for RecordingSink {
    async fn restore_object(
        &self,
        object: BackupObjectRef,
        _bytes: Bytes,
    ) -> Result<(), BackupError> {
        self.restored.lock().expect("restored").push(object);
        Ok(())
    }
}

#[tokio::test]
async fn copied_snapshot_is_refused_by_another_repository_with_the_same_key() {
    let source_dir = TempDir::new().unwrap();
    let target_dir = TempDir::new().unwrap();
    let source = repository(&source_dir);
    let target = repository(&target_dir);
    let manifest = backup_with_object(&source, "media/source.txt").await;
    // Initialize an independent destination with the same host key.
    backup_with_object(&target, "media/target.txt").await;
    for key in source.list_blobs("snapshots").await.unwrap() {
        target
            .put_blob(&key, source.get_blob(&key).await.unwrap())
            .await
            .unwrap();
    }
    for key in source.list_blobs("objects").await.unwrap() {
        target
            .put_blob(&key, source.get_blob(&key).await.unwrap())
            .await
            .unwrap();
    }
    assert!(load_manifest(&target, manifest.snapshot_id).await.is_err());
    assert!(
        verify_manifest_and_objects(&target, &manifest)
            .await
            .is_err()
    );
    assert!(
        restore_snapshot(
            &target,
            &MockDatabase::default(),
            manifest.snapshot_id,
            RestoreContext::dry_run()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn pinned_format_two_backup_restores_without_an_identity_record() {
    let dir = TempDir::new().unwrap();
    let repo = repository(&dir);
    let mut manifest = backup_with_object(&repo, "media/legacy.txt").await;
    seal_format_two(&mut manifest);
    repo.put_blob(
        &snapshot_manifest_key(manifest.snapshot_id),
        Bytes::from(serde_json::to_vec(&manifest).unwrap()),
    )
    .await
    .unwrap();
    repo.delete_blob("repository-id").await.unwrap();
    let pinned = LocalBackupRepository::new(dir.path()).with_manifest_authenticator(
        authenticator().with_legacy_manifest_policy(LegacyManifestPolicy::TrustPinned(
            BTreeMap::from([(manifest.snapshot_id, manifest.checksum.clone())]),
        )),
    );
    load_manifest(&pinned, manifest.snapshot_id).await.unwrap();
    verify_manifest_and_objects(&pinned, &manifest)
        .await
        .unwrap();
    restore_snapshot(
        &pinned,
        &MockDatabase::default(),
        manifest.snapshot_id,
        RestoreContext::dry_run(),
    )
    .await
    .unwrap();
    assert!(
        !pinned
            .blob_exists(graphql_orm_backup::REPOSITORY_ID_KEY)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn copying_an_identity_record_cannot_change_the_hosts_repository_binding() {
    let source_dir = TempDir::new().unwrap();
    let target_dir = TempDir::new().unwrap();
    let source = repository(&source_dir);
    let target = repository(&target_dir);
    let manifest = backup_with_object(&source, "media/source.txt").await;
    backup_with_object(&target, "media/target.txt").await;
    for key in source.list_blobs("").await.unwrap() {
        target
            .put_blob(&key, source.get_blob(&key).await.unwrap())
            .await
            .unwrap();
    }
    assert!(load_manifest(&target, manifest.snapshot_id).await.is_err());
    assert!(
        verify_manifest_and_objects(&target, &manifest)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn identity_enrollment_refuses_an_existing_record_without_a_host_pin() {
    let dir = TempDir::new().unwrap();
    let repo = LocalBackupRepository::new(dir.path()).with_manifest_authenticator(authenticator());
    repo.put_blob(
        graphql_orm_backup::REPOSITORY_ID_KEY,
        Bytes::from(Uuid::new_v4().to_string()),
    )
    .await
    .unwrap();
    assert!(
        graphql_orm_backup::initialize_repository_identity(&repo)
            .await
            .is_err()
    );
}

struct ReplacingIdentity<'a>(&'a LocalBackupRepository);

#[async_trait]
impl BackupRepository for ReplacingIdentity<'_> {
    fn manifest_authenticator(&self) -> Option<&ManifestAuthenticator> {
        self.0.manifest_authenticator()
    }
    async fn put_blob_if_absent(&self, key: &str, body: Bytes) -> Result<bool, BackupError> {
        let created = self.0.put_blob_if_absent(key, body).await?;
        if created && key == graphql_orm_backup::REPOSITORY_ID_KEY {
            self.0
                .put_blob(key, Bytes::from(Uuid::from_u128(1).to_string()))
                .await?;
        }
        Ok(created)
    }
    async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError> {
        self.0.put_blob(key, body).await
    }
    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
        self.0.get_blob(key).await
    }
    async fn blob_exists(&self, key: &str) -> Result<bool, BackupError> {
        self.0.blob_exists(key).await
    }
    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError> {
        self.0.list_blobs(prefix).await
    }
    async fn delete_blob(&self, key: &str) -> Result<(), BackupError> {
        self.0.delete_blob(key).await
    }
}

#[tokio::test]
async fn identity_enrollment_refuses_a_record_replaced_during_creation() {
    let dir = TempDir::new().unwrap();
    let repo = LocalBackupRepository::new(dir.path()).with_manifest_authenticator(authenticator());
    assert!(
        graphql_orm_backup::initialize_repository_identity(&ReplacingIdentity(&repo))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn identity_enrollment_creates_once_then_reopens_with_the_host_pin() {
    let dir = TempDir::new().unwrap();
    let bootstrap =
        LocalBackupRepository::new(dir.path()).with_manifest_authenticator(authenticator());
    let identity = graphql_orm_backup::initialize_repository_identity(&bootstrap)
        .await
        .unwrap();
    let pinned = LocalBackupRepository::new(dir.path())
        .with_manifest_authenticator(authenticator().with_repository_identity(identity));
    assert_eq!(
        graphql_orm_backup::initialize_repository_identity(&pinned)
            .await
            .unwrap(),
        identity
    );
    let manifest = backup_with_object(&pinned, "media/enrolled.txt").await;
    assert_eq!(
        manifest.authentication.as_ref().unwrap().repository_id,
        Some(identity)
    );
    load_manifest(&pinned, manifest.snapshot_id).await.unwrap();
}

fn seal_format_two(manifest: &mut BackupSnapshotManifest) {
    use hmac::{Hmac, Mac};
    manifest.format_version = 2;
    manifest.authentication = None;
    manifest.checksum.clear();
    let canonical = serde_json::to_vec(&*manifest).unwrap();
    let mut mac_key = [0u8; 32];
    hkdf::Hkdf::<sha2::Sha256>::new(
        Some(b"graphql-orm-backup manifest authentication"),
        &[7; 32],
    )
    .expand(b"manifest hmac-sha256 key v1", &mut mac_key)
    .unwrap();
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&mac_key).unwrap();
    mac.update(b"graphql-orm-backup/manifest/hmac-sha256/v2\n");
    mac.update(&canonical);
    let tag: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    manifest.checksum = bytes_sha256_hex(&canonical);
    manifest.authentication = Some(
        serde_json::from_value(serde_json::json!({
            "algorithm": "HmacSha256", "key_id": authenticator().key_id(), "mac": tag,
        }))
        .unwrap(),
    );
}
