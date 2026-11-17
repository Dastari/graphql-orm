//! Restore-in-progress marker for backends that cannot restore a snapshot in
//! one transaction through the ORM.
//!
//! PostgreSQL and SQLite restore every table of a snapshot in one transaction
//! (`restore_backup_rows`), so a failed restore
//! leaves nothing behind and needs no marker. SQL Server has no ORM restore:
//! a host restores it with its own statements, which may commit table by
//! table. Two things keep readers and writers away from the partly restored
//! tables:
//!
//! - an application lock. Every ORM transaction and autocommit statement
//!   takes it in shared mode, in the same batch as the statement; the restore
//!   holds it exclusively for its whole duration. A restore therefore starts
//!   only once in-flight ORM work has finished, and nothing starts under it.
//! - a marker row, for the restore whose session died: the lock is gone with
//!   the session, the partial rows are not. While the row exists the same
//!   gate refuses, in every process, until the restore is resumed and
//!   finished.

use super::backend::OrmBackend;

/// Table that holds the marker row while a host-run restore is in progress.
pub const RESTORE_MARKER_TABLE: &str = "dbo.__graphql_orm_restore_marker";

/// A host-run restore started and has not finished.
///
/// Returned by [`Database::restore_in_progress`](crate::db::Database::restore_in_progress),
/// and carried by the error of every SQL Server ORM transaction, autocommit
/// statement and schema validation attempted while the restore is in
/// progress, and of `begin_restore` when another restore's marker is present.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RestoreInProgress {
    /// The identifier the host gave the restore, for example a snapshot id.
    /// Empty, with `started_at` zero, when the identifier is unknown: the
    /// restore holds its lock but has not recorded its marker yet, or a
    /// statement was refused because the restore holds its lock and the
    /// marker could then not be read. When no restore is known to be
    /// running, a marker that cannot be read is reported as
    /// [`RestoreStateUnavailable`] instead.
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

    #[cfg_attr(not(feature = "mssql"), allow(dead_code))]
    pub(crate) fn new(restore_id: String, started_at: i64) -> Self {
        Self {
            restore_id,
            started_at,
        }
    }

    pub(crate) fn into_error(self) -> crate::Error {
        sqlx::Error::Decode(Box::new(self))
    }
}

/// Why SQL Server restore state could not be read.
///
/// Carried by [`RestoreStateUnavailable`]. The variants are a diagnosis for
/// the operator; none of them means a restore is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RestoreStateUnavailableReason {
    /// `dbo.__graphql_orm_restore_marker` does not exist. Install it with
    /// `MssqlPool::install_restore_marker()` as schema owner.
    MarkerNotInstalled,
    /// The login may not read the marker table. Grant it SELECT.
    PermissionDenied,
    /// The database could not be reached, or did not answer in time (a pool,
    /// network, TLS or timeout failure). Nothing is wrong with the marker.
    ConnectionFailed,
    /// Any other failure reading the marker, such as a table of the wrong shape.
    Other,
}

/// SQL Server restore state could not be determined, so every statement is
/// refused (fail closed).
///
/// This is distinct from [`RestoreInProgress`]: no restore is known to be
/// running. Returned by
/// [`Database::restore_in_progress`](crate::db::Database::restore_in_progress),
/// and carried by the error of every SQL Server ORM transaction, autocommit
/// statement and schema validation attempted while the marker table is
/// missing or unreadable. Read it with [`Self::from_error`].
///
/// The message names the fix and never contains SQL text. The driver or
/// server error that caused it is kept as [`std::error::Error::source`], for
/// server-side diagnostics only.
#[derive(Debug)]
#[non_exhaustive]
pub struct RestoreStateUnavailable {
    /// The category of failure.
    pub reason: RestoreStateUnavailableReason,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl std::fmt::Display for RestoreStateUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.reason {
            RestoreStateUnavailableReason::MarkerNotInstalled => write!(
                formatter,
                "restore state unavailable: {RESTORE_MARKER_TABLE} is not installed, so statements are refused; \
                 as schema owner call MssqlPool::install_restore_marker() (or run the documented CREATE TABLE) \
                 and grant SELECT on it to the runtime role"
            ),
            RestoreStateUnavailableReason::PermissionDenied => write!(
                formatter,
                "restore state unavailable: SELECT on {RESTORE_MARKER_TABLE} is denied, so statements are refused; \
                 grant SELECT on it to the runtime role (a restore role also needs INSERT and DELETE)"
            ),
            RestoreStateUnavailableReason::ConnectionFailed => write!(
                formatter,
                "restore state unavailable: the database could not be reached to read {RESTORE_MARKER_TABLE}, \
                 so statements are refused; this is not a restore in progress. Check the connection and retry"
            ),
            _ => write!(
                formatter,
                "restore state unavailable: {RESTORE_MARKER_TABLE} could not be read, so statements are refused; \
                 check that MssqlPool::install_restore_marker() ran, that the runtime role has SELECT on it, \
                 and that the table has the documented shape"
            ),
        }
    }
}

impl std::error::Error for RestoreStateUnavailable {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

impl RestoreStateUnavailable {
    /// The value carried by `error`, if that is what it reports.
    pub fn from_error(error: &crate::Error) -> Option<&Self> {
        match error {
            sqlx::Error::Decode(source) => source.downcast_ref::<Self>(),
            _ => None,
        }
    }

