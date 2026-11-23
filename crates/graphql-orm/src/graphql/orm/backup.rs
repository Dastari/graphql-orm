use super::{
    BackupValueKind, ColumnBackupPolicy, DatabaseBackend, EntityBackupDescriptor, EntityMetadata,
    GraphqlOrmSchemaSnapshot, MutationEvent, MutationPhase, SqlValue,
    backup_descriptors_from_entities, current_backend, execute_with_binds_on,
    schema_snapshot_from_entities,
};
use crate::DbPool;
#[cfg(not(feature = "postgres"))]
use crate::DefaultBackend;
#[cfg(feature = "postgres")]
use crate::PostgresBackend;
#[cfg(feature = "sqlite")]
use crate::SqliteBackend;
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
    /// Position of this entry in the journal. Entries are exported in
    /// ascending order; pass the last one's value as
    /// [`ChangeWindow::after_sequence`] to continue from it.
    ///
    /// An entry serialized by a release before 0.157.0 has no position and
    /// deserializes with `0`, which is never a journal position.
    #[serde(default)]
    pub sequence: i64,
    /// Random identifier of the entry. It does not order entries.
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

/// The part of the change journal one export reads.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangeWindow {
    /// Export only entries after this journal position: the
    /// [`BackupChange::sequence`] of the last entry the previous export
    /// returned. `None` starts from the beginning of the journal.
    pub after_sequence: Option<i64>,
    /// Export only entries recorded at or before this Unix time, in seconds.
    /// The export stops at the first entry recorded later, so it never skips
    /// an entry that a later export would have to return.
    pub until: i64,
}

/// How long [`Database::export_changes`](crate::db::Database::export_changes)
/// waits for journaled write transactions that were open when it started.
pub const CHANGE_EXPORT_DEFAULT_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// A change journal export gave up waiting for open journaled transactions.
///
/// Nothing was exported, so no entry was skipped: retry with the same window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeExportTimedOut {
    /// How long the export waited.
    pub waited: std::time::Duration,
}

impl std::fmt::Display for ChangeExportTimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "change journal export timed out after {:?} waiting for journaled write transactions that were open when it started",
            self.waited
        )
    }
}

impl std::error::Error for ChangeExportTimedOut {}

/// The change journal could not be given its sequence because a database
/// object already has the name the upgrade needs for the rebuilt journal.
///
/// Nothing was changed: the journal, the conflicting object and their rows are
/// as they were. Rename or remove the conflicting object, then prepare the
/// journal again. Read it from the error with
/// [`ChangeJournalUpgradeConflict::from_error`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeJournalUpgradeConflict {
    /// Name of the existing database object.
    pub name: String,
}

impl std::fmt::Display for ChangeJournalUpgradeConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the change journal upgrade needs the name {} but a database object already has it; nothing was changed",
            self.name
        )
    }
}

impl std::error::Error for ChangeJournalUpgradeConflict {}

impl ChangeJournalUpgradeConflict {
    /// The conflict carried by `error`, if that is what it reports.
    pub fn from_error(error: &crate::Error) -> Option<&Self> {
        match error {
            sqlx::Error::Decode(source) => source.downcast_ref::<Self>(),
            _ => None,
        }
    }
}

impl ChangeExportTimedOut {
    /// The timeout carried by `error`, if that is what it reports.
    pub fn from_error(error: &crate::Error) -> Option<&Self> {
        match error {
            sqlx::Error::Decode(source) => source.downcast_ref::<Self>(),
            _ => None,
        }
    }
}

#[cfg(feature = "sqlite")]
pub struct BackupSnapshot {
    tx: sqlx::Transaction<'static, sqlx::Sqlite>,
}

#[cfg(feature = "postgres")]
pub struct BackupSnapshot {
    tx: sqlx::Transaction<'static, sqlx::Postgres>,
    canonical_key_start: Option<i64>,
}

impl BackupSnapshot {
    /// The sequence reserved for canonical key cutover, if one is visible
    /// in this snapshot. It is a lower bound, not the snapshot's checkpoint.
    pub fn canonical_change_journal_start_sequence(&self) -> Option<i64> {
        #[cfg(feature = "postgres")]
        {
            self.canonical_key_start
        }
        #[cfg(not(feature = "postgres"))]
        None
    }

