use std::path::PathBuf;

/// Provider-neutral classification for remote storage failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum StorageProviderErrorKind {
    /// Name resolution or network connection failed.
    Connectivity,
    /// Supplied credentials were rejected.
    Authentication,
    /// Required signing, encryption, or protocol negotiation failed.
    SecurityNegotiation,
    /// The configured remote container or share does not exist.
    ContainerNotFound,
    /// The authenticated identity lacks permission.
    PermissionDenied,
    /// The requested remote key does not exist.
    RemotePathNotFound,
    /// The provider is out of quota or storage capacity.
    Capacity,
    /// A conditional create collided with an existing key.
    ConditionalCreateConflict,
    /// The operation exceeded its deadline.
    Timeout,
    /// An established remote connection was lost.
    ConnectionLost,
    /// The provider returned an invalid or unsupported protocol response.
    Protocol,
}

/// Errors returned by storage services and provider backends.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The selected provider is known but is not implemented by this build.
    #[error("unsupported storage backend: {backend}")]
    UnsupportedBackend {
        /// Stable backend name.
        backend: String,
    },

    /// A provider operation failed.
    #[error("storage provider error for {backend}: {message}")]
    Provider {
        /// Storage backend that returned the error.
        backend: String,
        /// Provider-specific error message.
        message: String,
        /// Whether retrying the operation may succeed.
        retryable: bool,
    },

    /// A remote provider operation failed with a structured classification.
    #[error("remote storage provider error for {backend} during {operation}: {message}")]
    RemoteProvider {
        /// Storage backend that returned the error.
        backend: String,
        /// Provider-neutral failure category.
        kind: StorageProviderErrorKind,
        /// Stable operation name such as `connect`, `read`, or `rename`.
        operation: String,
        /// Redaction-safe diagnostic message.
        message: String,
        /// Whether retrying the operation may succeed.
        retryable: bool,
    },

    /// A storage key is empty, absolute, or contains unsafe path components.
    #[error("invalid storage key: {key}")]
    InvalidStorageKey {
        /// Rejected storage key.
        key: String,
    },

    /// A requested blob is missing from the storage backend.
    #[error("storage blob is missing: {key}")]
    MissingBlob {
        /// Requested storage key.
        key: String,
    },

    /// A conditional storage operation could not be applied.
    #[error("storage precondition failed for {key}: {condition}")]
    PreconditionFailed {
        /// Storage key involved in the failed precondition.
        key: String,
        /// Human-readable precondition description.
        condition: String,
    },

    /// A local filesystem object path did not have a writable parent directory.
    #[error("local storage path has no parent: {path:?}")]
    MissingParent {
        /// Local path that had no usable parent directory.
        path: PathBuf,
    },

    /// A filesystem operation failed.
    #[error("storage io error at {path:?}")]
    Io {
        /// Local path involved in the failed filesystem operation.
        path: PathBuf,
        /// Original filesystem error.
        #[source]
        source: std::io::Error,
    },
}

impl StorageError {
    /// Returns whether retrying the failed operation may succeed.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        match self {
            Self::Provider { retryable, .. } => *retryable,
            Self::RemoteProvider { retryable, .. } => *retryable,
            Self::Io { .. } => true,
            Self::UnsupportedBackend { .. }
            | Self::InvalidStorageKey { .. }
            | Self::MissingBlob { .. }
            | Self::PreconditionFailed { .. }
            | Self::MissingParent { .. } => false,
        }
    }

    #[cfg(feature = "local")]
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
