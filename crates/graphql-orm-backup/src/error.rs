use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("unsupported backup provider: {provider}")]
    UnsupportedProvider { provider: String },

    #[error("invalid backup repository key: {key}")]
    InvalidRepositoryKey { key: String },

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

    #[error("unsupported operation: {operation}")]
    UnsupportedOperation { operation: String },

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
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
