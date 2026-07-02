use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

use crate::{BackupError, RestoreContext};

#[async_trait]
/// Database integration contract used by backup and restore operations.
pub trait GraphqlOrmBackupAdapter: Send + Sync {
    /// Returns backup-relevant schema metadata.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the adapter cannot read schema metadata.
    async fn schema_snapshot(&self) -> Result<GraphqlOrmBackupSchema, BackupError>;

    /// Returns whether the restore target currently has no rows.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the adapter cannot determine target
    /// emptiness. The default implementation returns
    /// [`BackupError::UnsupportedOperation`] so restore adapters must opt in
    /// explicitly.
    async fn restore_target_is_empty(&self) -> Result<bool, BackupError> {
        Err(BackupError::UnsupportedOperation {
            operation: "restore target emptiness check".to_string(),
        })
    }

    /// Exports all backup-enabled tables for a full snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the adapter cannot export table rows.
    async fn export_full(&self) -> Result<Vec<BackupTableExport>, BackupError>;

    /// Exports changed rows and tombstones since a parent snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if incremental export is unavailable or fails.
    async fn export_incremental(
        &self,
        parent_snapshot_id: Uuid,
    ) -> Result<Vec<BackupChangeExport>, BackupError>;

    /// Restores a full table export.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the adapter cannot import the rows into the
    /// target database.
    async fn restore_full(
        &self,
        export: Vec<BackupTableExport>,
        context: RestoreContext,
    ) -> Result<(), BackupError>;

    /// Restores incremental changes.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if incremental restore is unavailable or fails.
    async fn restore_incremental(
        &self,
        changes: Vec<BackupChangeExport>,
        context: RestoreContext,
    ) -> Result<(), BackupError>;
}

/// Schema metadata captured in backup manifests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphqlOrmBackupSchema {
    /// Database backend identifier, such as `sqlite` or `postgres`.
    pub backend: String,
    /// Application/ORM migration version.
    pub migration_version: String,
    /// Stable schema hash.
    pub schema_hash: String,
}

/// Full export for one table.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BackupTableExport {
    /// Table name.
    pub table_name: String,
    /// Rows exported for this table.
    pub rows: Vec<BackupRow>,
}

/// Logical database row used in table and change payloads.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BackupRow {
    /// Table name.
    pub table_name: String,
    /// Stable primary-key string.
    pub primary_key: String,
    /// Adapter-provided row hash.
    pub row_hash: String,
    /// JSON-compatible row values.
    pub values: serde_json::Map<String, Value>,
}

/// Incremental change action.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BackupChangeAction {
    /// New row.
    Create,
    /// Existing row updated.
    Update,
    /// Row deleted.
    Delete,
}

/// One incremental row change.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BackupChangeExport {
    /// Table name.
    pub table_name: String,
    /// Stable primary-key string.
    pub primary_key: String,
    /// Change action.
    pub action: BackupChangeAction,
    /// Row body for create/update changes.
    pub row: Option<BackupRow>,
    /// Change time as UTC Unix seconds.
    pub changed_at: i64,
}
