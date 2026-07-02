use async_trait::async_trait;
use bytes::Bytes;

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
