//! Restore-in-progress marker for backends that cannot restore a snapshot in
//! one transaction through the ORM.
//!
//! PostgreSQL and SQLite restore every table of a snapshot in one transaction
//! (`restore_backup_rows`), so a failed restore
//! leaves nothing behind and needs no marker. SQL Server has no ORM restore:
//! a host restores it with its own statements, which may commit table by
//! table. The marker records that such a restore has started and not yet
//! finished, so a process that starts in between refuses to serve a partly
//! restored database.

use super::backend::OrmBackend;
use super::dialect::{DatabaseBackend, SqlDialect};

/// Table that holds the marker row while a host-run restore is in progress.
pub const RESTORE_MARKER_TABLE: &str = "__graphql_orm_restore_marker";

/// A host-run restore started and has not finished.
///
/// Returned by [`Database::restore_in_progress`](crate::db::Database::restore_in_progress),
/// and carried by the error of
/// [`Database::ensure_no_restore_in_progress`](crate::db::Database::ensure_no_restore_in_progress),
/// `begin_restore_marker` and schema validation.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestoreInProgress {
    /// The identifier the host gave the restore, for example a snapshot id.
    pub restore_id: String,
    /// When the restore started, as UTC Unix seconds on the database clock.
    pub started_at: i64,
}

impl std::fmt::Display for RestoreInProgress {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "restore {} started at {} has not finished; complete it or remove its partial rows, then clear the restore marker",
            self.restore_id, self.started_at
        )
    }
}

impl std::error::Error for RestoreInProgress {}

impl RestoreInProgress {
    /// The marker carried by `error`, if that is what it reports.
    pub fn from_error(error: &crate::Error) -> Option<&Self> {
        match error {
            sqlx::Error::Decode(source) => source.downcast_ref::<Self>(),
            _ => None,
        }
    }

    fn into_error(self) -> crate::Error {
        sqlx::Error::Decode(Box::new(self))
    }
}

/// Read the marker. Backends that restore transactionally have none, and a
/// database without the marker table has no restore in progress.
pub(crate) async fn read_restore_marker<B: OrmBackend>(
    pool: &B::Pool,
) -> crate::Result<Option<RestoreInProgress>> {
    if B::DIALECT != DatabaseBackend::Mssql {
        return Ok(None);
    }
    // Name resolution is deferred, so the batch compiles without the table.
    let sql = format!(
        "IF OBJECT_ID(N'{RESTORE_MARKER_TABLE}', N'U') IS NOT NULL \
         SELECT restore_id, started_at FROM {table} \
         ELSE SELECT CAST(NULL AS NVARCHAR(200)) AS restore_id, CAST(NULL AS BIGINT) AS started_at WHERE 1 = 0",
        table = B::DIALECT.quote_identifier(RESTORE_MARKER_TABLE)
    );
    let rows = B::fetch_rows(pool, &sql, &[]).await?;
    rows.first()
        .map(|row| {
            Ok(RestoreInProgress {
                restore_id: B::try_get_string(row, "restore_id")?,
                started_at: B::try_get_i64(row, "started_at")?,
            })
        })
        .transpose()
}

/// Fail with [`RestoreInProgress`] while a marker is present.
pub(crate) async fn refuse_while_restoring<B: OrmBackend>(pool: &B::Pool) -> crate::Result<()> {
    match read_restore_marker::<B>(pool).await? {
        Some(marker) => Err(marker.into_error()),
        None => Ok(()),
    }
}

impl<B: OrmBackend> crate::db::Database<B> {
    /// The host-run restore that started on this database and has not
    /// finished, if any.
    ///
    /// Always `None` on PostgreSQL and SQLite, where a snapshot restores in
    /// one transaction and no marker is kept.
    pub async fn restore_in_progress(&self) -> crate::Result<Option<RestoreInProgress>> {
        read_restore_marker::<B>(self.pool()).await
    }

    /// Refuse to continue while a host-run restore is in progress.
    ///
    /// Call it when a service starts, before it serves requests.
    /// [`SchemaManager::validate_against_entities`](super::SchemaManager::validate_against_entities)
    /// and [`SchemaManager::validate_target`](super::SchemaManager::validate_target)
    /// make the same check, so a host that validates its schema at start is
    /// already covered.
    ///
    /// # Errors
    ///
    /// Fails with an error carrying [`RestoreInProgress`] (see
    /// [`RestoreInProgress::from_error`]) while a marker is present, or with a
    /// database failure.
    pub async fn ensure_no_restore_in_progress(&self) -> crate::Result<()> {
        refuse_while_restoring::<B>(self.pool()).await
    }
}

