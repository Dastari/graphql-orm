use super::{
    BackupValueKind, ColumnBackupPolicy, EntityBackupDescriptor, EntityMetadata,
    GraphqlOrmSchemaSnapshot, MutationEvent, MutationPhase, SqlValue,
    backup_descriptors_from_entities, current_backend, execute_with_binds_on,
    schema_snapshot_from_entities,
};
#[cfg(feature = "postgres")]
use crate::PostgresBackend;
#[cfg(feature = "sqlite")]
use crate::SqliteBackend;
use crate::{DbPool, DefaultBackend};
use sqlx::Row;
use std::collections::BTreeMap;

const REDACTED_BACKUP_VALUE: &str = "[graphql-orm:redacted]";
const CHANGE_JOURNAL_TABLE: &str = "__graphql_orm_change_log";

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BackupRow {
    pub table_name: String,
    pub primary_key: String,
    pub row_hash: String,
    pub values: BTreeMap<String, BackupValue>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BackupValue {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Uuid(uuid::Uuid),
    Json(serde_json::Value),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BackupCompatibility {
    Exact,
    OlderSchema {
        backup_hash: String,
        current_hash: String,
    },
    Incompatible {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RestoreContext {
    pub mode: RestoreMode,
    pub disable_policies: bool,
    pub disable_change_journal: bool,
}

impl RestoreContext {
    pub fn empty_database() -> Self {
        Self {
            mode: RestoreMode::EmptyDatabase,
            disable_policies: true,
            disable_change_journal: true,
        }
    }

    pub fn dry_run() -> Self {
        Self {
            mode: RestoreMode::DryRun,
            disable_policies: true,
            disable_change_journal: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RestoreMode {
    EmptyDatabase,
    ReplaceExisting,
    DryRun,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImportReport {
    pub table_name: String,
    pub rows_imported: usize,
    pub rows_validated: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BackupBackendCapabilities {
    pub consistent_snapshot: bool,
    pub typed_json: bool,
    pub typed_uuid: bool,
    pub bytes: bool,
    pub deferred_constraints: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BackupChangeAction {
    Create,
    Update,
    Delete,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BackupChange {
    pub id: uuid::Uuid,
    pub entity_name: String,
    pub table_name: String,
    pub primary_key: String,
    pub action: BackupChangeAction,
    pub changed_at: i64,
    pub transaction_id: Option<String>,
    pub row_hash: Option<String>,
    pub actor_id: Option<String>,
    pub correlation_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangeWindow {
    pub after_snapshot_id: Option<uuid::Uuid>,
    pub until: i64,
}

#[cfg(feature = "sqlite")]
pub struct BackupSnapshot {
    tx: sqlx::Transaction<'static, sqlx::Sqlite>,
}

#[cfg(feature = "postgres")]
pub struct BackupSnapshot {
    tx: sqlx::Transaction<'static, sqlx::Postgres>,
}

#[allow(async_fn_in_trait)]
pub trait GraphqlOrmBackupRuntime {
    async fn begin_consistent_snapshot(&self) -> Result<BackupSnapshot, sqlx::Error>;

    fn backup_backend_capabilities(&self) -> BackupBackendCapabilities;

    fn list_backup_entities(&self, entities: &[&EntityMetadata]) -> Vec<EntityBackupDescriptor>;

    fn schema_snapshot(
        &self,
        migration_version: impl Into<String>,
        entities: &[&EntityMetadata],
    ) -> GraphqlOrmSchemaSnapshot;

    async fn export_table_rows(
        &self,
        snapshot: &mut BackupSnapshot,
        entity: &EntityBackupDescriptor,
    ) -> Result<Vec<BackupRow>, sqlx::Error>;

    async fn import_table_rows(
        &self,
        entity: &EntityBackupDescriptor,
        rows: &[BackupRow],
        context: &RestoreContext,
    ) -> Result<ImportReport, sqlx::Error>;

    async fn restore_backup_rows(
        &self,
        backup_snapshot: &GraphqlOrmSchemaSnapshot,
        current_snapshot: &GraphqlOrmSchemaSnapshot,
        rows_by_table: &BTreeMap<String, Vec<BackupRow>>,
        context: &RestoreContext,
    ) -> Result<Vec<ImportReport>, sqlx::Error>;
}

impl GraphqlOrmBackupRuntime for crate::db::Database {
    async fn begin_consistent_snapshot(&self) -> Result<BackupSnapshot, sqlx::Error> {
        begin_consistent_snapshot(self.pool()).await
    }

    fn backup_backend_capabilities(&self) -> BackupBackendCapabilities {
        backup_backend_capabilities()
    }

    fn list_backup_entities(&self, entities: &[&EntityMetadata]) -> Vec<EntityBackupDescriptor> {
        backup_descriptors_from_entities(entities)
    }

    fn schema_snapshot(
        &self,
        migration_version: impl Into<String>,
        entities: &[&EntityMetadata],
    ) -> GraphqlOrmSchemaSnapshot {
        schema_snapshot_from_entities(current_backend(), migration_version, entities)
    }

    async fn export_table_rows(
        &self,
        snapshot: &mut BackupSnapshot,
        entity: &EntityBackupDescriptor,
    ) -> Result<Vec<BackupRow>, sqlx::Error> {
        export_table_rows(snapshot, entity).await
    }

    async fn import_table_rows(
        &self,
        entity: &EntityBackupDescriptor,
        rows: &[BackupRow],
        context: &RestoreContext,
    ) -> Result<ImportReport, sqlx::Error> {
        import_table_rows(self.pool(), entity, rows, context).await
    }

    async fn restore_backup_rows(
        &self,
        backup_snapshot: &GraphqlOrmSchemaSnapshot,
        current_snapshot: &GraphqlOrmSchemaSnapshot,
        rows_by_table: &BTreeMap<String, Vec<BackupRow>>,
        context: &RestoreContext,
    ) -> Result<Vec<ImportReport>, sqlx::Error> {
        restore_backup_rows(
            self,
            backup_snapshot,
            current_snapshot,
            rows_by_table,
            context,
        )
        .await
    }
}

impl crate::db::Database {
    pub async fn ensure_change_journal_table(&self) -> Result<(), sqlx::Error> {
        if !cfg!(feature = "change-journal") {
            return Err(sqlx::Error::Protocol(
                "change journal support requires the graphql-orm change-journal feature"
                    .to_string(),
            ));
        }
        ensure_change_journal_table(self.pool()).await
    }

    pub async fn export_changes(
        &self,
        window: ChangeWindow,
    ) -> Result<Vec<BackupChange>, sqlx::Error> {
        if !cfg!(feature = "change-journal") {
            return Err(sqlx::Error::Protocol(
                "incremental backup export requires the graphql-orm change-journal feature"
                    .to_string(),
            ));
        }
        export_changes(self.pool(), window).await
    }
}

pub fn compare_schema_snapshots(
    backup: &GraphqlOrmSchemaSnapshot,
    current: &GraphqlOrmSchemaSnapshot,
) -> BackupCompatibility {
    if backup.schema_hash == current.schema_hash {
        BackupCompatibility::Exact
    } else if backup.backend != current.backend {
        BackupCompatibility::Incompatible {
            reason: format!(
                "backup backend {} does not match current backend {}",
                backup.backend, current.backend
            ),
        }
    } else {
        BackupCompatibility::OlderSchema {
            backup_hash: backup.schema_hash.clone(),
            current_hash: current.schema_hash.clone(),
        }
    }
}

pub async fn restore_backup_rows(
    database: &crate::db::Database,
    backup_snapshot: &GraphqlOrmSchemaSnapshot,
    current_snapshot: &GraphqlOrmSchemaSnapshot,
    rows_by_table: &BTreeMap<String, Vec<BackupRow>>,
    context: &RestoreContext,
) -> Result<Vec<ImportReport>, sqlx::Error> {
    match compare_schema_snapshots(backup_snapshot, current_snapshot) {
        BackupCompatibility::Exact => {}
        BackupCompatibility::OlderSchema {
            backup_hash,
            current_hash,
        } => {
            return Err(sqlx::Error::Protocol(format!(
                "backup schema hash {backup_hash} does not match current schema hash {current_hash}; restore from older schemas requires an explicit compatibility mapper"
            )));
        }
        BackupCompatibility::Incompatible { reason } => {
            return Err(sqlx::Error::Protocol(format!(
                "backup schema is incompatible: {reason}"
            )));
        }
    }

    let known_tables = current_snapshot
        .entities
        .iter()
        .map(|entity| entity.table_name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for table_name in rows_by_table.keys() {
        if !known_tables.contains(table_name.as_str()) {
            return Err(sqlx::Error::Protocol(format!(
                "backup rows include unknown table {table_name}"
            )));
        }
    }

    let mut entities = current_snapshot.entities.clone();
    entities.sort_by_key(|entity| (entity.restore_order, entity.table_name.clone()));
    let mut reports = Vec::new();
    for entity in entities {
        let empty_rows = Vec::new();
        let rows = rows_by_table.get(&entity.table_name).unwrap_or(&empty_rows);
        reports.push(database.import_table_rows(&entity, rows, context).await?);
    }
    Ok(reports)
}

#[cfg(feature = "sqlite")]
async fn begin_consistent_snapshot(pool: &DbPool) -> Result<BackupSnapshot, sqlx::Error> {
    let tx = pool.begin().await?;
    Ok(BackupSnapshot { tx })
}

#[cfg(feature = "postgres")]
async fn begin_consistent_snapshot(pool: &DbPool) -> Result<BackupSnapshot, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    Ok(BackupSnapshot { tx })
}

#[cfg(feature = "sqlite")]
fn backup_backend_capabilities() -> BackupBackendCapabilities {
    BackupBackendCapabilities {
        consistent_snapshot: true,
        typed_json: false,
        typed_uuid: false,
        bytes: true,
        deferred_constraints: false,
    }
}

async fn ensure_change_journal_table(pool: &DbPool) -> Result<(), sqlx::Error> {
    #[cfg(feature = "sqlite")]
    let sql = format!(
        "CREATE TABLE IF NOT EXISTS {} (
            id TEXT PRIMARY KEY,
            entity_name TEXT NOT NULL,
            table_name TEXT NOT NULL,
            primary_key TEXT NOT NULL,
            action TEXT NOT NULL,
            changed_at INTEGER NOT NULL,
            transaction_id TEXT NULL,
            row_hash TEXT NULL,
            actor_id TEXT NULL,
            correlation_id TEXT NULL
        )",
        quote_identifier(CHANGE_JOURNAL_TABLE)
    );

    #[cfg(feature = "postgres")]
    let sql = format!(
        "CREATE TABLE IF NOT EXISTS {} (
            id UUID PRIMARY KEY,
            entity_name TEXT NOT NULL,
            table_name TEXT NOT NULL,
            primary_key TEXT NOT NULL,
            action TEXT NOT NULL,
            changed_at BIGINT NOT NULL,
            transaction_id TEXT NULL,
            row_hash TEXT NULL,
            actor_id TEXT NULL,
            correlation_id TEXT NULL
        )",
        quote_identifier(CHANGE_JOURNAL_TABLE)
    );

    sqlx::query(&sql).execute(pool).await?;
    Ok(())
}

pub(crate) async fn record_change_journal_event<B>(
    hook_ctx: &mut super::MutationContext<'_, B>,
    event: &MutationEvent,
) -> Result<(), sqlx::Error>
where
    B: super::WriteBackend,
    for<'c> &'c mut <B::Database as sqlx::Database>::Connection:
        sqlx::Executor<'c, Database = B::Database> + Send,
{
    if !cfg!(feature = "change-journal")
        || !hook_ctx.database().change_journal_enabled()
        || event.phase != MutationPhase::After
    {
        return Ok(());
    }

    let action = match event.action {
        super::ChangeAction::Created => "create",
        super::ChangeAction::Updated => "update",
        super::ChangeAction::Deleted => "delete",
    };
    let changed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let sql = format!(
        "INSERT INTO {} (
            id,
            entity_name,
            table_name,
            primary_key,
            action,
            changed_at,
            transaction_id,
            row_hash,
            actor_id,
            correlation_id
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        quote_identifier(CHANGE_JOURNAL_TABLE)
    );
    let values = vec![
        SqlValue::Uuid(uuid::Uuid::new_v4()),
        SqlValue::String(event.entity_name.to_string()),
        SqlValue::String(event.table_name.to_string()),
        SqlValue::String(event.id.clone()),
        SqlValue::String(action.to_string()),
        SqlValue::Int(changed_at),
        SqlValue::Null,
        SqlValue::Null,
        SqlValue::Null,
        SqlValue::Null,
    ];
    execute_with_binds_on::<B, _>(hook_ctx.executor(), &sql, &values).await?;
    Ok(())
}

async fn export_changes(
    pool: &DbPool,
    window: ChangeWindow,
) -> Result<Vec<BackupChange>, sqlx::Error> {
    let mut conditions = vec!["changed_at <= ?".to_string()];
    let mut values = vec![SqlValue::Int(window.until)];
    if let Some(after_snapshot_id) = window.after_snapshot_id {
        conditions.push("id > ?".to_string());
        values.push(SqlValue::Uuid(after_snapshot_id));
    }
    let sql = format!(
        "SELECT id, entity_name, table_name, primary_key, action, changed_at, transaction_id, row_hash, actor_id, correlation_id
         FROM {}
         WHERE {}
         ORDER BY changed_at ASC, id ASC",
        quote_identifier(CHANGE_JOURNAL_TABLE),
        conditions.join(" AND ")
    );
    let rows = super::fetch_rows::<DefaultBackend>(pool, &sql, &values).await?;
    rows.into_iter().map(decode_backup_change).collect()
}

#[cfg(feature = "postgres")]
fn backup_backend_capabilities() -> BackupBackendCapabilities {
    BackupBackendCapabilities {
        consistent_snapshot: true,
        typed_json: true,
        typed_uuid: true,
        bytes: true,
        deferred_constraints: true,
    }
}

async fn export_table_rows(
    snapshot: &mut BackupSnapshot,
    entity: &EntityBackupDescriptor,
) -> Result<Vec<BackupRow>, sqlx::Error> {
    let export_columns = export_columns(entity);
    let select_columns = export_columns
        .iter()
        .map(|column| quote_identifier(&column.column_name))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT {} FROM {} ORDER BY {}",
        select_columns,
        quote_identifier(&entity.table_name),
        quote_identifier(&entity.primary_key_column)
    );

    let rows = fetch_snapshot_rows(snapshot, &sql).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut values = BTreeMap::new();
        for column in &export_columns {
            let value =
                if column.backup_policy == ColumnBackupPolicy::Redact && !column.is_primary_key {
                    BackupValue::String(REDACTED_BACKUP_VALUE.to_string())
                } else {
                    decode_backup_value(&row, &column.column_name, column.logical_type)?
                };
            values.insert(column.column_name.clone(), value);
        }

        let primary_key = values
            .get(&entity.primary_key_column)
            .ok_or_else(|| {
                sqlx::Error::Protocol(format!(
                    "backup export for {} did not include primary key column {}",
                    entity.table_name, entity.primary_key_column
                ))
            })?
            .primary_key_string();
        let row_hash = canonical_row_hash(&entity.table_name, &values);
        out.push(BackupRow {
            table_name: entity.table_name.clone(),
            primary_key,
            row_hash,
            values,
        });
    }
    Ok(out)
}

async fn import_table_rows(
    pool: &DbPool,
    entity: &EntityBackupDescriptor,
    rows: &[BackupRow],
    context: &RestoreContext,
) -> Result<ImportReport, sqlx::Error> {
    match context.mode {
        RestoreMode::ReplaceExisting => {
            return Err(sqlx::Error::Protocol(
                "RestoreMode::ReplaceExisting is not supported yet".to_string(),
            ));
        }
        RestoreMode::DryRun => {
            validate_import_rows(entity, rows)?;
            return Ok(ImportReport {
                table_name: entity.table_name.clone(),
                rows_imported: 0,
                rows_validated: rows.len(),
            });
        }
        RestoreMode::EmptyDatabase => {}
    }

    validate_import_rows(entity, rows)?;
    let mut tx = pool.begin().await?;
    let count = table_row_count_on(&mut tx, &entity.table_name).await?;
    if count != 0 {
        return Err(sqlx::Error::Protocol(format!(
            "cannot restore table {} in EmptyDatabase mode because it already contains {} rows",
            entity.table_name, count
        )));
    }

    let import_columns = import_columns(entity, rows);
    let column_sql = import_columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let placeholder_sql = (0..import_columns.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let insert_sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quote_identifier(&entity.table_name),
        column_sql,
        placeholder_sql
    );

    for row in rows {
        let values = import_columns
            .iter()
            .map(|column| {
                backup_value_to_sql_value(row.values.get(column).ok_or_else(|| {
                    sqlx::Error::Protocol(format!(
                        "backup row {} for table {} is missing import column {}",
                        row.primary_key, entity.table_name, column
                    ))
                })?)
            })
            .collect::<Result<Vec<_>, _>>()?;
        execute_import_insert(&mut tx, &insert_sql, &values).await?;
    }

    let imported_count = table_row_count_on(&mut tx, &entity.table_name).await? as usize;
    if imported_count != rows.len() {
        let _ = tx.rollback().await;
        return Err(sqlx::Error::Protocol(format!(
            "restore row count validation failed for {}: imported {}, expected {}",
            entity.table_name,
            imported_count,
            rows.len()
        )));
    }
    tx.commit().await?;

    let mut snapshot = begin_consistent_snapshot(pool).await?;
    let exported = export_table_rows(&mut snapshot, entity).await?;
    let expected_hashes = rows
        .iter()
        .map(|row| (row.primary_key.as_str(), row.row_hash.as_str()))
        .collect::<BTreeMap<_, _>>();
    let actual_hashes = exported
        .iter()
        .map(|row| (row.primary_key.as_str(), row.row_hash.as_str()))
        .collect::<BTreeMap<_, _>>();
    if actual_hashes != expected_hashes {
        return Err(sqlx::Error::Protocol(format!(
            "restore row hash validation failed for table {}",
            entity.table_name
        )));
    }

    Ok(ImportReport {
        table_name: entity.table_name.clone(),
        rows_imported: rows.len(),
        rows_validated: rows.len(),
    })
}

fn validate_import_rows(
    entity: &EntityBackupDescriptor,
    rows: &[BackupRow],
) -> Result<(), sqlx::Error> {
    let known_columns = entity
        .columns
        .iter()
        .map(|column| (&column.column_name, column))
        .collect::<BTreeMap<_, _>>();
    for row in rows {
        if row.table_name != entity.table_name {
            return Err(sqlx::Error::Protocol(format!(
                "backup row table {} does not match import table {}",
                row.table_name, entity.table_name
            )));
        }
        for column_name in row.values.keys() {
            if !known_columns.contains_key(column_name) {
                return Err(sqlx::Error::Protocol(format!(
                    "backup row for table {} contains unknown column {}",
                    entity.table_name, column_name
                )));
            }
        }
        for column in entity
            .columns
            .iter()
            .filter(|column| column.backup_policy != ColumnBackupPolicy::Exclude)
        {
            let Some(value) = row.values.get(&column.column_name) else {
                return Err(sqlx::Error::Protocol(format!(
                    "backup row {} for table {} is missing column {}",
                    row.primary_key, entity.table_name, column.column_name
                )));
            };
            if !column.nullable && matches!(value, BackupValue::Null) {
                return Err(sqlx::Error::Protocol(format!(
                    "backup row {} for table {} has null for non-null column {}",
                    row.primary_key, entity.table_name, column.column_name
                )));
            }
        }
    }
    Ok(())
}

fn export_columns(entity: &EntityBackupDescriptor) -> Vec<&super::ColumnBackupDescriptor> {
    entity
        .columns
        .iter()
        .filter(|column| {
            column.backup_policy != ColumnBackupPolicy::Exclude || column.is_primary_key
        })
        .collect()
}

fn import_columns(entity: &EntityBackupDescriptor, rows: &[BackupRow]) -> Vec<String> {
    entity
        .columns
        .iter()
        .filter(|column| {
            column.backup_policy != ColumnBackupPolicy::Exclude || column.is_primary_key
        })
        .filter(|column| {
            rows.first()
                .map(|row| row.values.contains_key(&column.column_name))
                .unwrap_or(true)
        })
        .map(|column| column.column_name.clone())
        .collect()
}

#[cfg(feature = "sqlite")]
async fn table_row_count_on(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table_name: &str,
) -> Result<i64, sqlx::Error> {
    let sql = format!(
        "SELECT COUNT(*) AS count FROM {}",
        quote_identifier(table_name)
    );
    let row = sqlx::query(&sql).fetch_one(&mut **tx).await?;
    row.try_get::<i64, _>("count")
}

#[cfg(feature = "sqlite")]
async fn execute_import_insert(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    sql: &str,
    values: &[SqlValue],
) -> Result<(), sqlx::Error> {
    execute_with_binds_on::<SqliteBackend, _>(&mut **tx, sql, values).await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn table_row_count_on(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table_name: &str,
) -> Result<i64, sqlx::Error> {
    let sql = format!(
        "SELECT COUNT(*) AS count FROM {}",
        quote_identifier(table_name)
    );
    let row = sqlx::query(&sql).fetch_one(&mut **tx).await?;
    row.try_get::<i64, _>("count")
}

#[cfg(feature = "postgres")]
async fn execute_import_insert(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    sql: &str,
    values: &[SqlValue],
) -> Result<(), sqlx::Error> {
    execute_with_binds_on::<PostgresBackend, _>(&mut **tx, sql, values).await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn fetch_snapshot_rows(
    snapshot: &mut BackupSnapshot,
    sql: &str,
) -> Result<Vec<crate::DbRow>, sqlx::Error> {
    sqlx::query(sql).fetch_all(&mut *snapshot.tx).await
}

#[cfg(feature = "postgres")]
async fn fetch_snapshot_rows(
    snapshot: &mut BackupSnapshot,
    sql: &str,
) -> Result<Vec<crate::DbRow>, sqlx::Error> {
    sqlx::query(sql).fetch_all(&mut *snapshot.tx).await
}

#[cfg(feature = "sqlite")]
fn decode_backup_value(
    row: &crate::DbRow,
    column: &str,
    kind: BackupValueKind,
) -> Result<BackupValue, sqlx::Error> {
    match kind {
        BackupValueKind::Null => Ok(BackupValue::Null),
        BackupValueKind::Bool => row.try_get::<Option<i64>, _>(column).map(|value| {
            value
                .map(|value| BackupValue::Bool(value != 0))
                .unwrap_or(BackupValue::Null)
        }),
        BackupValueKind::Integer => row
            .try_get::<Option<i64>, _>(column)
            .map(|value| value.map(BackupValue::Integer).unwrap_or(BackupValue::Null)),
        BackupValueKind::Float => row
            .try_get::<Option<f64>, _>(column)
            .map(|value| value.map(BackupValue::Float).unwrap_or(BackupValue::Null)),
        BackupValueKind::String => row
            .try_get::<Option<String>, _>(column)
            .map(|value| value.map(BackupValue::String).unwrap_or(BackupValue::Null)),
        BackupValueKind::Uuid => row.try_get::<Option<String>, _>(column).and_then(|value| {
            value
                .map(|value| {
                    uuid::Uuid::parse_str(&value)
                        .map(BackupValue::Uuid)
                        .map_err(|error| sqlx::Error::Decode(Box::new(error)))
                })
                .unwrap_or(Ok(BackupValue::Null))
        }),
        BackupValueKind::Json => row.try_get::<Option<String>, _>(column).and_then(|value| {
            value
                .map(|value| {
                    serde_json::from_str(&value)
                        .map(BackupValue::Json)
                        .map_err(|error| sqlx::Error::Decode(Box::new(error)))
                })
                .unwrap_or(Ok(BackupValue::Null))
        }),
        BackupValueKind::Bytes => row
            .try_get::<Option<Vec<u8>>, _>(column)
            .map(|value| value.map(BackupValue::Bytes).unwrap_or(BackupValue::Null)),
    }
}

#[cfg(feature = "postgres")]
fn decode_backup_value(
    row: &crate::DbRow,
    column: &str,
    kind: BackupValueKind,
) -> Result<BackupValue, sqlx::Error> {
    match kind {
        BackupValueKind::Null => Ok(BackupValue::Null),
        BackupValueKind::Bool => row
            .try_get::<Option<bool>, _>(column)
            .map(|value| value.map(BackupValue::Bool).unwrap_or(BackupValue::Null)),
        BackupValueKind::Integer => row
            .try_get::<Option<i64>, _>(column)
            .map(|value| value.map(BackupValue::Integer).unwrap_or(BackupValue::Null)),
        BackupValueKind::Float => row
            .try_get::<Option<f64>, _>(column)
            .map(|value| value.map(BackupValue::Float).unwrap_or(BackupValue::Null)),
        BackupValueKind::String => row
            .try_get::<Option<String>, _>(column)
            .map(|value| value.map(BackupValue::String).unwrap_or(BackupValue::Null)),
        BackupValueKind::Uuid => row
            .try_get::<Option<uuid::Uuid>, _>(column)
            .map(|value| value.map(BackupValue::Uuid).unwrap_or(BackupValue::Null)),
        BackupValueKind::Json => row
            .try_get::<Option<sqlx::types::Json<serde_json::Value>>, _>(column)
            .map(|value| {
                value
                    .map(|value| BackupValue::Json(value.0))
                    .unwrap_or(BackupValue::Null)
            }),
        BackupValueKind::Bytes => row
            .try_get::<Option<Vec<u8>>, _>(column)
            .map(|value| value.map(BackupValue::Bytes).unwrap_or(BackupValue::Null)),
    }
}

fn backup_value_to_sql_value(value: &BackupValue) -> Result<SqlValue, sqlx::Error> {
    Ok(match value {
        BackupValue::Null => SqlValue::Null,
        BackupValue::Bool(value) => SqlValue::Bool(*value),
        BackupValue::Integer(value) => SqlValue::Int(*value),
        BackupValue::Float(value) => SqlValue::Float(*value),
        BackupValue::String(value) => SqlValue::String(value.clone()),
        BackupValue::Uuid(value) => SqlValue::Uuid(*value),
        BackupValue::Json(value) => SqlValue::Json(value.clone()),
        BackupValue::Bytes(value) => SqlValue::Bytes(value.clone()),
    })
}

#[cfg(feature = "sqlite")]
fn decode_backup_change(row: crate::DbRow) -> Result<BackupChange, sqlx::Error> {
    let id: String = row.try_get("id")?;
    decode_backup_change_parts(
        uuid::Uuid::parse_str(&id).map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
        row.try_get("entity_name")?,
        row.try_get("table_name")?,
        row.try_get("primary_key")?,
        row.try_get("action")?,
        row.try_get("changed_at")?,
        row.try_get("transaction_id")?,
        row.try_get("row_hash")?,
        row.try_get("actor_id")?,
        row.try_get("correlation_id")?,
    )
}

#[cfg(feature = "postgres")]
fn decode_backup_change(row: crate::DbRow) -> Result<BackupChange, sqlx::Error> {
    decode_backup_change_parts(
        row.try_get("id")?,
        row.try_get("entity_name")?,
        row.try_get("table_name")?,
        row.try_get("primary_key")?,
        row.try_get("action")?,
        row.try_get("changed_at")?,
        row.try_get("transaction_id")?,
        row.try_get("row_hash")?,
        row.try_get("actor_id")?,
        row.try_get("correlation_id")?,
    )
}

fn decode_backup_change_parts(
    id: uuid::Uuid,
    entity_name: String,
    table_name: String,
    primary_key: String,
    action: String,
    changed_at: i64,
    transaction_id: Option<String>,
    row_hash: Option<String>,
    actor_id: Option<String>,
    correlation_id: Option<String>,
) -> Result<BackupChange, sqlx::Error> {
    let action = match action.as_str() {
        "create" => BackupChangeAction::Create,
        "update" => BackupChangeAction::Update,
        "delete" => BackupChangeAction::Delete,
        other => {
            return Err(sqlx::Error::Protocol(format!(
                "unknown change journal action {other}"
            )));
        }
    };
    Ok(BackupChange {
        id,
        entity_name,
        table_name,
        primary_key,
        action,
        changed_at,
        transaction_id,
        row_hash,
        actor_id,
        correlation_id,
    })
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

impl BackupValue {
    fn primary_key_string(&self) -> String {
        match self {
            BackupValue::Null => "null".to_string(),
            BackupValue::Bool(value) => value.to_string(),
            BackupValue::Integer(value) => value.to_string(),
            BackupValue::Float(value) => value.to_string(),
            BackupValue::String(value) => value.clone(),
            BackupValue::Uuid(value) => value.to_string(),
            BackupValue::Json(value) => value.to_string(),
            BackupValue::Bytes(value) => hex_encode(value),
        }
    }
}

pub fn canonical_row_hash(table_name: &str, values: &BTreeMap<String, BackupValue>) -> String {
    let mut canonical = String::new();
    canonical.push_str("table:");
    canonical.push_str(table_name);
    canonical.push('\n');
    for (column, value) in values {
        canonical.push_str(column);
        canonical.push('=');
        canonical.push_str(&canonical_value(value));
        canonical.push('\n');
    }
    format!("{:016x}", fnv1a64(canonical.as_bytes()))
}

fn canonical_value(value: &BackupValue) -> String {
    match value {
        BackupValue::Null => "null".to_string(),
        BackupValue::Bool(value) => format!("bool:{value}"),
        BackupValue::Integer(value) => format!("int:{value}"),
        BackupValue::Float(value) => format!("float:{value:?}"),
        BackupValue::String(value) => format!("string:{value}"),
        BackupValue::Uuid(value) => format!("uuid:{value}"),
        BackupValue::Json(value) => format!("json:{}", canonical_json(value)),
        BackupValue::Bytes(value) => format!("bytes:{}", hex_encode(value)),
    }
}

fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let sorted = map
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<BTreeMap<_, _>>();
            serde_json::to_string(&sorted).unwrap_or_else(|_| value.to_string())
        }
        _ => value.to_string(),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
