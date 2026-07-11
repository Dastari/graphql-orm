use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
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
