use std::path::PathBuf;

/// Failures reported by backup, restore, verification, and retention operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BackupError {
    /// A configured buffering or decoding byte limit was exceeded.
    #[error("backup payload exceeds configured byte limit {limit_bytes}")]
    CapacityExceeded {
        /// Maximum accepted bytes.
        limit_bytes: u64,
    },

    /// A snapshot-chain restore failed; progress describes possible target effects.
    #[error("{progress}")]
    RestoreStageFailed {
        /// Reported completed stages and the boundary that failed.
        progress: crate::restore::RestoreFailureProgress,
        /// Original failure, retained as an error source without public display.
        #[source]
        source: Box<BackupError>,
    },

    /// The selected storage provider cannot be used for backup.
    #[error("unsupported backup provider: {provider}")]
    UnsupportedProvider {
        /// Name of the selected provider.
        provider: String,
    },

    /// A key cannot address a blob within the repository.
    #[error("invalid backup repository key: {key}")]
    InvalidRepositoryKey {
        /// Rejected key.
        key: String,
    },

    /// The local repository root is missing or cannot be used as a directory.
    #[error("invalid backup repository root: {path:?}")]
    InvalidRepositoryRoot {
        /// Rejected root path.
        path: PathBuf,
    },

    /// A requested blob could not be found.
    #[error("backup blob is missing: {key}")]
    MissingBlob {
        /// Key of the missing blob.
        key: String,
    },

    /// A computed checksum differs from the expected checksum.
    #[error("checksum mismatch for {key}: expected {expected}, actual {actual}")]
    ChecksumMismatch {
        /// Key or diagnostic identifier of the checked content.
        key: String,
        /// Checksum recorded for the content.
        expected: String,
        /// Checksum computed from the content read.
        actual: String,
    },

    /// A restore that requires an empty database found existing data.
    #[error("restore target is not empty")]
    RestoreTargetNotEmpty,

    /// The repository handle has no [`crate::ManifestAuthenticator`], so
    /// manifests can be neither trusted nor sealed. Nothing was read or written.
    #[error("manifest authentication is not configured for this repository")]
    ManifestAuthenticationRequired,

    /// Host key material was rejected.
    #[error("invalid manifest key: {reason}")]
    InvalidManifestKey {
        /// Validation rule that rejected the key material.
        reason: String,
    },

    /// A manifest's seal is missing, malformed, made with an unknown key, or
    /// does not match its contents.
    #[error("manifest authentication failed for {key}: {reason}")]
    ManifestAuthenticationFailed {
        /// Repository key of the manifest.
        key: String,
        /// Reason the seal could not be verified.
        reason: String,
    },

    /// The host's snapshot records do not admit this snapshot: it is on the
    /// revocation list, or missing from the known-snapshot list, of the
    /// repository's [`crate::ManifestAuthenticator`]. Its seal may be valid;
    /// a deleted snapshot that was put back into the repository fails here.
    #[error("snapshot {snapshot_id} is not admitted by the host's snapshot records: {reason}")]
    SnapshotNotAdmitted {
        /// Snapshot that was refused.
        snapshot_id: uuid::Uuid,
        /// Which record refused it.
        reason: String,
    },

    /// An unauthenticated legacy manifest was refused by the configured
    /// [`crate::LegacyManifestPolicy`].
    #[error("unauthenticated legacy manifest for snapshot {snapshot_id} refused by policy")]
    LegacyManifestRefused {
        /// Snapshot refused by the policy.
        snapshot_id: uuid::Uuid,
    },

    /// The manifest uses a version outside the supported snapshot formats.
    #[error("unsupported manifest format version {format_version} at {key}")]
    UnsupportedManifestFormat {
        /// Repository key of the manifest.
        key: String,
        /// Format version declared in the manifest.
        format_version: u32,
    },

    /// A manifest names a different snapshot than the path it was read from.
    #[error("manifest at {key} names snapshot {actual}, expected {expected}")]
    ManifestSnapshotMismatch {
        /// Repository key used to load the manifest.
        key: String,
        /// Snapshot ID derived from the repository key.
        expected: uuid::Uuid,
        /// Snapshot ID declared inside the manifest.
        actual: uuid::Uuid,
    },

    /// A manifest entry points outside the namespace it owns.
    #[error("invalid manifest entry in snapshot {snapshot_id}: {reason}")]
    InvalidManifestEntry {
        /// Snapshot containing the rejected entry.
        snapshot_id: uuid::Uuid,
        /// Entry validation failure.
        reason: String,
    },

    /// An object's live-store key is not a valid key, or is outside the
    /// sink's [`crate::RestoreKeyScope`].
    #[error("restore target key {storage_key} rejected: {reason}")]
    RestoreTargetRejected {
        /// Rejected live-store key.
        storage_key: String,
        /// Key validation or scope failure.
        reason: String,
    },

    /// The live store already holds different content at an object's key.
    /// Object restore never overwrites it.
    #[error("restore target already holds different content at {storage_key}")]
    RestoreObjectConflict {
        /// Live-store key holding the conflicting object.
        storage_key: String,
    },

    /// The snapshot's database backend or schema identity differs from the target.
    #[error(
        "restore schema is incompatible: backup backend/hash {backup_backend}/{backup_schema_hash}, target backend/hash {target_backend}/{target_schema_hash}"
    )]
    RestoreSchemaMismatch {
        /// Backend recorded in the snapshot.
        backup_backend: String,
        /// Schema hash recorded in the snapshot.
        backup_schema_hash: String,
        /// Backend reported by the restore target.
        target_backend: String,
        /// Schema hash computed for the restore target.
        target_schema_hash: String,
    },

    /// A canonical snapshot and its restore target have the same key layout
    /// entries, but disagree on the PostgreSQL type or type modifier of a key.
    /// Restore stops before checking target emptiness or writing data.
    #[error("canonical key storage differs at table {table_name:?}, column {column_name:?}")]
    CanonicalKeyStorageMismatch {
        /// Table whose key storage differs.
        table_name: String,
        /// Primary-key column whose storage differs.
        column_name: String,
    },

    /// A column policy override failed validation.
    #[error("invalid column backup policy override for {table_name}.{column_name}: {reason}")]
    InvalidColumnBackupPolicyOverride {
        /// Table named by the override.
        table_name: String,
        /// Column named by the override.
        column_name: String,
        /// Reason the override was refused.
        reason: String,
    },

    /// The requested restore inputs violate the adapter's restore contract.
    #[error("invalid restore context: {reason}")]
    InvalidRestoreContext {
        /// Explanation of the inconsistent inputs.
        reason: String,
    },

    /// Repository enumeration omitted a parent or changed between reads.
    #[error("incomplete repository listing: {reason}")]
    IncompleteRepositoryListing {
        /// Inconsistency detected during enumeration.
        reason: String,
    },

    /// The snapshots do not form a valid chain from a full or synthetic-full base.
    #[error("invalid manifest chain: {reason}")]
    InvalidManifestChain {
        /// Chain invariant that failed validation.
        reason: String,
    },

    /// Another run holds the repository lock. `owner` is its owner record, or
    /// `None` when the lock has no readable record (for example a lock written
    /// by an older release). Elapsed time never releases a lock; see
    /// [`crate::break_repository_lock`].
    #[error("backup repository is locked by {lock_key}{}", describe_owner(.owner.as_deref()))]
    RepositoryLocked {
        /// Key at which the existing lock was found.
        lock_key: String,
        /// Current owner record, when readable.
        owner: Option<Box<crate::RepositoryLockOwner>>,
    },

    /// Releasing a lock found that it no longer names this run, so it was left
    /// in place. An operator broke the lock, or the record was replaced.
    #[error("repository lock {lock_key} is not held by run {run_id}; it was left in place")]
    RepositoryLockNotHeld {
        /// Key at which the lock was expected.
        lock_key: String,
        /// Run that attempted to release the lock.
        run_id: uuid::Uuid,
        /// Record found at the lock key, if one was readable.
        found: Option<Box<crate::RepositoryLockOwner>>,
    },

    /// Lock options were rejected before any repository write.
    #[error("invalid repository lock options: {reason}")]
    InvalidRepositoryLockOptions {
        /// Lock option validation failure.
        reason: String,
    },

    /// [`crate::break_repository_lock`] refused to remove a lock.
    #[error("refusing to break repository lock {lock_key}: {reason}")]
    BreakLockRefused {
        /// Key of the lock the operator requested to break.
        lock_key: String,
        /// Reason the lock was left in place.
        reason: String,
    },

    /// The operation completed, but releasing the repository lock failed, so
    /// the repository may still be locked by `run_id`.
    ///
    /// `committed_snapshot_id` names the snapshot a backup or compaction
    /// sealed before the release failed; the snapshot is complete.
    #[error(
        "operation completed but repository lock {lock_key} held by run {run_id} was not released"
    )]
    LockReleaseFailed {
        /// Key of the lock whose release failed.
        lock_key: String,
        /// Run whose lock may still be held.
        run_id: uuid::Uuid,
        /// Snapshot sealed by the completed operation, if it created one.
        committed_snapshot_id: Option<uuid::Uuid>,
        /// Error reported while releasing the lock.
        #[source]
        source: Box<BackupError>,
    },

    /// The operation failed or was cancelled, and releasing its repository
    /// lock failed too, so the repository may still be locked by `run_id`.
    ///
    /// `operation` is the operation's own error and `release` the release
    /// failure. A later operation fails with [`BackupError::RepositoryLocked`]
    /// naming this run. After confirming that the run and every other writer
    /// have stopped, remove the lock with [`crate::break_repository_lock`] for
    /// `run_id`.
    #[error(
        "operation failed and repository lock {lock_key} held by run {run_id} was not released; \
         confirm no writer is running, then break the lock for run {run_id}"
    )]
    LockNotReleased {
        /// Key of the lock whose release failed.
        lock_key: String,
        /// Run whose lock may still be held.
        run_id: uuid::Uuid,
        /// Failure that stopped the operation.
        #[source]
        operation: Box<BackupError>,
        /// Additional failure encountered while releasing the lock.
        release: Box<BackupError>,
    },

    /// A [`crate::BackupCancellation`] was triggered. The operation stopped at
    /// a safe point, removed the payloads it had written and released the lock.
    #[error("backup operation was cancelled")]
    Cancelled,

    /// A backup or compaction named a snapshot id that the repository already
    /// uses. Nothing was written.
    #[error("snapshot {snapshot_id} already exists; snapshots are never overwritten")]
    SnapshotExists {
        /// Snapshot ID whose repository namespace is already in use.
        snapshot_id: uuid::Uuid,
    },

    /// A retention policy would delete every snapshot, or is otherwise unsafe.
    #[error("invalid retention policy: {reason}")]
    InvalidRetentionPolicy {
        /// Retention rule or deletion precondition that failed.
        reason: String,
    },

    /// A backup payload would exceed a limit that restoring it enforces.
    /// The snapshot was not sealed.
    #[error(
        "backup {payload} would exceed the {limit} limit of {limit_bytes} bytes ({observed_bytes} bytes)"
    )]
    BackupCapacityExceeded {
        /// Repository key, table name or object storage key concerned.
        payload: String,
        /// Limit that would be exceeded.
        limit: crate::BackupCapacityLimitKind,
        /// Configured maximum in bytes.
        limit_bytes: u64,
        /// Bytes observed when the limit was hit; may stop short of the full size.
        observed_bytes: u64,
    },

    /// A payload could not be compressed or decompressed.
    #[error("backup payload compression error")]
    Compression {
        /// Error from the codec or its reader or writer.
        #[source]
        source: std::io::Error,
    },

    /// The storage provider returned an error.
    #[error("backup storage error")]
    Storage {
        /// Provider error returned by the storage crate.
        #[source]
        source: graphql_orm_storage::StorageError,
    },

    /// The configured adapter or repository lacks a required capability.
    #[error("unsupported operation: {operation}")]
    UnsupportedOperation {
        /// Name or description of the unavailable operation.
        operation: String,
    },

    /// Another product owns backup and restore for this database.
    #[cfg(feature = "orm")]
    #[error(transparent)]
    BackupRestoreExternallyManaged(
        #[from] graphql_orm::graphql::orm::BackupRestoreExternallyManaged,
    ),

    /// The database adapter could not complete its part of the operation.
    #[error("database adapter error: {message}")]
    Database {
        /// Diagnostic returned by the adapter.
        message: String,
    },

    /// A JSON manifest or payload could not be encoded or decoded.
    #[error("serialization error")]
    Serialization(#[from] serde_json::Error),

    /// A local file-system operation failed.
    #[error("backup io error at {path:?}")]
    Io {
        /// Path associated with the failed operation.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

fn describe_owner(owner: Option<&crate::RepositoryLockOwner>) -> String {
    owner.map_or_else(
        || " (no owner record)".to_string(),
        |owner| format!(" ({owner})"),
    )
}

impl BackupError {
    pub(crate) fn compression(source: std::io::Error) -> Self {
        Self::Compression { source }
    }
}

impl From<graphql_orm_storage::StorageError> for BackupError {
    fn from(source: graphql_orm_storage::StorageError) -> Self {
        match source {
            graphql_orm_storage::StorageError::UnsupportedOperation { operation } => {
                Self::UnsupportedOperation { operation }
            }
            graphql_orm_storage::StorageError::UnsupportedBackend { backend } => {
                Self::UnsupportedProvider { provider: backend }
            }
            graphql_orm_storage::StorageError::InvalidStorageKey { key } => {
                Self::InvalidRepositoryKey { key }
            }
            graphql_orm_storage::StorageError::MissingBlob { key } => Self::MissingBlob { key },
            graphql_orm_storage::StorageError::Io { path, source } => Self::Io { path, source },
            source => Self::Storage { source },
        }
    }
}

impl BackupError {
    /// Whether this error refuses a snapshot because the host's snapshot
    /// records do not admit it: it is on the revocation list, or missing from
    /// the known-snapshot list, of the repository's
    /// [`crate::ManifestAuthenticator`]. Its seal may be valid; a deleted
    /// snapshot that was put back into the repository fails this way.
    ///
    /// True for [`BackupError::SnapshotNotAdmitted`], also when a restore
    /// reports it as the `source` of [`BackupError::RestoreStageFailed`].
    #[must_use]
    pub fn is_snapshot_not_admitted(&self) -> bool {
        match self {
            Self::SnapshotNotAdmitted { .. } => true,
            Self::RestoreStageFailed { source, .. } => source.is_snapshot_not_admitted(),
            _ => false,
        }
    }
}
