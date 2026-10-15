use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm_storage::{
    BlobPutOptions, BlobStore, StorageByteStream, validate_blob_key,
};

use crate::{BackupError, BackupCapacityLimits, collect_backup_stream};

#[async_trait]
/// Key-addressed backup repository.
pub trait BackupRepository: Send + Sync {
    /// Per-payload limits for buffered compatibility APIs and database decoding.
    fn capacity_limits(&self) -> BackupCapacityLimits { BackupCapacityLimits::default() }

    /// Writes a blob at a repository key.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the key is invalid or the backend cannot
    /// persist the blob.
    async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError>;

    /// Streams a blob to a repository key.
    ///
    /// The default implementation preserves compatibility by collecting the
    /// stream (64 MiB by default) and delegating to [`BackupRepository::put_blob`]. Repositories
    /// with native streaming support should override this method.
    async fn put_blob_stream(&self, key: &str, body: StorageByteStream) -> Result<(), BackupError> {
        self.put_blob(key, collect_backup_stream(body, self.capacity_limits().buffered_bytes).await?)
            .await
    }

    /// Writes a blob only when no blob exists at the key.
    ///
    /// Returns `true` when the blob was written and `false` when the key
    /// already existed. Implementations must atomically create the blob only
    /// if absent across all writers. Existence checks followed by writes are
    /// insufficient. The default fails closed without reading or writing.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the key is invalid or the backend cannot
    /// perform the conditional write.
    async fn put_blob_if_absent(&self, _key: &str, _body: Bytes) -> Result<bool, BackupError> {
        Err(BackupError::UnsupportedOperation {
            operation: "atomic conditional repository write".to_string(),
        })
    }

    /// Streams a blob only when no blob exists at the key.
    ///
    /// The default implementation collects for compatibility. Provider-backed
    /// repositories should override it with an atomic conditional write.
    async fn put_blob_stream_if_absent(
        &self,
        key: &str,
        body: StorageByteStream,
    ) -> Result<bool, BackupError> {
        self.put_blob_if_absent(key, collect_backup_stream(body, self.capacity_limits().buffered_bytes).await?)
            .await
    }

    /// Reads a blob from a repository key.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the key is invalid, the blob is missing, or
    /// the backend cannot read it.
    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError>;

    /// Loads a blob as a byte stream.
    ///
    /// The default implementation wraps the existing buffered API and checks the
    /// configured byte limit after loading. The provider allocation already exists;
    /// this cannot guarantee bounded process memory.
    async fn get_blob_stream(&self, key: &str) -> Result<StorageByteStream, BackupError> {
        let bytes = self.get_blob(key).await?;
        self.capacity_limits().buffered_bytes.check(bytes.len() as u64)?;
        Ok(StorageByteStream::from_bytes(bytes))
    }

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

    /// Deletes a blob without replaying deletion against a replacement object.
    ///
    /// Required for lock release. Once a deletion may have reached storage,
    /// implementations must not retry by key, including after cancellation or
    /// a lost response. Such failures leave the outcome unknown. The default
    /// fails closed without repository I/O; custom providers must opt in.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError::UnsupportedOperation`] by default, or another
    /// [`BackupError`] on failure. Never retry an ambiguous failure.
    async fn delete_blob_once(&self, _key: &str) -> Result<(), BackupError> {
        Err(BackupError::UnsupportedOperation {
            operation: "single-attempt repository deletion".to_string(),
        })
    }

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
    limits: BackupCapacityLimits,
}

