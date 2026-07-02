use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm_storage::{
    BlobPutOptions, BlobStore, StorageByteStream, collect_storage_stream, validate_blob_key,
};

use crate::BackupError;

#[async_trait]
/// Key-addressed backup repository.
pub trait BackupRepository: Send + Sync {
    /// Writes a blob at a repository key.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the key is invalid or the backend cannot
    /// persist the blob.
    async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError>;

    /// Writes a blob only when no blob exists at the key.
    ///
    /// Returns `true` when the blob was written and `false` when the key
    /// already existed.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the key is invalid or the backend cannot
    /// perform the conditional write.
    async fn put_blob_if_absent(&self, key: &str, body: Bytes) -> Result<bool, BackupError> {
        if self.blob_exists(key).await? {
            return Ok(false);
        }
        self.put_blob(key, body).await?;
        Ok(true)
    }

    /// Reads a blob from a repository key.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the key is invalid, the blob is missing, or
    /// the backend cannot read it.
    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError>;

    /// Checks whether a blob exists.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the key is invalid or the backend cannot
    /// check metadata.
    async fn blob_exists(&self, key: &str) -> Result<bool, BackupError>;

    /// Lists blobs below a key prefix.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the prefix is invalid or the backend cannot
    /// list blobs.
    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError>;

    /// Deletes a blob if it exists.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the key is invalid or the backend cannot
    /// delete the blob.
    async fn delete_blob(&self, key: &str) -> Result<(), BackupError>;
}

/// [`BackupRepository`] adapter over a `graphql-orm-storage` [`BlobStore`].
#[derive(Clone)]
pub struct BlobStoreBackupRepository {
    store: Arc<dyn BlobStore>,
    prefix: Option<String>,
}

impl BlobStoreBackupRepository {
    /// Creates an adapter without a repository prefix.
    #[must_use]
    pub fn new(store: Arc<dyn BlobStore>) -> Self {
        Self {
            store,
            prefix: None,
        }
    }

    /// Creates an adapter rooted under a blob-store prefix.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the prefix is not a safe blob key.
    pub fn with_prefix(
        store: Arc<dyn BlobStore>,
        prefix: impl Into<String>,
    ) -> Result<Self, BackupError> {
        let prefix = prefix.into();
        validate_blob_key(&prefix)?;
        Ok(Self {
            store,
            prefix: Some(prefix),
        })
    }

    /// Returns the wrapped blob store.
    #[must_use]
    pub fn store(&self) -> Arc<dyn BlobStore> {
        Arc::clone(&self.store)
    }

    fn apply_prefix(&self, key: &str) -> String {
        match &self.prefix {
            Some(prefix) => format!("{prefix}/{key}"),
            None => key.to_string(),
        }
    }

    fn apply_prefix_to_list(&self, prefix: &str) -> String {
        match (&self.prefix, prefix.is_empty()) {
            (Some(repository_prefix), true) => repository_prefix.clone(),
            (Some(repository_prefix), false) => format!("{repository_prefix}/{prefix}"),
            (None, _) => prefix.to_string(),
        }
    }

    fn strip_prefix(&self, key: String) -> String {
        let Some(prefix) = &self.prefix else {
            return key;
        };
        key.strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(&key)
            .to_string()
    }
}

impl std::fmt::Debug for BlobStoreBackupRepository {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BlobStoreBackupRepository")
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl BackupRepository for BlobStoreBackupRepository {
    async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError> {
        self.store
            .put_blob(
                &self.apply_prefix(key),
                StorageByteStream::from_bytes(body),
                BlobPutOptions::default(),
            )
            .await?;
        Ok(())
    }

    async fn put_blob_if_absent(&self, key: &str, body: Bytes) -> Result<bool, BackupError> {
        let outcome = self
            .store
            .put_blob_if_not_exists(
                &self.apply_prefix(key),
                StorageByteStream::from_bytes(body),
                BlobPutOptions::default(),
            )
            .await?;
        Ok(outcome.is_some())
    }

    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
        let body = self.store.get_blob(&self.apply_prefix(key)).await?;
        Ok(collect_storage_stream(body.body).await?)
    }

    async fn blob_exists(&self, key: &str) -> Result<bool, BackupError> {
        Ok(self.store.blob_exists(&self.apply_prefix(key)).await?)
    }

    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError> {
        let keys = self
            .store
            .list_blobs(&self.apply_prefix_to_list(prefix))
            .await?;
        Ok(keys.into_iter().map(|key| self.strip_prefix(key)).collect())
    }

    async fn delete_blob(&self, key: &str) -> Result<(), BackupError> {
        Ok(self.store.delete_blob(&self.apply_prefix(key)).await?)
    }
}
