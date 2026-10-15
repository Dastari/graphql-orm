//! `graphql-orm` runtime integration behind the `orm` feature.
//!
//! [`OrmBackupAdapter`] bridges the [`graphql_orm::graphql::orm::GraphqlOrmBackupRuntime`]
//! implementation on [`graphql_orm::db::Database`] to this crate's
//! [`GraphqlOrmBackupAdapter`] contract, and [`OrmBackupObjectIndex`] derives a
//! [`BackupObjectIndex`] from one backup-enabled table that records stored
//! object metadata. Host applications supply entity metadata and column names;
//! this module stays free of application domain assumptions.
//!
//! The `orm` feature requires the host application to enable exactly one
//! `graphql-orm` backend feature (`sqlite` or `postgres`).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use graphql_orm::db::Database;
use graphql_orm::graphql::orm::{
    BackupRow as OrmBackupRow, BackupValue, ColumnBackupPolicy, EntityBackupDescriptor,
    EntityMetadata, GraphqlOrmBackupRuntime, GraphqlOrmSchemaSnapshot,
    RestoreContext as OrmRestoreContext, RestoreMode as OrmRestoreMode, stable_schema_hash,
};
use graphql_orm::sqlx::Row as _;
use graphql_orm_storage::{BlobStore, StorageByteStream};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    BackupChangeExport, BackupError, BackupObjectIndex, BackupObjectRef, BackupRow,
    BackupTableExport, GraphqlOrmBackupAdapter, GraphqlOrmBackupSchema, RestoreContext,
    RestoreMode, BackupByteLimit, collect_backup_stream,
};

/// [`GraphqlOrmBackupAdapter`] over a `graphql-orm` [`Database`].
///
/// Full export and restore delegate to the `GraphqlOrmBackupRuntime`
/// implementation shipped with `graphql-orm`. Incremental export and restore
/// are unsupported until a change-journal integration lands.
pub struct OrmBackupAdapter {
    database: Arc<Database>,
    entities: Vec<&'static EntityMetadata>,
    migration_version: Option<String>,
    column_policy_overrides: Vec<ColumnPolicyOverride>,
}

struct ColumnPolicyOverride {
    table_name: String,
    column_name: String,
    policy: ColumnBackupPolicy,
}

impl OrmBackupAdapter {
    /// Creates an adapter over a database and its backup-enabled entities.
    ///
    /// The entity list must match the list used for migrations so exports and
    /// restores cover every application-owned table.
    #[must_use]
    pub fn new(database: Arc<Database>, entities: Vec<&'static EntityMetadata>) -> Self {
        Self {
            database,
            entities,
            migration_version: None,
            column_policy_overrides: Vec::new(),
        }
    }

    /// Overrides the migration version recorded in schema snapshots.
    ///
    /// Without an override the adapter reads the latest applied migration
    /// version from the database schema manager.
    #[must_use]
    pub fn with_migration_version(mut self, migration_version: impl Into<String>) -> Self {
        self.migration_version = Some(migration_version.into());
        self
    }

    /// Overrides one column's backup policy at the adapter level.
    ///
    /// Use this to exclude or redact columns whose database types the
    /// `graphql-orm` export cannot round-trip yet (for example PostGIS
    /// geometry) without editing entity metadata, which would change
    /// migration-planning inputs. Overrides apply to export, restore
    /// validation, restore imports, and the backup schema hash.
    ///
    /// Overrides may only keep or strengthen the metadata policy:
    /// `Include -> Redact -> Exclude`. Attempts to weaken a policy or target
    /// an unknown table/column fail when the adapter next resolves its
    /// descriptors. Sensitive opaque object references that must remain in a
    /// backup should therefore be declared `Include` in host-reviewed entity
    /// metadata rather than silently overriding `Redact` or `Exclude`.
    #[must_use]
    pub fn with_column_backup_policy(
        mut self,
        table_name: impl Into<String>,
        column_name: impl Into<String>,
        policy: ColumnBackupPolicy,
    ) -> Self {
        self.column_policy_overrides.push(ColumnPolicyOverride {
            table_name: table_name.into(),
            column_name: column_name.into(),
            policy,
        });
        self
    }

