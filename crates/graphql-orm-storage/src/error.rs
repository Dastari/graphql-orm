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
    /// The provider's enforcement of conditional creates, which exclusive
    /// locks rely on, failed or could not complete its verification.
    ConditionalCreateUnverified,
}

/// Errors returned by storage services and provider backends.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// A single-attempt operation failed; retrying by key is unsafe.
    ///
    /// Non-retryable does not establish that the operation had no effect.
    #[error("single-attempt storage operation failed; outcome may be unknown")]
    SingleAttemptFailed {
        /// Original failure, preserved only in the error source chain.
        #[source]
        source: Box<StorageError>,
    },

    /// The provider does not implement the required operation safely.
    #[error("unsupported storage operation: {operation}")]
    UnsupportedOperation {
        /// Operation that requires an explicit provider implementation.
        operation: String,
    },

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

    /// A storage key is empty, absolute, or contains unsafe path components,
    /// or a provider's key policy refuses it.
    ///
    /// The message is fixed and never contains the key, because a key can
    /// reveal a repository path or a configured root prefix. The `key` field,
    /// and so this error's `Debug` output, holds it for host diagnostics only;
    /// never return either to an untrusted client.
    #[error("invalid storage key")]
    InvalidStorageKey {
        /// Rejected storage key. Host diagnostics only.
        key: String,
    },

    /// A requested blob is missing from the storage backend.
    ///
    /// The message never contains the key; the `key` field, and so `Debug`,
    /// holds it for host diagnostics only.
    #[error("storage blob is missing")]
    MissingBlob {
        /// Requested storage key. Host diagnostics only.
        key: String,
    },

    /// A conditional storage operation could not be applied.
    ///
    /// The message names the condition but never the key; the `key` field,
    /// and so `Debug`, holds it for host diagnostics only.
    #[error("storage precondition failed: {condition}")]
    PreconditionFailed {
        /// Storage key involved in the failed precondition. Host diagnostics
        /// only.
        key: String,
        /// Human-readable precondition description.
        condition: String,
    },

    /// A multipart upload operation was refused.
    ///
    /// The message never contains the key; the `key` field, and so `Debug`,
    /// holds it for host diagnostics only.
    #[error("multipart upload refused: {kind}")]
    Multipart {
        /// Blob key of the upload. Host diagnostics only.
        key: String,
        /// Why the operation was refused.
        kind: crate::MultipartErrorKind,
    },

    /// A local filesystem object path did not have a writable parent directory.
    ///
    /// The message never contains the path; the `path` field, and so
    /// `Debug`, holds it for host diagnostics only.
    #[error("local storage path has no parent")]
    MissingParent {
        /// Local path that had no usable parent directory. Host diagnostics
        /// only.
        path: PathBuf,
    },

    /// A filesystem operation failed.
    ///
    /// The message names the error kind but never the path; the `path`
    /// field, and so `Debug`, holds it for host diagnostics only. Whether it
    /// is retryable depends on that kind; see [`StorageError::is_retryable`].
    #[error("storage io error: {}", source.kind())]
    Io {
        /// Local path involved in the failed filesystem operation. Host
        /// diagnostics only.
        path: PathBuf,
        /// Original filesystem error.
        #[source]
        source: std::io::Error,
    },
}

/// Whether a filesystem error of `kind` may succeed when retried.
///
/// Kinds that name a condition a retry cannot change are not retryable:
/// denied permission, a read-only or full file system, an exceeded quota or
/// file size, invalid input, data or names, an unsupported operation, an
/// entry that already exists or is of the wrong type, and links or seeks the
/// file system refuses. Every other kind, including interrupted and timed-out
/// operations and kinds the standard library does not categorize, stays
/// retryable.
#[must_use]
pub fn io_error_is_retryable(kind: std::io::ErrorKind) -> bool {
    use std::io::ErrorKind;
    !matches!(
        kind,
        ErrorKind::PermissionDenied
            | ErrorKind::ReadOnlyFilesystem
            | ErrorKind::StorageFull
            | ErrorKind::QuotaExceeded
            | ErrorKind::FileTooLarge
            | ErrorKind::InvalidInput
            | ErrorKind::InvalidData
            | ErrorKind::InvalidFilename
            | ErrorKind::Unsupported
            | ErrorKind::AlreadyExists
            | ErrorKind::IsADirectory
            | ErrorKind::NotADirectory
            | ErrorKind::DirectoryNotEmpty
            | ErrorKind::CrossesDevices
            | ErrorKind::TooManyLinks
            | ErrorKind::NotSeekable
    )
}

