use std::collections::HashSet;

use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{BackupError, BackupRepository, ManifestAuthenticator};

/// Manifest format written by this crate: sealed with a host-keyed MAC.
pub const BACKUP_FORMAT_VERSION: u32 = 3;
/// Format of manifests written before authentication existed. They carry no
/// seal and are read only under an explicit [`crate::LegacyManifestPolicy`].
pub const LEGACY_BACKUP_FORMAT_VERSION: u32 = 1;

/// Snapshot type stored in a manifest.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BackupKind {
    /// Complete table/object snapshot.
    Full,
    /// Changes relative to a parent snapshot.
    Incremental,
    /// Full snapshot synthesized by compacting a chain.
    SyntheticFull,
}

/// Compression applied to database payload blobs.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BackupCompression {
    /// Payload is stored without compression.
    #[default]
    None,
    /// Payload is zstd-compressed.
    Zstd,
}

/// Durable manifest describing one backup snapshot.
///
/// Unknown fields are rejected when reading, so every accepted field is
/// covered by the checksum and the seal.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupSnapshotManifest {
    /// Manifest format version.
    pub format_version: u32,
    /// Snapshot id.
    pub snapshot_id: Uuid,
    /// Parent snapshot id for incrementals.
    pub parent_snapshot_id: Option<Uuid>,
    /// Snapshot creation time as UTC Unix seconds.
    pub created_at: i64,
    /// Stable application identifier.
    pub app_id: String,
    /// Application version that produced the snapshot.
    pub app_version: String,
    /// GraphQL ORM migration/schema version.
    pub graphql_orm_schema_version: String,
    /// Stable hash of the GraphQL ORM schema snapshot.
    pub graphql_orm_schema_hash: String,
    /// Database backend identifier.
    pub database_backend: String,
    /// Snapshot kind.
    pub backup_kind: BackupKind,
    /// Database payload metadata.
    pub database: DatabaseBackupManifest,
    /// Stored object payload metadata.
    pub objects: Vec<ObjectBackupEntry>,
    /// Delete tombstones included in the snapshot.
    pub tombstones: Vec<BackupTombstone>,
    /// Manifest checksum. The checksum field itself is cleared while hashing.
    ///
    /// This plain SHA-256 detects accidental corruption only. Anyone who can
    /// write the repository can recompute it; [`Self::authentication`] is what
    /// ties the manifest to the host.
    pub checksum: String,
    /// Host-keyed seal over canonical manifest bytes and repository identity.
    /// Required for [`BACKUP_FORMAT_VERSION`]; format 2 seals omit the identity,
    /// and format 1 manifests have no seal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication: Option<ManifestMac>,
}

/// MAC algorithm used to seal a manifest.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ManifestMacAlgorithm {
    /// HMAC-SHA256 under an HKDF-derived [`crate::ManifestKey`].
    HmacSha256,
}

/// Seal stored in an authenticated manifest.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestMac {
    /// MAC algorithm.
    pub algorithm: ManifestMacAlgorithm,
    /// Public id of the sealing key; see [`crate::ManifestKey::key_id`].
    pub key_id: String,
    /// Repository identity covered by the MAC (required for stored format 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<Uuid>,
    /// Lowercase hex MAC.
    pub mac: String,
}

/// Database payload metadata stored in a snapshot manifest.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseBackupManifest {
    /// Logical payload format, currently `jsonl`.
    pub export_format: String,
    /// Compression applied to table/change payload blobs.
    #[serde(default)]
    pub compression: BackupCompression,
    /// Total full rows or incremental changes represented by this manifest.
    pub row_count: u64,
    /// Number of full table exports.
    pub table_count: u64,
    /// Full table payload entries.
    pub tables: Vec<TableBackupEntry>,
    /// Incremental change payload entries.
    #[serde(default)]
    pub changes: Vec<TableBackupEntry>,
}

/// Metadata for a table or change payload blob.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableBackupEntry {
    /// Table name represented by this payload.
    pub table_name: String,
    /// Number of rows or changes in this payload.
    pub row_count: u64,
    /// Repository content key.
    pub content_key: String,
    /// SHA-256 checksum of the stored payload bytes.
    pub sha256_hex: String,
}