    #[cfg_attr(not(feature = "mssql"), allow(dead_code))]
    pub(crate) fn new(
        reason: RestoreStateUnavailableReason,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self { reason, source }
    }

    #[cfg_attr(not(feature = "mssql"), allow(dead_code))]
    pub(crate) fn into_error(self) -> crate::Error {
        sqlx::Error::Decode(Box::new(self))
    }
}

/// Read the marker. Backends that restore transactionally have none, and a
/// SQL Server database with missing or unreadable infrastructure fails closed.
pub(crate) async fn read_restore_marker<B: OrmBackend>(
    pool: &B::Pool,
) -> crate::Result<Option<RestoreInProgress>> {
    #[cfg(feature = "mssql")]
    if let Some(pool) = (pool as &dyn std::any::Any).downcast_ref::<crate::db::mssql::MssqlPool>() {
        // Read outside the restore gate: this must answer during a restore.
        return Ok(pool
            .read_restore_marker()
            .await?
            .map(|(restore_id, started_at)| RestoreInProgress::new(restore_id, started_at)));
    }
    let _ = pool;
    Ok(None)
}

/// Fail with [`RestoreInProgress`] while a marker is present.
pub(crate) async fn refuse_while_restoring<B: OrmBackend>(pool: &B::Pool) -> crate::Result<()> {
    match read_restore_marker::<B>(pool).await? {
        Some(marker) => Err(marker.into_error()),
        None => Ok(()),
    }
}

impl<B: OrmBackend> crate::db::Database<B> {
    /// The host-run restore whose marker is present on this database, if any.
    ///
    /// Always `None` on PostgreSQL and SQLite, where a snapshot restores in
    /// one transaction and no marker is kept.
    ///
    /// # Errors
    ///
    /// Fails with an error carrying [`RestoreStateUnavailable`] when the
    /// marker table is missing, unreadable or the database cannot be reached;
    /// that is not a restore in progress.
    pub async fn restore_in_progress(&self) -> crate::Result<Option<RestoreInProgress>> {
        read_restore_marker::<B>(self.pool()).await
    }

    /// Refuse to continue while a host-run restore's marker is present.
    ///
    /// SQL Server statements are refused during a restore whether or not this
    /// is called; it gives a service a single check to make when it starts.
    /// [`SchemaManager::validate_against_entities`](super::SchemaManager::validate_against_entities)
    /// and [`SchemaManager::validate_target`](super::SchemaManager::validate_target)
    /// make the same check.
    ///
    /// # Errors
    ///
    /// Fails with an error carrying [`RestoreInProgress`] (see
    /// [`RestoreInProgress::from_error`]) while a marker is present, with an
    /// error carrying [`RestoreStateUnavailable`] when the marker table is
    /// missing, unreadable or the database cannot be reached, or with a
    /// database failure.
    pub async fn ensure_no_restore_in_progress(&self) -> crate::Result<()> {
        refuse_while_restoring::<B>(self.pool()).await
    }
}

#[cfg(feature = "mssql")]
impl crate::db::Database<super::MssqlBackend> {
    /// Begin a host-run restore of this SQL Server database.
    ///
    /// Waits up to `wait` for the restore lock: every ORM transaction and
    /// autocommit statement in flight, in any process, finishes first, and
    /// from then on each new one is refused with [`RestoreInProgress`] before
    /// it reads or writes. The marker row for `restore_id` is then recorded.
    /// Run the restore's statements through the returned
    /// [`MssqlRestore`](crate::db::mssql::MssqlRestore) and call its `finish`.
    ///
    /// If the process dies first, the lock is released with its session and
    /// the marker stays: statements remain refused until
    /// [`Self::resume_restore`] adopts the marker and the restore is finished.
    ///
    /// The marker is one row in `dbo.__graphql_orm_restore_marker`.
    /// Before runtime use, the schema owner must install this infrastructure
    /// with `MssqlPool::install_restore_marker`. Runtime roles need SELECT;
    /// the restore role additionally needs INSERT and DELETE.
    ///
    /// # Errors
    ///
    /// Fails with an error carrying [`RestoreInProgress`] when the lock is
    /// not granted within `wait` or another restore's marker is present, with
    /// an error carrying [`RestoreStateUnavailable`] when the marker table is
    /// missing or the role may not use it, and with a database failure
    /// otherwise.
    pub async fn begin_restore(
        &self,
        restore_id: &str,
        wait: std::time::Duration,
    ) -> crate::Result<crate::db::mssql::MssqlRestore> {
        self.pool().begin_restore(restore_id, wait, false).await
    }

    /// Take the restore lock again for the restore `restore_id`, whose marker
    /// a crashed or abandoned restore left behind, to complete it or remove
    /// its partial rows.
    ///
    /// # Errors
    ///
    /// Fails when no marker is present, with an error carrying
    /// [`RestoreInProgress`] when the marker belongs to another restore or
    /// the lock is not granted within `wait`, with an error carrying
    /// [`RestoreStateUnavailable`] when the marker table is missing or the
    /// role may not use it, and with a database failure otherwise.
    pub async fn resume_restore(
        &self,
        restore_id: &str,
        wait: std::time::Duration,
    ) -> crate::Result<crate::db::mssql::MssqlRestore> {
        self.pool().begin_restore(restore_id, wait, true).await
    }
}
