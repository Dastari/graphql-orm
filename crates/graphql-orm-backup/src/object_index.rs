use async_trait::async_trait;
use bytes::Bytes;
use uuid::Uuid;

use graphql_orm_storage::StorageByteStream;

use crate::BackupError;

#[async_trait]
/// Application object lookup contract used by backup operations.
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

    /// Loads an object's bytes as a stream.
    ///
    /// Existing indexes remain compatible through the buffered default.
    /// Implementations backed by a streaming object store should override this
    /// method so large objects remain bounded in memory.
    async fn load_object_stream(
        &self,
        object: &BackupObjectRef,
    ) -> Result<StorageByteStream, BackupError> {
        Ok(StorageByteStream::from_bytes(
            self.load_object(object).await?,
        ))
    }
}

/// Object metadata returned by an application object index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupObjectRef {
    /// Application object id.
    pub object_id: Uuid,
    /// Original application storage key.
    pub storage_key: String,
    /// Expected SHA-256 checksum of the object bytes.
    pub sha256_hex: String,
    /// Object size in bytes.
    pub size_bytes: u64,
    /// Optional MIME type.
    pub mime_type: Option<String>,
}