/// Metadata for a content-addressed stored object blob.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectBackupEntry {
    /// Application object id.
    pub object_id: Uuid,
    /// Original application storage key.
    pub storage_key: String,
    /// Repository content key.
    pub content_key: String,
    /// SHA-256 checksum of the object bytes.
    pub sha256_hex: String,
    /// Observed object size for new entries; historical values remain advisory.
    pub size_bytes: u64,
    /// Optional MIME type.
    pub mime_type: Option<String>,
}

/// Delete tombstone emitted by incremental snapshots.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupTombstone {
    /// Deleted table name, when the tombstone refers to a database row.
    pub table_name: Option<String>,
    /// Deleted primary key, when the tombstone refers to a database row.
    pub primary_key: Option<String>,
    /// Deleted object id, when the tombstone refers to a stored object.
    pub object_id: Option<Uuid>,
    /// Deletion time as UTC Unix seconds.
    pub deleted_at: i64,
}

/// Computes the manifest checksum with the manifest's checksum field cleared.
///
/// The checksum detects accidental corruption only; it is not authentication.
///
/// # Errors
///
/// Returns [`BackupError`] if the canonical checksum view cannot be serialized.
pub fn manifest_checksum(manifest: &BackupSnapshotManifest) -> Result<String, BackupError> {
    Ok(sha256_hex(&canonical_manifest_bytes(manifest)?))
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
/// This checks integrity only. Use [`verify_manifest_authenticity`] to decide
/// whether a manifest may be trusted.
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
            key: manifest_key(manifest.snapshot_id),
            expected: manifest.checksum.clone(),
            actual,
        })
    }
}

/// Sets the checksum and host-keyed seal on a current-format manifest.
///
/// The manifest's entries are validated first, so a manifest that restore
/// would refuse is never sealed.
///
/// # Errors
///
/// Returns [`BackupError::UnsupportedManifestFormat`] unless the manifest uses
/// [`BACKUP_FORMAT_VERSION`], [`BackupError::InvalidManifestEntry`] for an entry
/// outside its namespace, [`BackupError::ManifestAuthenticationFailed`] if no
/// repository identity is configured (initialize the repository, then pin its
/// identity on the authenticator), or another error if serialization fails.
pub fn authenticate_manifest(
    authenticator: &ManifestAuthenticator,
    manifest: &mut BackupSnapshotManifest,
) -> Result<(), BackupError> {
    if manifest.format_version != BACKUP_FORMAT_VERSION {
        return Err(BackupError::UnsupportedManifestFormat {
            key: manifest_key(manifest.snapshot_id),
            format_version: manifest.format_version,
        });
    }
    validate_manifest_entries(manifest)?;
    authenticator.require_repository_identity()?;
    let canonical = canonical_manifest_bytes(manifest)?;
    manifest.checksum = sha256_hex(&canonical);
    if let Some(identity) = manifest
        .authentication
        .as_ref()
        .and_then(|seal| seal.repository_id)
    {
        authenticator.check_repository_identity(identity)?;
    }
    manifest.authentication = Some(authenticator.seal(&canonical));
    Ok(())
}

