use std::{
    fmt,
    path::{Component, Path},
    pin::Pin,
};

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures_core::Stream;
use futures_util::{StreamExt, stream};
use time::OffsetDateTime;

use crate::{StorageBackend, StorageError};

/// Boxed stream of storage byte chunks.
pub type BoxedStorageStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send + 'static>>;

/// Streaming object or blob body.
pub struct StorageByteStream {
    inner: BoxedStorageStream,
    size_hint: Option<u64>,
}

impl StorageByteStream {
    /// Creates a stream without a known size.
    #[must_use]
    pub fn new(inner: BoxedStorageStream) -> Self {
        Self {
            inner,
            size_hint: None,
        }
    }

    /// Creates a stream with a known byte size.
    #[must_use]
    pub fn with_size_hint(inner: BoxedStorageStream, size_hint: u64) -> Self {
        Self {
            inner,
            size_hint: Some(size_hint),
        }
    }

    /// Creates a single-chunk stream from bytes.
    #[must_use]
    pub fn from_bytes(bytes: impl Into<Bytes>) -> Self {
        let bytes = bytes.into();
        let size_hint = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        Self::with_size_hint(Box::pin(stream::once(async move { Ok(bytes) })), size_hint)
    }

    /// Returns the known size hint when available.
    #[must_use]
    pub const fn size_hint(&self) -> Option<u64> {
        self.size_hint
    }

    /// Consumes the wrapper and returns the inner stream.
    #[must_use]
    pub fn into_inner(self) -> BoxedStorageStream {
        self.inner
    }
}

impl fmt::Debug for StorageByteStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StorageByteStream")
            .field("size_hint", &self.size_hint)
            .finish_non_exhaustive()
    }
}

/// Collects a storage byte stream into one byte buffer.
///
/// # Errors
///
/// Returns [`StorageError`] when the stream yields an error.
pub async fn collect_storage_stream(stream: StorageByteStream) -> Result<Bytes, StorageError> {
    let capacity = stream
        .size_hint()
        .and_then(|size| usize::try_from(size).ok())
        .unwrap_or_default();
    let mut bytes = BytesMut::with_capacity(capacity);
    let mut inner = stream.into_inner();

    while let Some(chunk) = inner.next().await {
        bytes.extend_from_slice(&chunk?);
    }

    Ok(bytes.freeze())
}

/// Result of writing a blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobWriteOutcome {
    /// Number of bytes written.
    pub size_bytes: u64,
    /// Lowercase hexadecimal SHA-256 checksum for the bytes written.
    pub sha256_hex: String,
}

/// Provider metadata for an existing blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobMetadata {
    /// Provider-neutral blob key.
    pub key: String,
    /// Blob size in bytes, when available.
    pub size_bytes: Option<u64>,
    /// Lowercase hexadecimal SHA-256 checksum, when known by the provider.
    pub sha256_hex: Option<String>,
    /// Provider ETag, when available.
    pub etag: Option<String>,
    /// Last modified timestamp, when available.
    pub last_modified: Option<OffsetDateTime>,
}

/// Blob metadata plus a streaming byte body.
#[derive(Debug)]
pub struct BlobBody {
    /// Provider-neutral blob key.
    pub key: String,
    /// Provider metadata, when available.
    pub metadata: Option<BlobMetadata>,
    /// Streaming blob bytes.
    pub body: StorageByteStream,
}

/// Low-level key-addressed blob storage contract.
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Returns the provider identifier for this store.
    fn backend(&self) -> StorageBackend;

    /// Writes a blob stream.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the key is invalid or the provider cannot
    /// write the blob.
    async fn put_blob(
        &self,
        key: &str,
        body: StorageByteStream,
    ) -> Result<BlobWriteOutcome, StorageError>;

    /// Loads a blob stream.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the key is invalid or the provider cannot
    /// load the blob.
    async fn get_blob(&self, key: &str) -> Result<BlobBody, StorageError>;

    /// Checks whether a blob exists.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the key is invalid or the provider cannot
    /// check the blob.
    async fn blob_exists(&self, key: &str) -> Result<bool, StorageError>;

    /// Loads provider metadata for a blob.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the key is invalid or the provider cannot
    /// load metadata.
    async fn head_blob(&self, key: &str) -> Result<Option<BlobMetadata>, StorageError>;

    /// Lists blob keys under a prefix. An empty prefix lists all blobs.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the prefix is invalid or the provider cannot
    /// list blobs.
    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, StorageError>;

    /// Deletes a blob.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the key is invalid or the provider cannot
    /// delete the blob.
    async fn delete_blob(&self, key: &str) -> Result<(), StorageError>;
}

/// Validates a provider-neutral blob key.
///
/// # Errors
///
/// Returns [`StorageError::InvalidStorageKey`] when the key can escape the
/// provider namespace or cannot be represented as a safe relative path.
pub fn validate_blob_key(key: &str) -> Result<(), StorageError> {
    if key.is_empty()
        || key.contains('\\')
        || key.contains('\0')
        || key
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(StorageError::InvalidStorageKey {
            key: key.to_string(),
        });
    }

    let path = Path::new(key);
    if path.is_absolute() {
        return Err(StorageError::InvalidStorageKey {
            key: key.to_string(),
        });
    }

    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(StorageError::InvalidStorageKey {
                key: key.to_string(),
            });
        }
    }

    Ok(())
}

#[cfg(feature = "local")]
pub(crate) fn validate_blob_prefix(prefix: &str) -> Result<(), StorageError> {
    if prefix.is_empty() {
        Ok(())
    } else {
        validate_blob_key(prefix)
    }
}