    /// Returns the current `graphql-orm` schema snapshot for the configured
    /// entities, with column policy overrides applied to its entity
    /// descriptors.
    ///
    /// Hosts should compare a manifest's schema hash against this snapshot
    /// before destructive restore steps.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if the migration version cannot be read or a
    /// configured column policy override is invalid.
    pub async fn current_schema_snapshot(&self) -> Result<GraphqlOrmSchemaSnapshot, BackupError> {
        let migration_version = self.resolve_migration_version().await?;
        let mut snapshot = GraphqlOrmBackupRuntime::schema_snapshot(
            self.database.as_ref(),
            migration_version,
            &self.entities,
        );
        self.apply_column_policy_overrides(&mut snapshot.entities)?;
        snapshot.schema_hash = stable_schema_hash(&snapshot.entities);
        Ok(snapshot)
    }

    fn apply_column_policy_overrides(
        &self,
        descriptors: &mut [EntityBackupDescriptor],
    ) -> Result<(), BackupError> {
        for policy_override in &self.column_policy_overrides {
            let Some(descriptor) = descriptors
                .iter_mut()
                .find(|descriptor| descriptor.table_name == policy_override.table_name)
            else {
                return Err(invalid_policy_override(
                    policy_override,
                    "table is not a backup-enabled entity",
                ));
            };
            let Some(column) = descriptor
                .columns
                .iter_mut()
                .find(|column| column.column_name == policy_override.column_name)
            else {
                return Err(invalid_policy_override(
                    policy_override,
                    "column is not present in the entity backup descriptor",
                ));
            };
            if policy_rank(policy_override.policy) < policy_rank(column.backup_policy) {
                return Err(invalid_policy_override(
                    policy_override,
                    "override would weaken the effective metadata policy",
                ));
            }
            column.backup_policy = policy_override.policy;
        }
        Ok(())
    }

    /// Deletes all rows from every backup-enabled table so an
    /// empty-database restore can run against a previously used database.
    ///
    /// `RESTRICT` foreign keys are enforced immediately on both backends, so
    /// PostgreSQL clears with one `TRUNCATE ... CASCADE` statement and SQLite
    /// suspends `PRAGMA foreign_keys` on a dedicated connection around a
    /// child-first delete transaction, mirroring the `graphql-orm` migration
    /// executor. Callers own any additional safety checks such as manifest
    /// verification and schema compatibility.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError`] if a column policy override is invalid or any
    /// clear statement or transaction fails.
    pub async fn clear_restore_target(&self) -> Result<(), BackupError> {
        let mut descriptors = self.descriptors()?;
        descriptors.sort_by(|left, right| {
            right
                .restore_order
                .cmp(&left.restore_order)
                .then_with(|| right.table_name.cmp(&left.table_name))
        });
        let backend = graphql_orm::graphql::orm::current_backend();

        if backend == graphql_orm::graphql::orm::DatabaseBackend::Sqlite {
            self.clear_restore_target_without_foreign_keys(&descriptors)
                .await
        } else {
            let tables = descriptors
                .iter()
                .map(|descriptor| quote_identifier(&descriptor.table_name))
                .collect::<Vec<_>>()
                .join(", ");
            graphql_orm::sqlx::query(&format!("TRUNCATE TABLE {tables} CASCADE"))
                .execute(self.database.pool())
                .await
                .map_err(database_error("clear restore target"))?;
            Ok(())
        }
    }

    async fn clear_restore_target_without_foreign_keys(
        &self,
        descriptors: &[EntityBackupDescriptor],
    ) -> Result<(), BackupError> {
        use graphql_orm::sqlx::Connection as _;

        let mut connection = self
            .database
            .pool()
            .acquire()
            .await
            .map_err(database_error("clear restore target"))?;
        graphql_orm::sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *connection)
            .await
            .map_err(database_error("suspend foreign keys"))?;