impl StorageError {
    /// Returns whether this failure permits retrying the operation.
    ///
    /// A filesystem error is retryable unless its kind names a condition a
    /// retry cannot change, such as denied permission, a full or read-only
    /// file system, an exceeded quota, or an invalid name or argument; see
    /// [`io_error_is_retryable`].
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Provider { retryable, .. } => *retryable,
            Self::RemoteProvider { retryable, .. } => *retryable,
            Self::Io { source, .. } => io_error_is_retryable(source.kind()),
            Self::SingleAttemptFailed { .. }
            | Self::UnsupportedOperation { .. }
            | Self::UnsupportedBackend { .. }
            | Self::InvalidStorageKey { .. }
            | Self::MissingBlob { .. }
            | Self::PreconditionFailed { .. }
            | Self::Multipart { .. }
            | Self::MissingParent { .. } => false,
        }
    }

    #[cfg(any(feature = "local", feature = "s3", feature = "smb"))]
    pub(crate) fn single_attempt(source: Self) -> Self {
        Self::SingleAttemptFailed {
            source: Box::new(source),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    fn io(kind: ErrorKind) -> StorageError {
        StorageError::Io {
            path: PathBuf::from("/repository/secret-root/objects/key.bin"),
            source: Error::from(kind),
        }
    }

    /// Permission and capacity failures are not retried; transient ones are
    /// (#540).
    #[test]
    fn io_errors_are_retryable_only_for_transient_kinds() {
        for kind in [
            ErrorKind::PermissionDenied,
            ErrorKind::StorageFull,
            ErrorKind::QuotaExceeded,
            ErrorKind::ReadOnlyFilesystem,
            ErrorKind::InvalidInput,
            ErrorKind::InvalidFilename,
            ErrorKind::IsADirectory,
            ErrorKind::NotADirectory,
        ] {
            assert!(!io(kind).is_retryable(), "{kind:?}");
        }
        for kind in [
            ErrorKind::Interrupted,
            ErrorKind::TimedOut,
            ErrorKind::WouldBlock,
            ErrorKind::ConnectionReset,
            ErrorKind::ResourceBusy,
            ErrorKind::Other,
        ] {
            assert!(io(kind).is_retryable(), "{kind:?}");
        }
    }

    /// No message repeats a key or path, which can reveal a repository path
    /// or configured root prefix; `Debug` keeps them for hosts (#540).
    #[test]
    fn messages_never_repeat_keys_or_paths() {
        let key = "secret-root/objects/key.bin".to_string();
        let errors = [
            StorageError::MissingBlob { key: key.clone() },
            StorageError::PreconditionFailed {
                key: key.clone(),
                condition: "object already exists".to_string(),
            },
            StorageError::Multipart {
                key: key.clone(),
                kind: crate::MultipartErrorKind::UnknownUpload,
            },
            StorageError::MissingParent {
                path: PathBuf::from("/repository/secret-root"),
            },
            io(ErrorKind::PermissionDenied),
            StorageError::InvalidStorageKey { key: key.clone() },
        ];
        for error in &errors {
            let message = error.to_string();
            assert!(!message.contains("secret-root"), "{message}");
            assert!(format!("{error:?}").contains("secret-root"), "{error:?}");
        }
        assert_eq!(
            errors[1].to_string(),
            "storage precondition failed: object already exists"
        );
        assert_eq!(errors[4].to_string(), "storage io error: permission denied");
    }
}
