use async_trait::async_trait;
use bytes::Bytes;
use uuid::Uuid;

use graphql_orm_storage::StorageByteStream;

use crate::{BackupByteLimit, BackupError};

#[async_trait]
/// Application object lookup contract used by backup operations.
pub trait BackupObjectIndex: Send + Sync {
    /// Limit checked after the buffered loader returns; provider allocation already exists.
    fn buffered_byte_limit(&self) -> BackupByteLimit {
        BackupByteLimit::default()
    }
    /// Derives the full object set from the rows already exported in the database
    /// snapshot. Implementations must not query a live database here. The default
    /// fails closed; objectless or externally frozen indexes must opt in explicitly.
    ///
    /// # Errors
    /// Returns [`BackupError::UnsupportedOperation`] by default, or an error if
    /// the exported metadata cannot establish a complete object set and digests.
    async fn objects_from_full_export(
        &self,
        _tables: &[crate::BackupTableExport],
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        Err(BackupError::UnsupportedOperation {
            operation: "object index must derive references from the full database export".into(),
        })
    }

    /// Lists all objects referenced by a full backup.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the object index cannot be queried.
    async fn list_objects_for_full_backup(&self) -> Result<Vec<BackupObjectRef>, BackupError>;

    /// Derives new/changed object references solely from the exact incremental
    /// change export. Do not query live metadata or recompute a missing digest
    /// from current bytes. The export must contain enough metadata to identify
    /// every new/changed reference and its recorded checksum. Objectless or
    /// externally frozen indexes must explicitly opt in with their fixed set.
    ///
    /// # Errors
    /// Returns [`BackupError::UnsupportedOperation`] by default, or an error if
    /// the export cannot establish a complete object set and recorded digests.
    async fn objects_from_incremental_export(
        &self,
        _parent_snapshot_id: Uuid,
        _changes: &[crate::BackupChangeExport],
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        Err(BackupError::UnsupportedOperation {
            operation: "object index must derive references from the incremental database export"
                .into(),
        })
    }

    /// Legacy live listing API; incremental backup no longer calls this method.
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
    /// method to avoid compatibility buffering. The default checks declared size
    /// before loading and actual size afterward; it cannot undo provider allocation.
    async fn load_object_stream(
        &self,
        object: &BackupObjectRef,
    ) -> Result<StorageByteStream, BackupError> {
        self.buffered_byte_limit().check(object.size_bytes)?;
        let bytes = self.load_object(object).await?;
        self.buffered_byte_limit().check(bytes.len() as u64)?;
        Ok(StorageByteStream::from_bytes(bytes))
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
