use super::{
    BackupValueKind, ColumnBackupPolicy, DatabaseBackend, EntityBackupDescriptor, EntityMetadata,
    GraphqlOrmSchemaSnapshot, OrmBackend, SqlValue,
};
use std::collections::BTreeMap;

mod sqlx_runtime;

#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
mod journal;
const CHANGE_JOURNAL_TABLE: &str = "__graphql_orm_change_log";
#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
pub(crate) use journal::{begin_change_journal_transaction, record_change_journal_event};

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

/// How long `Database::export_changes`
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

/// A consistent backup read transaction on a SQLite or PostgreSQL database.
pub struct BackupSnapshot(SnapshotTransaction);

enum SnapshotTransaction {
    #[cfg(feature = "sqlite")]
    Sqlite(sqlx::Transaction<'static, sqlx::Sqlite>),
    #[cfg(feature = "postgres")]
    Postgres(sqlx::Transaction<'static, sqlx::Postgres>),
}

/// Logical backup and restore operations for the runtime database.
///
/// A database configured with
/// [`DatabaseBuilder::externally_managed_backup_restore`](crate::db::DatabaseBuilder::externally_managed_backup_restore)
/// refuses snapshots, exports, and imports with
/// [`BackupRestoreExternallyManaged`](super::BackupRestoreExternallyManaged).
/// Metadata helpers remain available.
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
    let parsed = crate::temporal::parse_rfc3339(text)
        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
    // Rounding .9999995 within a leap second carries into the next minute.
    if parsed.leap_second {
        return Err(sqlx::Error::Protocol(
            "backup date-time cannot contain a leap second".to_owned(),
        ));
    }
    let canonical = super::RuntimeDateTime::from_utc(parsed.value);
    // Canonical UTC text has exactly 27 bytes for a four-digit year.
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
        if database
            .try_downcast_ref::<sqlx::sqlite::SqliteError>()
            .is_some()
        {
            return database.code().as_deref() == Some("1811"); // SQLITE_CONSTRAINT_TRIGGER
        }
    }
    #[cfg(feature = "postgres")]
    {
        if database
            .try_downcast_ref::<sqlx::postgres::PgDatabaseError>()
            .is_some()
        {
            return database.code().is_some_and(|code| {
                // Class 23 includes exclusion constraints, which SQLx's helpers do not cover.
                code.starts_with("23") || matches!(code.as_ref(), "P0001" | "09000" | "27000")
            });
        }
    }
    false
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

/// Restore a snapshot through the runtime for the database's concrete backend.
pub async fn restore_backup_rows<B: OrmBackend>(
    database: &crate::db::Database<B>,
    backup_snapshot: &GraphqlOrmSchemaSnapshot,
    current_snapshot: &GraphqlOrmSchemaSnapshot,
    rows_by_table: &BTreeMap<String, Vec<BackupRow>>,
    context: &RestoreContext,
) -> crate::Result<Vec<ImportReport>>
where
    crate::db::Database<B>: GraphqlOrmBackupRuntime,
{
    database
        .restore_backup_rows(backup_snapshot, current_snapshot, rows_by_table, context)
        .await
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
