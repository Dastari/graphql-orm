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
    Decimal(super::DecimalValue),
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
    async fn begin_consistent_snapshot(&self) -> crate::Result<BackupSnapshot>;

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
    ) -> crate::Result<Vec<BackupRow>>;

    async fn import_table_rows(
        &self,
        entity: &EntityBackupDescriptor,
        rows: &[BackupRow],
        context: &RestoreContext,
    ) -> crate::Result<ImportReport>;

    async fn restore_backup_rows(
        &self,
        backup_snapshot: &GraphqlOrmSchemaSnapshot,
        current_snapshot: &GraphqlOrmSchemaSnapshot,
        rows_by_table: &BTreeMap<String, Vec<BackupRow>>,
        context: &RestoreContext,
    ) -> crate::Result<Vec<ImportReport>>;
}

impl GraphqlOrmBackupRuntime for crate::db::Database {
    async fn begin_consistent_snapshot(&self) -> crate::Result<BackupSnapshot> {
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
    ) -> crate::Result<Vec<BackupRow>> {
        export_table_rows(snapshot, entity).await
    }

    async fn import_table_rows(
        &self,
        entity: &EntityBackupDescriptor,
        rows: &[BackupRow],
        context: &RestoreContext,
    ) -> crate::Result<ImportReport> {
        import_table_rows(self.pool(), entity, rows, context).await
    }

    async fn restore_backup_rows(
        &self,
        backup_snapshot: &GraphqlOrmSchemaSnapshot,
        current_snapshot: &GraphqlOrmSchemaSnapshot,
        rows_by_table: &BTreeMap<String, Vec<BackupRow>>,
        context: &RestoreContext,
    ) -> crate::Result<Vec<ImportReport>> {
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

/// The database a handle is connected to, as the server reports it rather than as a connection
/// string names it. Compare it with an operator-stated expectation before restoring into, or
/// otherwise writing destructively to, a database.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DatabaseIdentity {
    /// PostgreSQL: `current_database()`. SQLite: the main database file path, empty for an
    /// in-memory database.
    pub name: String,
    /// PostgreSQL: the database's `pg_database` OID, which distinguishes a dropped and recreated
    /// database of the same name. SQLite: `None`.
    pub object_id: Option<i64>,
}

impl crate::db::Database {
    /// Read the identity of the connected database. See [`DatabaseIdentity`].
    pub async fn database_identity(&self) -> crate::Result<DatabaseIdentity> {
        database_identity(self.pool()).await
    }

    pub async fn ensure_change_journal_table(&self) -> crate::Result<()> {
        if !cfg!(feature = "change-journal") {
            return Err(sqlx::Error::Protocol(
                "change journal support requires the graphql-orm change-journal feature"
                    .to_string(),
            ));
        }
        ensure_change_journal_table(self.pool()).await
    }

