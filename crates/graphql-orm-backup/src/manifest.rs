use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::BackupError;

pub const BACKUP_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BackupKind {
    Full,
    Incremental,
    SyntheticFull,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BackupSnapshotManifest {
    pub format_version: u32,
    pub snapshot_id: Uuid,
    pub parent_snapshot_id: Option<Uuid>,
    pub created_at: i64,
    pub app_id: String,
    pub app_version: String,
    pub graphql_orm_schema_version: String,
    pub graphql_orm_schema_hash: String,
    pub database_backend: String,
    pub backup_kind: BackupKind,
    pub database: DatabaseBackupManifest,
    pub objects: Vec<ObjectBackupEntry>,
    pub tombstones: Vec<BackupTombstone>,
    pub checksum: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DatabaseBackupManifest {
    pub export_format: String,
    pub row_count: u64,
    pub table_count: u64,
    pub tables: Vec<TableBackupEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TableBackupEntry {
    pub table_name: String,
    pub row_count: u64,
    pub content_key: String,
    pub sha256_hex: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ObjectBackupEntry {
    pub object_id: Uuid,
    pub storage_key: String,
    pub content_key: String,
    pub sha256_hex: String,
    pub size_bytes: u64,
    pub mime_type: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BackupTombstone {
    pub table_name: Option<String>,
    pub primary_key: Option<String>,
    pub object_id: Option<Uuid>,
    pub deleted_at: i64,
}

pub fn manifest_checksum(manifest: &BackupSnapshotManifest) -> Result<String, BackupError> {
    let mut canonical = manifest.clone();
    canonical.checksum.clear();
    let bytes = serde_json::to_vec(&canonical)?;
    Ok(sha256_hex(&bytes))
}

pub fn set_manifest_checksum(manifest: &mut BackupSnapshotManifest) -> Result<(), BackupError> {
    manifest.checksum = manifest_checksum(manifest)?;
    Ok(())
}

pub fn verify_manifest_checksum(manifest: &BackupSnapshotManifest) -> Result<(), BackupError> {
    let actual = manifest_checksum(manifest)?;
    if actual == manifest.checksum {
        Ok(())
    } else {
        Err(BackupError::ChecksumMismatch {
            key: format!("snapshots/{}/manifest.json", manifest.snapshot_id),
            expected: manifest.checksum.clone(),
            actual,
        })
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