    // Returns ordered storage metadata for the snapshot's single-column
    // keys. The metadata
    // excludes relation OIDs, which can differ on a matching restore target.
    #[cfg(feature = "postgres")]
    async fn canonical_key_storage_identity(
        &mut self,
        entities: &[EntityBackupDescriptor],
    ) -> crate::Result<Option<String>> {
        if self.canonical_key_start.is_none() {
            return Ok(None);
        }
        let mut keys = Vec::new();
        for entity in entities
            .iter()
            .filter(|entity| entity.primary_key_columns.is_empty())
        {
            let path = quote_table_identifier(&entity.table_name);
            let storage =
                super::journal_key::key_type(&mut self.tx, &path, &entity.primary_key_column)
                    .await?;
            keys.push((
                entity.table_name.clone(),
                entity.primary_key_column.clone(),
                storage,
            ));
        }
        keys.sort_unstable();
        serde_json::to_string(&keys)
            .map(Some)
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))
    }
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
        super::ensure_sql_write_allowed()?;
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

    /// Create the change journal table if it is missing and secure it.
    ///
    /// On PostgreSQL this enables RLS on the journal and, when the caller owns
    /// it, drops the `INSERT` policy that earlier releases created. It does
    /// not install the database-side recorder, without which PostgreSQL
    /// journaled writes fail: apply the schema target, or call
    /// [`Self::ensure_change_journal_recorder`] as the schema owner.
    pub async fn ensure_change_journal_table(&self) -> crate::Result<()> {
        super::ensure_sql_write_allowed()?;
        if !cfg!(feature = "change-journal") {
            return Err(sqlx::Error::Protocol(
                "change journal support requires the graphql-orm change-journal feature"
                    .to_string(),
            ));
        }
        ensure_change_journal_table(self.pool()).await
    }

    /// Create the change journal and, on PostgreSQL, the database-side
    /// recorder for `entities`.
    ///
    /// On PostgreSQL the database, not the runtime role, records journal
    /// entries: an `AFTER INSERT OR UPDATE OR DELETE` row trigger on each
    /// single-key entity table calls the `SECURITY DEFINER` function
    /// `graphql_orm.record_change_journal_entry()`, which runs as its owner
    /// with `search_path = pg_catalog, pg_temp`. The journal has RLS enabled
    /// and no policies; every privilege beyond `SELECT` other roles hold on
    /// it, and `EXECUTE` on the recorder, is revoked. Run this, or apply the
    /// schema target, as the
    /// role that owns the entity tables and the journal; never as the runtime
    /// role.
    ///
    /// On SQLite this is [`Self::ensure_change_journal_table`].
    pub async fn ensure_change_journal_recorder(
        &self,
        entities: &[&'static EntityMetadata],
    ) -> crate::Result<()> {
        super::ensure_sql_write_allowed()?;
        if !cfg!(feature = "change-journal") {
            return Err(sqlx::Error::Protocol(
                "change journal support requires the graphql-orm change-journal feature"
                    .to_string(),
            ));
        }
        #[cfg(feature = "postgres")]
        {
            let schema = super::SchemaModel::from_entities(entities);
            super::rls::refuse_change_journal_cascade_gaps(&schema)?;
            let tables = super::rls::change_journal_tables(&schema);
            let mut tx = self.pool().begin().await?;
            for statement in super::rls::postgres_change_journal_recorder_sql(&tables) {
                sqlx::query(&statement).execute(&mut *tx).await?;
            }
            tx.commit().await?;
            Ok(())
        }
        #[cfg(not(feature = "postgres"))]
        {
            let _ = entities;
            ensure_change_journal_table(self.pool()).await
        }
    }

    /// Switch the PostgreSQL journal resolved by this connection to canonical keys.
    ///
    /// First install the recorder as the owner, and stop writers, schema
    /// changes, and backup workers. Take a new full snapshot before resuming
    /// writes. Use the returned sequence as that snapshot's initial checkpoint
    /// while writers remain stopped. Legacy entries stay in the journal.
    /// Retrying a completed cutover returns the original sequence.
    ///
    /// # Errors
    ///
    /// Fails if the change-journal feature, managed journal, owner privileges,
    /// or a supported key column type is missing.
    #[cfg(feature = "postgres")]
    pub async fn enable_canonical_change_journal_keys(&self) -> crate::Result<i64> {
        super::ensure_sql_write_allowed()?;
        if !cfg!(feature = "change-journal") {
            return Err(sqlx::Error::Protocol(
                "canonical change journal keys require the graphql-orm change-journal feature"
                    .into(),
            ));
        }
        super::journal_key::enable(self.pool()).await
    }

    /// Read the PostgreSQL canonical key cutover sequence, if installed.
    ///
    /// # Errors
    ///
    /// Fails if the database cannot be read or the canonical recorder is altered.
    pub async fn canonical_change_journal_start_sequence(&self) -> crate::Result<Option<i64>> {
        #[cfg(feature = "postgres")]
        {
            let tx = begin_consistent_snapshot(self.pool()).await?;
            let start = tx.canonical_change_journal_start_sequence();
            tx.tx.commit().await?;
            Ok(start)
        }
        #[cfg(not(feature = "postgres"))]
        Ok(None)
    }

    /// Read the live primary-key storage identity for a canonical chain.
    ///
    /// Bind this opaque identity to the backup schema hash. Returns `None`
    /// for a legacy journal or on SQLite.
    ///
    /// # Errors
    ///
    /// Fails if the canonical recorder or any live key cannot be validated.
    pub async fn canonical_key_storage_identity(
        &self,
        entities: &[EntityBackupDescriptor],
    ) -> crate::Result<Option<String>> {
        #[cfg(feature = "postgres")]
        {
            let mut snapshot = begin_consistent_snapshot(self.pool()).await?;
            let identity = snapshot.canonical_key_storage_identity(entities).await?;
            snapshot.tx.commit().await?;
            Ok(identity)
        }
        #[cfg(not(feature = "postgres"))]
        {
            let _ = entities;
            Ok(None)
        }
    }

    /// Export the change journal entries in `window`, in journal order.
    ///
    /// Each entry carries its journal position in [`BackupChange::sequence`].
    /// A canonical PostgreSQL journal requires an explicit cursor at or
    /// after the cutover sequence. Start from a fresh full backup and its
    /// checkpoint.
    /// To export incrementally, pass the last returned entry's position as
    /// the next window's [`ChangeWindow::after_sequence`]. Consecutive exports
    /// then return every entry exactly once: positions are assigned by the
    /// database in increasing order, and an export returns a contiguous run of
    /// them.
    ///
    /// On PostgreSQL a position is assigned when an entry is written, which
    /// can be before its transaction commits. So that no entry appears later
    /// behind a position already exported, the export waits for the journaled
    /// write transactions that were open when it started, for up to
    /// [`CHANGE_EXPORT_DEFAULT_WAIT`]; see
    /// [`Self::export_changes_with_wait`]. SQLite has one writer at a time and
    /// never waits.
    ///
    /// # Errors
    ///
    /// Fails when the journal has no sequence column (a journal created by a
    /// release before 0.157.0: run [`Self::ensure_change_journal_table`], or
    /// on PostgreSQL apply the schema target, as the journal's owner), when a
    /// PostgreSQL role subject to row security reads the journal, when the
    /// PostgreSQL server is a standby (it cannot see the primary's open
    /// transactions), and with [`ChangeExportTimedOut`] when open
    /// transactions outlast the wait.
    pub async fn export_changes(&self, window: ChangeWindow) -> crate::Result<Vec<BackupChange>> {
        self.export_changes_with_wait(window, CHANGE_EXPORT_DEFAULT_WAIT)
            .await
    }

    /// [`Self::export_changes`] with an explicit limit on how long to wait
    /// for open journaled write transactions on PostgreSQL.
    pub async fn export_changes_with_wait(
        &self,
        window: ChangeWindow,
        max_wait: std::time::Duration,
    ) -> crate::Result<Vec<BackupChange>> {
        if !cfg!(feature = "change-journal") {
            return Err(sqlx::Error::Protocol(
                "incremental backup export requires the graphql-orm change-journal feature"
                    .to_string(),
            ));
        }
        export_changes(self.pool(), window, max_wait).await
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
    super::ensure_sql_write_allowed()?;
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

    // Preflight every table before starting any import transaction. Per-table validation alone
    // would let earlier tables commit before a later row's invalid value or shape is discovered.
    for entity in &current_snapshot.entities {
        let rows = rows_by_table
            .get(&entity.table_name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        validate_import_rows(entity, rows)?;
    }
    // These checks also precede every write.
    validate_backup_references(&current_snapshot.entities, rows_by_table)?;

    let mut entities = current_snapshot.entities.clone();
    entities.sort_by_key(|entity| (entity.restore_order, entity.table_name.clone()));
    let empty_rows = Vec::new();
    let mut reports = Vec::new();
    if context.mode != RestoreMode::EmptyDatabase {
        // `DryRun` validates and `ReplaceExisting` is refused; neither writes.
        for entity in &entities {
            let rows = rows_by_table.get(&entity.table_name).unwrap_or(&empty_rows);
            reports.push(database.import_table_rows(entity, rows, context).await?);
        }
        return Ok(reports);
    }

    // One transaction for the whole snapshot: every table's rows, their
    // row-count and row-hash checks, and the runtime search reset commit
    // together. Any error drops the transaction, which rolls all of it back,
    // so a failed restore leaves the target as it was.
    let mut restore = begin_restore_transaction(database.pool()).await?;
    for entity in &entities {
        let count = table_row_count_on(&mut restore.tx, &entity.table_name).await?;
        if count != 0 {
            return Err(sqlx::Error::Protocol(format!(
                "cannot restore table {} in EmptyDatabase mode because it already contains {} rows",
                entity.table_name, count
            )));
        }
    }
    for entity in &entities {
        let rows = rows_by_table.get(&entity.table_name).unwrap_or(&empty_rows);
        reports.push(import_table_rows_on(&mut restore, entity, rows).await?);
    }
    // The restored rows replace whatever a runtime search index was built
    // from. Clear each restored table's readiness-tracked index and leave it
    // `building`, so search is refused until a backfill rebuilds it.
    for entity in &entities {
        reset_restored_runtime_search(&mut restore, &entity.table_name).await?;
    }
    restore.tx.commit().await?;
    Ok(reports)
}

/// Clear the readiness-tracked (runtime) search index of `table_name`, if it
/// has one, and mark it `building`. Static search indexes do not track
/// readiness and are left alone.
async fn reset_restored_runtime_search(
    restore: &mut BackupSnapshot,
    table_name: &str,
) -> crate::Result<()> {
    let backend = current_backend();
    let metadata_table = super::search::search_metadata_table_name();
    // Probe the catalog first: a failed statement would abort the PostgreSQL
    // transaction, and most databases have no search metadata at all.
    let exists_sql = match backend {
        DatabaseBackend::Postgres => "SELECT pg_catalog.to_regclass($1) IS NOT NULL",
        _ => "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
    };
    let exists: bool = sqlx::query_scalar(exists_sql)
        .bind(metadata_table)
        .fetch_one(&mut *restore.tx)
        .await?;
    if !exists {
        return Ok(());
    }
    let select_sql = match backend {
        DatabaseBackend::Postgres => format!(
            "SELECT config_json::text FROM {metadata_table} WHERE table_name = $1 FOR UPDATE"
        ),
        _ => format!("SELECT config_json FROM {metadata_table} WHERE table_name = ?1"),
    };
    let stored: Option<String> = sqlx::query_scalar(&select_sql)
        .bind(table_name)
        .fetch_optional(&mut *restore.tx)
        .await?;
    let Some(mut config) =
        stored.and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    else {
        return Ok(());
    };
    if !super::runtime_search::mark_building(&mut config) {
        return Ok(());
    }
    let documents = match backend {
        DatabaseBackend::Postgres => super::search::search_table_name(table_name),
        _ => super::search::sqlite_fts_table_name(table_name),
    };
    sqlx::query(&format!("DELETE FROM {documents}"))
        .execute(&mut *restore.tx)
        .await?;
    let update_sql = match backend {
        DatabaseBackend::Postgres => {
            format!("UPDATE {metadata_table} SET config_json = $1::jsonb WHERE table_name = $2")
        }
        _ => format!("UPDATE {metadata_table} SET config_json = ?1 WHERE table_name = ?2"),
    };
    sqlx::query(&update_sql)
        .bind(config.to_string())
        .bind(table_name)
        .execute(&mut *restore.tx)
        .await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn database_identity(pool: &DbPool) -> crate::Result<DatabaseIdentity> {
    let row = sqlx::query("SELECT file FROM pragma_database_list WHERE name = 'main'")
        .fetch_one(pool)
        .await?;
    Ok(DatabaseIdentity {
        name: row
            .try_get::<Option<String>, _>("file")?
            .unwrap_or_default(),
        object_id: None,
    })
}

#[cfg(feature = "postgres")]
async fn database_identity(pool: &DbPool) -> crate::Result<DatabaseIdentity> {
    let row = sqlx::query(
        "SELECT d.datname::pg_catalog.text AS name, d.oid::pg_catalog.int8 AS object_id FROM pg_catalog.pg_database d WHERE d.datname OPERATOR(pg_catalog.=) pg_catalog.current_database()",
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
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await?;
    let canonical_key_start = super::journal_key::epoch(&mut tx).await?;
    Ok(BackupSnapshot {
        tx,
        canonical_key_start,
    })
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

/// Columns of the change journal other than its sequence.
const CHANGE_JOURNAL_ENTRY_COLUMNS: &str = "id, entity_name, table_name, primary_key, action, changed_at, transaction_id, row_hash, actor_id, correlation_id";

/// Name of the journal column that orders entries.
const CHANGE_JOURNAL_SEQUENCE_COLUMN: &str = "seq";

#[cfg(feature = "sqlite")]
fn sqlite_change_journal_table_sql(table: &str) -> String {
    // `AUTOINCREMENT` never reuses a position, even after the newest entries
    // are deleted, so a stored position cannot point past new entries.
    format!(
        "CREATE TABLE IF NOT EXISTS {} (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
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
        quote_identifier(table)
    )
}

/// Create the SQLite journal, or give one created by an earlier release its
/// sequence column.
///
/// SQLite cannot add an `AUTOINCREMENT` key to an existing table, so the
/// journal is rebuilt in one immediate transaction: existing entries are
/// copied in `changed_at` and then insertion (`rowid`) order. The transaction
/// guard rolls the rebuild back when a statement or the commit fails, and a
/// cancelled call closes its connection, which also rolls back, so the pool
/// never hands out a connection that is still inside the upgrade.
///
/// The rebuild needs the name `__graphql_orm_change_log_sequenced` for a
/// moment. Anything that already has that name was not created by this
/// rebuild, which never commits it, so the upgrade refuses with
/// [`ChangeJournalUpgradeConflict`] before it changes anything rather than
/// drop an object it does not own.
#[cfg(feature = "sqlite")]
async fn ensure_change_journal_table(pool: &DbPool) -> crate::Result<()> {
    // A caller that drops this future while `BEGIN IMMEDIATE` is in flight
    // leaves SQLx no transaction guard to roll back, and the connection would
    // return to the pool inside that transaction. Until the upgrade has run
    // to an end, the lease is marked to be closed instead, which rolls back.
    struct Lease {
        connection: sqlx::pool::PoolConnection<sqlx::Sqlite>,
        finished: bool,
    }
    impl Drop for Lease {
        fn drop(&mut self) {
            if !self.finished {
                self.connection.close_on_drop();
            }
        }
    }
    let mut lease = Lease {
        connection: pool.acquire().await?,
        finished: false,
    };
    let result = upgrade_sqlite_change_journal(&mut lease.connection).await;
    // Reached only when the upgrade was not cancelled: it committed, or its
    // transaction guard has rolled it back.
    lease.finished = true;
    result
}

#[cfg(feature = "sqlite")]
async fn upgrade_sqlite_change_journal(
    connection: &mut sqlx::SqliteConnection,
) -> crate::Result<()> {
    use sqlx::Connection;
    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
    let table = quote_identifier(CHANGE_JOURNAL_TABLE);
    let columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info(?, 'main')")
        .bind(CHANGE_JOURNAL_TABLE)
        .fetch_all(&mut *tx)
        .await?;
    if columns.is_empty() {
        sqlx::query(&sqlite_change_journal_table_sql(CHANGE_JOURNAL_TABLE))
            .execute(&mut *tx)
            .await?;
    } else if !columns
        .iter()
        .any(|column| column == CHANGE_JOURNAL_SEQUENCE_COLUMN)
    {
        let upgraded_name = format!("{CHANGE_JOURNAL_TABLE}_sequenced");
        let taken: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM main.sqlite_master WHERE name = ?1 COLLATE NOCASE) + (SELECT COUNT(*) FROM temp.sqlite_master WHERE name = ?1 COLLATE NOCASE)",
        )
        .bind(&upgraded_name)
        .fetch_one(&mut *tx)
        .await?;
        if taken != 0 {
            return Err(sqlx::Error::Decode(Box::new(
                ChangeJournalUpgradeConflict {
                    name: upgraded_name,
                },
            )));
        }
        let upgraded = quote_identifier(&upgraded_name);
        sqlx::query(&sqlite_change_journal_table_sql(&upgraded_name))
            .execute(&mut *tx)
            .await?;
        sqlx::query(&format!(
            "INSERT INTO {upgraded} ({CHANGE_JOURNAL_ENTRY_COLUMNS}) SELECT {CHANGE_JOURNAL_ENTRY_COLUMNS} FROM {table} ORDER BY changed_at, rowid"
        ))
        .execute(&mut *tx)
        .await?;
        sqlx::query(&format!("DROP TABLE {table}"))
            .execute(&mut *tx)
            .await?;
        sqlx::query(&format!("ALTER TABLE {upgraded} RENAME TO {table}"))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

#[cfg(feature = "postgres")]
async fn ensure_change_journal_table(pool: &DbPool) -> crate::Result<()> {
    sqlx::query(&super::rls::postgres_change_journal_table_sql())
        .execute(pool)
        .await?;
    // A journal from an earlier release gets its sequence column when the
    // caller owns it; otherwise the owner adds it by applying the target.
    sqlx::query(&super::rls::postgres_change_journal_sequence_sql())
        .execute(pool)
        .await?;
    // The journal names every tenant's keys and drives restore deletes. Only
    // privileged roles read or write it; the database-side recorder appends.
    sqlx::query(&super::rls::postgres_ensure_change_log_rls_sql())
        .execute(pool)
        .await?;
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
    if B::DIALECT == super::DatabaseBackend::Postgres {
        return verify_postgres_change_journal_recorder(hook_ctx, event).await;
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
        SqlValue::StringNull,
        SqlValue::StringNull,
        SqlValue::StringNull,
        SqlValue::StringNull,
    ];
    hook_ctx.execute(&sql, &values).await?;
    Ok(())
}

/// Turn on database-side journal recording for an ORM write transaction.
///
/// On PostgreSQL the recorder trigger records changes only in transactions
/// that set [`super::rls::CHANGE_JOURNAL_SETTING`]. The setting selects
/// transactions; it cannot name the entity, key or action of an entry. Restore
/// imports never set it, so they do not journal.
pub(crate) async fn begin_change_journal_transaction<B>(
    db: &crate::db::Database<B>,
    tx: &mut B::Transaction<'_>,
) -> crate::Result<()>
where
    B: super::WriteBackend,
{
    if !cfg!(feature = "change-journal")
        || B::DIALECT != super::DatabaseBackend::Postgres
        || current_backend() != super::DatabaseBackend::Postgres
        || !db.change_journal_enabled()
    {
        return Ok(());
    }
    // A utility statement: unlike `SELECT set_config(...)` it takes no MVCC
    // snapshot, so callers that must lock before their first snapshot (such
    // as dependent deletes) are unaffected.
    B::execute_in_transaction(
        tx,
        &format!("SET LOCAL {} = 'on'", super::rls::CHANGE_JOURNAL_SETTING),
        &[],
    )
    .await?;
    Ok(())
}

impl<B> super::MutationContext<'_, B>
where
    B: super::WriteBackend,
{
    /// Check that PostgreSQL's change journal recorder will record writes to
    /// `metadata`'s table in this transaction.
    ///
    /// Journaled ORM writes run this check once per table before they rely on
    /// the recorder. Hosts can call it to fail fast at startup. It returns the
    /// specific reason on failure. On SQLite it always succeeds.
    ///
    /// A current role that acts as the journal owner (is a member of the
    /// owning role, as a superuser always is) passes the privilege part of the
    /// check, because it can already do anything the checked privileges would
    /// allow. `validate_target` reports such a role; run it with the runtime
    /// pool.
    #[doc(hidden)]
    pub async fn check_change_journal_recorder(
        &mut self,
        metadata: &'static EntityMetadata,
    ) -> crate::Result<()> {
        if B::DIALECT != super::DatabaseBackend::Postgres {
            return Ok(());
        }
        check_postgres_change_journal_recorder(self, metadata).await
    }
}

/// Fail closed unless the PostgreSQL recorder records `event`'s table in this
/// transaction. The check runs once per table per transaction.
async fn verify_postgres_change_journal_recorder<B>(
    hook_ctx: &mut super::MutationContext<'_, B>,
    event: &MutationEvent,
) -> crate::Result<()>
where
    B: super::WriteBackend,
{
    if hook_ctx
        .change_journal_verified_tables
        .contains(&event.table_name)
    {
        return Ok(());
    }
    check_postgres_change_journal_recorder(hook_ctx, event.metadata).await?;
    hook_ctx
        .change_journal_verified_tables
        .push(event.table_name);
    Ok(())
}

/// Privilege condition shared by the recorder check and its diagnosis: the
/// current role either acts as the journal owner (is a member of it), or
/// holds no table or column privilege on the journal beyond `SELECT` and no
/// `EXECUTE` on the recorder, directly, through `PUBLIC`, or through any role
/// it is a member of, including `NOINHERIT` memberships it can `SET ROLE` to.
/// A role with `TRIGGER` on the journal could run code as its owner, one with
/// `INSERT` or `UPDATE` could write entries if it bypasses RLS, and one with
/// `EXECUTE` on the recorder could attach it to other triggers.
const RECORDER_PRIVILEGES_CLEAN: &str = "(pg_catalog.pg_has_role(CURRENT_USER, j.relowner, 'MEMBER')
          OR (NOT EXISTS (
                  SELECT 1
                  FROM (SELECT j.relacl AS acl
                        UNION ALL
                        SELECT att.attacl FROM pg_catalog.pg_attribute att
                        WHERE att.attrelid OPERATOR(pg_catalog.=) j.oid AND att.attnum OPERATOR(pg_catalog.>) 0 AND NOT att.attisdropped) acls
                  CROSS JOIN LATERAL pg_catalog.aclexplode(acls.acl) acl
                  WHERE acl.privilege_type OPERATOR(pg_catalog.<>) 'SELECT' AND acl.grantee OPERATOR(pg_catalog.<>) j.relowner
                    AND CASE WHEN acl.grantee OPERATOR(pg_catalog.=) 0::pg_catalog.oid THEN true
                             ELSE pg_catalog.pg_has_role(CURRENT_USER, acl.grantee, 'MEMBER') END)
              AND NOT EXISTS (
                  SELECT 1
                  FROM pg_catalog.aclexplode(COALESCE(p.proacl, pg_catalog.acldefault('f', p.proowner))) acl
                  WHERE acl.privilege_type OPERATOR(pg_catalog.=) 'EXECUTE' AND acl.grantee OPERATOR(pg_catalog.<>) p.proowner
                    AND CASE WHEN acl.grantee OPERATOR(pg_catalog.=) 0::pg_catalog.oid THEN true
                             ELSE pg_catalog.pg_has_role(CURRENT_USER, acl.grantee, 'MEMBER') END)))";

/// The registry is owned by the recorder's owner and grants no role other
/// than its owner anything beyond `SELECT`, at table or column level.
const REGISTRY_INTACT: &str = "EXISTS (
              SELECT 1 FROM pg_catalog.pg_class g
              WHERE g.oid OPERATOR(pg_catalog.=) pg_catalog.to_regclass('graphql_orm.change_journal_tables')
                AND g.relowner OPERATOR(pg_catalog.=) p.proowner
                AND NOT EXISTS (
                    SELECT 1
                    FROM (SELECT g.relacl AS acl
                          UNION ALL
                          SELECT att.attacl FROM pg_catalog.pg_attribute att
                          WHERE att.attrelid OPERATOR(pg_catalog.=) g.oid AND att.attnum OPERATOR(pg_catalog.>) 0 AND NOT att.attisdropped) acls
                    CROSS JOIN LATERAL pg_catalog.aclexplode(acls.acl) acl
                    WHERE acl.grantee OPERATOR(pg_catalog.<>) g.relowner AND acl.privilege_type OPERATOR(pg_catalog.<>) 'SELECT'))";

/// The recorder check for one table.
///
/// It requires the recording setting, a `session_replication_role` other
/// than `replica`, and an intact recorder trigger on the table: enabled,
/// unconditional, of the exact type, the only trigger on the table calling the
/// unmodified recorder, which the table owner owns, with arguments naming
/// this table's schema and name, entity, declared table and key, and an
/// existing journal owned by that owner that carries no trigger or rule. The
/// current role must not hold privileges that would let it run code as the
/// journal owner (see [`RECORDER_PRIVILEGES_CLEAN`]).
async fn check_postgres_change_journal_recorder<B>(
    hook_ctx: &mut super::MutationContext<'_, B>,
    metadata: &'static EntityMetadata,
) -> crate::Result<()>
where
    B: super::WriteBackend,
{
    // The main check names the registry directly. Probe the schema and the
    // registry first, each in its own statement, so a deployment that has
    // not applied the plan yet, or a role without `USAGE` on the schema,
    // gets a guided error rather than a raw lookup failure.
    if let Some(message) = probe_postgres_change_journal_registry(hook_ctx).await? {
        return Err(sqlx::Error::Protocol(format!(
            "{message}; writes to {} would not be journaled",
            metadata.table_name
        )));
    }
    let key = metadata
        .primary_keys
        .first()
        .copied()
        .unwrap_or(metadata.primary_key);
    let sql_type = super::SchemaModel::from_entities(&[metadata])
        .tables
        .into_iter()
        .next()
        .and_then(|table| {
            table
                .columns
                .into_iter()
                .find(|column| column.name == key)
                .map(|column| column.sql_type)
        })
        .unwrap_or_default();
    let mut expected_arguments = Vec::new();
    for argument in [
        metadata.entity_name,
        metadata.table_name,
        key,
        super::rls::change_journal_key_encoding(&sql_type),
    ] {
        expected_arguments.extend_from_slice(argument.as_bytes());
        expected_arguments.push(0);
    }
    // The table the ORM's own writes reach: the declared path rendered as the
    // ORM renders it. A bare name must resolve in the journal's schema.
    let table = super::rls::change_journal_orm_path(metadata.table_name);
    let qualified = !super::dialect::identifier_path_is_bare(
        super::DatabaseBackend::Postgres,
        metadata.table_name,
    );
    let sql = format!(
        "SELECT 1
        FROM pg_catalog.pg_trigger tr
        JOIN pg_catalog.pg_class t ON t.oid OPERATOR(pg_catalog.=) tr.tgrelid
        JOIN pg_catalog.pg_namespace tn ON tn.oid OPERATOR(pg_catalog.=) t.relnamespace
        JOIN pg_catalog.pg_proc p ON p.oid OPERATOR(pg_catalog.=) tr.tgfoid
        JOIN pg_catalog.pg_namespace pn ON pn.oid OPERATOR(pg_catalog.=) p.pronamespace
        CROSS JOIN LATERAL (
            SELECT pg_catalog.convert_from(
                pg_catalog.substr(tr.tgargs, 1, GREATEST(pg_catalog.position(tr.tgargs, '\\x00'::pg_catalog.bytea) OPERATOR(pg_catalog.-) 1, 0)),
                'UTF8'
            ) AS journal_schema,
            pg_catalog.substr(tr.tgargs, pg_catalog.position(tr.tgargs, '\\x00'::pg_catalog.bytea) OPERATOR(pg_catalog.+) 1) AS entry_arguments
        ) args
        JOIN pg_catalog.pg_class j
          ON j.oid OPERATOR(pg_catalog.=) pg_catalog.to_regclass(pg_catalog.format('%I.%I', args.journal_schema, ?))
        WHERE tr.tgrelid OPERATOR(pg_catalog.=) pg_catalog.to_regclass(?)
          AND tr.tgname OPERATOR(pg_catalog.=) ? AND NOT tr.tgisinternal AND tr.tgenabled OPERATOR(pg_catalog.=) 'O'
          AND tr.tgtype OPERATOR(pg_catalog.=) ? AND tr.tgqual IS NULL AND tr.tgnargs OPERATOR(pg_catalog.=) 7
          AND pn.nspname OPERATOR(pg_catalog.=) 'graphql_orm' AND p.proname OPERATOR(pg_catalog.=) ? AND p.pronargs OPERATOR(pg_catalog.=) 0
          AND p.prosecdef AND p.prosrc COLLATE pg_catalog.\"C\" OPERATOR(pg_catalog.=) ANY (ARRAY[?, ?]::pg_catalog.text[]) AND p.proconfig OPERATOR(pg_catalog.=) ARRAY[?]::pg_catalog.text[]
          AND p.proowner OPERATOR(pg_catalog.=) t.relowner AND j.relowner OPERATOR(pg_catalog.=) p.proowner AND j.relkind OPERATOR(pg_catalog.=) 'r'
          AND pg_catalog.position(tr.tgargs, '\\x00'::pg_catalog.bytea) OPERATOR(pg_catalog.>) 1
          AND args.entry_arguments OPERATOR(pg_catalog.=) (pg_catalog.convert_to(tn.nspname::pg_catalog.text, 'UTF8') OPERATOR(pg_catalog.||) '\\x00'::pg_catalog.bytea
              OPERATOR(pg_catalog.||) pg_catalog.convert_to(t.relname::pg_catalog.text, 'UTF8') OPERATOR(pg_catalog.||) '\\x00'::pg_catalog.bytea OPERATOR(pg_catalog.||) ?)
          AND NOT (args.journal_schema OPERATOR(pg_catalog.~~) 'pg\\_temp%')
          AND NOT (args.journal_schema OPERATOR(pg_catalog.~~) 'pg\\_toast\\_temp%')
          AND (? OR tn.nspname::pg_catalog.text OPERATOR(pg_catalog.=) args.journal_schema)
          AND EXISTS (
              SELECT 1 FROM graphql_orm.change_journal_tables r
              WHERE r.journal_schema OPERATOR(pg_catalog.=) args.journal_schema AND r.declared_table OPERATOR(pg_catalog.=) ?
                AND r.entity_name OPERATOR(pg_catalog.=) ? AND r.key_column OPERATOR(pg_catalog.=) ? AND r.relid::pg_catalog.oid OPERATOR(pg_catalog.=) tr.tgrelid
          )
          AND {REGISTRY_INTACT}
          AND NOT EXISTS (
              SELECT 1 FROM pg_catalog.pg_trigger other
              WHERE other.tgrelid OPERATOR(pg_catalog.=) tr.tgrelid AND other.tgfoid OPERATOR(pg_catalog.=) tr.tgfoid AND other.oid OPERATOR(pg_catalog.<>) tr.oid
          )
          AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_trigger jt WHERE jt.tgrelid OPERATOR(pg_catalog.=) j.oid AND NOT jt.tgisinternal)
          AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_rewrite jr WHERE jr.ev_class OPERATOR(pg_catalog.=) j.oid)
          AND {RECORDER_PRIVILEGES_CLEAN}
          AND pg_catalog.current_setting(?, true) OPERATOR(pg_catalog.=) 'on'
          AND pg_catalog.current_setting('session_replication_role') OPERATOR(pg_catalog.<>) 'replica'"
    );
    let values = vec![
        SqlValue::String(CHANGE_JOURNAL_TABLE.to_string()),
        SqlValue::String(table.clone()),
        SqlValue::String(super::rls::CHANGE_JOURNAL_TRIGGER.to_string()),
        SqlValue::Int(i64::from(super::rls::CHANGE_JOURNAL_TRIGGER_TYPE)),
        SqlValue::String(super::rls::CHANGE_JOURNAL_FUNCTION.to_string()),
        SqlValue::String(super::rls::CHANGE_JOURNAL_FUNCTION_BODY.to_string()),
        SqlValue::String(super::journal_key::recorder_body()),
        SqlValue::String(super::rls::CHANGE_JOURNAL_FUNCTION_CONFIG.to_string()),
        SqlValue::Bytes(expected_arguments),
        SqlValue::Bool(qualified),
        SqlValue::String(metadata.table_name.to_string()),
        SqlValue::String(metadata.entity_name.to_string()),
        SqlValue::String(key.to_string()),
        SqlValue::String(super::rls::CHANGE_JOURNAL_SETTING.to_string()),
    ];
    if !hook_ctx.fetch_rows(&sql, &values).await?.is_empty() {
        return Ok(());
    }
    Err(sqlx::Error::Protocol(
        diagnose_postgres_change_journal_recorder(hook_ctx, metadata.table_name).await?,
    ))
}

/// Describe why the registry cannot be read by this transaction, if it cannot.
async fn probe_postgres_change_journal_registry<B>(
    hook_ctx: &mut super::MutationContext<'_, B>,
) -> crate::Result<Option<String>>
where
    B: super::WriteBackend,
{
    if hook_ctx
        .fetch_rows(
            "SELECT 1 WHERE pg_catalog.to_regnamespace('graphql_orm') IS NOT NULL
               AND pg_catalog.has_schema_privilege(pg_catalog.to_regnamespace('graphql_orm')::pg_catalog.oid, 'USAGE')",
            &[],
        )
        .await?
        .is_empty()
    {
        return Ok(Some(
            "the current role has no USAGE on schema graphql_orm, or it does not exist; apply the schema target or Database::ensure_change_journal_recorder as the schema owner, which grants USAGE to PUBLIC".to_string(),
        ));
    }
    if hook_ctx
        .fetch_rows(
            "SELECT 1 WHERE pg_catalog.to_regclass('graphql_orm.change_journal_tables') IS NOT NULL",
            &[],
        )
        .await?
        .is_empty()
    {
        return Ok(Some(format!(
            "the change journal registry {} does not exist; apply the schema target or Database::ensure_change_journal_recorder as the schema owner",
            super::rls::CHANGE_JOURNAL_REGISTRY
        )));
    }
    Ok(None)
}

/// Name the first reason the recorder check failed for `table_name`.
async fn diagnose_postgres_change_journal_recorder<B>(
    hook_ctx: &mut super::MutationContext<'_, B>,
    table_name: &str,
) -> crate::Result<String>
where
    B: super::WriteBackend,
{
    let setting = vec![SqlValue::String(
        super::rls::CHANGE_JOURNAL_SETTING.to_string(),
    )];
    if hook_ctx
        .fetch_rows(
            "SELECT 1 WHERE pg_catalog.current_setting(?, true) OPERATOR(pg_catalog.=) 'on'",
            &setting,
        )
        .await?
        .is_empty()
    {
        return Ok(format!(
            "change journal recording is not enabled in this transaction ({} is not 'on'), so writes to {table_name} would not be journaled",
            super::rls::CHANGE_JOURNAL_SETTING
        ));
    }
    if hook_ctx
        .fetch_rows(
            "SELECT 1 WHERE pg_catalog.current_setting('session_replication_role') OPERATOR(pg_catalog.<>) 'replica'",
            &[],
        )
        .await?
        .is_empty()
    {
        return Ok(format!(
            "session_replication_role is replica, so the change journal trigger on {table_name} would not fire"
        ));
    }
    // Resolved through the session search_path, as the journal itself is.
    let journal = [
        SqlValue::String(format!("\"{CHANGE_JOURNAL_TABLE}\"")),
        SqlValue::String(super::rls::CHANGE_JOURNAL_FUNCTION.to_string()),
    ];
    if !hook_ctx
        .fetch_rows(
            "SELECT 1 FROM pg_catalog.pg_class j WHERE j.oid OPERATOR(pg_catalog.=) pg_catalog.to_regclass(?)
               AND (EXISTS (SELECT 1 FROM pg_catalog.pg_trigger jt WHERE jt.tgrelid OPERATOR(pg_catalog.=) j.oid AND NOT jt.tgisinternal)
                    OR EXISTS (SELECT 1 FROM pg_catalog.pg_rewrite jr WHERE jr.ev_class OPERATOR(pg_catalog.=) j.oid))",
            &journal[..1],
        )
        .await?
        .is_empty()
    {
        return Ok(format!(
            "the change journal table carries a trigger or rule, which would run as its owner; remove it before journaling writes to {table_name}"
        ));
    }
    let privileges = format!(
        "SELECT 1 FROM pg_catalog.pg_class j
         JOIN pg_catalog.pg_proc p
           ON p.oid OPERATOR(pg_catalog.=) pg_catalog.to_regprocedure(pg_catalog.format('graphql_orm.%I()', ?::pg_catalog.text))
         WHERE j.oid OPERATOR(pg_catalog.=) pg_catalog.to_regclass(?) AND NOT {RECORDER_PRIVILEGES_CLEAN}"
    );
    let privilege_values = vec![journal[1].clone(), journal[0].clone()];
    if !hook_ctx
        .fetch_rows(&privileges, &privilege_values)
        .await?
        .is_empty()
    {
        return Ok(format!(
            "the current role holds a privilege other than SELECT on the change journal, or EXECUTE on graphql_orm.{}(); revoke it before journaling writes to {table_name}",
            super::rls::CHANGE_JOURNAL_FUNCTION
        ));
    }
    let registry_integrity = format!(
        "SELECT 1 FROM pg_catalog.pg_proc p
         WHERE p.oid OPERATOR(pg_catalog.=) pg_catalog.to_regprocedure(pg_catalog.format('graphql_orm.%I()', ?::pg_catalog.text))
           AND {REGISTRY_INTACT}"
    );
    if hook_ctx
        .fetch_rows(&registry_integrity, &journal[1..])
        .await?
        .is_empty()
    {
        return Ok(format!(
            "the change journal registry {} is not owned by the recorder's owner, or a role other than its owner holds a privilege on it beyond SELECT; revoke it, or apply the schema target, before journaling writes to {table_name}",
            super::rls::CHANGE_JOURNAL_REGISTRY
        ));
    }
    let registry = vec![SqlValue::String(table_name.to_string())];
    if hook_ctx
        .fetch_rows(
            "SELECT 1 FROM pg_catalog.pg_class g
             WHERE g.oid OPERATOR(pg_catalog.=) pg_catalog.to_regclass('graphql_orm.change_journal_tables')
               AND EXISTS (SELECT 1 FROM graphql_orm.change_journal_tables r WHERE r.declared_table OPERATOR(pg_catalog.=) ?)",
            &registry,
        )
        .await?
        .is_empty()
    {
        return Ok(format!(
            "the change journal registry has no binding for table {table_name}; apply the schema target or Database::ensure_change_journal_recorder as the schema owner"
        ));
    }
    Ok(format!(
        "the change journal recorder, or its registry binding, is missing or altered on table {table_name}; apply the schema target or Database::ensure_change_journal_recorder as the schema owner"
    ))
}

fn missing_change_journal_sequence() -> sqlx::Error {
    sqlx::Error::Protocol(format!(
        "change journal {CHANGE_JOURNAL_TABLE} has no {CHANGE_JOURNAL_SEQUENCE_COLUMN} column and cannot be exported in order; run ensure_change_journal_table, or on PostgreSQL apply the schema target, as the journal's owner"
    ))
}

/// The entries to return from rows read in ascending sequence order: the run
/// that ends before the first entry recorded after `until`.
///
/// Filtering later entries out instead would let the caller's next position
/// pass an entry this export did not return.
fn changes_until(rows: Vec<crate::DbRow>, until: i64) -> crate::Result<Vec<BackupChange>> {
    let mut changes = Vec::with_capacity(rows.len());
    for row in rows {
        let change = decode_backup_change(row)?;
        if change.changed_at > until {
            break;
        }
        changes.push(change);
    }
    Ok(changes)
}

fn export_changes_sql(upper_bound: bool) -> String {
    format!(
        "SELECT {CHANGE_JOURNAL_SEQUENCE_COLUMN}, {CHANGE_JOURNAL_ENTRY_COLUMNS}
         FROM {}
         WHERE {CHANGE_JOURNAL_SEQUENCE_COLUMN} > ?{}
         ORDER BY {CHANGE_JOURNAL_SEQUENCE_COLUMN} ASC",
        quote_identifier(CHANGE_JOURNAL_TABLE),
        if upper_bound {
            format!(" AND {CHANGE_JOURNAL_SEQUENCE_COLUMN} <= ?")
        } else {
            String::new()
        }
    )
}

// SQLite admits one write transaction at a time, so positions are committed
// in the order they are assigned and a read sees every position below the
// highest one it returns.
#[cfg(feature = "sqlite")]
async fn export_changes(
    pool: &DbPool,
    window: ChangeWindow,
    _max_wait: std::time::Duration,
) -> crate::Result<Vec<BackupChange>> {
    let sequenced: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info(?, 'main') WHERE name = ?")
            .bind(CHANGE_JOURNAL_TABLE)
            .bind(CHANGE_JOURNAL_SEQUENCE_COLUMN)
            .fetch_one(pool)
            .await?;
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM main.sqlite_master WHERE type = 'table' AND name = ?",
    )
    .bind(CHANGE_JOURNAL_TABLE)
    .fetch_one(pool)
    .await?;
    if exists != 0 && sequenced == 0 {
        return Err(missing_change_journal_sequence());
    }
    let rows = super::fetch_rows::<DefaultBackend>(
        pool,
        &export_changes_sql(false),
        &[SqlValue::Int(window.after_sequence.unwrap_or(0))],
    )
    .await?;
    changes_until(rows, window.until)
}

