use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
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

    #[error("unsupported backup provider: {provider}")]
    UnsupportedProvider { provider: String },

    #[error("invalid backup repository key: {key}")]
    InvalidRepositoryKey { key: String },

    #[error("invalid backup repository root: {path:?}")]
    InvalidRepositoryRoot { path: PathBuf },

    #[error("backup blob is missing: {key}")]
    MissingBlob { key: String },

    #[error("checksum mismatch for {key}: expected {expected}, actual {actual}")]
    ChecksumMismatch {
        key: String,
        expected: String,
        actual: String,
    },

    #[error("restore target is not empty")]
    RestoreTargetNotEmpty,

    /// The repository handle has no [`crate::ManifestAuthenticator`], so
    /// manifests can be neither trusted nor sealed. Nothing was read or written.
    #[error("manifest authentication is not configured for this repository")]
    ManifestAuthenticationRequired,

    /// Host key material was rejected.
    #[error("invalid manifest key: {reason}")]
    InvalidManifestKey { reason: String },

    /// A manifest's seal is missing, malformed, made with an unknown key, or
    /// does not match its contents.
    #[error("manifest authentication failed for {key}: {reason}")]
    ManifestAuthenticationFailed { key: String, reason: String },

    /// An unauthenticated legacy manifest was refused by the configured
    /// [`crate::LegacyManifestPolicy`].
    #[error("unauthenticated legacy manifest for snapshot {snapshot_id} refused by policy")]
    LegacyManifestRefused { snapshot_id: uuid::Uuid },

    #[error("unsupported manifest format version {format_version} at {key}")]
    UnsupportedManifestFormat { key: String, format_version: u32 },

    /// A manifest names a different snapshot than the path it was read from.
    #[error("manifest at {key} names snapshot {actual}, expected {expected}")]
    ManifestSnapshotMismatch {
        key: String,
        expected: uuid::Uuid,
        actual: uuid::Uuid,
    },

    /// A manifest entry points outside the namespace it owns.
    #[error("invalid manifest entry in snapshot {snapshot_id}: {reason}")]
    InvalidManifestEntry {
        snapshot_id: uuid::Uuid,
        reason: String,
    },

    /// An object's live-store key is not a valid key, or is outside the
    /// sink's [`crate::RestoreKeyScope`].
    #[error("restore target key {storage_key} rejected: {reason}")]
    RestoreTargetRejected { storage_key: String, reason: String },

    /// The live store already holds different content at an object's key.
    /// Object restore never overwrites it.
    #[error("restore target already holds different content at {storage_key}")]
    RestoreObjectConflict { storage_key: String },

    #[error(
        "restore schema is incompatible: backup backend/hash {backup_backend}/{backup_schema_hash}, target backend/hash {target_backend}/{target_schema_hash}"
    )]
    RestoreSchemaMismatch {
        backup_backend: String,
        backup_schema_hash: String,
        target_backend: String,
        target_schema_hash: String,
    },

    #[error("invalid column backup policy override for {table_name}.{column_name}: {reason}")]
    InvalidColumnBackupPolicyOverride {
        table_name: String,
        column_name: String,
        reason: String,
    },

    #[error("invalid restore context: {reason}")]
    InvalidRestoreContext { reason: String },

    #[error("invalid manifest chain: {reason}")]
    InvalidManifestChain { reason: String },

    /// Another run holds the repository lock. `owner` is its owner record, or
    /// `None` when the lock has no readable record (for example a lock written
    /// by an older release). Elapsed time never releases a lock; see
    /// [`crate::break_repository_lock`].
    #[error("backup repository is locked by {lock_key}{}", describe_owner(.owner.as_deref()))]
    RepositoryLocked {
        lock_key: String,
        owner: Option<Box<crate::RepositoryLockOwner>>,
    },

    /// Releasing a lock found that it no longer names this run, so it was left
    /// in place. An operator broke the lock, or the record was replaced.
    #[error("repository lock {lock_key} is not held by run {run_id}; it was left in place")]
    RepositoryLockNotHeld {
        lock_key: String,
        run_id: uuid::Uuid,
        /// Record found at the lock key, if one was readable.
        found: Option<Box<crate::RepositoryLockOwner>>,
    },

    /// Lock options were rejected before any repository write.
    #[error("invalid repository lock options: {reason}")]
    InvalidRepositoryLockOptions { reason: String },

    /// [`crate::break_repository_lock`] refused to remove a lock.
    #[error("refusing to break repository lock {lock_key}: {reason}")]
    BreakLockRefused { lock_key: String, reason: String },

    /// The operation completed, but releasing the repository lock failed, so
    /// the repository may still be locked by `run_id`.
    ///
    /// `committed_snapshot_id` names the snapshot a backup or compaction
    /// sealed before the release failed; the snapshot is complete.
    #[error(
        "operation completed but repository lock {lock_key} held by run {run_id} was not released"
    )]
    LockReleaseFailed {
        lock_key: String,
        run_id: uuid::Uuid,
        committed_snapshot_id: Option<uuid::Uuid>,
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
        lock_key: String,
        run_id: uuid::Uuid,
        #[source]
        operation: Box<BackupError>,
        release: Box<BackupError>,
    },

    /// A [`crate::BackupCancellation`] was triggered. The operation stopped at
    /// a safe point, removed the payloads it had written and released the lock.
    #[error("backup operation was cancelled")]
    Cancelled,

    /// A backup or compaction named a snapshot id that the repository already
    /// uses. Nothing was written.
    #[error("snapshot {snapshot_id} already exists; snapshots are never overwritten")]
    SnapshotExists { snapshot_id: uuid::Uuid },

    /// A retention policy would delete every snapshot, or is otherwise unsafe.
    #[error("invalid retention policy: {reason}")]
    InvalidRetentionPolicy { reason: String },

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
        limit_bytes: u64,
        /// Bytes observed when the limit was hit; may stop short of the full size.
        observed_bytes: u64,
    },

    #[error("backup payload compression error")]
    Compression {
        #[source]
        source: std::io::Error,
    },

    #[error("backup storage error")]
    Storage {
        #[source]
        source: graphql_orm_storage::StorageError,
    },

    #[error("unsupported operation: {operation}")]
    UnsupportedOperation { operation: String },

    #[error("database adapter error: {message}")]
    Database { message: String },

    #[error("serialization error")]
    Serialization(#[from] serde_json::Error),

    #[error("backup io error at {path:?}")]
    Io {
        path: PathBuf,
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
