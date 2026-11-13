use crate::{
    BackupError, BackupObjectIndex, BackupObjectRef, BackupTableExport, GraphqlOrmBackupAdapter,
    GraphqlOrmBackupSchema,
};

#[derive(Clone, Debug, PartialEq)]
pub struct FullBackupPlan {
    pub schema: GraphqlOrmBackupSchema,
    pub tables: Vec<BackupTableExport>,
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
    let schema = database.schema_snapshot().await?;
    let tables = database.export_full().await?;
    let objects = objects.objects_from_full_export(&tables).await?;

    Ok(FullBackupPlan {
        schema,
        tables,
        objects,
    })
}
