use async_trait::async_trait;
use bytes::Bytes;

use crate::BackupError;

#[async_trait]
pub trait BackupRepository: Send + Sync {
    async fn put_blob(&self, key: &str, body: Bytes) -> Result<(), BackupError>;

    async fn get_blob(&self, key: &str) -> Result<Bytes, BackupError>;

    async fn blob_exists(&self, key: &str) -> Result<bool, BackupError>;

    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, BackupError>;

    async fn delete_blob(&self, key: &str) -> Result<(), BackupError>;
}