impl BlobStoreBackupRepository {
    /// Creates an adapter without a repository prefix.
    #[must_use]
    pub fn new(store: Arc<dyn BlobStore>) -> Self {
        Self {
            store,
            prefix: None,
            limits: BackupCapacityLimits::default(),
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
            limits: BackupCapacityLimits::default(),
        })
    }

    /// Configures per-payload buffering and decoding limits.
    #[must_use]
    pub fn with_capacity_limits(mut self, limits: BackupCapacityLimits) -> Self {
        self.limits = limits;
        self
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
    fn capacity_limits(&self) -> BackupCapacityLimits { self.limits }
    async fn delete_blob_once(&self, key: &str) -> Result<(), BackupError> {
        self.store
            .delete_blob_once(&self.apply_prefix(key))
            .await
            .map_err(|source| {
                // A custom provider may incorrectly advertise retryability. Never
                // propagate that hint through the lock-release adapter.
                let source = if source.is_retryable() {
                    graphql_orm_storage::StorageError::SingleAttemptFailed {
                        source: Box::new(source),
                    }
                } else {
                    source
                };
                BackupError::from(source)
            })
    }

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

    async fn put_blob_stream(&self, key: &str, body: StorageByteStream) -> Result<(), BackupError> {
        self.store
            .put_blob(&self.apply_prefix(key), body, BlobPutOptions::default())
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

    async fn put_blob_stream_if_absent(
        &self,
        key: &str,
        body: StorageByteStream,
    ) -> Result<bool, BackupError> {
        let outcome = self
            .store
            .put_blob_if_not_exists(&self.apply_prefix(key), body, BlobPutOptions::default())
            .await?;
        Ok(outcome.is_some())
    }

    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
        let body = self.store.get_blob(&self.apply_prefix(key)).await?;
        Ok(collect_backup_stream(body.body, self.limits.buffered_bytes).await?)
    }

    async fn get_blob_stream(&self, key: &str) -> Result<StorageByteStream, BackupError> {
        Ok(self.store.get_blob(&self.apply_prefix(key)).await?.body)
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

#[cfg(test)]
mod conditional_write_tests {
    use super::*;

    struct NoConditionalWrite;

    #[async_trait]
    impl BackupRepository for NoConditionalWrite {
        fn capacity_limits(&self) -> BackupCapacityLimits {
            BackupCapacityLimits { buffered_bytes: crate::BackupByteLimit::Bytes(2), ..BackupCapacityLimits::default() }
        }
        async fn put_blob(&self, _: &str, _: Bytes) -> Result<(), BackupError> {
            panic!("default conditional create must not write")
        }
        async fn get_blob(&self, _: &str) -> Result<Bytes, BackupError> {
            panic!("default conditional create must not read")
        }
        async fn blob_exists(&self, _: &str) -> Result<bool, BackupError> {
            panic!("default conditional create must not check existence")
        }
        async fn list_blobs(&self, _: &str) -> Result<Vec<String>, BackupError> {
            panic!("default conditional create must not list")
        }
        async fn delete_blob(&self, _: &str) -> Result<(), BackupError> {
            panic!("default conditional create must not delete")
        }
    }

    #[tokio::test]
    async fn inherited_conditional_create_fails_closed_without_repository_io() {
        let repository = NoConditionalWrite;
        let error = repository
            .put_blob_if_absent("lock", Bytes::new())
            .await
            .expect_err("atomic creation unsupported");
        assert!(matches!(error, BackupError::UnsupportedOperation { .. }));
        let error = repository
            .put_blob_stream_if_absent("lock", StorageByteStream::from_bytes(Bytes::new()))
            .await
            .expect_err("streaming default must also fail closed");
        assert!(matches!(error, BackupError::UnsupportedOperation { .. }));
        let error =
            crate::RepositoryLock::acquire(&repository, &crate::RepositoryLockOptions::default())
                .await
                .expect_err("lock requires atomic conditional create");
        assert!(matches!(error, BackupError::UnsupportedOperation { .. }));
    }
    #[tokio::test]
    async fn inherited_stream_write_rejects_capacity_before_provider_write() {
        let error = NoConditionalWrite.put_blob_stream("payload", StorageByteStream::from_bytes(Bytes::from_static(b"large")))
            .await.expect_err("fallback must reject before panic-on-write provider");
        assert!(matches!(error, BackupError::CapacityExceeded { limit_bytes: 2 }));
        let error = NoConditionalWrite.put_blob_stream_if_absent("payload", StorageByteStream::from_bytes(Bytes::from_static(b"large")))
            .await.expect_err("conditional fallback must reject before provider call");
        assert!(matches!(error, BackupError::CapacityExceeded { limit_bytes: 2 }));
    }
    struct CreateWithoutRelease;

    #[async_trait]
    impl BackupRepository for CreateWithoutRelease {
        async fn put_blob_if_absent(&self, _: &str, _: Bytes) -> Result<bool, BackupError> {
            Ok(true)
        }
        async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError> {
            NoConditionalWrite.put_blob(key, body).await
        }
        async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError> {
            NoConditionalWrite.get_blob(key).await
        }
        async fn blob_exists(&self, key: &str) -> Result<bool, BackupError> {
            NoConditionalWrite.blob_exists(key).await
        }
        async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError> {
            NoConditionalWrite.list_blobs(prefix).await
        }
        async fn delete_blob(&self, key: &str) -> Result<(), BackupError> {
            NoConditionalWrite.delete_blob(key).await
        }
    }

    #[tokio::test]
    async fn release_never_falls_back_to_ordinary_provider_delete() {
        let repository = CreateWithoutRelease;
        let lock =
            crate::RepositoryLock::acquire(&repository, &crate::RepositoryLockOptions::default())
                .await
                .expect("explicit atomic creation");
        let error = lock
            .release()
            .await
            .expect_err("release requires explicit opt-in");
        assert!(matches!(error, BackupError::UnsupportedOperation { .. }));
    }
}