// PostgreSQL assigns a position when an entry is inserted, and the entry
// becomes visible when its transaction commits. A transaction that inserted
// position 5 may commit after one that inserted position 6, so a plain read
// could return 6, and a later read "after 6" would never return 5.
//
// The recorder locks the journal in ROW EXCLUSIVE mode before it inserts and
// holds the lock until its transaction ends. The export therefore:
//
// 1. reads the highest committed position;
// 2. lists the transactions that hold that lock now. Any transaction with an
//    uncommitted position at or below the one read is among them, because it
//    took the lock before it took its position, and positions only grow;
// 3. waits until each listed transaction has ended;
// 4. returns the entries up to the position read in step 1. Every one of
//    them is now committed and visible, or was rolled back and never will be.
//
// `pg_locks` is readable by every role, so an export role needs only `SELECT`
// on the journal.
#[cfg(feature = "postgres")]
async fn export_changes(
    pool: &DbPool,
    window: ChangeWindow,
    max_wait: std::time::Duration,
) -> crate::Result<Vec<BackupChange>> {
    let journal = quote_identifier(CHANGE_JOURNAL_TABLE);
    let mut tx = pool.begin().await?;
    // Each statement must see transactions that committed while the export
    // waited, whatever the session's default isolation level is.
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await?;
    // `row_security = off` makes PostgreSQL refuse the read for a role
    // subject to RLS instead of silently exporting an empty journal.
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut *tx)
        .await?;
    // A standby shows committed entries only, and its `pg_locks` does not
    // list the primary's open write transactions, so an export there could
    // pass a position that is still uncommitted on the primary.
    let standby: bool = sqlx::query_scalar("SELECT pg_catalog.pg_is_in_recovery()")
        .fetch_one(&mut *tx)
        .await?;
    if standby {
        return Err(sqlx::Error::Protocol(
            "the change journal must be exported from the primary: a standby cannot see the primary's open write transactions"
                .to_string(),
        ));
    }
    let sequenced: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_attribute a
            WHERE a.attrelid OPERATOR(pg_catalog.=) pg_catalog.to_regclass($1)::pg_catalog.oid
              AND a.attname OPERATOR(pg_catalog.=) $2::pg_catalog.name
              AND NOT a.attisdropped)",
    )
    .bind(&journal)
    .bind(CHANGE_JOURNAL_SEQUENCE_COLUMN)
    .fetch_one(&mut *tx)
    .await?;
    if !sequenced {
        let exists: bool = sqlx::query_scalar("SELECT pg_catalog.to_regclass($1) IS NOT NULL")
            .bind(&journal)
            .fetch_one(&mut *tx)
            .await?;
        if exists {
            return Err(missing_change_journal_sequence());
        }
    }
    // Serialize epoch selection with cutover, without blocking journal writes.
    sqlx::query(&format!("LOCK TABLE {journal} IN ACCESS SHARE MODE"))
        .execute(&mut *tx)
        .await?;
    if let Some(start) = super::journal_key::epoch(&mut tx).await?
        && window.after_sequence.unwrap_or(0) < start
    {
        return Err(sqlx::Error::Protocol(format!(
            "change journal cursor predates the canonical key cutover at sequence {start}; start from a new full backup"
        )));
    }
    let after = window.after_sequence.unwrap_or(0);
    let high_water: Option<i64> = sqlx::query_scalar(&format!(
        "SELECT pg_catalog.max({CHANGE_JOURNAL_SEQUENCE_COLUMN}) FROM {journal}"
    ))
    .fetch_one(&mut *tx)
    .await?;
    let Some(high_water) = high_water.filter(|position| *position > after) else {
        tx.commit().await?;
        return Ok(Vec::new());
    };
    const OPEN_WRITERS: &str = "SELECT l.virtualtransaction::pg_catalog.text
         FROM pg_catalog.pg_locks l
         WHERE l.locktype OPERATOR(pg_catalog.=) 'relation'
           AND l.database OPERATOR(pg_catalog.=) (SELECT d.oid FROM pg_catalog.pg_database d WHERE d.datname OPERATOR(pg_catalog.=) pg_catalog.current_database())
           AND l.relation OPERATOR(pg_catalog.=) pg_catalog.to_regclass($1)::pg_catalog.oid
           AND l.mode OPERATOR(pg_catalog.=) 'RowExclusiveLock'
           AND l.granted
           AND l.pid IS DISTINCT FROM pg_catalog.pg_backend_pid()";
    let mut open: Vec<String> = sqlx::query_scalar(OPEN_WRITERS)
        .bind(&journal)
        .fetch_all(&mut *tx)
        .await?;
    let started = std::time::Instant::now();
    let mut pause = std::time::Duration::from_millis(5);
    while !open.is_empty() {
        if started.elapsed() >= max_wait {
            return Err(sqlx::Error::Decode(Box::new(ChangeExportTimedOut {
                waited: started.elapsed(),
            })));
        }
        tokio::time::sleep(pause.min(max_wait.saturating_sub(started.elapsed()))).await;
        pause = (pause * 2).min(std::time::Duration::from_millis(200));
        let current: Vec<String> = sqlx::query_scalar(OPEN_WRITERS)
            .bind(&journal)
            .fetch_all(&mut *tx)
            .await?;
        open.retain(|transaction| current.contains(transaction));
    }
    let sql = <PostgresBackend as super::OrmBackend>::normalize_sql(&export_changes_sql(true), 1);
    let rows = sqlx::query(&sql)
        .bind(after)
        .bind(high_water)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    changes_until(rows, window.until)
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
    #[cfg_attr(not(feature = "postgres"), allow(unused_mut))]
    let mut select_columns = export_columns
        .iter()
        .map(|column| quote_identifier(&column.column_name))
        .collect::<Vec<_>>()
        .join(", ");
    let key_columns = validated_key_columns(entity)?;
    #[cfg(feature = "postgres")]
    let canonical = snapshot.canonical_key_start.is_some() && key_columns.len() == 1;
    #[cfg(feature = "postgres")]
    if canonical {
        let table = quote_table_identifier(&entity.table_name);
        sqlx::query(&format!("LOCK TABLE {table} IN ACCESS SHARE MODE"))
            .execute(&mut *snapshot.tx)
            .await?;
        super::journal_key::key_type(&mut snapshot.tx, &table, key_columns[0]).await?;
        select_columns.push_str(", ");
        select_columns.push_str(&super::journal_key::projection(&quote_identifier(
            key_columns[0],
        )));
    }
    let sql = format!(
        "SELECT {} FROM {} ORDER BY {}",
        select_columns,
        quote_table_identifier(&entity.table_name),
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

        #[cfg(feature = "postgres")]
        let primary_key = if canonical {
            if matches!(values.get(key_columns[0]), Some(BackupValue::Float(value)) if !value.is_finite())
            {
                return Err(sqlx::Error::Protocol(format!(
                    "backup cannot restore a non-finite float primary key in {}",
                    entity.table_name
                )));
            }
            row.try_get(export_columns.len())?
        } else {
            row_identity(entity, &key_columns, &values)?
        };
        #[cfg(not(feature = "postgres"))]
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
    let mut restore = begin_restore_transaction(pool).await?;
    let report = import_table_rows_on(&mut restore, entity, rows).await?;
    restore.tx.commit().await?;
    Ok(report)
}