/// Decides whether a manifest may be trusted, and checks its entries.
///
/// A current-format manifest must carry a seal that verifies under one of the
/// authenticator's keys. A legacy manifest is accepted only as its
/// [`crate::LegacyManifestPolicy`] allows. In both cases the checksum must
/// match and every content key must stay inside its namespace.
///
/// This checks the cryptographic host identity pin but does not read storage.
/// [`load_manifest`] also compares the durable identity record and snapshot path.
///
/// # Errors
///
/// Returns [`BackupError::UnsupportedManifestFormat`],
/// [`BackupError::ChecksumMismatch`],
/// [`BackupError::ManifestAuthenticationFailed`],
/// [`BackupError::LegacyManifestRefused`] or
/// [`BackupError::InvalidManifestEntry`].
pub fn verify_manifest_authenticity(
    authenticator: &ManifestAuthenticator,
    manifest: &BackupSnapshotManifest,
) -> Result<(), BackupError> {
    let key = manifest_key(manifest.snapshot_id);
    match (manifest.format_version, &manifest.authentication) {
        (BACKUP_FORMAT_VERSION | 2, Some(seal)) => {
            if manifest.format_version == BACKUP_FORMAT_VERSION {
                authenticator.require_repository_identity()?;
            }

            if manifest.format_version == BACKUP_FORMAT_VERSION && seal.repository_id.is_none() {
                return Err(BackupError::ManifestAuthenticationFailed {
                    key,
                    reason: "manifest has no repository identity".into(),
                });
            }
            if manifest.format_version == 2 && seal.repository_id.is_some() {
                return Err(BackupError::ManifestAuthenticationFailed {
                    key,
                    reason: "format 2 does not carry a repository identity".into(),
                });
            }
            verify_manifest_checksum(manifest)?;
            authenticator
                .verify(seal, &canonical_manifest_bytes(manifest)?)
                .map_err(|reason| BackupError::ManifestAuthenticationFailed { key, reason })?;
            if manifest.format_version == 2 && !trusts_legacy_manifest(authenticator, manifest) {
                return Err(BackupError::LegacyManifestRefused {
                    snapshot_id: manifest.snapshot_id,
                });
            }
        }
        (BACKUP_FORMAT_VERSION, None) => {
            return Err(BackupError::ManifestAuthenticationFailed {
                key,
                reason: "manifest is not sealed".to_string(),
            });
        }
        (LEGACY_BACKUP_FORMAT_VERSION, None) => {
            verify_manifest_checksum(manifest)?;
            if !trusts_legacy_manifest(authenticator, manifest) {
                return Err(BackupError::LegacyManifestRefused {
                    snapshot_id: manifest.snapshot_id,
                });
            }
        }
        (LEGACY_BACKUP_FORMAT_VERSION, Some(_)) => {
            return Err(BackupError::ManifestAuthenticationFailed {
                key,
                reason: "legacy manifest format does not carry a seal".to_string(),
            });
        }
        (format_version, _) => {
            return Err(BackupError::UnsupportedManifestFormat {
                key,
                format_version,
            });
        }
    }
    validate_manifest_entries(manifest)
}

/// Checks that every manifest entry stays inside the namespace it owns.
///
/// Object content keys must be the content address of their declared digest
/// under `objects/sha256/`. Table and change payloads must live at this
/// snapshot's key for their table name. Digests must be lowercase SHA-256 hex,
/// and table names must be a single safe key segment. The parent link must
/// match the snapshot kind.
///
/// # Errors
///
/// Returns [`BackupError::InvalidManifestEntry`] naming the first violation.
pub fn validate_manifest_entries(manifest: &BackupSnapshotManifest) -> Result<(), BackupError> {
    let invalid = |reason: String| BackupError::InvalidManifestEntry {
        snapshot_id: manifest.snapshot_id,
        reason,
    };
    match (&manifest.backup_kind, manifest.parent_snapshot_id) {
        (BackupKind::Full | BackupKind::SyntheticFull, None) => {
            if !manifest.database.changes.is_empty() {
                return Err(invalid(
                    "a full snapshot must not carry changes".to_string(),
                ));
            }
        }
        (BackupKind::Incremental, Some(parent)) => {
            if parent == manifest.snapshot_id {
                return Err(invalid("snapshot is its own parent".to_string()));
            }
            if !manifest.database.tables.is_empty() {
                return Err(invalid(
                    "an incremental snapshot must not carry full tables".to_string(),
                ));
            }
        }
        (kind, parent) => {
            return Err(invalid(format!(
                "snapshot kind {kind:?} does not match parent {parent:?}"
            )));
        }
    }
    for (entries, expected_key) in [
        (
            &manifest.database.tables,
            crate::database_table_key as fn(Uuid, &str) -> String,
        ),
        (&manifest.database.changes, crate::database_changes_key),
    ] {
        for entry in entries {
            validate_table_name(&entry.table_name).map_err(invalid)?;
            validate_sha256_hex(&entry.sha256_hex).map_err(invalid)?;
            let expected = expected_key(manifest.snapshot_id, &entry.table_name);
            if entry.content_key != expected {
                return Err(invalid(format!(
                    "payload key {} for table {} is not {expected}",
                    entry.content_key, entry.table_name
                )));
            }
        }
    }
    for object in &manifest.objects {
        validate_sha256_hex(&object.sha256_hex).map_err(invalid)?;
        let expected = crate::object_content_key(&object.sha256_hex);
        if object.content_key != expected {
            return Err(invalid(format!(
                "object {} content key {} is not {expected}",
                object.object_id, object.content_key
            )));
        }
    }
    Ok(())
}

