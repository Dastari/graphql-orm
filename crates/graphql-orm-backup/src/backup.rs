use bytes::Bytes;
use serde::Serialize;
use uuid::Uuid;

use crate::{
    BACKUP_FORMAT_VERSION, BackupError, BackupKind, BackupObjectIndex, BackupRepository, BackupRow,
    BackupSnapshotManifest, BackupTableExport, DatabaseBackupManifest, GraphqlOrmBackupAdapter,
    ObjectBackupEntry, TableBackupEntry, manifest::sha256_hex, plan_full_backup,
    set_manifest_checksum,
};

pub const DATABASE_EXPORT_FORMAT: &str = "jsonl";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullBackupRequest {
    pub snapshot_id: Uuid,
    pub created_at: i64,
    pub app_id: String,
    pub app_version: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FullBackupResult {
    pub manifest: BackupSnapshotManifest,
}

#[must_use]
pub fn snapshot_manifest_key(snapshot_id: Uuid) -> String {
    format!("snapshots/{snapshot_id}/manifest.json")
}

/// Returns the reserved table export key.
///
/// Full backup currently writes uncompressed JSON Lines. The `.zst` suffix is
/// retained for the stable repository layout reserved by the snapshot format.
#[must_use]
pub fn database_table_key(snapshot_id: Uuid, table_name: &str) -> String {
    format!("snapshots/{snapshot_id}/database/tables/{table_name}.jsonl.zst")
}

#[must_use]
pub fn database_changes_key(snapshot_id: Uuid, table_name: &str) -> String {
    format!("snapshots/{snapshot_id}/database/changes/{table_name}.jsonl.zst")
}

#[must_use]
pub fn object_content_key(sha256_hex: &str) -> String {
    let shard_a = sha256_hex.get(0..2).unwrap_or("00");
    let shard_b = sha256_hex.get(2..4).unwrap_or("00");
    format!("objects/sha256/{shard_a}/{shard_b}/{sha256_hex}")
}

pub async fn create_full_backup(
    repository: &dyn BackupRepository,
    database: &dyn GraphqlOrmBackupAdapter,
    objects: &dyn BackupObjectIndex,
    request: FullBackupRequest,
) -> Result<FullBackupResult, BackupError> {
    let plan = plan_full_backup(database, objects).await?;

    let mut table_entries = Vec::with_capacity(plan.tables.len());
    let mut row_count = 0_u64;
    for table in &plan.tables {
        let bytes = serialize_table_export(table)?;
        let content_key = database_table_key(request.snapshot_id, &table.table_name);
        let sha256_hex = sha256_hex(&bytes);
        repository
            .put_blob(&content_key, Bytes::from(bytes))
            .await?;

        let table_row_count = table.rows.len() as u64;
        row_count += table_row_count;
        table_entries.push(TableBackupEntry {
            table_name: table.table_name.clone(),
            row_count: table_row_count,
            content_key,
            sha256_hex,
        });
    }

    let mut object_entries = Vec::with_capacity(plan.objects.len());
    for object in &plan.objects {
        let bytes = objects.load_object(object).await?;
        let actual = sha256_hex(&bytes);
        let content_key = object_content_key(&object.sha256_hex);
        if actual != object.sha256_hex {
            return Err(BackupError::ChecksumMismatch {
                key: content_key,
                expected: object.sha256_hex.clone(),
                actual,
            });
        }

        if repository.blob_exists(&content_key).await? {
            let existing = repository.get_blob(&content_key).await?;
            let existing_hash = sha256_hex(&existing);
            if existing_hash != object.sha256_hex {
                return Err(BackupError::ChecksumMismatch {
                    key: content_key,
                    expected: object.sha256_hex.clone(),
                    actual: existing_hash,
                });
            }
        } else {
            repository.put_blob(&content_key, bytes).await?;
        }

        object_entries.push(ObjectBackupEntry {
            object_id: object.object_id,
            storage_key: object.storage_key.clone(),
            content_key,
            sha256_hex: object.sha256_hex.clone(),
            size_bytes: object.size_bytes,
            mime_type: object.mime_type.clone(),
        });
    }

    let mut manifest = BackupSnapshotManifest {
        format_version: BACKUP_FORMAT_VERSION,
        snapshot_id: request.snapshot_id,
        parent_snapshot_id: None,
        created_at: request.created_at,
        app_id: request.app_id,
        app_version: request.app_version,
        graphql_orm_schema_version: plan.schema.migration_version,
        graphql_orm_schema_hash: plan.schema.schema_hash,
        database_backend: plan.schema.backend,
        backup_kind: BackupKind::Full,
        database: DatabaseBackupManifest {
            export_format: DATABASE_EXPORT_FORMAT.to_string(),
            row_count,
            table_count: table_entries.len() as u64,
            tables: table_entries,
        },
        objects: object_entries,
        tombstones: Vec::new(),
        checksum: String::new(),
    };

    write_manifest(repository, &mut manifest).await?;

    Ok(FullBackupResult { manifest })
}

pub async fn write_manifest(
    repository: &dyn BackupRepository,
    manifest: &mut BackupSnapshotManifest,
) -> Result<(), BackupError> {
    set_manifest_checksum(manifest)?;
    let body = serde_json::to_vec_pretty(manifest)?;
    repository
        .put_blob(
            &snapshot_manifest_key(manifest.snapshot_id),
            Bytes::from(body),
        )
        .await
}

#[must_use]
pub fn bytes_sha256_hex(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

fn serialize_table_export(table: &BackupTableExport) -> Result<Vec<u8>, BackupError> {
    let mut bytes = Vec::new();
    for row in &table.rows {
        let serialized = SerializedBackupRow::from(row);
        serde_json::to_writer(&mut bytes, &serialized)?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}

#[derive(Serialize)]
struct SerializedBackupRow<'a> {
    table_name: &'a str,
    primary_key: &'a str,
    row_hash: &'a str,
    values: &'a serde_json::Map<String, serde_json::Value>,
}

impl<'a> From<&'a BackupRow> for SerializedBackupRow<'a> {
    fn from(row: &'a BackupRow) -> Self {
        Self {
            table_name: &row.table_name,
            primary_key: &row.primary_key,
            row_hash: &row.row_hash,
            values: &row.values,
        }
    }
}