        let cleared = async {
            let mut tx = connection
                .begin()
                .await
                .map_err(database_error("clear restore target"))?;
            for descriptor in descriptors {
                let sql = format!("DELETE FROM {}", quote_identifier(&descriptor.table_name));
                graphql_orm::sqlx::query(&sql)
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error(&format!(
                        "clear restore target table {}",
                        descriptor.table_name
                    )))?;
            }
            tx.commit()
                .await
                .map_err(database_error("clear restore target commit"))
        }
        .await;

        let reenabled = graphql_orm::sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&mut *connection)
            .await
            .map_err(database_error("re-enable foreign keys"));
        if reenabled.is_err() {
            // Never return a connection with foreign keys disabled to the pool.
            let _ = connection.detach().close().await;
        }

        cleared?;
        reenabled?;
        Ok(())
    }

    fn descriptors(&self) -> Result<Vec<EntityBackupDescriptor>, BackupError> {
        let mut descriptors = self.database.list_backup_entities(&self.entities);
        self.apply_column_policy_overrides(&mut descriptors)?;
        Ok(descriptors)
    }

    async fn resolve_migration_version(&self) -> Result<String, BackupError> {
        if let Some(migration_version) = &self.migration_version {
            return Ok(migration_version.clone());
        }
        Ok(self
            .database
            .schema()
            .current_version()
            .await
            .map_err(database_error("read migration version"))?
            .unwrap_or_else(|| "unversioned".to_string()))
    }
}

#[async_trait]
impl GraphqlOrmBackupAdapter for OrmBackupAdapter {
    async fn schema_snapshot(&self) -> Result<GraphqlOrmBackupSchema, BackupError> {
        let snapshot = self.current_schema_snapshot().await?;
        Ok(GraphqlOrmBackupSchema {
            backend: snapshot.backend,
            migration_version: snapshot.migration_version,
            schema_hash: snapshot.schema_hash,
        })
    }