/// Begin the transaction a restore writes in.
async fn begin_restore_transaction(pool: &DbPool) -> crate::Result<BackupSnapshot> {
    #[cfg_attr(not(feature = "postgres"), allow(unused_mut))]
    let mut tx = pool.begin().await?;
    // Restore never journals, even when a role or database default turns the
    // recording setting on for every session.
    #[cfg(feature = "postgres")]
    sqlx::query(&format!(
        "SET LOCAL {} = 'off'",
        super::rls::CHANGE_JOURNAL_SETTING
    ))
    .execute(&mut *tx)
    .await?;
    #[cfg(feature = "postgres")]
    let canonical_key_start = super::journal_key::epoch(&mut tx).await?;
    Ok(BackupSnapshot {
        tx,
        #[cfg(feature = "postgres")]
        canonical_key_start,
    })
}

/// Import one table's rows inside `restore` and check the stored rows against
/// the backup's row count and row hashes. Nothing is committed: an error
/// leaves the caller to roll the transaction back by dropping it.
async fn import_table_rows_on(
    restore: &mut BackupSnapshot,
    entity: &EntityBackupDescriptor,
    rows: &[BackupRow],
) -> crate::Result<ImportReport> {
    let canonical_keys = restore.canonical_change_journal_start_sequence().is_some();
    let tx = &mut restore.tx;
    let count = table_row_count_on(tx, &entity.table_name).await?;
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
        quote_table_identifier(&entity.table_name),
        column_sql,
        placeholder_sql
    );

    let order = import_row_order(entity, rows, canonical_keys)?;
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
        execute_import_statement(tx, &insert_sql, &values).await?;
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
            execute_import_statement(tx, &insert_sql, &values).await?;
        }
        let update_sql = format!(
            "UPDATE {} SET {} WHERE {} = {}",
            quote_table_identifier(&entity.table_name),
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
            match execute_import_statement(tx, &update_sql, &values).await {
                Err(error) if is_staged_cycle_refusal(&error) => {
                    return Err(sqlx::Error::Decode(Box::new(BackupCycleFailure {
                        cycle: BackupSelfReferenceCycle {
                            table_name: entity.table_name.clone(),
                            primary_key: row.primary_key.clone(),
                            columns: self_columns
                                .iter()
                                .map(|column| column.column_name.clone())
                                .collect(),
                        },
                        source: error,
                    })));
                }
                result => result?,
            }
        }
    }

    let imported_count = table_row_count_on(tx, &entity.table_name).await? as usize;
    if imported_count != rows.len() {
        return Err(sqlx::Error::Protocol(format!(
            "restore row count validation failed for {}: imported {}, expected {}",
            entity.table_name,
            imported_count,
            rows.len()
        )));
    }

    // Read the rows back in the same transaction, so a mismatch rolls the
    // import back instead of leaving it committed.
    let exported = export_table_rows(restore, entity).await?;
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

