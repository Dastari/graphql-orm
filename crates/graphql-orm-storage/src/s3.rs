use std::fmt;

use async_trait::async_trait;

use crate::{
    ObjectStorage, StorageBackend, StorageError, StorageObjectBody, StoredObject,
    unsupported_backend,
};

/// Configuration for a future S3-compatible storage backend.
#[derive(Clone, PartialEq, Eq)]
pub struct S3StorageConfig {
    /// S3-compatible endpoint URL.
    pub endpoint_url: String,
    /// Provider region.
    pub region: String,
    /// Bucket name.
    pub bucket: String,
    /// Optional key prefix prepended by the provider implementation.
    pub key_prefix: Option<String>,
    /// Access key identifier.
    pub access_key_id: String,
    /// Secret access key. Redacted from debug output.
    pub secret_access_key: String,
    /// Whether to use path-style addressing.
    pub path_style: bool,
}

impl fmt::Debug for S3StorageConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3StorageConfig")
            .field("endpoint_url", &self.endpoint_url)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("key_prefix", &self.key_prefix)
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .field("path_style", &self.path_style)
            .finish()
    }
}

/// Placeholder S3-compatible storage backend.
///
/// This type exposes the planned provider shape behind the `s3` feature, but
/// object operations return [`StorageError::UnsupportedBackend`] until real S3
/// support is implemented.
#[derive(Clone, Debug)]
pub struct S3StorageBackend {
    config: S3StorageConfig,
}

impl S3StorageBackend {
    /// Creates a new unsupported S3-compatible backend placeholder.
    #[must_use]
    pub fn new(config: S3StorageConfig) -> Self {
        Self { config }
    }

    /// Returns the backend configuration.
    #[must_use]
    pub const fn config(&self) -> &S3StorageConfig {
        &self.config
    }
}

#[async_trait]
impl ObjectStorage for S3StorageBackend {
    fn backend(&self) -> StorageBackend {
        StorageBackend::S3
    }

    async fn put_object(
        &self,
        _object: StoredObject,
        _bytes: Vec<u8>,
    ) -> Result<StoredObject, StorageError> {
        Err(unsupported_backend(StorageBackend::S3))
    }

    async fn get_object(&self, _object: &StoredObject) -> Result<StorageObjectBody, StorageError> {
        Err(unsupported_backend(StorageBackend::S3))
    }

    async fn delete_object(&self, _object: &StoredObject) -> Result<(), StorageError> {
        Err(unsupported_backend(StorageBackend::S3))
    }
}
