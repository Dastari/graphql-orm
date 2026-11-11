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

    #[error("backup repository is locked by {lock_key}")]
    RepositoryLocked { lock_key: String },

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