    async fn restore_target_is_empty(&self) -> Result<bool, BackupError> {
        for descriptor in self.descriptors()? {
            let sql = format!(
                "SELECT COUNT(*) AS row_count FROM {}",
                quote_identifier(&descriptor.table_name)
            );
            let row = graphql_orm::sqlx::query(&sql)
                .fetch_one(self.database.pool())
                .await
                .map_err(database_error(&format!(
                    "count rows in table {}",
                    descriptor.table_name
                )))?;
            let row_count: i64 = row
                .try_get("row_count")
                .map_err(database_error("decode row count"))?;
            if row_count != 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn export_full(&self) -> Result<Vec<BackupTableExport>, BackupError> {
        let mut descriptors = self.descriptors()?;
        descriptors.sort_by(|left, right| {
            left.export_order
                .cmp(&right.export_order)
                .then_with(|| left.table_name.cmp(&right.table_name))
        });

        let mut snapshot = self
            .database
            .begin_consistent_snapshot()
            .await
            .map_err(database_error("begin export snapshot"))?;
        let mut exports = Vec::with_capacity(descriptors.len());
        for descriptor in &descriptors {
            let rows = self
                .database
                .export_table_rows(&mut snapshot, descriptor)
                .await
                .map_err(database_error(&format!(
                    "export table {}",
                    descriptor.table_name
                )))?;
            exports.push(BackupTableExport {
                table_name: descriptor.table_name.clone(),
                rows: rows
                    .into_iter()
                    .map(crate_row_from_orm)
                    .collect::<Result<Vec<_>, _>>()?,
            });
        }
        Ok(exports)
    }

    async fn export_incremental(
        &self,
        _parent_snapshot_id: Uuid,
    ) -> Result<Vec<BackupChangeExport>, BackupError> {
        Err(BackupError::UnsupportedOperation {
            operation: "incremental export requires a graphql-orm change journal integration"
                .to_string(),
        })
    }

    async fn restore_full(
        &self,
        backup_schema: GraphqlOrmBackupSchema,
        export: Vec<BackupTableExport>,
        context: RestoreContext,
    ) -> Result<(), BackupError> {
        let context = orm_restore_context(&context)?;
        let current_snapshot = self.current_schema_snapshot().await?;
        if backup_schema.backend != current_snapshot.backend
            || backup_schema.schema_hash != current_snapshot.schema_hash
        {
            return Err(BackupError::RestoreSchemaMismatch {
                backup_backend: backup_schema.backend,
                backup_schema_hash: backup_schema.schema_hash,
                target_backend: current_snapshot.backend,
                target_schema_hash: current_snapshot.schema_hash,
            });
        }
        let mut backup_snapshot = current_snapshot.clone();
        backup_snapshot.backend = backup_schema.backend;
        backup_snapshot.migration_version = backup_schema.migration_version;
        backup_snapshot.schema_hash = backup_schema.schema_hash;
        let mut rows_by_table = BTreeMap::new();
        for table in export {
            rows_by_table.insert(
                table.table_name.clone(),
                table
                    .rows
                    .into_iter()
                    .map(orm_row_from_crate)
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        self.database
            .restore_backup_rows(
                &backup_snapshot,
                &current_snapshot,
                &rows_by_table,
                &context,
            )
            .await
            .map_err(database_error("restore rows"))?;
        Ok(())
    }

    async fn restore_incremental(
        &self,
        _changes: Vec<BackupChangeExport>,
        _context: RestoreContext,
    ) -> Result<(), BackupError> {
        Err(BackupError::UnsupportedOperation {
            operation: "incremental restore requires a graphql-orm change journal integration"
                .to_string(),
        })
    }
}

/// Column names an [`OrmBackupObjectIndex`] reads from an object metadata
/// table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrmObjectIndexColumns {
    /// Backup-enabled table that records stored object metadata.
    pub table_name: String,
    /// UUID column identifying each object.
    pub object_id_column: String,
    /// Column holding the provider-neutral storage key.
    pub storage_key_column: String,
    /// Column holding the lowercase hex SHA-256 of the object bytes.
    pub sha256_hex_column: String,
    /// Column holding the object size in bytes.
    pub size_bytes_column: String,
    /// Optional column holding the object MIME type.
    pub mime_type_column: Option<String>,
}

/// [`BackupObjectIndex`] over one `graphql-orm` object metadata table plus the
/// application's primary [`BlobStore`].
///
/// Rows whose checksum column does not contain a valid SHA-256 (for example
/// when a hash backfill queue has not caught up) are hashed from the loaded
/// blob stream at listing time, with checked byte counting and no whole-blob
/// collection. Backup verifies recorded checksums against object bytes; recorded
/// sizes are not independently verified and cannot constrain buffered allocation.
pub struct OrmBackupObjectIndex {
    database: Arc<Database>,
    entities: Vec<&'static EntityMetadata>,
    columns: OrmObjectIndexColumns,
    store: Arc<dyn BlobStore>,
    buffered_limit: BackupByteLimit,
}

impl OrmBackupObjectIndex {
    /// Creates an object index over a metadata table and blob store.
    #[must_use]
    pub fn new(
        database: Arc<Database>,
        entities: Vec<&'static EntityMetadata>,
        columns: OrmObjectIndexColumns,
        store: Arc<dyn BlobStore>,
    ) -> Self {
        Self {
            database,
            entities,
            columns,
            store,
            buffered_limit: BackupByteLimit::default(),
        }
    }

    /// Configures the direct buffered object loader (default 64 MiB per body).
    /// Native streaming and metadata hash backfill do not collect whole objects.
    /// Provider-owned chunks/allocations remain outside this byte limit.
    #[must_use]
    pub fn with_buffered_byte_limit(mut self, limit: BackupByteLimit) -> Self {
        self.buffered_limit = limit;
        self
    }

    fn object_table_descriptor(&self) -> Result<EntityBackupDescriptor, BackupError> {
        self.database
            .list_backup_entities(&self.entities)
            .into_iter()
            .find(|descriptor| descriptor.table_name == self.columns.table_name)
            .ok_or_else(|| BackupError::Database {
                message: format!(
                    "object index table {} is not a backup-enabled entity",
                    self.columns.table_name
                ),
            })
    }

    async fn object_ref_from_row(
        &self,
        row: &OrmBackupRow,
    ) -> Result<BackupObjectRef, BackupError> {
        let object_id = row_uuid(row, &self.columns.object_id_column)?;
        let storage_key = row_string(row, &self.columns.storage_key_column)?;
        let mime_type = match &self.columns.mime_type_column {
            Some(column) => row_optional_string(row, column),
            None => None,
        };

        let recorded_sha256 = row_optional_string(row, &self.columns.sha256_hex_column);
        let recorded_size = row_optional_i64(row, &self.columns.size_bytes_column);
        let (sha256_hex, size_bytes) = object_metadata_from_store(
            self.store.as_ref(), &storage_key, recorded_sha256, recorded_size,
        ).await?;

        Ok(BackupObjectRef {
            object_id,
            storage_key,
            sha256_hex,
            size_bytes,
            mime_type,
        })
    }
}

#[async_trait]
impl BackupObjectIndex for OrmBackupObjectIndex {
    fn buffered_byte_limit(&self) -> BackupByteLimit { self.buffered_limit }
    async fn list_objects_for_full_backup(&self) -> Result<Vec<BackupObjectRef>, BackupError> {
        let descriptor = self.object_table_descriptor()?;
        let mut snapshot = self
            .database
            .begin_consistent_snapshot()
            .await
            .map_err(database_error("begin object index snapshot"))?;
        let rows = self
            .database
            .export_table_rows(&mut snapshot, &descriptor)
            .await
            .map_err(database_error(&format!(
                "export object index table {}",
                descriptor.table_name
            )))?;
        drop(snapshot);

        let mut objects = Vec::with_capacity(rows.len());
        for row in &rows {
            objects.push(self.object_ref_from_row(row).await?);
        }
        Ok(objects)
    }

    async fn list_objects_for_incremental_backup(
        &self,
        _since_snapshot_id: Uuid,
    ) -> Result<Vec<BackupObjectRef>, BackupError> {
        Err(BackupError::UnsupportedOperation {
            operation:
                "incremental object discovery requires a graphql-orm change journal integration"
                    .to_string(),
        })
    }

    async fn load_object(&self, object: &BackupObjectRef) -> Result<Bytes, BackupError> {
        load_blob(self.store.as_ref(), &object.storage_key, self.buffered_limit).await
    }

    async fn load_object_stream(&self, object: &BackupObjectRef) -> Result<StorageByteStream, BackupError> {
        load_blob_stream(self.store.as_ref(), &object.storage_key).await
    }
}

async fn load_blob_stream(store: &dyn BlobStore, storage_key: &str) -> Result<StorageByteStream, BackupError> {
    Ok(store.get_blob(storage_key).await?.body)
}
async fn load_blob(store: &dyn BlobStore, storage_key: &str, limit: BackupByteLimit) -> Result<Bytes, BackupError> {
    collect_backup_stream(load_blob_stream(store, storage_key).await?, limit).await
}
async fn object_metadata_from_store(
    store: &dyn BlobStore, storage_key: &str, recorded_sha256: Option<String>, recorded_size: Option<i64>,
) -> Result<(String, u64), BackupError> {
    if let (Some(checksum), Some(size)) = (recorded_sha256, recorded_size) {
        if is_valid_sha256_hex(&checksum) && size >= 0 {
            return Ok((checksum.to_ascii_lowercase(), size as u64));
        }
    }
    // Ignore untrusted size hints: hashing needs neither reservation nor a buffer.
    let mut stream = load_blob_stream(store, storage_key).await?.into_inner();
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        size = size.checked_add(chunk.len() as u64)
            .ok_or(BackupError::CapacityExceeded { limit_bytes: u64::MAX })?;
        hasher.update(&chunk);
    }
    Ok((format!("{:x}", hasher.finalize()), size))
}

fn crate_row_from_orm(row: OrmBackupRow) -> Result<BackupRow, BackupError> {
    let mut values = serde_json::Map::new();
    for (column, value) in row.values {
        values.insert(column, serde_json::to_value(&value)?);
    }
    Ok(BackupRow {
        table_name: row.table_name,
        primary_key: row.primary_key,
        row_hash: row.row_hash,
        values,
    })
}

fn orm_row_from_crate(row: BackupRow) -> Result<OrmBackupRow, BackupError> {
    let mut values = BTreeMap::new();
    for (column, value) in row.values {
        values.insert(column, serde_json::from_value::<BackupValue>(value)?);
    }
    Ok(OrmBackupRow {
        table_name: row.table_name,
        primary_key: row.primary_key,
        row_hash: row.row_hash,
        values,
    })
}

fn orm_restore_context(context: &RestoreContext) -> Result<OrmRestoreContext, BackupError> {
    if matches!(context.mode, RestoreMode::EmptyDatabase)
        && (!context.disable_policies || !context.disable_change_journal)
    {
        return Err(BackupError::InvalidRestoreContext {
            reason: "applying graphql-orm restore requires administrative policy and change-journal suppression"
                .to_string(),
        });
    }
    let mode = match context.mode {
        RestoreMode::EmptyDatabase => OrmRestoreMode::EmptyDatabase,
        RestoreMode::DryRun => OrmRestoreMode::DryRun,
    };
    Ok(OrmRestoreContext {
        mode,
        disable_policies: context.disable_policies,
        disable_change_journal: context.disable_change_journal,
    })
}

fn policy_rank(policy: ColumnBackupPolicy) -> u8 {
    match policy {
        ColumnBackupPolicy::Include => 0,
        ColumnBackupPolicy::Redact => 1,
        ColumnBackupPolicy::Exclude => 2,
    }
}

fn invalid_policy_override(policy_override: &ColumnPolicyOverride, reason: &str) -> BackupError {
    BackupError::InvalidColumnBackupPolicyOverride {
        table_name: policy_override.table_name.clone(),
        column_name: policy_override.column_name.clone(),
        reason: reason.to_string(),
    }
}

fn row_value<'a>(row: &'a OrmBackupRow, column: &str) -> Result<&'a BackupValue, BackupError> {
    row.values.get(column).ok_or_else(|| BackupError::Database {
        message: format!(
            "object index row {} in table {} is missing column {}",
            row.primary_key, row.table_name, column
        ),
    })
}

