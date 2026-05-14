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

pub async fn plan_full_backup(
    database: &dyn GraphqlOrmBackupAdapter,
    objects: &dyn BackupObjectIndex,
) -> Result<FullBackupPlan, BackupError> {
    let schema = database.schema_snapshot().await?;
    let tables = database.export_full().await?;
    let objects = objects.list_objects_for_full_backup().await?;

    Ok(FullBackupPlan {
        schema,
        tables,
        objects,
    })
}
