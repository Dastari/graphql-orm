use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm_backup::{
    BACKUP_FORMAT_VERSION, BackupCompression, BackupError, BackupKind, BackupRepository,
    BackupSnapshotManifest, DatabaseBackupManifest, load_manifest_chain, set_manifest_checksum,
    snapshot_manifest_key, validate_manifest_chain,
};
use uuid::Uuid;

#[tokio::test]
async fn load_manifest_chain_returns_single_full_snapshot() {
    let repository = RecordingRepository::default();
    let manifest = sample_manifest(full_id(), None, BackupKind::Full);
    write_manifest_blob(&repository, &manifest).await;

    let chain = load_manifest_chain(&repository, full_id())
        .await
        .expect("load chain");

    assert_eq!(chain, vec![manifest]);
}

#[tokio::test]
async fn load_manifest_chain_returns_full_then_incremental() {
    let repository = RecordingRepository::default();
    let full = sample_manifest(full_id(), None, BackupKind::Full);
    let incremental = sample_manifest(
        incremental_id(),
        Some(full.snapshot_id),
        BackupKind::Incremental,
    );
    write_manifest_blob(&repository, &full).await;
    write_manifest_blob(&repository, &incremental).await;

    let chain = load_manifest_chain(&repository, incremental_id())
        .await
        .expect("load chain");

    assert_eq!(chain, vec![full, incremental]);
}

#[tokio::test]
async fn load_manifest_chain_rejects_missing_parent() {
    let repository = RecordingRepository::default();
    let incremental = sample_manifest(incremental_id(), Some(full_id()), BackupKind::Incremental);
    write_manifest_blob(&repository, &incremental).await;

    let err = load_manifest_chain(&repository, incremental_id())
        .await
        .expect_err("missing parent rejected");

    assert!(matches!(err, BackupError::MissingBlob { .. }));
}

#[tokio::test]
async fn load_manifest_chain_rejects_checksum_mismatch() {
    let repository = RecordingRepository::default();
    let mut manifest = sample_manifest(full_id(), None, BackupKind::Full);
    manifest.checksum = "not-the-real-checksum".to_string();
    repository
        .put_blob(
            &snapshot_manifest_key(manifest.snapshot_id),
            Bytes::from(serde_json::to_vec_pretty(&manifest).expect("serialize manifest")),
        )
        .await
        .expect("write manifest");

    let err = load_manifest_chain(&repository, full_id())
        .await
        .expect_err("checksum mismatch rejected");

    assert!(matches!(err, BackupError::ChecksumMismatch { .. }));
}

#[test]
fn validate_manifest_chain_rejects_duplicate_snapshot_id() {
    let first = sample_manifest(full_id(), None, BackupKind::Full);
    let duplicate = sample_manifest(full_id(), Some(first.snapshot_id), BackupKind::Incremental);

    let err = validate_manifest_chain(&[first, duplicate]).expect_err("duplicate rejected");

    assert!(matches!(err, BackupError::InvalidManifestChain { .. }));
}

#[test]
fn validate_manifest_chain_rejects_chain_without_full_root() {
    let incremental = sample_manifest(incremental_id(), None, BackupKind::Incremental);

    let err = validate_manifest_chain(&[incremental]).expect_err("root kind rejected");

    assert!(matches!(err, BackupError::InvalidManifestChain { .. }));
}

#[test]
fn validate_manifest_chain_rejects_schema_hash_mismatch() {
    let full = sample_manifest(full_id(), None, BackupKind::Full);
    let mut incremental = sample_manifest(
        incremental_id(),
        Some(full.snapshot_id),
        BackupKind::Incremental,
    );
    incremental.graphql_orm_schema_hash = "different-schema".to_string();
    set_manifest_checksum(&mut incremental).expect("reset checksum");

    let err = validate_manifest_chain(&[full, incremental]).expect_err("schema mismatch rejected");

    assert!(matches!(err, BackupError::InvalidManifestChain { .. }));
}