fn row_uuid(row: &OrmBackupRow, column: &str) -> Result<Uuid, BackupError> {
    match row_value(row, column)? {
        BackupValue::Uuid(value) => Ok(*value),
        BackupValue::String(value) => {
            Uuid::parse_str(value).map_err(|error| BackupError::Database {
                message: format!(
                    "object index column {column} in table {} is not a uuid: {error}",
                    row.table_name
                ),
            })
        }
        other => Err(BackupError::Database {
            message: format!(
                "object index column {column} in table {} has unsupported uuid value {other:?}",
                row.table_name
            ),
        }),
    }
}

fn row_string(row: &OrmBackupRow, column: &str) -> Result<String, BackupError> {
    match row_value(row, column)? {
        BackupValue::String(value) => Ok(value.clone()),
        other => Err(BackupError::Database {
            message: format!(
                "object index column {column} in table {} has unsupported string value {other:?}",
                row.table_name
            ),
        }),
    }
}

fn row_optional_string(row: &OrmBackupRow, column: &str) -> Option<String> {
    match row.values.get(column) {
        Some(BackupValue::String(value)) if !value.trim().is_empty() => Some(value.clone()),
        _ => None,
    }
}

fn row_optional_i64(row: &OrmBackupRow, column: &str) -> Option<i64> {
    match row.values.get(column) {
        Some(BackupValue::Integer(value)) => Some(*value),
        _ => None,
    }
}