    pub async fn export_changes(&self, window: ChangeWindow) -> crate::Result<Vec<BackupChange>> {
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
) -> crate::Result<Vec<ImportReport>> {
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

    // Refuse before any write: every reference must resolve inside the backup, and an
    // empty-database restore must find every table empty. A later failure can then only be a
    // database error, never a partial restore of a backup that was invalid from the start.
    validate_backup_references(&current_snapshot.entities, rows_by_table)?;
    if context.mode == RestoreMode::EmptyDatabase {
        let mut tx = database.pool().begin().await?;
        for entity in &current_snapshot.entities {
            let count = table_row_count_on(&mut tx, &entity.table_name).await?;
            if count != 0 {
                return Err(sqlx::Error::Protocol(format!(
                    "cannot restore table {} in EmptyDatabase mode because it already contains {} rows",
                    entity.table_name, count
                )));
            }
        }
        tx.rollback().await?;
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
async fn database_identity(pool: &DbPool) -> crate::Result<DatabaseIdentity> {
    let row = sqlx::query("SELECT file FROM pragma_database_list WHERE name = 'main'")
        .fetch_one(pool)
        .await?;
    Ok(DatabaseIdentity {
        name: row.try_get::<Option<String>, _>("file")?.unwrap_or_default(),
        object_id: None,
    })
}

#[cfg(feature = "postgres")]
async fn database_identity(pool: &DbPool) -> crate::Result<DatabaseIdentity> {
    let row = sqlx::query(
        "SELECT datname::text AS name, oid::bigint AS object_id FROM pg_database WHERE datname = current_database()",
    )
    .fetch_one(pool)
    .await?;
    Ok(DatabaseIdentity {
        name: row.try_get("name")?,
        object_id: Some(row.try_get("object_id")?),
    })
}

#[cfg(feature = "sqlite")]
async fn begin_consistent_snapshot(pool: &DbPool) -> crate::Result<BackupSnapshot> {
    let tx = pool.begin().await?;
    Ok(BackupSnapshot { tx })
}

#[cfg(feature = "postgres")]
async fn begin_consistent_snapshot(pool: &DbPool) -> crate::Result<BackupSnapshot> {
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

async fn ensure_change_journal_table(pool: &DbPool) -> crate::Result<()> {
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
) -> crate::Result<()>
where
    B: super::WriteBackend,
{
    if !cfg!(feature = "change-journal")
        || !hook_ctx.database().change_journal_enabled()
        || event.phase != MutationPhase::After
    {
        return Ok(());
    }
    // The journal identifies rows by entity id, which cannot name a composite key; recording
    // such a row would make incremental chains resurrect or duplicate it. Fail closed.
    if event.metadata.primary_keys.len() > 1 {
        return Err(sqlx::Error::Protocol(format!(
            "the change journal cannot record composite-key entity {}; back it up with full snapshots",
            event.entity_name
        )));
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
    hook_ctx.execute(&sql, &values).await?;
    Ok(())
}

async fn export_changes(pool: &DbPool, window: ChangeWindow) -> crate::Result<Vec<BackupChange>> {
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
) -> crate::Result<Vec<BackupRow>> {
    let export_columns = export_columns(entity);
    let select_columns = export_columns
        .iter()
        .map(|column| quote_identifier(&column.column_name))
        .collect::<Vec<_>>()
        .join(", ");
    let key_columns = validated_key_columns(entity)?;
    let sql = format!(
        "SELECT {} FROM {} ORDER BY {}",
        select_columns,
        quote_identifier(&entity.table_name),
        key_columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ")
    );

    check_portable_date_times(snapshot, entity, &export_columns).await?;
    let rows = fetch_snapshot_rows(snapshot, &sql).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut values = BTreeMap::new();
        for column in &export_columns {
            let value =
                if column.backup_policy == ColumnBackupPolicy::Redact && !column.is_primary_key {
                    BackupValue::String(REDACTED_BACKUP_VALUE.to_string())
                } else {
                    decode_backup_value(&row, column)?
                };
            values.insert(column.column_name.clone(), value);
        }

        let primary_key = row_identity(entity, &key_columns, &values)?;
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
) -> crate::Result<ImportReport> {
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
        .map(|column| quote_identifier(&column.column_name))
        .collect::<Vec<_>>()
        .join(", ");
    let placeholder_sql = import_columns
        .iter()
        .map(|column| import_placeholder(column))
        .collect::<Vec<_>>()
        .join(", ");
    let insert_sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quote_identifier(&entity.table_name),
        column_sql,
        placeholder_sql
    );

    let order = import_row_order(entity, rows)?;
    let stored = |row: &BackupRow, column: &super::ColumnBackupDescriptor| {
        backup_value_to_sql_value(
            row.values.get(&column.column_name).ok_or_else(|| {
                sqlx::Error::Protocol(format!(
                    "backup row {} for table {} is missing import column {}",
                    row.primary_key, entity.table_name, column.column_name
                ))
            })?,
            column,
        )
    };
    for row in &order.ordered {
        let values = import_columns
            .iter()
            .map(|column| stored(row, column))
            .collect::<Result<Vec<_>, _>>()?;
        execute_import_statement(&mut tx, &insert_sql, &values).await?;
    }
    if !order.deferred.is_empty() {
        // Rows on (or below) a self-reference cycle. Insert each with placeholder self-reference
        // values that satisfy the foreign key on their own: NULL when the column is nullable,
        // the row's own key otherwise (a row that references itself passes the check at the end
        // of its statement). Then set the stored values. NULL placeholders never collide on a
        // UNIQUE column; the stored values are distinct already.
        let self_columns = import_columns
            .iter()
            .copied()
            .filter(|column| order.self_columns.contains(&column.column_name.as_str()))
            .collect::<Vec<_>>();
        let key_column = entity
            .columns
            .iter()
            .find(|column| column.column_name == entity.primary_key_column)
            .ok_or_else(|| {
                sqlx::Error::Protocol(format!(
                    "backup descriptor for {} does not describe its primary key column",
                    entity.table_name
                ))
            })?;
        for row in &order.deferred {
            let values = import_columns
                .iter()
                .map(|column| {
                    if !order.self_columns.contains(&column.column_name.as_str()) {
                        stored(row, column)
                    } else if column.nullable {
                        backup_value_to_sql_value(&BackupValue::Null, column)
                    } else {
                        stored(row, key_column)
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            execute_import_statement(&mut tx, &insert_sql, &values).await?;
        }
        let update_sql = format!(
            "UPDATE {} SET {} WHERE {} = {}",
            quote_identifier(&entity.table_name),
            self_columns
                .iter()
                .map(|column| format!(
                    "{} = {}",
                    quote_identifier(&column.column_name),
                    import_placeholder(column)
                ))
                .collect::<Vec<_>>()
                .join(", "),
            quote_identifier(&key_column.column_name),
            import_placeholder(key_column)
        );
        for row in &order.deferred {
            let mut values = self_columns
                .iter()
                .map(|column| stored(row, column))
                .collect::<Result<Vec<_>, _>>()?;
            values.push(stored(row, key_column)?);
            match execute_import_statement(&mut tx, &update_sql, &values).await {
                Err(sqlx::Error::Database(_)) => {
                    return Err(sqlx::Error::Decode(Box::new(BackupSelfReferenceCycle {
                        table_name: entity.table_name.clone(),
                        primary_key: row.primary_key.clone(),
                        columns: self_columns
                            .iter()
                            .map(|column| column.column_name.clone())
                            .collect(),
                    })));
                }
                result => result?,
            }
        }
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

fn validate_import_rows(entity: &EntityBackupDescriptor, rows: &[BackupRow]) -> crate::Result<()> {
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
            if column.backup_policy == ColumnBackupPolicy::Include {
                validate_backup_value(column, value)?;
            }
        }
    }
    Ok(())
}

fn validate_backup_value(
    descriptor: &super::ColumnBackupDescriptor,
    value: &BackupValue,
) -> crate::Result<()> {
    if matches!(value, BackupValue::Null) {
        return Ok(());
    }
    let valid = matches!(
        (descriptor.logical_type, value),
        (BackupValueKind::Bool, BackupValue::Bool(_))
            | (BackupValueKind::Integer, BackupValue::Integer(_))
            | (BackupValueKind::Float, BackupValue::Float(_))
            | (BackupValueKind::String, BackupValue::String(_))
            | (BackupValueKind::Uuid, BackupValue::Uuid(_))
            | (BackupValueKind::Json, BackupValue::Json(_))
            | (BackupValueKind::Bytes, BackupValue::Bytes(_))
    );
    if valid {
        return Ok(());
    }
    if let (BackupValueKind::DateTime, BackupValue::String(text)) = (descriptor.logical_type, value)
    {
        // Only the canonical form the export writes: a hand-edited or foreign representation
        // would restore a different stored text than the one the snapshot's row hash describes.
        if super::RuntimeDateTime::parse(text).is_ok_and(|canonical| canonical.as_str() == text) {
            return Ok(());
        }
        return Err(sqlx::Error::Protocol(format!(
            "backup date-time for column {} is not canonical UTC RFC 3339 text",
            descriptor.column_name
        )));
    }
    if let (BackupValueKind::Decimal, BackupValue::Decimal(value)) =
        (descriptor.logical_type, value)
    {
        if value.definition() == require_decimal_definition(descriptor)? {
            return Ok(());
        }
        return Err(sqlx::Error::Protocol(
            "backup decimal definition does not match destination column".to_string(),
        ));
    }
    Err(sqlx::Error::Protocol(format!(
        "backup value type does not match destination column {}",
        descriptor.column_name
    )))
}

/// A backup row references a row that the backup does not contain. Restore refuses it before
/// writing anything; read it from the error with [`BackupOrphanReference::from_error`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupOrphanReference {
    /// Table of the referencing row.
    pub table_name: String,
    /// Identity of the referencing row.
    pub primary_key: String,
    /// Referencing column.
    pub column: String,
    /// Referenced table and column.
    pub target_table: String,
    pub target_column: String,
}
impl std::fmt::Display for BackupOrphanReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "backup row {} of {} references a missing {}.{} through {}",
            self.primary_key, self.table_name, self.target_table, self.target_column, self.column
        )
    }
}
impl std::error::Error for BackupOrphanReference {}
impl BackupOrphanReference {
    /// The orphan reference that refused a restore, if `error` is one.
    pub fn from_error(error: &crate::Error) -> Option<&Self> {
        match error {
            sqlx::Error::Decode(source) => source.downcast_ref(),
            _ => None,
        }
    }
}

/// A restore could not set the stored self-reference values of a row on a self-reference cycle:
/// the table's constraints or triggers refused the staged update. A non-null UNIQUE
/// self-reference column cannot be staged without a temporary value (each row's own-key
/// placeholder is another row's stored value), and an append-only table refuses updates. The
/// table's import transaction rolls back. Read it from the error with
/// [`BackupSelfReferenceCycle::from_error`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct BackupSelfReferenceCycle {
    /// Table of the row.
    pub table_name: String,
    /// Identity of the row whose update was refused.
    pub primary_key: String,
    /// The self-reference columns the update set.
    pub columns: Vec<String>,
}
impl std::fmt::Display for BackupSelfReferenceCycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "backup row {} of {} could not take its stored self-reference values ({}) after a cycle was staged",
            self.primary_key,
            self.table_name,
            self.columns.join(", ")
        )
    }
}
impl std::error::Error for BackupSelfReferenceCycle {}
impl BackupSelfReferenceCycle {
    /// The self-reference cycle that refused a restore, if `error` is one.
    pub fn from_error(error: &crate::Error) -> Option<&Self> {
        match error {
            sqlx::Error::Decode(source) => source.downcast_ref(),
            _ => None,
        }
    }
}

