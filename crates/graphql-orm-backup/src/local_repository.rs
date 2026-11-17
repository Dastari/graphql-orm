use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm_storage::{LocalStorageBackend, StorageByteStream};

use crate::{
    BackupCapacityLimits, BackupError, BackupRepository, BlobStoreBackupRepository,
    ManifestAuthenticator,
};

/// Local filesystem implementation of [`BackupRepository`].
#[derive(Clone, Debug)]
pub struct LocalBackupRepository {
    inner: BlobStoreBackupRepository,
}

impl LocalBackupRepository {
    /// Creates a local repository rooted at a filesystem path.
    ///
    /// Not available on Windows: its path operations cannot promise the link
    /// boundary this constructor stands for. Use
    /// [`Self::new_trusted_windows_root`] there.
    #[cfg(not(windows))]
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::from_backend(LocalStorageBackend::new(root))
    }

    /// Creates a local repository over a host-controlled Windows root.
    ///
    /// Links are not enforced as a security boundary: the host must prevent
    /// other processes from changing the root, its ancestors and descendants
    /// while the repository is in use. The plain constructor `new` does not
    /// exist on Windows so that a host cannot obtain a repository by accident.
    /// See [`LocalStorageBackend::new_trusted_windows_root`].
    ///
    /// On Unix this is equivalent to `new`; the link boundary remains
    /// enforced there.
    #[must_use]
    pub fn new_trusted_windows_root(root: impl Into<PathBuf>) -> Self {
        Self::from_backend(LocalStorageBackend::new_trusted_windows_root(root))
    }

    fn from_backend(backend: LocalStorageBackend) -> Self {
        Self {
            inner: BlobStoreBackupRepository::new(Arc::new(backend)),
        }
    }

    /// Configures per-payload buffering and decoding limits.
    #[must_use]
    pub fn with_capacity_limits(mut self, limits: BackupCapacityLimits) -> Self {
        self.inner = self.inner.with_capacity_limits(limits);
        self
    }

    /// Seals and verifies manifests with a host-held key.
    ///
    /// Required: without it every manifest read or write fails closed.
    #[must_use]
    pub fn with_manifest_authenticator(mut self, authenticator: ManifestAuthenticator) -> Self {
        self.inner = self.inner.with_manifest_authenticator(authenticator);
        self
    }

    /// Opens an existing local repository root.
    ///
    /// Not available on Windows; use
    /// [`Self::open_existing_trusted_windows_root`] there.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the path cannot be inspected or is not a
    /// directory.
    #[cfg(not(windows))]
    pub async fn open_existing(root: impl Into<PathBuf>) -> Result<Self, BackupError> {
        let root = check_root_directory(root.into()).await?;
        Ok(Self::new(root))
    }

    /// Opens an existing local repository root on a host-controlled Windows
    /// tree.
    ///
    /// Links are not enforced as a security boundary: the host must prevent
    /// other processes from changing the root, its ancestors and descendants
    /// while the repository is in use. See [`Self::new_trusted_windows_root`].
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the path cannot be inspected or is not a
    /// directory.
    pub async fn open_existing_trusted_windows_root(
        root: impl Into<PathBuf>,
    ) -> Result<Self, BackupError> {
        let root = check_root_directory(root.into()).await?;
        Ok(Self::new_trusted_windows_root(root))
    }
}

async fn check_root_directory(root: PathBuf) -> Result<PathBuf, BackupError> {
    let metadata = tokio::fs::metadata(&root)
        .await
        .map_err(|source| BackupError::Io {
            path: root.clone(),
            source,
        })?;
    if !metadata.is_dir() {
        return Err(BackupError::InvalidRepositoryRoot { path: root });
    }
    Ok(root)
}

#[async_trait]
impl BackupRepository for LocalBackupRepository {
    fn capacity_limits(&self) -> BackupCapacityLimits {
        self.inner.capacity_limits()
    }
    fn manifest_authenticator(&self) -> Option<&ManifestAuthenticator> {
        self.inner.manifest_authenticator()
    }
    async fn delete_blob_once(&self, key: &str) -> Result<(), BackupError> {
        self.inner.delete_blob_once(key).await
    }

    async fn put_blob_stream(&self, key: &str, body: StorageByteStream) -> Result<(), BackupError> {
        self.inner.put_blob_stream(key, body).await
    }
    async fn put_blob_stream_if_absent(
        &self,
        key: &str,
        body: StorageByteStream,
    ) -> Result<bool, BackupError> {
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
