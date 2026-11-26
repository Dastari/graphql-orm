//! Shared SQLx implementation instantiated with each concrete backend.
//! The backend-local cfg predicates select dialect-specific SQL even when
//! Cargo enables both SQLx drivers in the same ORM build.

use super::*;
use crate::graphql::orm;

const REDACTED_BACKUP_VALUE: &str = "[graphql-orm:redacted]";

fn require_change_journal(feature_error: &str) -> crate::Result<()> {
    if !cfg!(feature = "change-journal") {
        return Err(sqlx::Error::Protocol(feature_error.to_owned()));
    }
    if cfg!(any(
        feature = "mssql",
        all(feature = "sqlite", feature = "postgres")
    )) {
        return Err(sqlx::Error::Protocol(
            "the change journal requires a single graphql-orm backend feature".to_owned(),
        ));
    }
    Ok(())
}

macro_rules! sqlx_backup_runtime {
    ($module:ident, $backend:ident, $db:ty, $variant:ident, $sqlite:meta, $postgres:meta) => {
        mod $module {
            use super::*;
            use orm::{
                backup_descriptors_from_entities, execute_with_binds_on,
                schema_snapshot_from_entities,
            };
            use crate::$backend;
            use sqlx::Row;

            type Database = crate::db::Database<$backend>;
            type DbPool = sqlx::Pool<$db>;
            type DbRow = <$db as sqlx::Database>::Row;
            type Tx<'a> = sqlx::Transaction<'a, $db>;
            const BACKEND: DatabaseBackend = <$backend as OrmBackend>::DIALECT;

            impl GraphqlOrmBackupRuntime for Database {
                async fn begin_consistent_snapshot(&self) -> crate::Result<BackupSnapshot> {
                    self.ensure_backup_restore_allowed()?;
                    let mut tx = begin_consistent_snapshot(self.pool()).await?;
                    let start = canonical_key_epoch(&mut tx).await?;
                    Ok(BackupSnapshot(SnapshotTransaction::$variant(tx), start))
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
                    schema_snapshot_from_entities(BACKEND, migration_version, entities)
                }

                async fn export_table_rows(
                    &self,
                    snapshot: &mut BackupSnapshot,
                    entity: &EntityBackupDescriptor,
                ) -> crate::Result<Vec<BackupRow>> {
                    self.ensure_backup_restore_allowed()?;
                    match &mut snapshot.0 {
                        SnapshotTransaction::$variant(tx) => export_table_rows(tx, entity, snapshot.1.is_some()).await,
                        #[cfg(all(feature = "sqlite", feature = "postgres"))]
                        _ => Err(sqlx::Error::Protocol("backup snapshot backend does not match the database".into())),
                    }
                }

                async fn import_table_rows(
                    &self,
                    entity: &EntityBackupDescriptor,
                    rows: &[BackupRow],
                    context: &RestoreContext,
                ) -> crate::Result<ImportReport> {
                    self.ensure_backup_restore_allowed()?;
                    orm::ensure_sql_write_allowed()?;
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

            impl Database {
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
                    orm::ensure_sql_write_allowed()?;
                    require_change_journal("change journal support requires the graphql-orm change-journal feature")?;
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
                    orm::ensure_sql_write_allowed()?;
                    require_change_journal("change journal support requires the graphql-orm change-journal feature")?;
                    #[cfg($postgres)]
                    {
                        let schema = orm::SchemaModel::from_entities(entities);
                        orm::rls::refuse_change_journal_cascade_gaps(&schema)?;
                        let tables = orm::rls::change_journal_tables(&schema);
                        let mut tx = self.pool().begin().await?;
                        for statement in orm::rls::postgres_change_journal_recorder_sql(&tables) {
                            sqlx::query(&statement).execute(&mut *tx).await?;
                        }
                        tx.commit().await?;
                        Ok(())
                    }
                    #[cfg($sqlite)]
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
                #[cfg($postgres)]
                pub async fn enable_canonical_change_journal_keys(&self) -> crate::Result<i64> {
                    orm::ensure_sql_write_allowed()?;
                    require_change_journal("canonical change journal keys require the graphql-orm change-journal feature")?;
                    orm::journal_key::enable(self.pool()).await
                }

                /// Read the canonical key cutover sequence. Returns `None` on SQLite
                /// or when PostgreSQL still uses legacy keys.
                ///
                /// # Errors
                ///
                /// Fails if the database cannot be read or the canonical recorder is altered.
                pub async fn canonical_change_journal_start_sequence(&self) -> crate::Result<Option<i64>> {
                    #[cfg($postgres)]
                    {
                        let mut tx = begin_consistent_snapshot(self.pool()).await?;
                        let start = canonical_key_epoch(&mut tx).await?;
                        tx.commit().await?;
                        Ok(start)
                    }
                    #[cfg($sqlite)]
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
                    #[cfg($postgres)]
                    {
                        let mut tx = begin_consistent_snapshot(self.pool()).await?;
                        if canonical_key_epoch(&mut tx).await?.is_none() {
                            return Ok(None);
                        }
                        // Relation OIDs can differ on a matching restore target.
                        let mut keys = Vec::new();
                        for entity in entities.iter().filter(|entity| entity.primary_key_columns.is_empty()) {
                            let path = quote_table_identifier(&entity.table_name);
                            let storage = orm::journal_key::key_type(&mut tx, &path, &entity.primary_key_column).await?;
                            keys.push((&entity.table_name, &entity.primary_key_column, storage));
                        }
                        keys.sort_unstable();
                        let identity = serde_json::to_string(&keys)
                            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
                        tx.commit().await?;
                        Ok(Some(identity))
                    }
                    #[cfg($sqlite)]
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
                    self.ensure_backup_restore_allowed()?;
                    require_change_journal("incremental backup export requires the graphql-orm change-journal feature")?;
                    export_changes(self.pool(), window, max_wait).await
                }
            }

            async fn restore_backup_rows(
                database: &Database,
                backup_snapshot: &GraphqlOrmSchemaSnapshot,
                current_snapshot: &GraphqlOrmSchemaSnapshot,
                rows_by_table: &BTreeMap<String, Vec<BackupRow>>,
                context: &RestoreContext,
            ) -> crate::Result<Vec<ImportReport>> {
                database.ensure_backup_restore_allowed()?;
                orm::ensure_sql_write_allowed()?;
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
                let canonical_keys = canonical_key_epoch(&mut restore).await?.is_some();
                for entity in &entities {
                    let count = table_row_count_on(&mut restore, &entity.table_name).await?;
                    if count != 0 {
                        return Err(sqlx::Error::Protocol(format!(
                            "cannot restore table {} in EmptyDatabase mode because it already contains {} rows",
                            entity.table_name, count
                        )));
                    }
                }
                for entity in &entities {
                    let rows = rows_by_table.get(&entity.table_name).unwrap_or(&empty_rows);
                    reports.push(import_table_rows_on(&mut restore, entity, rows, canonical_keys).await?);
                }
                // The restored rows replace whatever a runtime search index was built
                // from. Clear each restored table's readiness-tracked index and leave it
                // `building`, so search is refused until a backfill rebuilds it.
                for entity in &entities {
                    reset_restored_runtime_search(&mut restore, &entity.table_name).await?;
                }
                restore.commit().await?;
                Ok(reports)
            }

            /// Clear the readiness-tracked (runtime) search index of `table_name`, if it
            /// has one, and mark it `building`. Static search indexes do not track
            /// readiness and are left alone.
            async fn reset_restored_runtime_search(
                restore: &mut Tx<'_>,
                table_name: &str,
            ) -> crate::Result<()> {
                let backend = BACKEND;
                let metadata_table = orm::search::search_metadata_table_name();
                // Probe the catalog first: a failed statement would abort the PostgreSQL
                // transaction, and most databases have no search metadata at all.
                let exists_sql = match backend {
                    DatabaseBackend::Postgres => "SELECT pg_catalog.to_regclass($1) IS NOT NULL",
                    _ => "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                };
                let exists: bool = sqlx::query_scalar(exists_sql)
                    .bind(metadata_table)
                    .fetch_one(&mut **restore)
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
                    .fetch_optional(&mut **restore)
                    .await?;
                let Some(mut config) =
                    stored.and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                else {
                    return Ok(());
                };
                if !orm::runtime_search::mark_building(&mut config) {
                    return Ok(());
                }
                let documents = match backend {
                    DatabaseBackend::Postgres => orm::search::search_table_name(table_name),
                    _ => orm::search::sqlite_fts_table_name(table_name),
                };
                sqlx::query(&format!("DELETE FROM {documents}"))
                    .execute(&mut **restore)
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
                    .execute(&mut **restore)
                    .await?;
                Ok(())
            }

            #[cfg($sqlite)]
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

            #[cfg($postgres)]
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

            #[cfg($sqlite)]
            async fn begin_consistent_snapshot(pool: &DbPool) -> crate::Result<Tx<'static>> {
                let tx = pool.begin().await?;
                Ok(tx)
            }

            #[cfg($postgres)]
            async fn begin_consistent_snapshot(pool: &DbPool) -> crate::Result<Tx<'static>> {
                let mut tx = pool.begin().await?;
                sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("SET LOCAL row_security = off")
                    .execute(&mut *tx)
                    .await?;
                Ok(tx)
            }

            #[cfg($sqlite)]
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

            #[cfg($sqlite)]
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
            #[cfg($sqlite)]
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

            #[cfg($sqlite)]
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

            #[cfg($postgres)]
            async fn ensure_change_journal_table(pool: &DbPool) -> crate::Result<()> {
                sqlx::query(&orm::rls::postgres_change_journal_table_sql())
                    .execute(pool)
                    .await?;
                // A journal from an earlier release gets its sequence column when the
                // caller owns it; otherwise the owner adds it by applying the target.
                sqlx::query(&orm::rls::postgres_change_journal_sequence_sql())
                    .execute(pool)
                    .await?;
                // The journal names every tenant's keys and drives restore deletes. Only
                // privileged roles read or write it; the database-side recorder appends.
                sqlx::query(&orm::rls::postgres_ensure_change_log_rls_sql())
                    .execute(pool)
                    .await?;
                Ok(())
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
            fn changes_until(rows: Vec<DbRow>, until: i64) -> crate::Result<Vec<BackupChange>> {
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
            #[cfg($sqlite)]
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
                let rows = orm::fetch_rows::<SqliteBackend>(
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
            #[cfg($postgres)]
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
                if let Some(start) = orm::journal_key::epoch(&mut tx).await?
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
                let sql = <PostgresBackend as orm::OrmBackend>::normalize_sql(&export_changes_sql(true), 1);
                let rows = sqlx::query(&sql)
                    .bind(after)
                    .bind(high_water)
                    .fetch_all(&mut *tx)
                    .await?;
                tx.commit().await?;
                changes_until(rows, window.until)
            }

            #[cfg($postgres)]
            fn backup_backend_capabilities() -> BackupBackendCapabilities {
                BackupBackendCapabilities {
                    consistent_snapshot: true,
                    typed_json: true,
                    typed_uuid: true,
                    bytes: true,
                    deferred_constraints: true,
                }
            }

            async fn canonical_key_epoch(tx: &mut Tx<'_>) -> crate::Result<Option<i64>> {
                #[cfg($postgres)]
                {
                    orm::journal_key::epoch(tx).await
                }
                #[cfg($sqlite)]
                {
                    let _ = tx;
                    Ok(None)
                }
            }

            async fn export_table_rows(
                snapshot: &mut Tx<'_>,
                entity: &EntityBackupDescriptor,
                canonical_keys: bool,
            ) -> crate::Result<Vec<BackupRow>> {
                let export_columns = export_columns(entity);
                #[cfg_attr($sqlite, allow(unused_mut))]
                let mut select_columns = export_columns
                    .iter()
                    .map(|column| quote_identifier(&column.column_name))
                    .collect::<Vec<_>>()
                    .join(", ");
                let key_columns = validated_key_columns(entity)?;
                #[cfg($sqlite)]
                let _ = canonical_keys;
                #[cfg($postgres)]
                let canonical = canonical_keys && key_columns.len() == 1;
                #[cfg($postgres)]
                if canonical {
                    let table = quote_table_identifier(&entity.table_name);
                    sqlx::query(&format!("LOCK TABLE {table} IN ACCESS SHARE MODE"))
                        .execute(&mut **snapshot)
                        .await?;
                    orm::journal_key::key_type(snapshot, &table, key_columns[0]).await?;
                    select_columns.push_str(", ");
                    select_columns.push_str(&orm::journal_key::projection(&quote_identifier(
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

                    #[cfg($postgres)]
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
                    #[cfg($sqlite)]
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
                let canonical_keys = canonical_key_epoch(&mut restore).await?.is_some();
                let report = import_table_rows_on(&mut restore, entity, rows, canonical_keys).await?;
                restore.commit().await?;
                Ok(report)
            }

            /// Begin the transaction a restore writes in.
            async fn begin_restore_transaction(pool: &DbPool) -> crate::Result<Tx<'static>> {
                #[cfg_attr($sqlite, allow(unused_mut))]
                let mut tx = pool.begin().await?;
                // Restore never journals, even when a role or database default turns the
                // recording setting on for every session.
                #[cfg($postgres)]
                sqlx::query(&format!(
                    "SET LOCAL {} = 'off'",
                    orm::rls::CHANGE_JOURNAL_SETTING
                ))
                .execute(&mut *tx)
                .await?;
                Ok(tx)
            }

            /// Import one table's rows inside `restore` and check the stored rows against
            /// the backup's row count and row hashes. Nothing is committed: an error
            /// leaves the caller to roll the transaction back by dropping it.
            async fn import_table_rows_on(
                restore: &mut Tx<'_>,
                entity: &EntityBackupDescriptor,
                rows: &[BackupRow],
                canonical_keys: bool,
            ) -> crate::Result<ImportReport> {
                let tx = &mut *restore;
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
                let stored = |row: &BackupRow, column: &orm::ColumnBackupDescriptor| {
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
                let exported = export_table_rows(restore, entity, canonical_keys).await?;
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

            #[cfg($sqlite)]
            async fn table_row_count_on(
                tx: &mut Tx<'_>,
                table_name: &str,
            ) -> crate::Result<i64> {
                let sql = format!(
                    "SELECT COUNT(*) AS count FROM {}",
                    quote_table_identifier(table_name)
                );
                let row = sqlx::query(&sql).fetch_one(&mut **tx).await?;
                row.try_get::<i64, _>("count")
            }

            #[cfg($sqlite)]
            async fn execute_import_statement(
                tx: &mut Tx<'_>,
                sql: &str,
                values: &[SqlValue],
            ) -> crate::Result<()> {
                execute_with_binds_on::<SqliteBackend, _>(&mut **tx, sql, values).await?;
                Ok(())
            }

            #[cfg($postgres)]
            async fn table_row_count_on(
                tx: &mut Tx<'_>,
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

            #[cfg($postgres)]
            async fn execute_import_statement(
                tx: &mut Tx<'_>,
                sql: &str,
                values: &[SqlValue],
            ) -> crate::Result<()> {
                execute_with_binds_on::<PostgresBackend, _>(&mut **tx, sql, values).await?;
                Ok(())
            }

            async fn fetch_snapshot_rows(
                snapshot: &mut Tx<'_>,
                sql: &str,
            ) -> crate::Result<Vec<DbRow>> {
                sqlx::query(sql).fetch_all(&mut **snapshot).await
            }

            #[cfg($sqlite)]
            fn decode_backup_value(
                row: &DbRow,
                descriptor: &orm::ColumnBackupDescriptor,
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
                                        orm::DecimalValue::new(definition.from_scaled_i64(value), definition)
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
                                parse_backup_date_time(&value).map(BackupValue::String)
                            })
                            .unwrap_or(Ok(BackupValue::Null))
                    }),
                }
            }

            #[cfg($postgres)]
            fn decode_backup_value(
                row: &DbRow,
                descriptor: &orm::ColumnBackupDescriptor,
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
                                        orm::DecimalValue::new(value, definition)
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
                    // `TimeZone`. `check_portable_date_times` has already refused values outside the portable range.
                    BackupValueKind::DateTime => row
                        .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(column)
                        .map(|value| {
                            value
                                .map(|value| {
                                    BackupValue::String(
                                        crate::temporal::Timestamp::from_chrono(value.fixed_offset())
                                            .canonical_utc(),
                                    )
                                })
                                .unwrap_or(BackupValue::Null)
                        }),
                }
            }

            /// The bind placeholder of an import column. A PostgreSQL `TIMESTAMPTZ` takes the canonical text
            /// through an explicit cast; its `Z` offset makes the cast independent of the session `TimeZone`.
            fn import_placeholder(column: &orm::ColumnBackupDescriptor) -> &'static str {
                if cfg!($postgres) && column.logical_type == BackupValueKind::DateTime {
                    "CAST(? AS TIMESTAMPTZ)"
                } else {
                    "?"
                }
            }

            /// Refuse a PostgreSQL date-time the portable encoding cannot carry before any row is decoded:
            /// `±infinity` and instants outside years 0001–9999 have no RFC 3339 form (and overflow the
            /// typed decode). Runs inside the export's consistent snapshot.
            #[cfg($postgres)]
            async fn check_portable_date_times(
                snapshot: &mut Tx<'_>,
                entity: &EntityBackupDescriptor,
                columns: &[&orm::ColumnBackupDescriptor],
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

            #[cfg($sqlite)]
            async fn check_portable_date_times(
                _: &mut Tx<'_>,
                _: &EntityBackupDescriptor,
                _: &[&orm::ColumnBackupDescriptor],
            ) -> crate::Result<()> {
                // SQLite stores text; the typed decode parses and canonicalizes each value instead.
                Ok(())
            }

            #[cfg($sqlite)]
            fn decode_backup_change(row: DbRow) -> crate::Result<BackupChange> {
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

            #[cfg($postgres)]
            fn decode_backup_change(row: DbRow) -> crate::Result<BackupChange> {
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

            fn quote_table_identifier(identifier: &str) -> String {
                if BACKEND == DatabaseBackend::Postgres {
                    BACKEND.render_table_identifier_path(identifier)
                } else {
                    quote_identifier(identifier)
                }
            }
        }
    };
}

#[cfg(feature = "sqlite")]
sqlx_backup_runtime!(sqlite, SqliteBackend, sqlx::Sqlite, Sqlite, all(), any());
#[cfg(feature = "postgres")]
sqlx_backup_runtime!(
    postgres,
    PostgresBackend,
    sqlx::Postgres,
    Postgres,
    any(),
    all()
);