/// Reject parsed leap seconds before the record engine's canonicalization can round them away,
/// then keep the UTC result inside the range both backup backends can store. Offsets and
/// microsecond rounding can move a four-digit input outside this range, so check the output year.
fn parse_backup_date_time(text: &str) -> crate::Result<super::RuntimeDateTime> {
    let parsed = chrono::DateTime::parse_from_rfc3339(text)
        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
    // Chrono represents seconds 60 as second 59 with nanoseconds >= 1_000_000_000.
    // Inspect that representation before rounding: .9999995 within the leap second would
    // otherwise carry into the next minute and lose the evidence needed for this refusal.
    if chrono::Timelike::nanosecond(&parsed) >= 1_000_000_000 {
        return Err(sqlx::Error::Protocol(
            "backup date-time cannot contain a leap second".to_owned(),
        ));
    }
    let canonical = super::RuntimeDateTime::from_utc(parsed.with_timezone(&chrono::Utc));
    // Canonical UTC text has exactly 27 bytes for a four-digit year. Chrono can also represent
    // year zero and signed extended years, which PostgreSQL's portable range excludes.
    if canonical.as_str().len() != 27 || canonical.as_str().starts_with("0000-") {
        return Err(sqlx::Error::Protocol(
            "backup date-time is outside the portable range 0001-9999".to_owned(),
        ));
    }
    Ok(canonical)
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
        if parse_backup_date_time(text).is_ok_and(|canonical| canonical.as_str() == text) {
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
/// [`BackupSelfReferenceCycle::from_error`]. The enclosing SQLx error's standard source chain
/// retains the original database error. Operational failures such as cancellation and deadlock
/// propagate unchanged instead of receiving this classification.
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
            sqlx::Error::Decode(source) => source.downcast_ref().or_else(|| {
                source
                    .downcast_ref::<BackupCycleFailure>()
                    .map(|failure| &failure.cycle)
            }),
            _ => None,
        }
    }
}

