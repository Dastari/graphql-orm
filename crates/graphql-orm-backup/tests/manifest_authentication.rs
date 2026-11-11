//! Manifest authentication, snapshot identity and restore-target constraints.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm_backup::{
    BackupChangeExport, BackupError, BackupObjectIndex, BackupObjectRef, BackupRepository,
    BackupRow, BackupSnapshotManifest, BackupTableExport, BlobStoreRestoreObjectSink,
    FullBackupRequest, GraphqlOrmBackupAdapter, GraphqlOrmBackupSchema, LocalBackupRepository,
    RestoreContext, bytes_sha256_hex, compress_payload, create_full_backup, load_manifest,
    restore_objects, restore_snapshot, set_manifest_checksum, snapshot_manifest_key,
};
use graphql_orm_storage::{BlobPutOptions, BlobStore, LocalStorageBackend, StorageByteStream};
use serde_json::{Map, Value};
use tempfile::TempDir;
use uuid::Uuid;

fn repository(temp: &TempDir) -> LocalBackupRepository {
    LocalBackupRepository::new(temp.path())
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
        RestoreContext::empty_database(),
    )
    .await
    .expect_err("forged manifest must be refused");
    assert!(target.restored().is_empty(), "nothing may be applied");
    assert!(format!("{error:?}").contains("Authentication"), "{error:?}");
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
    assert!(
        format!("{error:?}").contains("ManifestSnapshotMismatch"),
        "{error:?}"
    );
}

#[tokio::test]
async fn object_content_key_outside_its_namespace_is_refused() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let manifest = backup_with_object(&repository, "media/a.txt").await;

    // Point the object at the table payload, with a matching digest.
    let mut forged = manifest.clone();
    forged.objects[0].content_key = forged.database.tables[0].content_key.clone();
    forged.objects[0].sha256_hex = forged.database.tables[0].sha256_hex.clone();
    write_raw_manifest(&repository, &mut forged).await;

    let error = load_manifest(&repository, manifest.snapshot_id)
        .await
        .expect_err("content key must stay in its namespace");
    assert!(
        format!("{error:?}").contains("InvalidManifestEntry"),
        "{error:?}"
    );

    let sink = RecordingSink::default();
    restore_objects(&repository, &forged, &sink)
        .await
        .expect_err("restore must refuse an out-of-namespace content key");
    assert!(sink.restored().is_empty());
}

#[tokio::test]
async fn table_content_key_outside_its_snapshot_is_refused() {
    let temp = TempDir::new().expect("temp dir");
    let repository = repository(&temp);
    let manifest = backup_with_object(&repository, "media/a.txt").await;

    let mut forged = manifest.clone();
    forged.database.tables[0].content_key = "locks/../outside.jsonl.zst".to_string();
    write_raw_manifest(&repository, &mut forged).await;

    let error = load_manifest(&repository, manifest.snapshot_id)
        .await
        .expect_err("table payload key must belong to its snapshot");
    assert!(
        format!("{error:?}").contains("InvalidManifestEntry"),
        "{error:?}"
    );
}

#[tokio::test]
async fn restore_target_outside_the_allowed_keys_is_refused() {
    let temp = TempDir::new().expect("temp dir");
    let live_root = TempDir::new().expect("live store");
    let repository = repository(&temp);
    let live: Arc<dyn BlobStore> = Arc::new(LocalStorageBackend::new(live_root.path()));
    live.put_blob(
        "config/settings.json",
        StorageByteStream::from_bytes(Bytes::from_static(b"live")),
        BlobPutOptions::default(),
    )
    .await
    .expect("seed live key");

    let manifest = backup_with_object(&repository, "config/settings.json").await;
    let sink = BlobStoreRestoreObjectSink::new(Arc::clone(&live));
    restore_objects(&repository, &manifest, &sink)
        .await
        .expect_err("restore must not write outside the allowed namespace");

    let body = live
        .get_blob("config/settings.json")
        .await
        .expect("live key");
    let bytes = graphql_orm_backup::collect_backup_stream(
        body.body,
        graphql_orm_backup::BackupByteLimit::Unbounded,
    )
    .await
    .expect("read live key");
    assert_eq!(bytes, Bytes::from_static(b"live"), "live key unchanged");
}

async fn backup_with_object(
    repository: &LocalBackupRepository,
    storage_key: &str,
) -> BackupSnapshotManifest {
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
    .expect("create full backup")
    .manifest
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
