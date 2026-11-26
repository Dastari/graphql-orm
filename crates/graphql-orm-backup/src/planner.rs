use crate::{
    BackupError, BackupObjectIndex, BackupObjectRef, BackupTableExport, GraphqlOrmBackupAdapter,
    GraphqlOrmBackupSchema,
};

/// Database rows, schema, and referenced objects collected for a full snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct FullBackupPlan {
    /// Source schema and compatibility identity for the snapshot manifest.
    pub schema: GraphqlOrmBackupSchema,
    /// Exported rows grouped by table.
    pub tables: Vec<BackupTableExport>,
    /// Live object references derived from the exported rows.
    pub objects: Vec<BackupObjectRef>,
}

/// Collects a full table snapshot and derives its object refs from those rows.
///
/// # Errors
///
/// Returns [`BackupError`] if schema export, full row export, or snapshot-based
/// object reference derivation fails.
pub async fn plan_full_backup(
    database: &dyn GraphqlOrmBackupAdapter,
    objects: &dyn BackupObjectIndex,
) -> Result<FullBackupPlan, BackupError> {
    database.ensure_backup_restore_allowed()?;
    objects.ensure_backup_allowed()?;
    let schema = database.schema_snapshot().await?;
    let tables = database.export_full().await?;
    schema.ensure_canonical_identity_unchanged(database).await?;
    let objects = objects.objects_from_full_export(&tables).await?;

    Ok(FullBackupPlan {
        schema,
        tables,
        objects,
    })
}