// Keep the public, cloneable diagnostic intact; attach the non-cloneable SQLx cause privately.
#[derive(Debug)]
struct BackupCycleFailure {
    cycle: BackupSelfReferenceCycle,
    source: sqlx::Error,
}
impl std::fmt::Display for BackupCycleFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.cycle.fmt(f)
    }
}
impl std::error::Error for BackupCycleFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn is_staged_cycle_refusal(error: &sqlx::Error) -> bool {
    let Some(database) = error.as_database_error() else {
        return false;
    };
    if database.is_unique_violation()
        || database.is_foreign_key_violation()
        || matches!(database.kind(), sqlx::error::ErrorKind::NotNullViolation)
        || database.is_check_violation()
    {
        return true;
    }
    // Use structured backend codes, never message matching. Triggers can also raise operational
    // SQLSTATEs; those must preserve their original category, e.g. 57014 and 40P01 on PostgreSQL.
    #[cfg(feature = "sqlite")]
    {
        database
            .try_downcast_ref::<sqlx::sqlite::SqliteError>()
            .is_some()
            && database.code().as_deref() == Some("1811") // SQLITE_CONSTRAINT_TRIGGER
    }
    #[cfg(feature = "postgres")]
    {
        database
            .try_downcast_ref::<sqlx::postgres::PgDatabaseError>()
            .is_some()
            && database.code().is_some_and(|code| {
                // Class 23 includes exclusion constraints, which SQLx's helpers do not cover.
                code.starts_with("23") || matches!(code.as_ref(), "P0001" | "09000" | "27000")
            })
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
    canonical_keys: bool,
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
        .map(|(index, row)| {
            // Foreign keys are typed values. Canonical identities encode
            // physical key bytes, so use the key value for dependency lookup.
            let key = if canonical_keys {
                let value = row.values.get(&entity.primary_key_column).ok_or_else(|| {
                    sqlx::Error::Protocol(format!(
                        "backup row {} for table {} is missing primary key column {}",
                        row.primary_key, entity.table_name, entity.primary_key_column
                    ))
                })?;
                std::borrow::Cow::Owned(value.primary_key_string())
            } else {
                std::borrow::Cow::Borrowed(row.primary_key.as_str())
            };
            Ok((key, index))
        })
        .collect::<crate::Result<BTreeMap<_, _>>>()?;
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
        quote_table_identifier(table_name)
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
        quote_table_identifier(table_name)
    );
    sqlx::query("SET LOCAL row_security = off")
        .execute(&mut **tx)
        .await?;
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
                    parse_backup_date_time(&value)
                        .map(|value| BackupValue::String(value.as_str().to_owned()))
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
        quote_table_identifier(&entity.table_name),
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
        row.try_get(CHANGE_JOURNAL_SEQUENCE_COLUMN)?,
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
        row.try_get(CHANGE_JOURNAL_SEQUENCE_COLUMN)?,
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

#[allow(clippy::too_many_arguments)]
fn decode_backup_change_parts(
    sequence: i64,
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
        sequence,
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

fn quote_table_identifier(identifier: &str) -> String {
    if current_backend() == DatabaseBackend::Postgres {
        current_backend().render_table_identifier_path(identifier)
    } else {
        quote_identifier(identifier)
    }
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

        let order = import_row_order(&entity, &rows, false).expect("rows should order");

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

        let order =
            import_row_order(&entity, &rows, false).expect("cycles defer instead of failing");
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
    fn backup_date_time_refuses_leap_seconds_before_rounding() {
        for leap in [
            "2016-12-31T23:59:60.9999995Z",
            "2016-12-31T23:59:60.9999994Z",
            "2016-12-31T23:59:60.9999999Z",
            "2017-01-01T05:44:60.9999995+05:45",
            "9999-12-31T23:59:60.9999995Z",
        ] {
            let result = parse_backup_date_time(leap);
            assert!(result.is_err(), "accepted {leap}: {result:?}");
            assert!(
                result.unwrap_err().to_string().contains("leap second"),
                "{leap}"
            );
        }
    }

    #[test]
    fn backup_date_time_preserves_non_leap_rounding_and_boundaries() {
        for (raw, expected) in [
            (
                "2016-12-31T23:59:59.9999994Z",
                "2016-12-31T23:59:59.999999Z",
            ),
            (
                "2016-12-31T23:59:59.9999995Z",
                "2017-01-01T00:00:00.000000Z",
            ),
            (
                "2001-02-03T09:50:06.1234564+05:45",
                "2001-02-03T04:05:06.123456Z",
            ),
            (
                "2001-02-03T09:50:06.1234565+05:45",
                "2001-02-03T04:05:06.123457Z",
            ),
            ("0001-01-01T00:00:00Z", "0001-01-01T00:00:00.000000Z"),
            (
                "9999-12-31T23:59:59.9999994Z",
                "9999-12-31T23:59:59.999999Z",
            ),
        ] {
            let parsed = parse_backup_date_time(raw).unwrap();
            assert_eq!(parsed.as_str(), expected, "{raw}");
            assert_eq!(parsed, super::super::RuntimeDateTime::parse(raw).unwrap());
        }
        assert!(parse_backup_date_time("9999-12-31T23:59:59.9999995Z").is_err());
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
            text("0000-01-01T00:00:00.000000Z"),
            text("+10000-01-01T00:00:00.000000Z"),
            text("2016-12-31T23:59:60.000000Z"),
            text("9999-12-31T23:59:60.000000Z"),
            BackupValue::Integer(1),
        ] {
            assert!(
                validate_backup_value(&descriptor, &refused).is_err(),
                "{refused:?}"
            );
        }
        for accepted in ["0001-01-01T00:00:00.000000Z", "9999-12-31T23:59:59.999999Z"] {
            assert!(validate_backup_value(&descriptor, &text(accepted)).is_ok());
        }
        for outside in [
            "2016-12-31T23:59:60.000000Z",
            "9999-12-31T23:59:60.000000Z",
            "0000-01-01T00:00:00Z",
            "0001-01-01T00:00:00+01:00",
            "9999-12-31T23:59:59-01:00",
            "9999-12-31T23:59:59.9999995Z",
        ] {
            assert!(parse_backup_date_time(outside).is_err(), "{outside}");
        }
        assert_eq!(BackupValueKind::DateTime.as_schema_str(), "date_time");
        assert!(matches!(
            backup_value_to_sql_value(&BackupValue::Null, &descriptor),
            Ok(SqlValue::StringNull)
        ));
    }
}