/// Every non-null dependency value of every backup row names a row of the backup's own target
/// table. Dependencies on tables outside the snapshot, or on a column the backup excludes, are
/// left to the database.
fn validate_backup_references(
    entities: &[EntityBackupDescriptor],
    rows_by_table: &BTreeMap<String, Vec<BackupRow>>,
) -> crate::Result<()> {
    let empty = Vec::new();
    let mut targets = BTreeMap::<(&str, &str), Option<std::collections::BTreeSet<String>>>::new();
    for entity in entities {
        let rows = rows_by_table.get(&entity.table_name).unwrap_or(&empty);
        for dependency in &entity.dependencies {
            let Some(target) = entities
                .iter()
                .find(|candidate| candidate.table_name == dependency.table_name)
            else {
                continue;
            };
            // Only verbatim values can be compared; redacted or excluded columns are left to the
            // database.
            if !entity.columns.iter().any(|c| {
                c.column_name == dependency.source_column
                    && c.backup_policy == ColumnBackupPolicy::Include
            }) {
                continue;
            }
            let values = targets
                .entry((
                    target.table_name.as_str(),
                    dependency.target_column.as_str(),
                ))
                .or_insert_with(|| {
                    let column = target
                        .columns
                        .iter()
                        .find(|c| c.column_name == dependency.target_column)?;
                    if column.backup_policy != ColumnBackupPolicy::Include {
                        return None;
                    }
                    Some(
                        rows_by_table
                            .get(&target.table_name)
                            .unwrap_or(&empty)
                            .iter()
                            .filter_map(|row| row.values.get(&dependency.target_column))
                            .map(BackupValue::primary_key_string)
                            .collect(),
                    )
                });
            let Some(values) = values else {
                continue;
            };
            for row in rows {
                match row.values.get(&dependency.source_column) {
                    None | Some(BackupValue::Null) => {}
                    Some(value) if values.contains(&value.primary_key_string()) => {}
                    Some(_) => {
                        return Err(sqlx::Error::Decode(Box::new(BackupOrphanReference {
                            table_name: entity.table_name.clone(),
                            primary_key: row.primary_key.clone(),
                            column: dependency.source_column.clone(),
                            target_table: dependency.table_name.clone(),
                            target_column: dependency.target_column.clone(),
                        })));
                    }
                }
            }
        }
    }
    Ok(())
}