/// Loads, authenticates and validates a single snapshot manifest.
///
/// The repository must supply a [`ManifestAuthenticator`]. The manifest must
/// name the snapshot id of the path it was read from, use a supported format,
/// contain no unknown fields, pass [`verify_manifest_authenticity`], and keep
/// every entry inside its namespace.
///
/// This is a low-level read that takes no repository lock; callers that need
/// a consistent view coordinate with other operations themselves.
///
/// # Errors
///
/// Returns [`BackupError::ManifestAuthenticationRequired`] without reading
/// when no authenticator is configured,
/// [`BackupError::ManifestSnapshotMismatch`] for a manifest stored under
/// another snapshot's path, or another [`BackupError`] if the blob is missing,
/// cannot be deserialized, or fails verification.
pub async fn load_manifest(
    repository: &dyn BackupRepository,
    snapshot_id: Uuid,
) -> Result<BackupSnapshotManifest, BackupError> {
    let authenticator = required_authenticator(repository)?;
    let key = manifest_key(snapshot_id);
    let bytes = crate::collect_backup_stream(
        repository.get_blob_stream(&key).await?,
        repository.capacity_limits().buffered_bytes,
    )
    .await?;
    let manifest: BackupSnapshotManifest = serde_json::from_slice(&bytes)?;
    if manifest.snapshot_id != snapshot_id {
        return Err(BackupError::ManifestSnapshotMismatch {
            key,
            expected: snapshot_id,
            actual: manifest.snapshot_id,
        });
    }
    verify_manifest_authenticity(authenticator, &manifest)?;
    verify_repository_binding(repository, &manifest).await?;
    Ok(manifest)
}

/// Largest zstd window log that decoding accepts under a decoded-size limit.
/// Backups encode within it so they never write a frame restore refuses.
pub(crate) fn decode_window_log(limit_bytes: u64) -> u32 {
    (63 - limit_bytes.max(1).leading_zeros()).clamp(10, 30)
}

pub(crate) fn required_authenticator(
    repository: &dyn BackupRepository,
) -> Result<&ManifestAuthenticator, BackupError> {
    repository
        .manifest_authenticator()
        .ok_or(BackupError::ManifestAuthenticationRequired)
}

pub(crate) fn validate_table_name(table_name: &str) -> Result<(), String> {
    if table_name.contains('/')
        || graphql_orm_storage::validate_blob_key(table_name).is_err()
        || table_name.chars().any(char::is_control)
    {
        return Err(format!(
            "table name {table_name:?} is not a safe key segment"
        ));
    }
    Ok(())
}

pub(crate) fn validate_sha256_hex(value: &str) -> Result<(), String> {
    if value.len() == 64 && is_lower_hex(value) {
        Ok(())
    } else {
        Err(format!("digest {value:?} is not lowercase SHA-256 hex"))
    }
}

pub(crate) fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn canonical_manifest_bytes(manifest: &BackupSnapshotManifest) -> Result<Vec<u8>, BackupError> {
    Ok(serde_json::to_vec(&ChecksumManifestView::from(manifest))?)
}

/// Loads a manifest and all of its parents in restore order.
///
/// Like [`load_manifest`], this is a low-level read that takes no repository
/// lock; a concurrent prune or deletion can remove a manifest between reads.
/// Use [`crate::restore_snapshot`] or verification for locked reads.
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

