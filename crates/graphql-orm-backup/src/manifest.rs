use std::collections::HashSet;

use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{BackupError, BackupRepository};

pub const BACKUP_FORMAT_VERSION: u32 = 1;

#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BackupKind {
    Full,
    Incremental,
    SyntheticFull,
}

#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BackupCompression {
    #[default]
    None,
    Zstd,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BackupSnapshotManifest {
    pub format_version: u32,
    pub snapshot_id: Uuid,
    pub parent_snapshot_id: Option<Uuid>,
    /// Snapshot creation time as UTC Unix seconds.
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
    #[serde(default)]
    pub compression: BackupCompression,
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
    /// Deletion time as UTC Unix seconds.
    pub deleted_at: i64,
}

/// Computes the manifest checksum with the manifest's checksum field cleared.
///
/// # Errors
///
/// Returns [`BackupError`] if the canonical checksum view cannot be serialized.
pub fn manifest_checksum(manifest: &BackupSnapshotManifest) -> Result<String, BackupError> {
    let canonical = ChecksumManifestView::from(manifest);
    let bytes = serde_json::to_vec(&canonical)?;
    Ok(sha256_hex(&bytes))
}

/// Sets the checksum field on a manifest.
///
/// # Errors
///
/// Returns [`BackupError`] if the manifest checksum cannot be computed.
pub fn set_manifest_checksum(manifest: &mut BackupSnapshotManifest) -> Result<(), BackupError> {
    manifest.checksum = manifest_checksum(manifest)?;
    Ok(())
}

/// Verifies a manifest checksum.
///
/// # Errors
///
/// Returns [`BackupError::ChecksumMismatch`] if the checksum does not match, or
/// another [`BackupError`] if the canonical checksum cannot be computed.
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

/// Loads and verifies a single snapshot manifest.
///
/// # Errors
///
/// Returns [`BackupError`] if the manifest blob is missing, cannot be
/// deserialized, or fails checksum verification.
pub async fn load_manifest(
    repository: &dyn BackupRepository,
    snapshot_id: Uuid,
) -> Result<BackupSnapshotManifest, BackupError> {
    let key = manifest_key(snapshot_id);
    let bytes = repository.get_blob(&key).await?;
    let manifest: BackupSnapshotManifest = serde_json::from_slice(&bytes)?;
    verify_manifest_checksum(&manifest)?;
    Ok(manifest)
}

/// Loads a manifest and all of its parents in restore order.
///
/// # Errors
///
/// Returns [`BackupError`] if any manifest in the chain cannot be loaded,
/// verified, or if the resulting chain is invalid.
pub async fn load_manifest_chain(
    repository: &dyn BackupRepository,
    snapshot_id: Uuid,
) -> Result<Vec<BackupSnapshotManifest>, BackupError> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut next = Some(snapshot_id);

    while let Some(current_snapshot_id) = next {
        if !seen.insert(current_snapshot_id) {
            return Err(BackupError::InvalidManifestChain {
                reason: format!("duplicate snapshot id {current_snapshot_id}"),
            });
        }

        let manifest = load_manifest(repository, current_snapshot_id).await?;
        next = manifest.parent_snapshot_id;
        chain.push(manifest);
    }

    chain.reverse();
    validate_manifest_chain(&chain)?;
    Ok(chain)
}