/// The key columns of `entity`, checked: a composite key lists at least two distinct primary-key
/// columns of the descriptor, starting with `primary_key_column`.
fn validated_key_columns(entity: &EntityBackupDescriptor) -> crate::Result<Vec<&str>> {
    let keys = entity.key_columns();
    let composite = !entity.primary_key_columns.is_empty();
    let distinct = keys.iter().collect::<std::collections::BTreeSet<_>>().len() == keys.len();
    let declared = keys.iter().all(|key| {
        entity
            .columns
            .iter()
            .any(|column| column.column_name == *key && column.is_primary_key)
    });
    if composite
        && (keys.len() < 2 || keys[0] != entity.primary_key_column || !distinct || !declared)
    {
        return Err(sqlx::Error::Protocol(format!(
            "backup descriptor for {} has an invalid composite primary key",
            entity.table_name
        )));
    }
    Ok(keys)
}

/// A row's stable identity: the single key's value text, unchanged from earlier releases, or the
/// canonical JSON array of every key part for a composite key.
fn row_identity(
    entity: &EntityBackupDescriptor,
    key_columns: &[&str],
    values: &BTreeMap<String, BackupValue>,
) -> crate::Result<String> {
    let composite = !entity.primary_key_columns.is_empty();
    let parts = key_columns
        .iter()
        .map(|column| {
            let value = values.get(*column).ok_or_else(|| {
                sqlx::Error::Protocol(format!(
                    "backup export for {} did not include primary key column {}",
                    entity.table_name, column
                ))
            })?;
            // A composite part must be unambiguous: no null (which would read as the text
            // "null"), and JSON in canonical form. Single-column keys keep their exact identity.
            match value {
                BackupValue::Null if composite => Err(sqlx::Error::Protocol(format!(
                    "backup row of {} has a null primary key part {}",
                    entity.table_name, column
                ))),
                BackupValue::Json(json) if composite => Ok(canonical_json(json)),
                value => Ok(value.primary_key_string()),
            }
        })
        .collect::<crate::Result<Vec<_>>>()?;
    Ok(match parts.as_slice() {
        [single] if entity.primary_key_columns.is_empty() => single.clone(),
        _ => serde_json::to_string(&parts)
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?,
    })
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

fn import_columns<'a>(
    entity: &'a EntityBackupDescriptor,
    rows: &[BackupRow],
) -> Vec<&'a super::ColumnBackupDescriptor> {
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
        .collect()
}

/// A table's rows in insert order. Rows a self-reference cycle keeps from parent-first order
/// (the cycle's rows and every row below them) are `deferred`: inserted after `ordered` with
/// placeholder values in `self_columns`, then updated to their stored values.
struct ImportRowOrder<'a> {
    ordered: Vec<&'a BackupRow>,
    deferred: Vec<&'a BackupRow>,
    self_columns: Vec<&'a str>,
}

