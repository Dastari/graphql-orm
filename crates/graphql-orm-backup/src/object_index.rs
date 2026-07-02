use async_trait::async_trait;
use bytes::Bytes;
use uuid::Uuid;

use crate::BackupError;

#[async_trait]
pub trait BackupObjectIndex: Send + Sync {
    /// Lists all objects referenced by a full backup.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the object index cannot be queried.
    async fn list_objects_for_full_backup(&self) -> Result<Vec<BackupObjectRef>, BackupError>;

    /// Lists objects newly referenced or changed since a parent snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the object index cannot be queried or
    /// incremental discovery is unavailable.
    async fn list_objects_for_incremental_backup(
        &self,
        since_snapshot_id: Uuid,
    ) -> Result<Vec<BackupObjectRef>, BackupError>;

    /// Loads the bytes for an object reference.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the object bytes cannot be loaded.
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
