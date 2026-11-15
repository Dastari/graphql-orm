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

/// Largest buffer [`collect_storage_stream`] preallocates from a size hint.
/// Providers may report a size the remote side declared, so the hint is never
/// trusted for allocation beyond this; larger bodies grow as bytes arrive.
const MAX_COLLECT_PREALLOCATION: usize = 8 * 1024 * 1024;

/// Collects a storage byte stream into one byte buffer.
///
/// The whole body is held in memory. Use it only for bodies whose size the
/// caller bounds; the size hint is used for preallocation up to 8 MiB only.
///
/// # Errors
///
/// Returns [`StorageError`] when the stream yields an error.
pub async fn collect_storage_stream(stream: StorageByteStream) -> Result<Bytes, StorageError> {
    let capacity = stream
        .size_hint()
        .and_then(|size| usize::try_from(size).ok())
        .unwrap_or_default()
        .min(MAX_COLLECT_PREALLOCATION);
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

    /// Atomically writes a blob only when the destination key does not exist.
    ///
    /// Implementations must preserve create-if-absent across all writers.
    /// Existence checks followed by writes do not satisfy this contract.
    /// The default fails closed without provider I/O.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the key is invalid or the provider cannot
    /// conditionally write the blob.
    async fn put_blob_if_not_exists(
        &self,
        _key: &str,
        _body: StorageByteStream,
        _options: BlobPutOptions,
    ) -> Result<Option<BlobWriteOutcome>, StorageError> {
        Err(StorageError::UnsupportedOperation {
            operation: "atomic conditional blob write".to_string(),
        })
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

    /// Lists one page of blob keys that start with a prefix, in byte order.
    ///
    /// The prefix is a plain string prefix of the key on every provider: `a`
    /// matches `a/x`, `a-b/x` and `ab`, while `a/` matches only keys below
    /// `a/`. An empty prefix lists all blobs. A non-empty prefix must be a
    /// valid key (see [`validate_blob_key`]), optionally followed by one `/`.
    ///
    /// A listing returns only keys the provider accepts: a stored name that
    /// is not a valid key, for example one that is not valid Unicode or lies
    /// in the reserved internal namespace, is skipped.
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

    /// Lists all blob keys that start with a prefix, in byte order. The
    /// prefix follows the contract of [`Self::list_blobs_page`].
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

    /// Deletes a blob without replaying deletion against a replacement object.
    ///
    /// After a deletion may have reached the server, implementations must never
    /// retry by key. A lost response leaves the outcome unknown. This capability
    /// is required for lock release; ordinary idempotent deletion is insufficient
    /// when another writer can immediately reuse the key. Cancellation must not
    /// schedule a later replay. The default fails closed without provider I/O.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::UnsupportedOperation`] unless the provider opts in.
    /// Operation failures must be non-retryable (for example,
    /// [`StorageError::SingleAttemptFailed`]). This does not mean the operation
    /// had no effect. Never retry an ambiguous failure.
    async fn delete_blob_once(&self, _key: &str) -> Result<(), StorageError> {
        Err(StorageError::UnsupportedOperation {
            operation: "single-attempt blob deletion".to_string(),
        })
    }

    /// Deletes a blob.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the key is invalid or the provider cannot
    /// delete the blob.
    async fn delete_blob(&self, key: &str) -> Result<(), StorageError>;

    /// Streams an existing blob and returns its size and SHA-256.
    ///
    /// Use it to verify an object whose write did not report a digest, such as
    /// an S3 multipart object, for example from a background job. It reads the
    /// whole object.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the key is invalid, the blob is missing or
    /// the provider cannot read it.
    async fn compute_blob_sha256(&self, key: &str) -> Result<BlobWriteOutcome, StorageError> {
        use futures_util::StreamExt;
        use sha2::Digest;
        let mut stream = self.get_blob(key).await?.body.into_inner();
        let mut hasher = sha2::Sha256::new();
        let mut size_bytes = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            size_bytes = size_bytes.saturating_add(chunk.len() as u64);
            hasher.update(&chunk);
        }
        Ok(BlobWriteOutcome {
            size_bytes,
            sha256_hex: format!("{:x}", hasher.finalize()),
        })
    }

    /// Returns this provider's resumable multipart uploads, if it supports them.
    ///
    /// The default returns none. Providers that implement
    /// [`MultipartBlobStore`](crate::MultipartBlobStore) return themselves.
    fn as_multipart(&self) -> Option<&dyn crate::MultipartBlobStore> {
        None
    }
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

/// Name of the internal namespace providers reserve at the root of a store,
/// for example for local multipart staging and object metadata.
pub(crate) const INTERNAL_NAMESPACE: &str = ".graphql-orm-storage";

/// Whether a key's first component names the reserved internal namespace.
/// The comparison ignores ASCII case and trailing dots and spaces, which
/// case-insensitive filesystems and SMB servers treat as the same name.
fn is_internal_namespace(component: &str) -> bool {
    component
        .trim_end_matches(['.', ' '])
        .eq_ignore_ascii_case(INTERNAL_NAMESPACE)
}

/// Validates a provider-neutral blob key.
///
/// A key's first component must not name the reserved internal namespace,
/// `.graphql-orm-storage`, in any ASCII case and with or without trailing dots
/// or spaces. The same name further down a key, such as
/// `data/.graphql-orm-storage`, is an ordinary component.
///
/// # Errors
///
/// Returns [`StorageError::InvalidStorageKey`] when the key can escape the
/// provider namespace, cannot be represented as a safe relative path, or lies
/// in the reserved internal namespace.
pub fn validate_blob_key(key: &str) -> Result<(), StorageError> {
    if key.is_empty()
        || key.contains('\\')
        || key.contains('\0')
        || key
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
        || key.split('/').next().is_some_and(is_internal_namespace)
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

/// Validates a blob key for SMB servers and Windows file systems.
///
/// Applies [`validate_blob_key`], and also refuses a key with any component
/// that such targets cannot store under the name as written, or that they
/// treat as another name:
///
/// - `:`, the alternate data stream separator, and the characters
///   `< > " | ? *`;
/// - control characters U+0001 to U+001F;
/// - a trailing `.` or space, which Windows strips, so `report.` and
///   `report` would name the same file;
/// - a reserved device name, in any ASCII case and with or without an
///   extension: `CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, and `COM` or
///   `LPT` followed by a digit `0`-`9` or a superscript `¹`, `²` or `³`. A
///   name such as `CON.txt` or `CON .txt` opens the device, not a file.
///
/// It does not fold case: on a case-insensitive target, keys that differ only
/// in case still name the same object. Keys that are valid here are valid
/// everywhere `validate_blob_key` keys are.
///
/// # Errors
///
/// Returns [`StorageError::InvalidStorageKey`] when the key fails
/// [`validate_blob_key`] or any of the rules above.
pub fn validate_portable_blob_key(key: &str) -> Result<(), StorageError> {
    validate_blob_key(key)?;
    if key.split('/').all(is_portable_component) {
        Ok(())
    } else {
        Err(StorageError::InvalidStorageKey {
            key: key.to_string(),
        })
    }
}

/// Whether one complete key component is portable (see
/// [`validate_portable_blob_key`]).
fn is_portable_component(component: &str) -> bool {
    has_portable_characters(component)
        && !component.ends_with(['.', ' '])
        && !is_reserved_device_name(component)
}

/// Whether `text` avoids every character Windows refuses in a name.
fn has_portable_characters(text: &str) -> bool {
    !text.chars().any(|character| {
        matches!(character, ':' | '<' | '>' | '"' | '|' | '?' | '*')
            || ('\u{1}'..='\u{1f}').contains(&character)
    })
}

/// Whether `component` opens a Windows device: its part before the first
/// `.`, without trailing spaces, is a reserved device name in any ASCII case.
fn is_reserved_device_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim_end_matches(' ');
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
        .iter()
        .any(|name| stem.eq_ignore_ascii_case(name))
    {
        return true;
    }
    let mut characters = stem.chars();
    let device: String = characters.by_ref().take(3).collect();
    let rest: Vec<char> = characters.collect();
    (device.eq_ignore_ascii_case("COM") || device.eq_ignore_ascii_case("LPT"))
        && matches!(rest.as_slice(), [digit] if digit.is_ascii_digit() || matches!(digit, '\u{b9}' | '\u{b2}' | '\u{b3}'))
}

/// Validates a listing prefix: empty, or a valid key optionally followed by
/// one `/`.
#[cfg(any(feature = "local", feature = "s3", feature = "smb"))]
pub(crate) fn validate_blob_prefix(prefix: &str) -> Result<(), StorageError> {
    if prefix.is_empty() {
        return Ok(());
    }
    validate_blob_key(prefix.strip_suffix('/').unwrap_or(prefix)).map_err(|_| {
        StorageError::InvalidStorageKey {
            key: prefix.to_string(),
        }
    })
}