fn import_row_order<'a>(
    entity: &'a EntityBackupDescriptor,
    rows: &'a [BackupRow],
) -> crate::Result<ImportRowOrder<'a>> {
    // Self-references order rows by a single-column key; a composite-key table has none.
    let single_key = entity.primary_key_columns.is_empty();
    let self_dependencies = entity
        .dependencies
        .iter()
        .filter(|dependency| {
            single_key
                && dependency.table_name == entity.table_name
                && dependency.target_column == entity.primary_key_column
                && dependency.source_column != entity.primary_key_column
        })
        .collect::<Vec<_>>();
    if self_dependencies.is_empty() || rows.len() < 2 {
        return Ok(ImportRowOrder {
            ordered: rows.iter().collect(),
            deferred: Vec::new(),
            self_columns: Vec::new(),
        });
    }

    let row_indexes = rows
        .iter()
        .enumerate()
        .map(|(index, row)| (row.primary_key.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut children = vec![Vec::new(); rows.len()];
    let mut indegrees = vec![0_usize; rows.len()];

    for (child_index, row) in rows.iter().enumerate() {
        let mut parents = std::collections::BTreeSet::new();
        for dependency in &self_dependencies {
            let Some(value) = row.values.get(&dependency.source_column) else {
                return Err(sqlx::Error::Protocol(format!(
                    "backup row {} for table {} is missing self-reference column {}",
                    row.primary_key, entity.table_name, dependency.source_column
                )));
            };
            if matches!(value, BackupValue::Null) {
                continue;
            }
            let parent_key = value.primary_key_string();
            let Some(&parent_index) = row_indexes.get(parent_key.as_str()) else {
                return Err(sqlx::Error::Protocol(format!(
                    "backup row {} for table {} references missing row {} through {}",
                    row.primary_key, entity.table_name, parent_key, dependency.source_column
                )));
            };
            if parent_index != child_index {
                parents.insert(parent_index);
            }
        }
        for parent_index in parents {
            children[parent_index].push(child_index);
            indegrees[child_index] += 1;
        }
    }

    let mut ready = indegrees
        .iter()
        .enumerate()
        .filter_map(|(index, indegree)| (*indegree == 0).then_some(index))
        .collect::<std::collections::BTreeSet<_>>();
    let mut placed = vec![false; rows.len()];
    let mut ordered = Vec::with_capacity(rows.len());
    while let Some(index) = ready.pop_first() {
        placed[index] = true;
        ordered.push(&rows[index]);
        for child_index in &children[index] {
            indegrees[*child_index] -= 1;
            if indegrees[*child_index] == 0 {
                ready.insert(*child_index);
            }
        }
    }
    // A row whose parent is deferred is never ready, so it is deferred too: no ordered row
    // references a deferred one.
    let deferred = rows
        .iter()
        .zip(&placed)
        .filter_map(|(row, placed)| (!placed).then_some(row))
        .collect::<Vec<_>>();
    let mut self_columns = self_dependencies
        .iter()
        .map(|dependency| dependency.source_column.as_str())
        .collect::<Vec<_>>();
    self_columns.sort_unstable();
    self_columns.dedup();
    Ok(ImportRowOrder {
        ordered,
        deferred,
        self_columns,
    })
}

#[cfg(feature = "sqlite")]
async fn table_row_count_on(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table_name: &str,
) -> crate::Result<i64> {
    let sql = format!(
        "SELECT COUNT(*) AS count FROM {}",
        quote_identifier(table_name)
    );
    let row = sqlx::query(&sql).fetch_one(&mut **tx).await?;
    row.try_get::<i64, _>("count")
}

#[cfg(feature = "sqlite")]
async fn execute_import_statement(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    sql: &str,
    values: &[SqlValue],
) -> crate::Result<()> {
    execute_with_binds_on::<SqliteBackend, _>(&mut **tx, sql, values).await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn table_row_count_on(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table_name: &str,
) -> crate::Result<i64> {
    let sql = format!(
        "SELECT COUNT(*) AS count FROM {}",
        quote_identifier(table_name)
    );
    let row = sqlx::query(&sql).fetch_one(&mut **tx).await?;
    row.try_get::<i64, _>("count")
}

#[cfg(feature = "postgres")]
async fn execute_import_statement(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    sql: &str,
    values: &[SqlValue],
) -> crate::Result<()> {
    execute_with_binds_on::<PostgresBackend, _>(&mut **tx, sql, values).await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn fetch_snapshot_rows(
    snapshot: &mut BackupSnapshot,
    sql: &str,
) -> crate::Result<Vec<crate::DbRow>> {
    sqlx::query(sql).fetch_all(&mut *snapshot.tx).await
}

#[cfg(feature = "postgres")]
async fn fetch_snapshot_rows(
    snapshot: &mut BackupSnapshot,
    sql: &str,
) -> crate::Result<Vec<crate::DbRow>> {
    sqlx::query(sql).fetch_all(&mut *snapshot.tx).await
}

#[cfg(feature = "sqlite")]
fn decode_backup_value(
    row: &crate::DbRow,
    descriptor: &super::ColumnBackupDescriptor,
) -> crate::Result<BackupValue> {
    let column = descriptor.column_name.as_str();
    let kind = descriptor.logical_type;
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
        BackupValueKind::Decimal => {
            let definition = require_decimal_definition(descriptor)?;
            row.try_get::<Option<i64>, _>(column).map(|value| {
                value
                    .map(|value| {
                        BackupValue::Decimal(
                            super::DecimalValue::new(definition.from_scaled_i64(value), definition)
                                .expect("scaled i64 is exact for its validated decimal definition"),
                        )
                    })
                    .unwrap_or(BackupValue::Null)
            })
        }
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
        // The record engine's own read: any RFC 3339 text, canonicalized. Text it cannot read
        // fails the export instead of entering a snapshot.
        BackupValueKind::DateTime => row.try_get::<Option<String>, _>(column).and_then(|value| {
            value
                .map(|value| {
                    super::RuntimeDateTime::parse(&value)
                        .map(|value| BackupValue::String(value.as_str().to_owned()))
                        .map_err(|error| sqlx::Error::Decode(Box::new(error)))
                })
                .unwrap_or(Ok(BackupValue::Null))
        }),
    }
}

#[cfg(feature = "postgres")]
fn decode_backup_value(
    row: &crate::DbRow,
    descriptor: &super::ColumnBackupDescriptor,
) -> crate::Result<BackupValue> {
    let column = descriptor.column_name.as_str();
    let kind = descriptor.logical_type;
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
        BackupValueKind::Decimal => {
            let definition = require_decimal_definition(descriptor)?;
            row.try_get::<Option<rust_decimal::Decimal>, _>(column)
                .and_then(|value| {
                    value
                        .map(|value| {
                            super::DecimalValue::new(value, definition)
                                .map(BackupValue::Decimal)
                                .map_err(|error| sqlx::Error::Decode(Box::new(error)))
                        })
                        .unwrap_or(Ok(BackupValue::Null))
                })
        }
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
        // A `TIMESTAMPTZ` is an instant; reading it typed is independent of the session
        // `TimeZone`. `check_portable_date_times` has already refused values chrono cannot hold.
        BackupValueKind::DateTime => row
            .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(column)
            .map(|value| {
                value
                    .map(|value| {
                        BackupValue::String(
                            super::RuntimeDateTime::from_utc(value).as_str().to_owned(),
                        )
                    })
                    .unwrap_or(BackupValue::Null)
            }),
    }
}

fn backup_value_to_sql_value(
    value: &BackupValue,
    descriptor: &super::ColumnBackupDescriptor,
) -> crate::Result<SqlValue> {
    let logical_type = descriptor.logical_type;
    Ok(match value {
        BackupValue::Null => match logical_type {
            BackupValueKind::Null | BackupValueKind::String | BackupValueKind::DateTime => {
                SqlValue::StringNull
            }
            BackupValueKind::Bool => SqlValue::BoolNull,
            BackupValueKind::Integer => SqlValue::IntNull,
            BackupValueKind::Float => SqlValue::FloatNull,
            BackupValueKind::Decimal => {
                SqlValue::DecimalNull(require_decimal_definition(descriptor)?)
            }
            BackupValueKind::Uuid => SqlValue::UuidNull,
            BackupValueKind::Json => SqlValue::JsonNull,
            BackupValueKind::Bytes => SqlValue::BytesNull,
        },
        BackupValue::Bool(value) => SqlValue::Bool(*value),
        BackupValue::Integer(value) => SqlValue::Int(*value),
        BackupValue::Float(value) => SqlValue::Float(*value),
        BackupValue::Decimal(value) => {
            let definition = require_decimal_definition(descriptor)?;
            if value.definition() != definition {
                return Err(sqlx::Error::Protocol(
                    "backup decimal definition does not match destination column".to_string(),
                ));
            }
            SqlValue::Decimal(*value)
        }
        BackupValue::String(value) => SqlValue::String(value.clone()),
        BackupValue::Uuid(value) => SqlValue::Uuid(*value),
        BackupValue::Json(value) => SqlValue::Json(value.clone()),
        BackupValue::Bytes(value) => SqlValue::Bytes(value.clone()),
    })
}

/// The bind placeholder of an import column. A PostgreSQL `TIMESTAMPTZ` takes the canonical text
/// through an explicit cast; its `Z` offset makes the cast independent of the session `TimeZone`.
fn import_placeholder(column: &super::ColumnBackupDescriptor) -> &'static str {
    if cfg!(feature = "postgres") && column.logical_type == BackupValueKind::DateTime {
        "CAST(? AS TIMESTAMPTZ)"
    } else {
        "?"
    }
}

