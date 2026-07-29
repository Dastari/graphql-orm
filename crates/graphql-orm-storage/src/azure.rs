use std::fmt;

use async_trait::async_trait;

use crate::{
    BlobBody, BlobListPage, BlobMetadata, BlobPutOptions, BlobStore, BlobWriteOutcome,
    ObjectStorage, StorageBackend, StorageByteStream, StorageError, StorageObjectBody,
    StoredObject, unsupported_backend,
};

/// Configuration for a future Azure Blob Storage backend.
#[derive(Clone, PartialEq, Eq)]
pub struct AzureBlobStorageConfig {
    /// Azure storage account name, when not using a connection string.
    pub account: Option<String>,
    /// Azure connection string. Redacted from debug output.
    pub connection_string: Option<String>,
    /// Blob container name.
    pub container: String,
    /// Optional key prefix prepended by the provider implementation.
    pub key_prefix: Option<String>,
    /// Provider credential material. Redacted from debug output.
    pub credential: Option<String>,
}

impl fmt::Debug for AzureBlobStorageConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AzureBlobStorageConfig")
            .field("account", &self.account)
            .field(
                "connection_string",
                &self.connection_string.as_ref().map(|_| "<redacted>"),
            )
            .field("container", &self.container)
            .field("key_prefix", &self.key_prefix)
            .field(
                "credential",
                &self.credential.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Placeholder Azure Blob Storage backend.
///
/// This type exposes the planned provider shape behind the `azure` feature, but
/// object operations return [`StorageError::UnsupportedBackend`] until real
/// Azure Blob support is implemented.
#[derive(Clone, Debug)]
pub struct AzureBlobStorageBackend {
    config: AzureBlobStorageConfig,
}

impl AzureBlobStorageBackend {
    /// Creates a new unsupported Azure Blob backend placeholder.
    #[must_use]
    pub fn new(config: AzureBlobStorageConfig) -> Self {
        Self { config }
    }

    /// Returns the backend configuration.
    #[must_use]
    pub const fn config(&self) -> &AzureBlobStorageConfig {
        &self.config
    }
}

#[async_trait]
impl BlobStore for AzureBlobStorageBackend {
    fn backend(&self) -> StorageBackend {
        StorageBackend::AzureBlob
    }

    async fn put_blob(
        &self,
        _key: &str,
        _body: StorageByteStream,
        _options: BlobPutOptions,
    ) -> Result<BlobWriteOutcome, StorageError> {
        Err(unsupported_backend(StorageBackend::AzureBlob))
    }

    async fn put_blob_if_not_exists(
        &self,
        _key: &str,
        _body: StorageByteStream,
        _options: BlobPutOptions,
    ) -> Result<Option<BlobWriteOutcome>, StorageError> {
        Err(unsupported_backend(StorageBackend::AzureBlob))
    }

    async fn get_blob(&self, _key: &str) -> Result<BlobBody, StorageError> {
        Err(unsupported_backend(StorageBackend::AzureBlob))
    }

    async fn get_blob_range(
        &self,
        _key: &str,
        _range: std::ops::Range<u64>,
    ) -> Result<BlobBody, StorageError> {
        Err(unsupported_backend(StorageBackend::AzureBlob))
    }

    async fn blob_exists(&self, _key: &str) -> Result<bool, StorageError> {
        Err(unsupported_backend(StorageBackend::AzureBlob))
    }

    async fn head_blob(&self, _key: &str) -> Result<Option<BlobMetadata>, StorageError> {
        Err(unsupported_backend(StorageBackend::AzureBlob))
    }

    async fn list_blobs_page(
        &self,
        _prefix: &str,
        _continuation: Option<String>,
        _limit: usize,
    ) -> Result<BlobListPage, StorageError> {
        Err(unsupported_backend(StorageBackend::AzureBlob))
    }

    async fn copy_blob(&self, _from: &str, _to: &str) -> Result<(), StorageError> {
        Err(unsupported_backend(StorageBackend::AzureBlob))
    }

    async fn delete_blob(&self, _key: &str) -> Result<(), StorageError> {
        Err(unsupported_backend(StorageBackend::AzureBlob))
    }
}

#[async_trait]
impl ObjectStorage for AzureBlobStorageBackend {
    async fn put_object(
        &self,
        _object: StoredObject,
        _bytes: Vec<u8>,
    ) -> Result<StoredObject, StorageError> {
        Err(unsupported_backend(StorageBackend::AzureBlob))
    }

    async fn get_object(&self, _object: &StoredObject) -> Result<StorageObjectBody, StorageError> {
        Err(unsupported_backend(StorageBackend::AzureBlob))
    }

    async fn delete_object(&self, _object: &StoredObject) -> Result<(), StorageError> {
        Err(unsupported_backend(StorageBackend::AzureBlob))
    }
}
