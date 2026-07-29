use std::{collections::BTreeMap, ops::Range};

use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{StorageByteStream, StorageError};

/// Application-provided object metadata values.
pub type ObjectMetadata = BTreeMap<String, String>;

/// Metadata for a completed bucket/key object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectInfo {
    /// Logical storage bucket.
    pub bucket: String,
    /// Object key inside the bucket.
    pub key: String,
    /// Provider content type, when supplied.
    pub content_type: Option<String>,
    /// Application metadata stored with the object.
    pub metadata: ObjectMetadata,
    /// Object size in bytes.
    pub size_bytes: u64,
    /// Lowercase hexadecimal SHA-256 checksum for the object bytes.
    pub sha256_hex: String,
    /// Provider ETag or equivalent checksum, when available.
    pub etag: Option<String>,
    /// UTC completion timestamp.
    pub last_modified: OffsetDateTime,
}

/// Byte range metadata for a partial object response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectContentRange {
    /// Inclusive range start.
    pub start: u64,
    /// Exclusive range end.
    pub end: u64,
    /// Total object size in bytes.
    pub total_size: u64,
}

impl ObjectContentRange {
    /// Returns the number of bytes in this range.
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.end - self.start
    }

    /// Returns whether the range is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// Streaming response for an object byte range.
#[derive(Debug)]
pub struct ObjectRangeBody {
    /// Completed object metadata.
    pub object: ObjectInfo,
    /// Returned range metadata.
    pub range: ObjectContentRange,
    /// Number of bytes in the returned body.
    pub content_length: u64,
    /// Streaming range body.
    pub body: StorageByteStream,
}

/// Boxed multipart object writer.
pub type BoxedMultipartWriter = Box<dyn MultipartWriter>;

/// Incremental object writer for large uploads.
#[async_trait]
pub trait MultipartWriter: Send {
    /// Appends one byte chunk to the in-progress object.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the provider cannot write the chunk.
    async fn write_chunk(&mut self, bytes: Bytes) -> Result<(), StorageError>;

    /// Finalizes the object and makes it visible to readers and listings.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the provider cannot atomically finalize the
    /// object.
    async fn complete(self: Box<Self>) -> Result<ObjectInfo, StorageError>;

    /// Aborts the in-progress object and removes temporary state.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the provider cannot remove temporary
    /// state.
    async fn abort(self: Box<Self>) -> Result<(), StorageError>;
}

/// Bucket/key streaming object API for large media or recording workloads.
#[async_trait]
pub trait StreamingObjectStore: Send + Sync {
    /// Stores an object from a byte stream.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the bucket/key is invalid or the provider
    /// cannot store the stream.
    async fn put_object_stream(
        &self,
        bucket: &str,
        key: &str,
        content_type: Option<String>,
        metadata: ObjectMetadata,
        stream: StorageByteStream,
    ) -> Result<ObjectInfo, StorageError>;

    /// Creates a multipart writer for an object.
    ///
    /// The object is not visible to readers or listings until
    /// [`MultipartWriter::complete`] succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the bucket/key is invalid or the provider
    /// cannot create the writer.
    async fn create_multipart_object(
        &self,
        bucket: &str,
        key: &str,
        content_type: Option<String>,
        metadata: ObjectMetadata,
    ) -> Result<BoxedMultipartWriter, StorageError>;

    /// Loads a byte range from an object.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the bucket/key or range is invalid, the
    /// object is missing, or the provider cannot stream the range.
    async fn get_object_range(
        &self,
        bucket: &str,
        key: &str,
        range: Range<u64>,
    ) -> Result<ObjectRangeBody, StorageError>;

    /// Loads completed object metadata.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the bucket/key is invalid, the object is
    /// missing, or the provider cannot load metadata.
    async fn get_object_metadata(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<ObjectInfo, StorageError>;

    /// Lists completed objects in a bucket whose keys start with `prefix`.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the bucket/prefix is invalid or the
    /// provider cannot list objects.
    async fn list_objects(
        &self,
        bucket: &str,
        prefix: &str,
    ) -> Result<Vec<ObjectInfo>, StorageError>;

    /// Deletes a completed object and its metadata.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the bucket/key is invalid or the provider
    /// cannot delete object state.
    async fn delete_object(&self, bucket: &str, key: &str) -> Result<(), StorageError>;
}

/// Validates a logical object bucket name.
///
/// # Errors
///
/// Returns [`StorageError::InvalidStorageKey`] when the bucket name is empty or
/// path-like.
pub fn validate_object_bucket(bucket: &str) -> Result<(), StorageError> {
    crate::validate_blob_key(bucket)?;
    if bucket.contains('/') {
        return Err(StorageError::InvalidStorageKey {
            key: bucket.to_string(),
        });
    }
    Ok(())
}