/// Refuse a PostgreSQL date-time the portable encoding cannot carry before any row is decoded:
/// `±infinity` and instants outside years 0001–9999 have no RFC 3339 form (and overflow the
/// typed decode). Runs inside the export's consistent snapshot.
#[cfg(feature = "postgres")]
async fn check_portable_date_times(
    snapshot: &mut BackupSnapshot,
    entity: &EntityBackupDescriptor,
    columns: &[&super::ColumnBackupDescriptor],
) -> crate::Result<()> {
    let outside = columns
        .iter()
        .filter(|column| column.logical_type == BackupValueKind::DateTime)
        .map(|column| {
            let column = quote_identifier(&column.column_name);
            format!(
                "NOT ({column} >= TIMESTAMPTZ '0001-01-01 00:00:00+00' AND {column} < TIMESTAMPTZ '10000-01-01 00:00:00+00')"
            )
        })
        .collect::<Vec<_>>();
    if outside.is_empty() {
        return Ok(());
    }
    let sql = format!(
        "SELECT 1 FROM {} WHERE {} LIMIT 1",
        quote_identifier(&entity.table_name),
        outside.join(" OR ")
    );
    if fetch_snapshot_rows(snapshot, &sql).await?.is_empty() {
        Ok(())
    } else {
        Err(sqlx::Error::Protocol(format!(
            "backup table {} has a date-time outside the portable range 0001-9999",
            entity.table_name
        )))
    }
}

