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

fn repository(temp: &TempDir) -> LocalBackupRepository {
    repository_with(temp, authenticator())
}

fn repository_with(temp: &TempDir, authenticator: ManifestAuthenticator) -> LocalBackupRepository {
    LocalBackupRepository::new(temp.path()).with_manifest_authenticator(authenticator)
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

    let unkeyed = LocalBackupRepository::new(temp.path());
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
    assert_eq!(
        unkeyed.list_blobs("").await.expect("list"),
        before,
        "nothing written"
    );
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
    future.format_version = 3;
    future.authentication = None;
    write_raw_manifest(&repository, &mut future).await;
    let error = load_manifest(&repository, manifest.snapshot_id)
        .await
        .expect_err("unknown format refused");
    assert!(matches!(
        error,
        BackupError::UnsupportedManifestFormat {
            format_version: 3,
            ..
        }
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

    // Pinned: accepted only with the recorded checksum.
    let pinned = BTreeMap::from([(snapshot_id, legacy_manifest.checksum.clone())]);
    restore(legacy_repository(
        &temp,
        LegacyManifestPolicy::TrustPinned(pinned.clone()),
    ))
    .await
    .expect("pinned legacy manifest restores");
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
    repository: &LocalBackupRepository,
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
    restored: Arc<Mutex<Vec<BackupTableExport>>>,
}

impl MockDatabase {
    fn restored(&self) -> Vec<BackupTableExport> {
        self.restored.lock().expect("restored").clone()
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
        Ok(Vec::new())
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
        _changes: Vec<BackupChangeExport>,
        _context: RestoreContext,
    ) -> Result<(), BackupError> {
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
