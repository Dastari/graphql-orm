use std::sync::Arc;

use async_trait::async_trait;
use graphql_orm_backup::{
    BackupError, BlobStoreBackupRepository, RepositoryLock, RepositoryLockOptions,
};
use graphql_orm_storage::{
    BlobBody, BlobListPage, BlobMetadata, BlobPutOptions, BlobStore, BlobWriteOutcome,
    StorageBackend, StorageByteStream, StorageError,
};

struct NoAtomicCreate;

#[async_trait]
impl BlobStore for NoAtomicCreate {
    fn backend(&self) -> StorageBackend {
        StorageBackend::Local
    }

    async fn put_blob(
        &self,
        _: &str,
        _: StorageByteStream,
        _: BlobPutOptions,
    ) -> Result<BlobWriteOutcome, StorageError> {
        panic!("conditional create must not write")
    }

    async fn get_blob(&self, _: &str) -> Result<BlobBody, StorageError> {
        panic!("conditional create must not read")
    }

    async fn blob_exists(&self, _: &str) -> Result<bool, StorageError> {
        panic!("conditional create must not check existence")
    }

    async fn head_blob(&self, _: &str) -> Result<Option<BlobMetadata>, StorageError> {
        panic!("conditional create must not inspect metadata")
    }

    async fn list_blobs_page(
        &self,
        _: &str,
        _: Option<String>,
        _: usize,
    ) -> Result<BlobListPage, StorageError> {
        panic!("conditional create must not list")
    }

    async fn delete_blob(&self, _: &str) -> Result<(), StorageError> {
        panic!("conditional create must not delete")
    }
}

#[tokio::test]
async fn inherited_storage_default_fails_closed_directly_and_through_backup_adapter() {
    let store = Arc::new(NoAtomicCreate);
    let error = store
        .put_blob_if_not_exists(
            "locks/repository.lock",
            StorageByteStream::from_bytes(Vec::new()),
            BlobPutOptions::default(),
        )
        .await
        .expect_err("atomic create unsupported");
    assert!(matches!(error, StorageError::UnsupportedOperation { .. }));
    assert!(!error.is_retryable());

    let repository = BlobStoreBackupRepository::new(store);
    let error = RepositoryLock::acquire(&repository, &RepositoryLockOptions::default())
        .await
        .expect_err("adapter must not bypass fail-closed conditional creation");
    assert!(matches!(error, BackupError::UnsupportedOperation { .. }));
}
