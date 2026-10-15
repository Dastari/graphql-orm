use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm_storage::{LocalStorageBackend, StorageByteStream};

use crate::{BackupCapacityLimits, BackupError, BackupRepository, BlobStoreBackupRepository};

/// Local filesystem implementation of [`BackupRepository`].
#[derive(Clone, Debug)]
pub struct LocalBackupRepository {
    inner: BlobStoreBackupRepository,
}

impl LocalBackupRepository {
    /// Creates a local repository rooted at a filesystem path.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            inner: BlobStoreBackupRepository::new(Arc::new(LocalStorageBackend::new(root))),
        }
    }

    /// Configures per-payload buffering and decoding limits.
    #[must_use]
    pub fn with_capacity_limits(mut self, limits: BackupCapacityLimits) -> Self {
        self.inner = self.inner.with_capacity_limits(limits);
        self
    }

    /// Opens an existing local repository root.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the path cannot be inspected or is not a
    /// directory.
    pub async fn open_existing(root: impl Into<PathBuf>) -> Result<Self, BackupError> {
        let root = root.into();
        let metadata = std::fs::metadata(&root).map_err(|source| BackupError::Io {
            path: root.clone(),
            source,
        })?;
        if !metadata.is_dir() {
            return Err(BackupError::InvalidRepositoryRoot { path: root });
        }

        Ok(Self::new(root))
    }
}

#[async_trait]
impl BackupRepository for LocalBackupRepository {
    fn capacity_limits(&self) -> BackupCapacityLimits { self.inner.capacity_limits() }
    async fn delete_blob_once(&self, key: &str) -> Result<(), BackupError> {
        self.inner.delete_blob_once(key).await
    }

    async fn put_blob_stream(&self, key: &str, body: StorageByteStream) -> Result<(), BackupError> {
        self.inner.put_blob_stream(key, body).await
    }
    async fn put_blob_stream_if_absent(&self, key: &str, body: StorageByteStream) -> Result<bool, BackupError> {
        self.inner.put_blob_stream_if_absent(key, body).await
    }
    async fn get_blob_stream(&self, key: &str) -> Result<StorageByteStream, BackupError> {
        self.inner.get_blob_stream(key).await
    }
    async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError> {
        self.inner.put_blob(key, body).await
    }

    async fn put_blob_if_absent(&self, key: &str, body: Bytes) -> Result<bool, BackupError> {
        self.inner.put_blob_if_absent(key, body).await
    }

    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
        self.inner.get_blob(key).await
    }

    async fn blob_exists(&self, key: &str) -> Result<bool, BackupError> {
        self.inner.blob_exists(key).await
    }

    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError> {
        self.inner.list_blobs(prefix).await
    }

    async fn delete_blob(&self, key: &str) -> Result<(), BackupError> {
        self.inner.delete_blob(key).await
    }
}
