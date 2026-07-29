use std::{
    fmt,
    ops::Range,
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

/// Provider options for writing a blob.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlobPutOptions {
    /// MIME content type to pass through to providers that support it.
    pub content_type: Option<String>,
}

/// One page of blob listing results.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlobListPage {
    /// Blob keys in this page.
    pub keys: Vec<String>,
    /// Opaque continuation token for the next page.
    pub next_continuation: Option<String>,
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
        options: BlobPutOptions,
    ) -> Result<BlobWriteOutcome, StorageError>;

    /// Writes a blob only when the destination key does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the key is invalid or the provider cannot
    /// conditionally write the blob.
    async fn put_blob_if_not_exists(
        &self,
        key: &str,
        body: StorageByteStream,
        options: BlobPutOptions,
    ) -> Result<Option<BlobWriteOutcome>, StorageError> {
        if self.blob_exists(key).await? {
            return Ok(None);
        }

        self.put_blob(key, body, options).await.map(Some)
    }

    /// Loads a blob stream.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the key is invalid or the provider cannot
    /// load the blob.
    async fn get_blob(&self, key: &str) -> Result<BlobBody, StorageError>;

    /// Loads a byte range from a blob stream.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the key or range is invalid, or when the
    /// provider cannot load the blob.
    async fn get_blob_range(&self, key: &str, range: Range<u64>) -> Result<BlobBody, StorageError> {
        if range.end < range.start {
            return Err(StorageError::PreconditionFailed {
                key: key.to_string(),
                condition: "range end is before range start".to_string(),
            });
        }

        let length = range.end - range.start;
        let blob = self.get_blob(key).await?;
        Ok(BlobBody {
            key: blob.key,
            metadata: blob.metadata,
            body: ranged_storage_stream(blob.body, range.start, length),
        })
    }

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

    /// Lists one page of blob keys under a prefix. An empty prefix lists all blobs.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the prefix is invalid or the provider cannot
    /// list blobs.
    async fn list_blobs_page(
        &self,
        prefix: &str,
        continuation: Option<String>,
        limit: usize,
    ) -> Result<BlobListPage, StorageError>;

    /// Lists blob keys under a prefix. An empty prefix lists all blobs.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the prefix is invalid or the provider cannot
    /// list blobs.
    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        let mut keys = Vec::new();
        let mut continuation = None;

        loop {
            let page = self.list_blobs_page(prefix, continuation, 1_000).await?;
            keys.extend(page.keys);
            continuation = page.next_continuation;
            if continuation.is_none() {
                break;
            }
        }

        Ok(keys)
    }

    /// Copies a blob to another key.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when either key is invalid or the provider cannot
    /// copy the blob.
    async fn copy_blob(&self, from: &str, to: &str) -> Result<(), StorageError> {
        let blob = self.get_blob(from).await?;
        self.put_blob(to, blob.body, BlobPutOptions::default())
            .await?;
        Ok(())
    }

    /// Deletes a blob.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the key is invalid or the provider cannot
    /// delete the blob.
    async fn delete_blob(&self, key: &str) -> Result<(), StorageError>;
}

fn ranged_storage_stream(body: StorageByteStream, skip: u64, length: u64) -> StorageByteStream {
    let size_hint = body
        .size_hint()
        .map(|hint| hint.saturating_sub(skip).min(length));
    let stream = stream::try_unfold(
        (body.into_inner(), skip, length),
        |(mut inner, mut skip, mut remaining)| async move {
            if remaining == 0 {
                return Ok(None);
            }

            while let Some(chunk) = inner.next().await {
                let chunk = chunk?;
                let chunk_len = u64::try_from(chunk.len()).unwrap_or(u64::MAX);

                if skip >= chunk_len {
                    skip -= chunk_len;
                    continue;
                }

                let start =
                    usize::try_from(skip).map_err(|_| StorageError::PreconditionFailed {
                        key: String::new(),
                        condition: "range start cannot fit in memory".to_string(),
                    })?;
                let take_len = (chunk_len - skip).min(remaining);
                let end = start
                    + usize::try_from(take_len).map_err(|_| StorageError::PreconditionFailed {
                        key: String::new(),
                        condition: "range length cannot fit in memory".to_string(),
                    })?;
                let output = chunk.slice(start..end);
                remaining -= take_len;

                return Ok(Some((output, (inner, 0, remaining))));
            }

            Ok(None)
        },
    );

    match size_hint {
        Some(size_hint) => StorageByteStream::with_size_hint(Box::pin(stream), size_hint),
        None => StorageByteStream::new(Box::pin(stream)),
    }
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

#[cfg(any(feature = "local", feature = "smb"))]
pub(crate) fn validate_blob_prefix(prefix: &str) -> Result<(), StorageError> {
    if prefix.is_empty() {
        Ok(())
    } else {
        validate_blob_key(prefix)
    }
}