/// Decompresses a zstd payload with the default 256 MiB output limit.
///
/// # Errors
///
/// Returns [`BackupError::Compression`] if zstd decoding fails, or
/// [`BackupError::CapacityExceeded`] if the default output limit is exceeded.
pub fn decompress_payload(bytes: &[u8]) -> Result<Vec<u8>, BackupError> {
    decompress_payload_with_limit(
        bytes,
        crate::BackupCapacityLimits::default().decompressed_bytes,
    )
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

/// Decodes zstd with an explicit output limit, checked before extending the buffer.
/// Finite limits also cap the decoder history window before reading frame input:
/// round down to a power of two, with a 1 KiB minimum and portable 1 GiB maximum.
/// This bounds the window, not all decoder overhead or process memory. Unbounded
/// leaves the zstd library's default window policy in effect.
///
/// # Errors
/// Returns a compression error or [`BackupError::CapacityExceeded`].
pub fn decompress_payload_with_limit(
    bytes: &[u8],
    limit: crate::BackupByteLimit,
) -> Result<Vec<u8>, BackupError> {
    use std::io::Read;
    let mut decoder =
        zstd::stream::read::Decoder::with_buffer(bytes).map_err(BackupError::compression)?;
    if let crate::BackupByteLimit::Bytes(limit_bytes) = limit {
        decoder
            .window_log_max(decode_window_log(limit_bytes))
            .map_err(BackupError::compression)?;
    }
    let mut result = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let count = decoder.read(&mut chunk).map_err(BackupError::compression)?;
        if count == 0 {
            break;
        }
        let size = (result.len() as u64).checked_add(count as u64).ok_or(
            BackupError::CapacityExceeded {
                limit_bytes: u64::MAX,
            },
        )?;
        limit.check(size)?;
        result.extend_from_slice(&chunk[..count]);
    }
    Ok(result)
}

/// Durable identity key, outside the snapshot and object namespaces.
pub const REPOSITORY_ID_KEY: &str = "repository-id";

pub(crate) async fn repository_identity(
    repository: &dyn BackupRepository,
    create: bool,
) -> Result<Uuid, BackupError> {
    let authenticator = required_authenticator(repository)?;
    let expected = if create {
        let proposed = authenticator
            .expected_repository_identity()
            .unwrap_or_else(Uuid::new_v4);
        let created = repository
            .put_blob_if_absent(REPOSITORY_ID_KEY, bytes::Bytes::from(proposed.to_string()))
            .await?;
        if !created {
            // An existing storage record cannot enroll itself as a host trust anchor.
            authenticator.require_repository_identity()?;
        }
        proposed
    } else {
        authenticator.require_repository_identity()?
    };
    let bytes = crate::collect_backup_stream(
        repository.get_blob_stream(REPOSITORY_ID_KEY).await?,
        crate::BackupByteLimit::Bytes(64),
    )
    .await?;
    let identity = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| BackupError::ManifestAuthenticationFailed {
            key: REPOSITORY_ID_KEY.into(),
            reason: "invalid repository identity".into(),
        })?;
    if identity != expected {
        return Err(BackupError::ManifestAuthenticationFailed {
            key: REPOSITORY_ID_KEY.into(),
            reason: "repository identity changed or differs from the host pin".into(),
        });
    }
    Ok(expected)
}

pub(crate) async fn verify_repository_binding(
    repository: &dyn BackupRepository,
    manifest: &BackupSnapshotManifest,
) -> Result<(), BackupError> {
    if manifest.format_version < BACKUP_FORMAT_VERSION {
        return Ok(());
    }
    let identity = repository_identity(repository, false).await?;
    if manifest
        .authentication
        .as_ref()
        .and_then(|seal| seal.repository_id)
        != Some(identity)
    {
        return Err(BackupError::ManifestAuthenticationFailed {
            key: manifest_key(manifest.snapshot_id),
            reason: "manifest belongs to a different repository identity".into(),
        });
    }
    Ok(())
}

/// Creates the repository identity atomically once. Reading an existing record
/// requires an independently configured host identity pin; storage cannot enroll
/// its own identity. A newly created record must still match the proposed UUID.
/// Record the returned value outside storage and pin it with
/// [`ManifestAuthenticator::with_repository_identity`].
///
/// # Errors
/// Returns an error for missing authentication, malformed records, a mismatched
/// host pin, or a provider without atomic conditional creation.
pub async fn initialize_repository_identity(
    repository: &dyn BackupRepository,
) -> Result<Uuid, BackupError> {
    repository_identity(repository, true).await
}

fn trusts_legacy_manifest(
    authenticator: &ManifestAuthenticator,
    manifest: &BackupSnapshotManifest,
) -> bool {
    match authenticator.legacy_manifest_policy() {
        crate::LegacyManifestPolicy::TrustPinned(pins) => {
            pins.get(&manifest.snapshot_id) == Some(&manifest.checksum)
        }
        crate::LegacyManifestPolicy::AcceptSealedLegacy => manifest.format_version == 2,
        crate::LegacyManifestPolicy::TrustUnauthenticated => true,
        crate::LegacyManifestPolicy::Refuse => false,
    }
}