fn is_valid_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn database_error(operation: &str) -> impl Fn(graphql_orm::Error) -> BackupError + '_ {
    move |error| BackupError::Database {
        message: format!("{operation}: {error}"),
    }
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

#[cfg(test)]
mod object_stream_capacity_tests {
    use super::*;
    use graphql_orm_storage::{BlobBody, BlobListPage, BlobMetadata, BlobPutOptions, BlobWriteOutcome, StorageBackend, StorageError};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ChunkStore { hint: u64, reads: AtomicUsize }
    #[async_trait]
    impl BlobStore for ChunkStore {
        fn backend(&self) -> StorageBackend { StorageBackend::Local }
        async fn get_blob(&self, key: &str) -> Result<BlobBody, StorageError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(BlobBody { key: key.to_string(), metadata: None,
                body: StorageByteStream::with_size_hint(Box::pin(futures::stream::iter([
                    Ok(Bytes::from_static(b"abc")), Ok(Bytes::from_static(b"def")),
                ])), self.hint) })
        }
        async fn put_blob(&self, _: &str, _: StorageByteStream, _: BlobPutOptions) -> Result<BlobWriteOutcome, StorageError> { panic!("read-only fixture") }
        async fn blob_exists(&self, _: &str) -> Result<bool, StorageError> { panic!("read-only fixture") }
        async fn head_blob(&self, _: &str) -> Result<Option<BlobMetadata>, StorageError> { panic!("read-only fixture") }
        async fn list_blobs_page(&self, _: &str, _: Option<String>, _: usize) -> Result<BlobListPage, StorageError> { panic!("read-only fixture") }
        async fn delete_blob(&self, _: &str) -> Result<(), StorageError> { panic!("read-only fixture") }
    }

    #[cfg(feature = "orm-sqlite")]
    #[tokio::test]
    async fn built_in_index_uses_streaming_backfill_and_configurable_buffered_limit() {
        let pool = graphql_orm::sqlx::SqlitePool::connect("sqlite::memory:").await.expect("owned in-memory DB");
        let store = Arc::new(ChunkStore { hint: 1, reads: AtomicUsize::new(0) });
        let index = OrmBackupObjectIndex::new(Arc::new(Database::new(pool)), Vec::new(), OrmObjectIndexColumns {
            table_name: "objects".to_string(), object_id_column: "id".to_string(),
            storage_key_column: "key".to_string(), sha256_hex_column: "hash".to_string(),
            size_bytes_column: "size".to_string(), mime_type_column: None,
        }, store.clone()).with_buffered_byte_limit(BackupByteLimit::Bytes(4));
        let mut row = OrmBackupRow { table_name: "objects".to_string(), primary_key: "1".to_string(), row_hash: String::new(), values: BTreeMap::from([
            ("id".to_string(), BackupValue::Uuid(Uuid::nil())),
            ("key".to_string(), BackupValue::String("owned".to_string())),
        ]) };
        let checksum = crate::bytes_sha256_hex(b"abcdef");
        for (hash, size) in [(BackupValue::Null, BackupValue::Integer(6)),
            (BackupValue::String("invalid".to_string()), BackupValue::Integer(6)),
            (BackupValue::String(checksum.clone()), BackupValue::Null),
            (BackupValue::String(checksum.clone()), BackupValue::Integer(-1))] {
            row.values.insert("hash".to_string(), hash);
            row.values.insert("size".to_string(), size);
            let reference = index.object_ref_from_row(&row).await.expect("actual built-in metadata path");
            assert_eq!(reference.sha256_hex, checksum);
            assert_eq!(reference.size_bytes, 6);
        }
        row.values.insert("hash".to_string(), BackupValue::String(checksum));
        row.values.insert("size".to_string(), BackupValue::Integer(1));
        let understated = index.object_ref_from_row(&row).await.expect("understated recorded size");
        assert!(matches!(index.load_object(&understated).await, Err(BackupError::CapacityExceeded { limit_bytes: 4 })));
        let mut body = index.load_object_stream(&understated).await.expect("native object stream").into_inner();
        let mut size = 0;
        while let Some(chunk) = body.next().await { size += chunk.expect("chunk").len(); }
        assert_eq!(size, 6);
        let reviewed = index.with_buffered_byte_limit(BackupByteLimit::Bytes(6));
        assert_eq!(reviewed.load_object(&understated).await.expect("finite override"), b"abcdef"[..]);
    }

    #[tokio::test]
    async fn missing_or_invalid_metadata_hashes_stream_without_reserving_huge_hint() {
        let store = ChunkStore { hint: u64::MAX, reads: AtomicUsize::new(0) };
        let checksum = crate::bytes_sha256_hex(b"abcdef");
        for (hash, size) in [
            (None, Some(6)),
            (Some("invalid".to_string()), Some(6)),
            (Some(checksum.clone()), None),
            (Some(checksum.clone()), Some(-1)),
        ] {
            let actual = object_metadata_from_store(&store, "owned", hash, size).await.expect("streamed backfill ignores allocation hint");
            assert_eq!(actual, (checksum.clone(), 6));
        }
        assert_eq!(store.reads.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn understated_metadata_cannot_bypass_buffer_cap_but_native_stream_stays_usable() {
        let store = ChunkStore { hint: 1, reads: AtomicUsize::new(0) };
        let recorded = object_metadata_from_store(&store, "owned", Some(crate::bytes_sha256_hex(b"abcdef")), Some(1))
            .await.expect("valid metadata deferred to backup integrity verification");
        assert_eq!(recorded.1, 1);
        assert_eq!(store.reads.load(Ordering::SeqCst), 0);
        assert!(matches!(load_blob(&store, "owned", BackupByteLimit::Bytes(4)).await,
            Err(BackupError::CapacityExceeded { limit_bytes: 4 })));
        let mut stream = load_blob_stream(&store, "owned").await.expect("native stream").into_inner();
        let mut observed = 0;
        while let Some(chunk) = stream.next().await { observed += chunk.expect("chunk").len(); }
        assert_eq!(observed, 6, "streaming remains usable beyond a small buffered cap");
        assert_eq!(load_blob(&store, "owned", BackupByteLimit::Bytes(6)).await.expect("reviewed finite override"), b"abcdef"[..]);
    }
}