/// Validates manifest parent/child consistency.
///
/// # Errors
///
/// Returns [`BackupError::InvalidManifestChain`] when the chain is empty,
/// starts from a non-full root, contains duplicate snapshots, has broken parent
/// references, or mixes incompatible application/schema/backend metadata.
pub fn validate_manifest_chain(chain: &[BackupSnapshotManifest]) -> Result<(), BackupError> {
    let Some(first) = chain.first() else {
        return Err(BackupError::InvalidManifestChain {
            reason: "manifest chain is empty".to_string(),
        });
    };

    if !matches!(
        first.backup_kind,
        BackupKind::Full | BackupKind::SyntheticFull
    ) {
        return Err(BackupError::InvalidManifestChain {
            reason: "manifest chain does not start with a full or synthetic-full snapshot"
                .to_string(),
        });
    }

    if first.parent_snapshot_id.is_some() {
        return Err(BackupError::InvalidManifestChain {
            reason: "root full snapshot must not have a parent".to_string(),
        });
    }

    let mut seen = HashSet::new();
    seen.insert(first.snapshot_id);

    for pair in chain.windows(2) {
        let parent = &pair[0];
        let child = &pair[1];

        if !seen.insert(child.snapshot_id) {
            return Err(BackupError::InvalidManifestChain {
                reason: format!("duplicate snapshot id {}", child.snapshot_id),
            });
        }

        if child.parent_snapshot_id != Some(parent.snapshot_id) {
            return Err(BackupError::InvalidManifestChain {
                reason: format!(
                    "snapshot {} does not reference expected parent {}",
                    child.snapshot_id, parent.snapshot_id
                ),
            });
        }

        if child.app_id != parent.app_id {
            return Err(BackupError::InvalidManifestChain {
                reason: format!(
                    "snapshot {} app_id does not match parent {}",
                    child.snapshot_id, parent.snapshot_id
                ),
            });
        }

        if child.database_backend != parent.database_backend {
            return Err(BackupError::InvalidManifestChain {
                reason: format!(
                    "snapshot {} database backend does not match parent {}",
                    child.snapshot_id, parent.snapshot_id
                ),
            });
        }

        if child.graphql_orm_schema_hash != parent.graphql_orm_schema_hash {
            return Err(BackupError::InvalidManifestChain {
                reason: format!(
                    "snapshot {} schema hash does not match parent {}",
                    child.snapshot_id, parent.snapshot_id
                ),
            });
        }
    }

    Ok(())
}

/// Compresses a payload with zstd.
///
/// # Errors
///
/// Returns [`BackupError::Compression`] if zstd encoding fails.
pub fn compress_payload(bytes: &[u8]) -> Result<Vec<u8>, BackupError> {
    zstd::stream::encode_all(bytes, 0).map_err(BackupError::compression)
}

/// Decompresses a zstd payload.
///
/// # Errors
///
/// Returns [`BackupError::Compression`] if zstd decoding fails.
pub fn decompress_payload(bytes: &[u8]) -> Result<Vec<u8>, BackupError> {
    zstd::stream::decode_all(bytes).map_err(BackupError::compression)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn manifest_key(snapshot_id: Uuid) -> String {
    format!("snapshots/{snapshot_id}/manifest.json")
}

#[derive(Serialize)]
struct ChecksumManifestView<'a> {
    format_version: u32,
    snapshot_id: Uuid,
    parent_snapshot_id: Option<Uuid>,
    created_at: i64,
    app_id: &'a str,
    app_version: &'a str,
    graphql_orm_schema_version: &'a str,
    graphql_orm_schema_hash: &'a str,
    database_backend: &'a str,
    backup_kind: &'a BackupKind,
    database: &'a DatabaseBackupManifest,
    objects: &'a [ObjectBackupEntry],
    tombstones: &'a [BackupTombstone],
    checksum: &'static str,
}

impl<'a> From<&'a BackupSnapshotManifest> for ChecksumManifestView<'a> {
    fn from(manifest: &'a BackupSnapshotManifest) -> Self {
        Self {
            format_version: manifest.format_version,
            snapshot_id: manifest.snapshot_id,
            parent_snapshot_id: manifest.parent_snapshot_id,
            created_at: manifest.created_at,
            app_id: &manifest.app_id,
            app_version: &manifest.app_version,
            graphql_orm_schema_version: &manifest.graphql_orm_schema_version,
            graphql_orm_schema_hash: &manifest.graphql_orm_schema_hash,
            database_backend: &manifest.database_backend,
            backup_kind: &manifest.backup_kind,
            database: &manifest.database,
            objects: &manifest.objects,
            tombstones: &manifest.tombstones,
            checksum: "",
        }
    }
}
