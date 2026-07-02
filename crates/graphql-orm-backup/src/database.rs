use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

use crate::{BackupError, RestoreContext};

#[async_trait]
pub trait GraphqlOrmBackupAdapter: Send + Sync {
    /// Returns backup-relevant schema metadata.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the adapter cannot read schema metadata.
    async fn schema_snapshot(&self) -> Result<GraphqlOrmBackupSchema, BackupError>;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphqlOrmBackupSchema {
    pub backend: String,
    pub migration_version: String,
    pub schema_hash: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BackupTableExport {
    pub table_name: String,
    pub rows: Vec<BackupRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BackupRow {
    pub table_name: String,
    pub primary_key: String,
    pub row_hash: String,
    pub values: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BackupChangeAction {
    Create,
    Update,
    Delete,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BackupChangeExport {
    pub table_name: String,
    pub primary_key: String,
    pub action: BackupChangeAction,
    pub row: Option<BackupRow>,
    /// Change time as UTC Unix seconds.
    pub changed_at: i64,
}