#[cfg(feature = "sqlite")]
async fn check_portable_date_times(
    _: &mut BackupSnapshot,
    _: &EntityBackupDescriptor,
    _: &[&super::ColumnBackupDescriptor],
) -> crate::Result<()> {
    // SQLite stores text; the typed decode parses and canonicalizes each value instead.
    Ok(())
}

fn require_decimal_definition(
    descriptor: &super::ColumnBackupDescriptor,
) -> crate::Result<super::DecimalDef> {
    descriptor.decimal.ok_or_else(|| {
        sqlx::Error::Protocol(format!(
            "decimal backup column {} is missing its storage definition",
            descriptor.column_name
        ))
    })
}

#[cfg(feature = "sqlite")]
fn decode_backup_change(row: crate::DbRow) -> crate::Result<BackupChange> {
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
fn decode_backup_change(row: crate::DbRow) -> crate::Result<BackupChange> {
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
) -> crate::Result<BackupChange> {
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
            BackupValue::Decimal(value) => value.value().to_string(),
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
        BackupValue::Decimal(value) => format!(
            "decimal:{}:{}:{}",
            value.definition().precision(),
            value.definition().scale(),
            value.value()
        ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::orm::{DeletePolicy, EntityDependencyDescriptor};

    #[test]
    fn composite_identities_are_canonical_and_single_keys_unchanged() {
        let composite = EntityBackupDescriptor {
            entity_name: "Pairs".into(),
            table_name: "pairs".into(),
            primary_key_column: "a".into(),
            primary_key_columns: vec!["a".into(), "b".into()],
            retention_purge: false,
            export_order: 0,
            restore_order: 0,
            columns: vec![],
            dependencies: vec![],
        };
        let values = |json: &str| {
            BTreeMap::from([
                ("a".to_string(), BackupValue::Integer(1)),
                (
                    "b".to_string(),
                    BackupValue::Json(serde_json::from_str(json).unwrap()),
                ),
            ])
        };
        let keys = ["a", "b"];
        // JSON parts are canonical, so equal documents give one identity.
        assert_eq!(
            row_identity(&composite, &keys, &values(r#"{"y":1,"x":2}"#)).unwrap(),
            row_identity(&composite, &keys, &values(r#"{"x":2,"y":1}"#)).unwrap()
        );
        let mut null = values("{}");
        null.insert("b".into(), BackupValue::Null);
        assert!(row_identity(&composite, &keys, &null).is_err());
        // A single-column key keeps its identity text, including null and JSON as before.
        let single = EntityBackupDescriptor {
            primary_key_columns: vec![],
            ..composite
        };
        let raw = serde_json::json!({"y": 1, "x": 2});
        let value = BTreeMap::from([("a".to_string(), BackupValue::Json(raw.clone()))]);
        assert_eq!(
            row_identity(&single, &["a"], &value).unwrap(),
            raw.to_string()
        );
    }

    #[test]
    fn decimal_backup_values_validate_definition_and_wire_shape() {
        let definition = super::super::DecimalDef::new(12, 2).expect("valid decimal definition");
        let value = super::super::DecimalValue::new(
            "1234.50".parse().expect("valid decimal literal"),
            definition,
        )
        .expect("decimal should fit definition");
        let backup = BackupValue::Decimal(value);
        let encoded = serde_json::to_value(&backup).expect("decimal backup should serialize");
        let decoded: BackupValue =
            serde_json::from_value(encoded.clone()).expect("decimal backup should deserialize");
        assert_eq!(decoded, backup);

        let mut invalid_definition = encoded;
        invalid_definition["Decimal"]["definition"]["scale"] = serde_json::json!(13);
        assert!(serde_json::from_value::<BackupValue>(invalid_definition).is_err());

        let descriptor = super::super::ColumnBackupDescriptor {
            column_name: "amount".to_string(),
            rust_field_name: "amount".to_string(),
            logical_type: BackupValueKind::Decimal,
            decimal: Some(definition),
            nullable: false,
            is_primary_key: false,
            is_generated: false,
            backup_policy: ColumnBackupPolicy::Include,
        };
        validate_backup_value(&descriptor, &backup).expect("matching definition should validate");

        let different = super::super::DecimalDef::new(10, 2).expect("valid decimal definition");
        let mismatched = BackupValue::Decimal(
            super::super::DecimalValue::new(value.value(), different)
                .expect("value fits narrower definition"),
        );
        assert!(validate_backup_value(&descriptor, &mismatched).is_err());
        assert!(
            validate_backup_value(&descriptor, &BackupValue::String("1234.50".into())).is_err()
        );
    }

    #[test]
    fn self_referential_rows_are_ordered_parent_first() {
        let parent_id = uuid::Uuid::new_v4();
        let child_id = uuid::Uuid::new_v4();
        let row = |id, parent_id: Option<uuid::Uuid>| {
            let mut values = BTreeMap::new();
            values.insert("id".to_string(), BackupValue::Uuid(id));
            values.insert(
                "parent_id".to_string(),
                parent_id
                    .map(BackupValue::Uuid)
                    .unwrap_or(BackupValue::Null),
            );
            BackupRow {
                table_name: "items".to_string(),
                primary_key: id.to_string(),
                row_hash: String::new(),
                values,
            }
        };
        let rows = vec![row(child_id, Some(parent_id)), row(parent_id, None)];
        let entity = EntityBackupDescriptor {
            entity_name: "Item".to_string(),
            table_name: "items".to_string(),
            primary_key_column: "id".to_string(),
            primary_key_columns: Vec::new(),
            retention_purge: false,
            export_order: 0,
            restore_order: 0,
            columns: Vec::new(),
            dependencies: vec![EntityDependencyDescriptor {
                entity_name: "Item".to_string(),
                table_name: "items".to_string(),
                source_column: "parent_id".to_string(),
                target_column: "id".to_string(),
                nullable: true,
                on_delete: DeletePolicy::Restrict,
            }],
        };

        let order = import_row_order(&entity, &rows).expect("rows should order");

        assert!(order.deferred.is_empty());
        assert_eq!(order.ordered[0].primary_key, parent_id.to_string());
        assert_eq!(order.ordered[1].primary_key, child_id.to_string());
    }

    #[test]
    fn self_reference_cycles_and_the_rows_below_them_are_deferred() {
        let row = |id: i64, parent: Option<i64>| BackupRow {
            table_name: "items".to_string(),
            primary_key: id.to_string(),
            row_hash: String::new(),
            values: BTreeMap::from([
                ("id".to_string(), BackupValue::Integer(id)),
                (
                    "parent_id".to_string(),
                    parent.map_or(BackupValue::Null, BackupValue::Integer),
                ),
            ]),
        };
        // 1 ⇄ 2 is a cycle, 3 hangs below it, 4 is a root, 5 a self-loop below 4.
        let rows = vec![
            row(1, Some(2)),
            row(2, Some(1)),
            row(3, Some(1)),
            row(4, None),
            row(5, Some(5)),
        ];
        let entity = EntityBackupDescriptor {
            entity_name: "Item".to_string(),
            table_name: "items".to_string(),
            primary_key_column: "id".to_string(),
            primary_key_columns: Vec::new(),
            retention_purge: false,
            export_order: 0,
            restore_order: 0,
            columns: Vec::new(),
            dependencies: vec![EntityDependencyDescriptor {
                entity_name: "Item".to_string(),
                table_name: "items".to_string(),
                source_column: "parent_id".to_string(),
                target_column: "id".to_string(),
                nullable: true,
                on_delete: DeletePolicy::Restrict,
            }],
        };

        let order = import_row_order(&entity, &rows).expect("cycles defer instead of failing");
        let keys = |rows: &[&BackupRow]| {
            rows.iter()
                .map(|row| row.primary_key.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(&order.ordered), ["4", "5"]);
        assert_eq!(keys(&order.deferred), ["1", "2", "3"]);
        assert_eq!(order.self_columns, ["parent_id"]);
    }

    #[test]
    fn date_time_values_must_be_canonical_utc_text() {
        let descriptor = super::super::ColumnBackupDescriptor {
            column_name: "created_at".to_string(),
            rust_field_name: "created_at".to_string(),
            logical_type: BackupValueKind::DateTime,
            decimal: None,
            nullable: true,
            is_primary_key: false,
            is_generated: false,
            backup_policy: ColumnBackupPolicy::Include,
        };
        let text = |value: &str| BackupValue::String(value.to_string());
        assert!(validate_backup_value(&descriptor, &text("2026-10-27T01:02:03.000004Z")).is_ok());
        assert!(validate_backup_value(&descriptor, &BackupValue::Null).is_ok());
        for refused in [
            text("2026-10-27T01:02:03Z"),
            text("2026-10-27T11:02:03.000004+10:00"),
            text("2026-10-27 01:02:03.000004"),
            text("2026-10-27T01:02:03.0000041Z"),
            BackupValue::Integer(1),
        ] {
            assert!(
                validate_backup_value(&descriptor, &refused).is_err(),
                "{refused:?}"
            );
        }
        assert_eq!(BackupValueKind::DateTime.as_schema_str(), "date_time");
        assert!(matches!(
            backup_value_to_sql_value(&BackupValue::Null, &descriptor),
            Ok(SqlValue::StringNull)
        ));
    }
}