#[test]
fn validate_manifest_chain_rejects_database_backend_mismatch() {
    let full = sample_manifest(full_id(), None, BackupKind::Full);
    let mut incremental = sample_manifest(
        incremental_id(),
        Some(full.snapshot_id),
        BackupKind::Incremental,
    );
    incremental.database_backend = "postgres".to_string();
    set_manifest_checksum(&mut incremental).expect("reset checksum");

    let err = validate_manifest_chain(&[full, incremental]).expect_err("backend mismatch rejected");

    assert!(matches!(err, BackupError::InvalidManifestChain { .. }));
}

#[test]
fn validate_manifest_chain_rejects_app_id_mismatch() {
    let full = sample_manifest(full_id(), None, BackupKind::Full);
    let mut incremental = sample_manifest(
        incremental_id(),
        Some(full.snapshot_id),
        BackupKind::Incremental,
    );
    incremental.app_id = "other-app".to_string();
    set_manifest_checksum(&mut incremental).expect("reset checksum");

    let err = validate_manifest_chain(&[full, incremental]).expect_err("app mismatch rejected");

    assert!(matches!(err, BackupError::InvalidManifestChain { .. }));
}

async fn write_manifest_blob(repository: &RecordingRepository, manifest: &BackupSnapshotManifest) {
    repository
        .put_blob(
            &snapshot_manifest_key(manifest.snapshot_id),
            Bytes::from(serde_json::to_vec_pretty(manifest).expect("serialize manifest")),
        )
        .await
        .expect("write manifest");
}

fn sample_manifest(
    snapshot_id: Uuid,
    parent_snapshot_id: Option<Uuid>,
    backup_kind: BackupKind,
) -> BackupSnapshotManifest {
    let mut manifest = BackupSnapshotManifest {
        format_version: BACKUP_FORMAT_VERSION,
        snapshot_id,
        parent_snapshot_id,
        created_at: 1_775_174_400,
        app_id: "example-app".to_string(),
        app_version: "0.1.0".to_string(),
        graphql_orm_schema_version: "20260514000000".to_string(),
        graphql_orm_schema_hash: "schema-hash".to_string(),
        database_backend: "sqlite".to_string(),
        backup_kind,
        database: DatabaseBackupManifest {
            export_format: "jsonl".to_string(),
            compression: BackupCompression::Zstd,
            row_count: 0,
            table_count: 0,
            tables: Vec::new(),
        },
        objects: Vec::new(),
        tombstones: Vec::new(),
        checksum: String::new(),
    };
    set_manifest_checksum(&mut manifest).expect("set checksum");
    manifest
}

fn full_id() -> Uuid {
    Uuid::parse_str("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa").expect("valid uuid")
}

fn incremental_id() -> Uuid {
    Uuid::parse_str("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb").expect("valid uuid")
}

#[derive(Clone, Default)]
struct RecordingRepository {
    blobs: Arc<Mutex<HashMap<String, Bytes>>>,
}

#[async_trait]
impl BackupRepository for RecordingRepository {
    async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError> {
        self.blobs
            .lock()
            .expect("blobs lock")
            .insert(key.to_string(), body);
        Ok(())
    }

    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
        self.blobs
            .lock()
            .expect("blobs lock")
            .get(key)
            .cloned()
            .ok_or_else(|| BackupError::MissingBlob {
                key: key.to_string(),
            })
    }

    async fn blob_exists(&self, key: &str) -> Result<bool, BackupError> {
        Ok(self.blobs.lock().expect("blobs lock").contains_key(key))
    }

    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError> {
        let mut keys = self
            .blobs
            .lock()
            .expect("blobs lock")
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        Ok(keys)
    }

    async fn delete_blob(&self, key: &str) -> Result<(), BackupError> {
        self.blobs.lock().expect("blobs lock").remove(key);
        Ok(())
    }
}
