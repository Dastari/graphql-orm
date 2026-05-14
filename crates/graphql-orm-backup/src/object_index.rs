use async_trait::async_trait;
use bytes::Bytes;
use uuid::Uuid;

use crate::BackupError;

#[async_trait]
pub trait BackupObjectIndex: Send + Sync {
    async fn list_objects_for_full_backup(&self) -> Result<Vec<BackupObjectRef>, BackupError>;

    async fn list_objects_for_incremental_backup(
        &self,
        since_snapshot_id: Uuid,
    ) -> Result<Vec<BackupObjectRef>, BackupError>;

    async fn load_object(&self, object: &BackupObjectRef) -> Result<Bytes, BackupError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupObjectRef {
    pub object_id: Uuid,
    pub storage_key: String,
    pub sha256_hex: String,
    pub size_bytes: u64,
    pub mime_type: Option<String>,
}
