use std::path::PathBuf;

/// Errors returned by storage services and provider backends.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The selected provider is known but is not implemented by this build.
    #[error("unsupported storage backend: {backend}")]
    UnsupportedBackend { backend: String },

    /// A storage key is empty, absolute, or contains unsafe path components.
    #[error("invalid storage key: {key}")]
    InvalidStorageKey { key: String },

    /// A requested blob is missing from the storage backend.
    #[error("storage blob is missing: {key}")]
    MissingBlob { key: String },

    /// A local filesystem object path did not have a writable parent directory.
    #[error("local storage path has no parent: {path:?}")]
    MissingParent { path: PathBuf },

    /// A filesystem operation failed.
    #[error("storage io error at {path:?}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl StorageError {
    #[cfg(feature = "local")]
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