#[cfg(feature = "mssql")]
impl crate::db::Database<super::MssqlBackend> {
    /// Record that a host-run restore of this SQL Server database has started.
    ///
    /// SQL Server restores run as host statements that may commit table by
    /// table. Call this before the first restore write and
    /// [`Self::finish_restore_marker`] after the last one. A restore that
    /// fails in between leaves the marker, and
    /// [`Self::ensure_no_restore_in_progress`] and schema validation then
    /// refuse until the restore is completed, or its partial rows are removed,
    /// and the marker is cleared.
    ///
    /// The marker is one row in `__graphql_orm_restore_marker`, which this
    /// call creates in the connection's default schema when it is missing.
    /// It needs an external-writable pool whose login may create that table,
    /// or a table created beforehand by the schema owner.
    ///
    /// # Errors
    ///
    /// Fails with an error carrying [`RestoreInProgress`] when another
    /// restore's marker is present, and with a database failure otherwise.
    pub async fn begin_restore_marker(&self, restore_id: &str) -> crate::Result<()> {
        use super::backend::WriteBackend;
        let table = DatabaseBackend::Mssql.quote_identifier(RESTORE_MARKER_TABLE);
        let create = format!(
            "IF OBJECT_ID(N'{RESTORE_MARKER_TABLE}', N'U') IS NULL \
             CREATE TABLE {table} (\
             marker_key TINYINT NOT NULL PRIMARY KEY CHECK (marker_key = 1), \
             restore_id NVARCHAR(200) NOT NULL, \
             started_at BIGINT NOT NULL)"
        );
        super::MssqlBackend::execute_with_binds(self.pool(), &create, &[]).await?;
        // The single-row key makes the claim atomic: of two concurrent
        // restores, one inserts and the other is refused.
        let insert = format!(
            "INSERT INTO {table} (marker_key, restore_id, started_at) \
             SELECT 1, @P1, DATEDIFF_BIG(SECOND, '1970-01-01', SYSUTCDATETIME()) \
             WHERE NOT EXISTS (SELECT 1 FROM {table} WITH (UPDLOCK, HOLDLOCK))"
        );
        let inserted = super::MssqlBackend::execute_with_binds(
            self.pool(),
            &insert,
            &[super::SqlValue::String(restore_id.to_string())],
        )
        .await?;
        if super::MssqlBackend::rows_affected(&inserted) == 0 {
            return Err(
                match read_restore_marker::<super::MssqlBackend>(self.pool()).await? {
                    Some(marker) => marker.into_error(),
                    None => sqlx::Error::Protocol(
                        "the restore marker changed while it was being claimed; retry".to_string(),
                    ),
                },
            );
        }
        Ok(())
    }

    /// Clear the marker of the restore `restore_id`.
    ///
    /// Call it once the restore has completed, or after a failed restore's
    /// partial rows were removed so the database is consistent again.
    ///
    /// # Errors
    ///
    /// Fails when no marker with that identifier is present: the marker of a
    /// different restore is never cleared.
    pub async fn finish_restore_marker(&self, restore_id: &str) -> crate::Result<()> {
        use super::backend::WriteBackend;
        if read_restore_marker::<super::MssqlBackend>(self.pool())
            .await?
            .is_none()
        {
            return Err(sqlx::Error::Protocol(format!(
                "no restore marker is present for restore {restore_id}"
            )));
        }
        let table = DatabaseBackend::Mssql.quote_identifier(RESTORE_MARKER_TABLE);
        let deleted = super::MssqlBackend::execute_with_binds(
            self.pool(),
            &format!("DELETE FROM {table} WHERE marker_key = 1 AND restore_id = @P1"),
            &[super::SqlValue::String(restore_id.to_string())],
        )
        .await?;
        if super::MssqlBackend::rows_affected(&deleted) == 0 {
            return Err(sqlx::Error::Protocol(format!(
                "the restore marker present does not belong to restore {restore_id}"
            )));
        }
        Ok(())
    }
}
