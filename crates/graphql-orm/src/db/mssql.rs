mod notifications;
#[cfg(test)]
#[allow(dead_code)]
#[path = "../../tests/support/owned_mssql.rs"]
pub(crate) mod owned_mssql;
mod transport;
pub use notifications::{
    MssqlBrokerQueue, MssqlQueryNotificationConnection, MssqlQueryNotificationKind,
    MssqlQueryNotificationMessage,
};
pub use transport::MssqlTransportPolicy;

use crate::graphql::orm::{
    BackupRestoreExternallyManaged, DatabaseBackend, MssqlParameterType, SqlTokenKind, SqlValue,
    TemporalKind, TemporalValue, scan_sql,
};
use crate::temporal;
use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tiberius::{ColumnData, Config, Query};
use time::{Date, OffsetDateTime, PrimitiveDateTime, Time};
use tokio::net::TcpStream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

pub type MssqlClient = tiberius::Client<Compat<TcpStream>>;

/// Physical access mode of a SQL Server pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MssqlAccessMode {
    /// Queries only. This remains the default for every compatibility constructor.
    ReadOnly,
    /// Deliberately enabled entity DML against an externally managed schema.
    ExternalWritable,
}

/// Exact affected-row result returned by SQL Server DML.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MssqlWriteResult {
    rows_affected: u64,
}

impl MssqlWriteResult {
    pub(crate) fn rows_affected(self) -> u64 {
        self.rows_affected
    }
}

/// Session state every pooled connection is returned to.
///
/// It runs when a connection is opened, in the same batch that ends each
/// transaction, before a connection is returned after a non-fatal server
/// error, and before an idle connection is reused after
/// [`MssqlPoolOptions::health_check_after_idle`]. Each transaction's begin
/// batch also rolls back any transaction left open first. Isolation level persists on a
/// SQL Server session until changed, so a `SERIALIZABLE` transaction would
/// otherwise leak onto every later read. A server, database or login default
/// of `NOCOUNT ON` or `IMPLICIT_TRANSACTIONS ON` would make every affected-row
/// count 0 or leave autocommit writes uncommitted.
const SESSION_RESET: &str = "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION; \
    SET TRANSACTION ISOLATION LEVEL READ COMMITTED; \
    SET NOCOUNT OFF; \
    SET IMPLICIT_TRANSACTIONS OFF; \
    WHILE APPLOCK_MODE('public', N'graphql_orm_restore', 'Session') <> 'NoLock' \
    EXEC sp_releaseapplock @Resource = N'graphql_orm_restore', @LockOwner = 'Session'; \
    WHILE APPLOCK_MODE('public', N'graphql_orm_restore_marker_install', 'Session') <> 'NoLock' \
    EXEC sp_releaseapplock @Resource = N'graphql_orm_restore_marker_install', @LockOwner = 'Session';";

/// Ends an ORM transaction. A `COMMIT` that leaves `@@TRANCOUNT` above zero
/// only decremented a nested transaction; nothing is durable yet. Roll back
/// and raise rather than let the reset's rollback discard work that would
/// otherwise be reported as committed.
const COMMIT_TRANSACTION: &str = "COMMIT TRANSACTION; \
    IF @@TRANCOUNT > 0 BEGIN \
        ROLLBACK TRANSACTION; \
        THROW 50000, N'SQL Server transaction was nested; its commit was not durable and was rolled back', 1; \
    END;";

/// Connection bounds and transport policy for a SQL Server pool.
///
/// The driver cannot cancel a running statement, so a statement that exceeds
/// [`query_timeout`](Self::query_timeout) closes its connection; SQL Server then
/// rolls back any open transaction on that session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MssqlPoolOptions {
    max_connections: usize,
    acquire_timeout: Duration,
    connect_timeout: Duration,
    query_timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
    max_lifetime: Option<Duration>,
    health_check_after_idle: Option<Duration>,
    transport: MssqlTransportPolicy,
    typed_temporal_parameters: bool,
    typed_parameters: bool,
}

impl Default for MssqlPoolOptions {
    fn default() -> Self {
        Self {
            max_connections: 5,
            acquire_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(15),
            query_timeout: None,
            idle_timeout: Some(Duration::from_secs(10 * 60)),
            max_lifetime: Some(Duration::from_secs(30 * 60)),
            health_check_after_idle: Some(Duration::from_secs(30)),
            transport: MssqlTransportPolicy::verified(),
            typed_temporal_parameters: false,
            typed_parameters: false,
        }
    }
}

impl MssqlPoolOptions {
    /// Default bounds: 5 connections, a 30 s acquire timeout, a 15 s connect
    /// timeout, no statement timeout, a 10 min idle timeout, a 30 min maximum
    /// lifetime, a health check after 30 s idle, and verified transport.
    pub fn new() -> Self {
        Self::default()
    }

    /// Maximum open connections, at least 1.
    pub fn max_connections(mut self, max_connections: usize) -> Self {
        self.max_connections = max_connections.max(1);
        self
    }

    /// Bound on waiting for a connection, including opening a new one.
    /// Exceeding it returns [`sqlx::Error::PoolTimedOut`].
    pub fn acquire_timeout(mut self, timeout: Duration) -> Self {
        self.acquire_timeout = timeout;
        self
    }

    /// Bound on opening one connection: TCP, TLS, login and session setup,
    /// and on the health check of an idle connection.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Bound on each statement, or `None` for no bound. A statement that
    /// exceeds it fails and its connection is closed.
    pub fn query_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.query_timeout = timeout;
        self
    }

    /// Close connections idle for longer than this, or `None` to keep them.
    pub fn idle_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.idle_timeout = timeout;
        self
    }

    /// Close connections older than this when they are next released or
    /// acquired, or `None` for no limit.
    pub fn max_lifetime(mut self, lifetime: Option<Duration>) -> Self {
        self.max_lifetime = lifetime;
        self
    }

    /// Reset and validate a connection that has been idle at least this long
    /// before reusing it, or `None` to reuse idle connections unchecked.
    pub fn health_check_after_idle(mut self, idle: Option<Duration>) -> Self {
        self.health_check_after_idle = idle;
        self
    }

    /// Transport security every connection must meet.
    pub fn transport(mut self, policy: MssqlTransportPolicy) -> Self {
        self.transport = policy;
        self
    }

    /// Bind temporal text values as native SQL Server parameters for this pool.
    ///
    /// Chrono values always bind natively. The default sends temporal text as
    /// written. Native date, datetime2 and datetimeoffset parameters avoid
    /// session-dependent text conversion. Legacy datetime columns can round
    /// and compare differently.
    pub fn typed_temporal_parameters(mut self, enabled: bool) -> Self {
        self.typed_temporal_parameters = enabled;
        self
    }

    /// Whether temporal text values use native SQL Server parameters.
    pub fn get_typed_temporal_parameters(&self) -> bool {
        self.typed_temporal_parameters
    }

    /// Opt in to binding declared SQL Server types for generated scalar parameters.
    ///
    /// Printable ASCII operands in ordinary VARCHAR comparisons may use a
    /// parameter-side conversion. Pattern matching, non-ASCII text, and
    /// authorization predicates retain Unicode binding. Disabled by default.
    pub fn typed_parameters(mut self, enabled: bool) -> Self {
        self.typed_parameters = enabled;
        self
    }

    /// Whether generated scalar parameters use their declared SQL Server types.
    pub fn get_typed_parameters(&self) -> bool {
        self.typed_parameters
    }

    /// Configured maximum open connections.
    pub fn get_max_connections(&self) -> usize {
        self.max_connections
    }

    /// Configured acquire timeout.
    pub fn get_acquire_timeout(&self) -> Duration {
        self.acquire_timeout
    }

    /// Configured connect timeout.
    pub fn get_connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    /// Configured statement timeout.
    pub fn get_query_timeout(&self) -> Option<Duration> {
        self.query_timeout
    }

    /// Configured idle timeout.
    pub fn get_idle_timeout(&self) -> Option<Duration> {
        self.idle_timeout
    }

    /// Configured maximum connection lifetime.
    pub fn get_max_lifetime(&self) -> Option<Duration> {
        self.max_lifetime
    }

    /// Configured idle period after which a connection is checked before reuse.
    pub fn get_health_check_after_idle(&self) -> Option<Duration> {
        self.health_check_after_idle
    }

    /// Configured transport policy.
    pub fn get_transport(&self) -> MssqlTransportPolicy {
        self.transport
    }
}

/// One connection-pinned SQL Server transaction.
///
/// The client is returned to the pool only after a successful explicit commit
/// or rollback, which also resets its session. Cancellation, protocol errors,
/// timeouts and drop discard the socket.
///
/// A statement the server refused (a constraint violation, a deadlock) keeps
/// the connection but fails the transaction: later statements and `commit` are
/// refused, and `rollback` ends whatever the server left open. Rolling back a
/// transaction whose connection was already discarded is a no-op, because SQL
/// Server rolled it back when the session closed.
pub struct MssqlTransaction {
    pool: MssqlPool,
    lease: Option<Lease>,
    failed: bool,
}

/// An error SQL Server reported for a statement.
///
/// Carried inside [`sqlx::Error::Database`], so `as_database_error()`, `code()`
/// (the error number as text) and `kind()` classify it like any other backend:
/// 2627 and 2601 are unique violations, 547 is a foreign-key or check
/// violation and 515 is a not-null violation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MssqlServerError {
    number: u32,
    state: u8,
    class: u8,
    message: String,
}

impl MssqlServerError {
    /// Build an error from the fields of a server error token.
    pub fn new(number: u32, state: u8, class: u8, message: impl Into<String>) -> Self {
        Self {
            number,
            state,
            class,
            message: message.into(),
        }
    }

    /// SQL Server error number, such as 2627 or 1205.
    pub fn number(&self) -> u32 {
        self.number
    }

    /// Error state reported with the number.
    pub fn state(&self) -> u8 {
        self.state
    }

    /// Severity class. The server closes the session at 20 and above.
    pub fn class(&self) -> u8 {
        self.class
    }

    /// Whether running the whole transaction again can succeed.
    ///
    /// Deadlock victim (1205), lock request timeout (1222), snapshot update
    /// conflict (3960), the memory-optimized table conflicts (41301, 41302,
    /// 41305, 41325, 41839), and the service-busy and failover errors 40501,
    /// 40197 and 40613. Constraint and truncation errors are never retried.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.number,
            1205 | 1222 | 3960 | 41301 | 41302 | 41305 | 41325 | 41839 | 40501 | 40197 | 40613
        )
    }
}

impl std::fmt::Display for MssqlServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (code: {}, state: {}, class: {})",
            self.message, self.number, self.state, self.class
        )
    }
}

impl std::error::Error for MssqlServerError {}

impl sqlx::error::DatabaseError for MssqlServerError {
    fn message(&self) -> &str {
        &self.message
    }

    fn code(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Owned(self.number.to_string()))
    }

    fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        self
    }

    fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
        self
    }

    fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
        self
    }

    fn kind(&self) -> sqlx::error::ErrorKind {
        use sqlx::error::ErrorKind;
        match self.number {
            2627 | 2601 => ErrorKind::UniqueViolation,
            515 => ErrorKind::NotNullViolation,
            // 547 is raised for FOREIGN KEY, REFERENCE and CHECK conflicts
            // only; the message names which one.
            547 if self.message.contains("CHECK") => ErrorKind::CheckViolation,
            547 => ErrorKind::ForeignKeyViolation,
            _ => ErrorKind::Other,
        }
    }
}

#[derive(Clone)]
pub struct MssqlPool {
    inner: Arc<MssqlPoolInner>,
    // The policy travels with the Database's pool handle. Other handles may
    // share the connection pool without inheriting a change to their policy.
    pub(super) externally_managed_backup_restore: bool,
}

struct MssqlPoolInner {
    config: Config,
    access_mode: MssqlAccessMode,
    options: MssqlPoolOptions,
    idle: Mutex<Vec<IdleClient>>,
    permits: Arc<Semaphore>,
}

struct IdleClient {
    client: MssqlClient,
    created: Instant,
    released: Instant,
}

/// A checked-out connection and the pool permit it holds.
struct Lease {
    client: MssqlClient,
    created: Instant,
    permit: OwnedSemaphorePermit,
}

/// A failed statement, and whether its connection is still in a known state.
struct StatementFailure {
    error: sqlx::Error,
    reusable: bool,
}

impl From<sqlx::Error> for StatementFailure {
    fn from(error: sqlx::Error) -> Self {
        Self {
            error,
            reusable: false,
        }
    }
}

pub struct MssqlRow {
    inner: tiberius::Row,
}

enum MssqlParamValue {
    String(String),
    Bytes(Vec<u8>),
    Uuid(uuid::Uuid),
    Int(i64),
    Float(f64),
    Bool(bool),
    Decimal(rust_decimal::Decimal),
    Native(ColumnData<'static>),
    NullString,
    NullBytes,
    NullUuid,
    NullInt,
    NullFloat,
    NullBool,
    NullDecimal,
}

pub trait MssqlColumnIndex: Copy {
    fn try_get_raw<'a, T>(self, row: &'a tiberius::Row) -> tiberius::Result<Option<T>>
    where
        T: tiberius::FromSql<'a>;

    fn display(self) -> String;
}

pub trait MssqlScalar: Sized {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
    where
        I: MssqlColumnIndex;
}

pub trait MssqlDecode: Sized {
    fn try_get<I>(row: &MssqlRow, index: I) -> crate::Result<Self>
    where
        I: MssqlColumnIndex;
}

impl MssqlPool {
    /// Open a read-only pool from an ADO connection string with default options.
    ///
    /// The connection string must meet [`MssqlTransportPolicy::verified`]:
    /// full-session TLS and a validated server certificate. An omitted
    /// `Encrypt` key means `Encrypt=true`.
    pub async fn connect_ado(connection_string: &str) -> crate::Result<Self> {
        Self::connect_ado_with(connection_string, MssqlPoolOptions::default()).await
    }

    /// Open a read-only pool from an ADO connection string.
    pub async fn connect_ado_with(
        connection_string: &str,
        options: MssqlPoolOptions,
    ) -> crate::Result<Self> {
        let config = transport::config_from_ado(connection_string, options.transport)?;
        Ok(Self::with_access_mode(
            config,
            options,
            MssqlAccessMode::ReadOnly,
        ))
    }

    /// Open a pool that deliberately permits DML against an externally managed schema.
    pub async fn connect_ado_external_writable(connection_string: &str) -> crate::Result<Self> {
        Self::connect_ado_external_writable_with(connection_string, MssqlPoolOptions::default())
            .await
    }

    /// Open a deliberately writable pool from an ADO connection string.
    pub async fn connect_ado_external_writable_with(
        connection_string: &str,
        options: MssqlPoolOptions,
    ) -> crate::Result<Self> {
        let config = transport::config_from_ado(connection_string, options.transport)?;
        Ok(Self::with_access_mode(
            config,
            options,
            MssqlAccessMode::ExternalWritable,
        ))
    }

    /// Construct a read-only pool from a caller-built driver configuration.
    ///
    /// Its encryption level is raised to `Required` unless the default
    /// transport policy is replaced; see [`MssqlTransportPolicy`].
    pub fn new(config: Config) -> Self {
        Self::with_options(config, MssqlPoolOptions::default())
    }

    pub fn with_max_connections(config: Config, max_connections: usize) -> Self {
        Self::with_options(
            config,
            MssqlPoolOptions::default().max_connections(max_connections),
        )
    }

    /// Construct a read-only pool from a caller-built driver configuration.
    pub fn with_options(config: Config, options: MssqlPoolOptions) -> Self {
        Self::with_access_mode(config, options, MssqlAccessMode::ReadOnly)
    }

    /// Construct a deliberately writable pool for an externally managed schema.
    pub fn new_external_writable(config: Config) -> Self {
        Self::with_options_external_writable(config, MssqlPoolOptions::default())
    }

    /// Construct a deliberately writable pool with a bounded connection count.
    pub fn with_max_connections_external_writable(config: Config, max_connections: usize) -> Self {
        Self::with_options_external_writable(
            config,
            MssqlPoolOptions::default().max_connections(max_connections),
        )
    }

    /// Construct a deliberately writable pool from a caller-built driver configuration.
    pub fn with_options_external_writable(config: Config, options: MssqlPoolOptions) -> Self {
        Self::with_access_mode(config, options, MssqlAccessMode::ExternalWritable)
    }

    fn with_access_mode(
        mut config: Config,
        options: MssqlPoolOptions,
        access_mode: MssqlAccessMode,
    ) -> Self {
        config.readonly(matches!(access_mode, MssqlAccessMode::ReadOnly));
        options.transport.apply(&mut config);
        let options = MssqlPoolOptions {
            max_connections: options.max_connections.max(1),
            ..options
        };
        Self {
            externally_managed_backup_restore: false,
            inner: Arc::new(MssqlPoolInner {
                config,
                access_mode,
                permits: Arc::new(Semaphore::new(options.max_connections)),
                options,
                idle: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Report the immutable physical access mode.
    pub fn access_mode(&self) -> MssqlAccessMode {
        self.inner.access_mode
    }

    /// Report the pool's bounds and transport policy.
    pub fn options(&self) -> &MssqlPoolOptions {
        &self.inner.options
    }

    /// Fetch rows in autocommit. With restore fencing enabled, the statement holds
    /// the shared restore lock while it runs and is refused with
    /// [`RestoreInProgress`](crate::graphql::orm::RestoreInProgress) during a
    /// host-run restore; see [`MssqlRestore`].
    pub async fn fetch_rows(&self, sql: &str, values: &[SqlValue]) -> crate::Result<Vec<MssqlRow>> {
        if self.externally_managed_backup_restore {
            return self.fetch_rows_ungated(sql, values).await;
        }
        let result = self
            .fetch_rows_ungated(&restore_gated(sql, true), values)
            .await;
        self.typed_restore_refusal(result).await
    }

    /// Fetch rows without taking the restore lock: for the restore machinery
    /// itself, which must work while a restore is in progress.
    async fn fetch_rows_ungated(
        &self,
        sql: &str,
        values: &[SqlValue],
    ) -> crate::Result<Vec<MssqlRow>> {
        let mut lease = self.acquire().await?;
        let result = self
            .bounded(fetch_rows_with_client(
                &mut lease.client,
                sql,
                values,
                self.options(),
                true,
            ))
            .await;
        self.finish(lease, result).await
    }

    pub(crate) async fn execute(
        &self,
        sql: &str,
        values: &[SqlValue],
    ) -> crate::Result<MssqlWriteResult> {
        self.require_external_writable()?;
        // The statement must be the last of its batch: its row count is the
        // batch's last one. The shared lock is therefore released by a
        // control batch that follows it, before the connection is reused.
        let statement = if self.externally_managed_backup_restore {
            Cow::Borrowed(sql)
        } else {
            Cow::Owned(restore_gated(sql, false))
        };
        let mut lease = self.acquire().await?;
        let result = self
            .bounded(execute_with_client(
                &mut lease.client,
                &statement,
                values,
                self.options(),
            ))
            .await;
        if !self.externally_managed_backup_restore
            && result.is_ok()
            && let Err(failure) = self
                .bounded(execute_control_statement(
                    &mut lease.client,
                    RELEASE_SESSION_RESTORE_LOCK,
                ))
                .await
        {
            // The write is done, but this session may still hold the lock:
            // close it rather than return it to the pool.
            drop((lease, failure));
            return result.map_err(|failure| failure.error);
        }
        // A refused or failed statement goes through the session reset in
        // `finish`, which releases the lock.
        let result = self.finish(lease, result).await;
        self.typed_restore_refusal(result).await
    }

    /// Replace the server's refusal of the restore gate with a typed error:
    /// [`RestoreInProgress`](crate::graphql::orm::RestoreInProgress) when a
    /// restore holds the lock or its marker is present, and
    /// [`RestoreStateUnavailable`](crate::graphql::orm::RestoreStateUnavailable)
    /// when the gate could not read the marker at all.
    async fn typed_restore_refusal<T>(&self, result: crate::Result<T>) -> crate::Result<T> {
        if self.externally_managed_backup_restore {
            return result;
        }
        match result {
            Err(error) => match restore_refusal(&error) {
                Some(RestoreRefusal::InProgress) => Err(self.restore_in_progress_error().await),
                Some(RestoreRefusal::Unavailable(reason)) => {
                    Err(restore_state_unavailable(reason, error))
                }
                None => Err(error),
            },
            ok => ok,
        }
    }

    /// The refusal for a gate that saw a restore: its lock was held or its
    /// marker present. The marker, read again, names the restore. A restore
    /// that holds the lock but has not recorded its marker yet has no
    /// identifier to report, and neither has one whose marker cannot be read
    /// now (a timeout, or a role denied SELECT): the gate has already seen
    /// the restore, so the refusal stays `RestoreInProgress` and never says
    /// that no restore is running.
    async fn restore_in_progress_error(&self) -> sqlx::Error {
        let (restore_id, started_at) = self
            .read_restore_marker()
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        crate::graphql::orm::RestoreInProgress::new(restore_id, started_at).into_error()
    }

    /// Install the canonical infrastructure table as the schema owner, before
    /// using the ORM. Runtime roles need SELECT; restore roles also need INSERT
    /// and DELETE. This does not clear an existing marker.
    ///
    /// Idempotent and safe to run from several replicas at once: installs
    /// serialise on an application lock, a table created by someone else in
    /// the meantime (error 2714) counts as installed, and the table's shape is
    /// verified afterwards, so a table of the wrong shape fails here rather
    /// than at runtime.
    ///
    /// Needs a pool built with the explicit external-writable constructor.
    /// Returns [`BackupRestoreExternallyManaged`] for an externally managed
    /// backup and restore configuration, before acquiring a connection.
    pub async fn install_restore_marker(&self) -> crate::Result<()> {
        BackupRestoreExternallyManaged::ensure_allowed(self.externally_managed_backup_restore)?;
        self.require_external_writable()?;
        let mut lease = self.acquire().await?;
        let result = self
            .bounded(execute_with_client(
                &mut lease.client,
                CREATE_RESTORE_MARKER,
                &[],
                self.options(),
            ))
            .await;
        self.finish(lease, result).await.map(|_| ())
    }

    /// The restore marker row. Unreadable or missing infrastructure fails
    /// closed with [`RestoreStateUnavailable`](crate::graphql::orm::RestoreStateUnavailable),
    /// which says why.
    pub(crate) async fn read_restore_marker(&self) -> crate::Result<Option<(String, i64)>> {
        BackupRestoreExternallyManaged::ensure_allowed(self.externally_managed_backup_restore)?;
        let rows = self
            .fetch_rows_ungated(READ_RESTORE_MARKER, &[])
            .await
            .map_err(|error| restore_state_unavailable(classify_marker_error(&error), error))?;
        rows.first()
            .map(restore_marker_from_row)
            .transpose()
            .map_err(|error| {
                restore_state_unavailable(
                    crate::graphql::orm::RestoreStateUnavailableReason::Other,
                    error,
                )
            })
    }

    /// Take the exclusive restore lock on a dedicated connection and record
    /// (or, with `resume`, re-adopt) the marker of `restore_id`.
    pub(crate) async fn begin_restore(
        &self,
        restore_id: &str,
        wait: Duration,
        resume: bool,
    ) -> crate::Result<MssqlRestore> {
        BackupRestoreExternallyManaged::ensure_allowed(self.externally_managed_backup_restore)?;
        self.require_external_writable()?;
        let mut lease = self.acquire().await?;
        let wait_ms = i32::try_from(wait.as_millis()).unwrap_or(i32::MAX);
        let lock = format!(
            "EXEC sp_getapplock @Resource = N'{RESTORE_LOCK_RESOURCE}', @LockMode = 'Exclusive', @LockOwner = 'Session', @LockTimeout = {wait_ms}; \
             IF APPLOCK_MODE('public', N'{RESTORE_LOCK_RESOURCE}', 'Session') <> 'Exclusive' BEGIN \
             RAISERROR(N'{RESTORE_REFUSAL}: open transactions or another restore hold the restore lock', 16, 1); RETURN; END;"
        );
        // The lock wait is the caller's bound, not the pool's query timeout.
        // On any failure the lease is dropped, which closes the connection
        // and with it releases whatever the session held.
        let locked = tokio::time::timeout(
            wait + self.inner.options.connect_timeout,
            execute_control_statement(&mut lease.client, &lock),
        )
        .await
        .map_err(|_| timed_out("SQL Server restore lock was not granted in time"))?;
        if let Err(failure) = locked {
            drop(lease);
            return self.typed_restore_refusal(Err(failure.error)).await;
        }
        let claimed = async {
            let id = [SqlValue::String(restore_id.to_string())];
            if !resume {
                execute_with_client(
                    &mut lease.client,
                    INSERT_RESTORE_MARKER,
                    &id,
                    self.options(),
                )
                .await?;
            }
            fetch_rows_with_client(
                &mut lease.client,
                READ_RESTORE_MARKER,
                &[],
                self.options(),
                false,
            )
            .await
        };
        let rows = self
            .bounded(claimed)
            .await
            .map_err(|failure| marker_access_error(failure.error))?;
        match rows.first().map(restore_marker_from_row).transpose()? {
            Some((present, _)) if present == restore_id => Ok(MssqlRestore {
                pool: self.clone(),
                lease: Some(lease),
                restore_id: restore_id.to_string(),
            }),
            // Another restore's marker: one that crashed, or was abandoned.
            Some((present, started_at)) => {
                Err(crate::graphql::orm::RestoreInProgress::new(present, started_at).into_error())
            }
            None => Err(sqlx::Error::Protocol(format!(
                "no restore marker is present for restore {restore_id}"
            ))),
        }
    }

    pub(crate) async fn begin_transaction(
        &self,
        mode: crate::graphql::orm::TransactionMode,
    ) -> crate::Result<MssqlTransaction> {
        self.require_external_writable()?;
        let lease = self.acquire().await?;
        let isolation = match mode {
            crate::graphql::orm::TransactionMode::Default => "READ COMMITTED",
            crate::graphql::orm::TransactionMode::StateMachine => "SERIALIZABLE",
        };
        self.begin_transaction_on_lease(lease, isolation).await
    }

    pub(crate) async fn begin_read_snapshot(
        &self,
    ) -> Result<MssqlTransaction, crate::graphql::orm::ReadSnapshotError> {
        use crate::graphql::orm::ReadSnapshotError;

        let classify = classify_read_snapshot_start_error;
        let mut lease = self.acquire().await.map_err(classify)?;
        let rows = self
            .bounded(fetch_rows_with_client(
                &mut lease.client,
                "SELECT CAST(snapshot_isolation_state AS INT) AS snapshot_state \
                 FROM sys.databases WHERE database_id = DB_ID()",
                &[],
                self.options(),
                false,
            ))
            .await
            .map_err(|failure| classify(failure.error))?;
        let state = rows
            .first()
            .map(|row| row.try_get::<i32, _>("snapshot_state"))
            .transpose()
            .map_err(classify)?;
        if state != Some(1) {
            self.release(lease);
            return Err(ReadSnapshotError::MssqlSnapshotIsolationRequired);
        }
        self.begin_transaction_on_lease(lease, "SNAPSHOT")
            .await
            .map_err(classify)
    }

    async fn begin_transaction_on_lease(
        &self,
        mut lease: Lease,
        isolation: &str,
    ) -> crate::Result<MssqlTransaction> {
        // Never nest inside a transaction raw SQL left open on this session:
        // its COMMIT would only decrement the count and nothing would be durable.
        // The transaction takes the shared restore lock in the batch that
        // begins it and holds it until it ends, so a restore cannot start
        // under it and it cannot start under a restore.
        let gate = if self.externally_managed_backup_restore {
            String::new()
        } else {
            restore_gate(
                "Transaction",
                "ROLLBACK TRANSACTION;",
                isolation == "SNAPSHOT",
            )
        };
        let begin = format!(
            "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION; \
             SET TRANSACTION ISOLATION LEVEL {isolation}; BEGIN TRANSACTION; {gate}",
        );
        // A failed begin may have changed the isolation level; never reuse it.
        let begun = self
            .bounded(execute_control_statement(&mut lease.client, &begin))
            .await;
        if let Err(failure) = begun {
            drop(lease);
            return self.typed_restore_refusal(Err(failure.error)).await;
        }
        Ok(MssqlTransaction {
            pool: self.clone(),
            lease: Some(lease),
            failed: false,
        })
    }

    fn require_external_writable(&self) -> crate::Result<()> {
        crate::graphql::orm::ensure_sql_write_allowed()?;
        if self.access_mode() == MssqlAccessMode::ExternalWritable {
            Ok(())
        } else {
            Err(sqlx::Error::Protocol(
                "SQL Server pool is physically read-only; use the explicit external-writable constructor"
                    .to_string(),
            ))
        }
    }

    /// Check out a connection within the acquire timeout.
    async fn acquire(&self) -> crate::Result<Lease> {
        tokio::time::timeout(self.inner.options.acquire_timeout, self.acquire_unbounded())
            .await
            .map_err(|_| sqlx::Error::PoolTimedOut)?
    }

    async fn acquire_unbounded(&self) -> crate::Result<Lease> {
        let permit = self
            .inner
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        while let Some(idle) = self.take_idle() {
            let IdleClient {
                mut client,
                created,
                released,
            } = idle;
            let checked = match self.inner.options.health_check_after_idle {
                Some(after) if released.elapsed() >= after => self.reset(&mut client).await,
                _ => true,
            };
            if checked {
                return Ok(Lease {
                    client,
                    created,
                    permit,
                });
            }
        }
        let client = self.connect().await?;
        Ok(Lease {
            client,
            created: Instant::now(),
            permit,
        })
    }

    /// Take the most recently released live connection, closing expired ones.
    fn take_idle(&self) -> Option<IdleClient> {
        let options = &self.inner.options;
        let mut idle = self
            .inner
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        idle.retain(|client| {
            !options
                .idle_timeout
                .is_some_and(|timeout| client.released.elapsed() >= timeout)
                && !options
                    .max_lifetime
                    .is_some_and(|lifetime| client.created.elapsed() >= lifetime)
        });
        idle.pop()
    }

    /// Open one connection and reset its session within the connect timeout.
    async fn connect(&self) -> crate::Result<MssqlClient> {
        let config = self.inner.config.clone();
        let connect = async move {
            let mut client = open_client(config).await?;
            execute_control_statement(&mut client, SESSION_RESET)
                .await
                .map_err(|failure| failure.error)?;
            Ok(client)
        };
        tokio::time::timeout(self.inner.options.connect_timeout, connect)
            .await
            .map_err(|_| {
                timed_out("SQL Server connection exceeded the configured connect timeout")
            })?
    }

    /// Bound one statement by the query timeout. A timed-out statement cannot
    /// be cancelled on the wire, so its connection is not reusable.
    async fn bounded<T>(
        &self,
        statement: impl std::future::Future<Output = Result<T, StatementFailure>>,
    ) -> Result<T, StatementFailure> {
        match self.inner.options.query_timeout {
            None => statement.await,
            Some(timeout) => tokio::time::timeout(timeout, statement)
                .await
                .unwrap_or_else(|_| {
                    Err(
                        timed_out("SQL Server statement exceeded the configured query timeout")
                            .into(),
                    )
                }),
        }
    }

    /// Return an autocommit connection to the pool unless its state is unknown.
    ///
    /// After a non-fatal server error the batch may have left a transaction
    /// open (`XACT_ABORT` is off), so the session is reset before it is
    /// released, and closed if the reset fails.
    async fn finish<T>(
        &self,
        mut lease: Lease,
        result: Result<T, StatementFailure>,
    ) -> crate::Result<T> {
        match result {
            Ok(value) => {
                self.release(lease);
                Ok(value)
            }
            Err(failure) => {
                if failure.reusable && self.reset(&mut lease.client).await {
                    self.release(lease);
                }
                Err(failure.error)
            }
        }
    }

    /// Reset a session within the connect timeout; `false` means close it.
    async fn reset(&self, client: &mut MssqlClient) -> bool {
        tokio::time::timeout(
            self.inner.options.connect_timeout,
            execute_control_statement(client, SESSION_RESET),
        )
        .await
        .is_ok_and(|result| result.is_ok())
    }

    fn release(&self, lease: Lease) {
        let Lease {
            client,
            created,
            permit,
        } = lease;
        let expired = self
            .inner
            .options
            .max_lifetime
            .is_some_and(|lifetime| created.elapsed() >= lifetime);
        if !expired {
            self.inner
                .idle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(IdleClient {
                    client,
                    created,
                    released: Instant::now(),
                });
        }
        // Release the permit only once the connection is back in the idle list.
        drop(permit);
    }
}

/// Application lock every ORM transaction and autocommit statement takes in
/// shared mode, and a restore takes exclusively.
const RESTORE_LOCK_RESOURCE: &str = "graphql_orm_restore";

/// How long an ORM statement waits for the shared restore lock, in
/// milliseconds, before it is refused. Shared holders never block each other,
/// so the wait only occurs while a restore holds or is waiting for the lock.
const RESTORE_LOCK_WAIT_MS: u32 = 250;

/// Prefix of the message the restore gate raises.
const RESTORE_REFUSAL: &str = "graphql-orm restore in progress";

/// Prefix of the message the restore gate raises when it cannot read the
/// marker. The server error number follows `error `.
const RESTORE_UNAVAILABLE: &str = "graphql-orm restore state unavailable";

const READ_RESTORE_MARKER: &str =
    "SELECT restore_id, started_at FROM [dbo].[__graphql_orm_restore_marker]";

/// Install batch for the marker table: serialised on a session application
/// lock (not a transaction, which error 2714 would doom), tolerant of a
/// concurrent creator (2714), and ending in a shape check.
const CREATE_RESTORE_MARKER: &str = "\
     DECLARE @gorm_install_lock INT, @gorm_install_error INT; \
     EXEC @gorm_install_lock = sp_getapplock @Resource = N'graphql_orm_restore_marker_install', \
         @LockMode = 'Exclusive', @LockOwner = 'Session', @LockTimeout = 30000; \
     IF @gorm_install_lock < 0 \
         THROW 50000, N'graphql-orm restore marker install: another installation held the install lock too long', 1; \
     BEGIN TRY \
         IF OBJECT_ID(N'dbo.__graphql_orm_restore_marker', N'U') IS NULL \
         CREATE TABLE [dbo].[__graphql_orm_restore_marker] (\
         marker_key TINYINT NOT NULL PRIMARY KEY CHECK (marker_key = 1), \
         restore_id NVARCHAR(200) NOT NULL, \
         started_at BIGINT NOT NULL); \
     END TRY \
     BEGIN CATCH \
         SET @gorm_install_error = ERROR_NUMBER(); \
         IF @gorm_install_error <> 2714 BEGIN \
             EXEC sp_releaseapplock @Resource = N'graphql_orm_restore_marker_install', @LockOwner = 'Session'; \
             THROW; \
         END; \
     END CATCH; \
     EXEC sp_releaseapplock @Resource = N'graphql_orm_restore_marker_install', @LockOwner = 'Session'; \
     IF OBJECT_ID(N'dbo.__graphql_orm_restore_marker', N'U') IS NULL \
        OR (SELECT COUNT(*) FROM sys.columns WHERE object_id = OBJECT_ID(N'dbo.__graphql_orm_restore_marker', N'U')) <> 3 \
        OR (SELECT COUNT(*) FROM sys.columns AS c JOIN sys.types AS t ON t.user_type_id = c.user_type_id \
            WHERE c.object_id = OBJECT_ID(N'dbo.__graphql_orm_restore_marker', N'U') AND c.is_nullable = 0 AND ( \
                (c.name = N'marker_key' AND t.name = N'tinyint') \
                OR (c.name = N'restore_id' AND t.name = N'nvarchar' AND c.max_length = 400) \
                OR (c.name = N'started_at' AND t.name = N'bigint'))) <> 3 \
        OR (SELECT COUNT(*) FROM sys.indexes AS i \
            JOIN sys.index_columns AS ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
            JOIN sys.columns AS c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
            WHERE i.object_id = OBJECT_ID(N'dbo.__graphql_orm_restore_marker', N'U') AND i.is_primary_key = 1) <> 1 \
        OR NOT EXISTS (SELECT 1 FROM sys.indexes AS i \
            JOIN sys.index_columns AS ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
            JOIN sys.columns AS c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
            WHERE i.object_id = OBJECT_ID(N'dbo.__graphql_orm_restore_marker', N'U') AND i.is_primary_key = 1 \
              AND c.name = N'marker_key') \
        OR NOT EXISTS (SELECT 1 FROM sys.check_constraints AS k \
            WHERE k.parent_object_id = OBJECT_ID(N'dbo.__graphql_orm_restore_marker', N'U') \
              AND k.is_disabled = 0 AND k.is_not_trusted = 0 \
              AND REPLACE(REPLACE(REPLACE(k.definition, N' ', N''), N'(', N''), N')', N'') = N'[marker_key]=1') \
         THROW 50000, N'dbo.__graphql_orm_restore_marker exists but does not have the documented shape', 1;";

const INSERT_RESTORE_MARKER: &str = "INSERT INTO [dbo].[__graphql_orm_restore_marker] (marker_key, restore_id, started_at) \
     SELECT 1, @P1, DATEDIFF_BIG(SECOND, '1970-01-01', SYSUTCDATETIME()) \
     WHERE NOT EXISTS (SELECT 1 FROM [dbo].[__graphql_orm_restore_marker])";

fn restore_marker_from_row(row: &MssqlRow) -> crate::Result<(String, i64)> {
    Ok((row.try_get("restore_id")?, row.try_get("started_at")?))
}

/// T-SQL that takes the shared restore lock for `owner` (`Session` or
/// `Transaction`) and refuses, after running `undo`, when the lock is not
/// granted or a restore marker is present.
///
/// The lock makes the check race-free: a restore holds the lock exclusively,
/// so it waits for every holder of the shared lock to finish, and nothing
/// takes the shared lock while it runs. The marker covers a restore whose
/// session died: its lock is gone, its partial rows are not.
fn restore_gate(owner: &str, undo: &str, preserve_snapshot_refusal: bool) -> String {
    // A snapshot can be disabled between admission and the marker read.
    // Preserve that server number before the usual marker-error translation.
    let rethrow_snapshot_refusal = if preserve_snapshot_refusal {
        "IF @gorm_restore_error = 3952 BEGIN THROW; END;"
    } else {
        ""
    };
    let release = if owner == "Session" {
        format!(
            "EXEC sp_releaseapplock @Resource = N'{RESTORE_LOCK_RESOURCE}', @LockOwner = 'Session';"
        )
    } else {
        String::new()
    };
    format!(
        "EXEC sp_getapplock @Resource = N'{RESTORE_LOCK_RESOURCE}', @LockMode = 'Shared', @LockOwner = '{owner}', @LockTimeout = {RESTORE_LOCK_WAIT_MS}; \
         IF APPLOCK_MODE('public', N'{RESTORE_LOCK_RESOURCE}', '{owner}') = 'NoLock' BEGIN {undo} \
         RAISERROR(N'{RESTORE_REFUSAL}: the restore lock is held', 16, 1); RETURN; END; \
         DECLARE @gorm_restore_present BIT, @gorm_restore_error INT; BEGIN TRY \
         EXEC sp_executesql N'SELECT @present = CASE WHEN EXISTS (SELECT 1 FROM [dbo].[__graphql_orm_restore_marker]) THEN 1 ELSE 0 END', \
         N'@present BIT OUTPUT', @present = @gorm_restore_present OUTPUT; \
         END TRY BEGIN CATCH SET @gorm_restore_error = ERROR_NUMBER(); {rethrow_snapshot_refusal}{release} {undo} \
         RAISERROR(N'{RESTORE_UNAVAILABLE}: marker read failed with error %d', 16, 1, @gorm_restore_error); RETURN; END CATCH; \
         IF @gorm_restore_present IS NULL OR @gorm_restore_present <> 0 BEGIN {release} {undo} \
         RAISERROR(N'{RESTORE_REFUSAL}: a restore marker is present', 16, 1); RETURN; END;"
    )
}

/// Wrap an autocommit statement so it holds the shared restore lock while it
/// runs. The lock is session-owned. A session that still holds one from a
/// batch that ended early releases it first, and so does the session reset.
///
/// With `release_inline` the batch releases the lock after the statement.
/// That makes the release the batch's last statement, whose row count SQL
/// Server then reports, so a write leaves the release to its caller.
///
/// A statement that must be the first of its batch (`CREATE VIEW` and the
/// like) reads and writes no rows and runs as it is.
fn restore_gated(sql: &str, release_inline: bool) -> String {
    if requires_own_batch(sql) {
        return sql.to_string();
    }
    let gate = restore_gate("Session", "", false);
    if release_inline {
        format!(
            "{RELEASE_SESSION_RESTORE_LOCK} {gate}\n{sql}\n;EXEC sp_releaseapplock @Resource = N'{RESTORE_LOCK_RESOURCE}', @LockOwner = 'Session';"
        )
    } else {
        format!("{RELEASE_SESSION_RESTORE_LOCK} {gate}\n{sql}")
    }
}

/// Releases every shared restore lock the session still owns.
const RELEASE_SESSION_RESTORE_LOCK: &str = "WHILE APPLOCK_MODE('public', N'graphql_orm_restore', 'Session') <> 'NoLock' \
     EXEC sp_releaseapplock @Resource = N'graphql_orm_restore', @LockOwner = 'Session';";

/// Whether SQL Server requires the statement to be the only one in its batch.
fn requires_own_batch(sql: &str) -> bool {
    let mut words = sql
        .split(|character: char| !character.is_ascii_alphabetic())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_uppercase);
    if !matches!(words.next().as_deref(), Some("CREATE" | "ALTER")) {
        return false;
    }
    let mut object = words.next();
    if object.as_deref() == Some("OR") {
        // CREATE OR ALTER <object>
        words.next();
        object = words.next();
    }
    matches!(
        object.as_deref(),
        Some(
            "PROCEDURE" | "PROC" | "FUNCTION" | "TRIGGER" | "VIEW" | "SCHEMA" | "DEFAULT" | "RULE"
        )
    )
}

/// What the restore gate's refusal reports.
enum RestoreRefusal {
    /// The restore lock is held or a marker row is present.
    InProgress,
    /// The marker could not be read.
    Unavailable(crate::graphql::orm::RestoreStateUnavailableReason),
}

/// The restore gate's refusal carried by `error`, if it is one.
fn restore_refusal(error: &sqlx::Error) -> Option<RestoreRefusal> {
    let message = error.as_database_error()?.message();
    if let Some(detail) = message.strip_prefix(RESTORE_UNAVAILABLE) {
        let number = detail
            .rsplit("error ")
            .next()
            .and_then(|digits| digits.trim().parse::<u32>().ok());
        return Some(RestoreRefusal::Unavailable(marker_error_reason(number)));
    }
    message
        .starts_with(RESTORE_REFUSAL)
        .then_some(RestoreRefusal::InProgress)
}

/// Category of the server error number raised while reading the marker.
fn marker_error_reason(number: Option<u32>) -> crate::graphql::orm::RestoreStateUnavailableReason {
    use crate::graphql::orm::RestoreStateUnavailableReason as Reason;
    match number {
        // Invalid object name.
        Some(208) => Reason::MarkerNotInstalled,
        // SELECT, column and object permission denied, and the like.
        Some(229 | 230 | 262 | 297 | 300) => Reason::PermissionDenied,
        _ => Reason::Other,
    }
}

/// Category of a failure to read the marker through the pool.
fn classify_marker_error(
    error: &sqlx::Error,
) -> crate::graphql::orm::RestoreStateUnavailableReason {
    use crate::graphql::orm::RestoreStateUnavailableReason as Reason;
    match error {
        sqlx::Error::Database(database) => {
            marker_error_reason(database.code().and_then(|code| code.parse().ok()))
        }
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed => Reason::ConnectionFailed,
        _ => Reason::Other,
    }
}

/// The typed refusal for restore state that cannot be read, with the cause
/// kept as its source. Its message never includes the cause.
fn restore_state_unavailable(
    reason: crate::graphql::orm::RestoreStateUnavailableReason,
    cause: sqlx::Error,
) -> sqlx::Error {
    crate::graphql::orm::RestoreStateUnavailable::new(reason, Some(Box::new(cause))).into_error()
}

/// A failure while claiming the marker: a missing table or a denied
/// permission is reported as unavailable restore state, anything else as is.
fn marker_access_error(error: sqlx::Error) -> sqlx::Error {
    use crate::graphql::orm::RestoreStateUnavailableReason as Reason;
    match classify_marker_error(&error) {
        reason @ (Reason::MarkerNotInstalled | Reason::PermissionDenied) => {
            restore_state_unavailable(reason, error)
        }
        _ => error,
    }
}

/// A host-run restore of a SQL Server database, in progress.
///
/// Created by `Database::begin_restore` (or `resume_restore`). It owns one
/// connection that holds the restore lock exclusively: while it lives, every
/// ORM transaction and autocommit statement, in every process, is refused
/// with [`RestoreInProgress`](crate::graphql::orm::RestoreInProgress) before
/// it reads or writes. Run the restore's statements through
/// [`Self::execute`] and [`Self::fetch_rows`], then call [`Self::finish`].
///
/// Dropping it without `finish`, or losing its connection, releases the lock
/// but leaves the marker row, so statements stay refused until the restore
/// is resumed and finished.
pub struct MssqlRestore {
    pool: MssqlPool,
    lease: Option<Lease>,
    restore_id: String,
}

impl MssqlRestore {
    /// The identifier recorded in the marker.
    pub fn restore_id(&self) -> &str {
        &self.restore_id
    }

    fn connection(&mut self) -> crate::Result<&mut Lease> {
        self.lease.as_mut().ok_or_else(|| {
            sqlx::Error::Protocol(
                "the restore connection was lost; its marker remains, resume the restore"
                    .to_string(),
            )
        })
    }

    fn keep_if_reusable<T>(&mut self, result: Result<T, StatementFailure>) -> crate::Result<T> {
        result.map_err(|failure| {
            if !failure.reusable {
                self.lease.take();
            }
            failure.error
        })
    }

    /// Execute a restore statement on the connection that holds the lock.
    /// Returns the statement's affected-row count.
    pub async fn execute(&mut self, sql: &str, values: &[SqlValue]) -> crate::Result<u64> {
        let pool = self.pool.clone();
        let lease = self.connection()?;
        let result = pool
            .bounded(execute_with_client(
                &mut lease.client,
                sql,
                values,
                pool.options(),
            ))
            .await;
        self.keep_if_reusable(result)
            .map(MssqlWriteResult::rows_affected)
    }

    /// Fetch rows on the connection that holds the lock.
    pub async fn fetch_rows(
        &mut self,
        sql: &str,
        values: &[SqlValue],
    ) -> crate::Result<Vec<MssqlRow>> {
        let pool = self.pool.clone();
        let lease = self.connection()?;
        let result = pool
            .bounded(fetch_rows_with_client(
                &mut lease.client,
                sql,
                values,
                pool.options(),
                false,
            ))
            .await;
        self.keep_if_reusable(result)
    }

    /// Clear the marker and release the lock: the restore is complete, or its
    /// partial rows were removed.
    ///
    /// # Errors
    ///
    /// Fails, leaving the marker, when the connection still has an open
    /// transaction (commit or roll it back first) or was lost.
    pub async fn finish(mut self) -> crate::Result<()> {
        let pool = self.pool.clone();
        let id = [SqlValue::String(self.restore_id.clone())];
        let lease = self.connection()?;
        let cleared = pool
            .bounded(execute_with_client(
                &mut lease.client,
                "IF @@TRANCOUNT > 0 BEGIN RAISERROR(N'the restore connection has an open transaction; commit or roll it back before finishing the restore', 16, 1); RETURN; END; \
                 DELETE FROM [dbo].[__graphql_orm_restore_marker] WHERE restore_id = @P1",
                &id,
                pool.options(),
            ))
            .await;
        self.keep_if_reusable(cleared)?;
        // The reset releases the session's restore lock.
        if let Some(mut lease) = self.lease.take()
            && pool.reset(&mut lease.client).await
        {
            pool.release(lease);
        }
        Ok(())
    }
}

impl MssqlTransaction {
    pub(crate) async fn fetch_rows(
        &mut self,
        sql: &str,
        values: &[SqlValue],
    ) -> crate::Result<Vec<MssqlRow>> {
        if self.failed {
            return Err(failed_transaction());
        }
        let lease = self.lease.as_mut().ok_or_else(inactive_transaction)?;
        let result = self
            .pool
            .bounded(fetch_rows_with_client(
                &mut lease.client,
                sql,
                values,
                self.pool.options(),
                // Pinned reads can drive authorization and write decisions.
                false,
            ))
            .await;
        result.map_err(|failure| self.poison(failure))
    }

    pub(crate) async fn execute(
        &mut self,
        sql: &str,
        values: &[SqlValue],
    ) -> crate::Result<MssqlWriteResult> {
        crate::graphql::orm::ensure_sql_write_allowed()?;
        if self.failed {
            return Err(failed_transaction());
        }
        let lease = self.lease.as_mut().ok_or_else(inactive_transaction)?;
        let result = self
            .pool
            .bounded(execute_with_client(
                &mut lease.client,
                sql,
                values,
                self.pool.options(),
            ))
            .await;
        result.map_err(|failure| self.poison(failure))
    }

    pub(crate) async fn commit(mut self) -> crate::Result<()> {
        if self.lease.is_none() {
            return Err(inactive_transaction());
        }
        if self.failed {
            // Never commit the statements that ran before the failed one.
            self.finish(SESSION_RESET).await?;
            return Err(failed_transaction());
        }
        self.finish(&format!("{COMMIT_TRANSACTION} {SESSION_RESET}"))
            .await
    }

    /// Roll back whatever the server still holds open and return the
    /// connection to the pool.
    ///
    /// A no-op when the connection was already discarded: the server rolled
    /// the transaction back when the session closed. After a failed statement
    /// the server may already have ended the transaction (a deadlock victim),
    /// so the batch rolls back only while `@@TRANCOUNT` is above zero.
    pub(crate) async fn rollback(mut self) -> crate::Result<()> {
        if self.lease.is_none() {
            return Ok(());
        }
        self.finish(SESSION_RESET).await
    }

    /// End the transaction and reset the session in one batch, then return
    /// the connection to the pool. On any failure the connection is closed.
    async fn finish(&mut self, batch: &str) -> crate::Result<()> {
        let mut lease = self.lease.take().ok_or_else(inactive_transaction)?;
        self.pool
            .bounded(execute_control_statement(&mut lease.client, batch))
            .await
            .map_err(|failure| failure.error)?;
        self.pool.release(lease);
        Ok(())
    }

    /// Record a failed statement. A server error keeps the connection, whose
    /// state is known, and fails the transaction; anything else discards it.
    fn poison(&mut self, failure: StatementFailure) -> sqlx::Error {
        if failure.reusable {
            self.failed = true;
        } else {
            self.lease.take();
        }
        failure.error
    }
}

fn classify_read_snapshot_start_error(
    error: sqlx::Error,
) -> crate::graphql::orm::ReadSnapshotError {
    use crate::graphql::orm::{MssqlBackend, ReadSnapshotError};

    if error
        .as_database_error()
        .and_then(|error| error.try_downcast_ref::<MssqlServerError>())
        .is_some_and(|error| error.number() == 3952)
    {
        ReadSnapshotError::MssqlSnapshotIsolationRequired
    } else {
        crate::db::classify_read_snapshot_error::<MssqlBackend>(error)
    }
}

fn inactive_transaction() -> sqlx::Error {
    sqlx::Error::Protocol("SQL Server transaction is no longer active".to_string())
}

fn failed_transaction() -> sqlx::Error {
    sqlx::Error::Protocol(
        "SQL Server transaction had a failed statement and can only be rolled back".to_string(),
    )
}

fn timed_out(message: &'static str) -> sqlx::Error {
    sqlx::Error::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, message))
}

impl std::fmt::Debug for MssqlPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MssqlPool").finish_non_exhaustive()
    }
}

impl MssqlRow {
    pub fn new(inner: tiberius::Row) -> Self {
        Self { inner }
    }

    pub fn try_get<T, I>(&self, index: I) -> crate::Result<T>
    where
        T: MssqlDecode,
        I: MssqlColumnIndex,
    {
        T::try_get(self, index)
    }
}

impl MssqlColumnIndex for &str {
    fn try_get_raw<'a, T>(self, row: &'a tiberius::Row) -> tiberius::Result<Option<T>>
    where
        T: tiberius::FromSql<'a>,
    {
        row.try_get::<T, _>(self)
    }

    fn display(self) -> String {
        self.to_string()
    }
}

impl MssqlColumnIndex for usize {
    fn try_get_raw<'a, T>(self, row: &'a tiberius::Row) -> tiberius::Result<Option<T>>
    where
        T: tiberius::FromSql<'a>,
    {
        if self >= row.len() {
            return Err(tiberius::error::Error::Conversion(
                format!("Could not find column with index {self}").into(),
            ));
        }
        row.try_get::<T, _>(self)
    }

    fn display(self) -> String {
        self.to_string()
    }
}

impl<T> MssqlDecode for T
where
    T: MssqlScalar,
{
    fn try_get<I>(row: &MssqlRow, index: I) -> crate::Result<Self>
    where
        I: MssqlColumnIndex,
    {
        T::try_get_optional(row, index)?.ok_or_else(|| sqlx::Error::ColumnDecode {
            index: index.display(),
            source: "unexpected NULL from SQL Server".into(),
        })
    }
}

impl<T> MssqlDecode for Option<T>
where
    T: MssqlScalar,
{
    fn try_get<I>(row: &MssqlRow, index: I) -> crate::Result<Self>
    where
        I: MssqlColumnIndex,
    {
        T::try_get_optional(row, index)
    }
}

/// Read the wire value once, so compatible conversions select by type and
/// never treat a failed decode as a signal to try the next type.
struct RawColumn<'a>(&'a ColumnData<'static>);

impl<'a> tiberius::FromSql<'a> for RawColumn<'a> {
    fn from_sql(value: &'a ColumnData<'static>) -> tiberius::Result<Option<Self>> {
        Ok(Some(Self(value)))
    }
}

fn decode_column<I, T>(
    row: &MssqlRow,
    index: I,
    decode: impl FnOnce(&ColumnData<'static>) -> tiberius::Result<Option<T>>,
) -> tiberius::Result<Option<T>>
where
    I: MssqlColumnIndex,
{
    match index.try_get_raw::<RawColumn<'_>>(&row.inner)? {
        Some(RawColumn(value)) => decode(value),
        None => Ok(None),
    }
}

fn incompatible_column(expected: &'static str) -> tiberius::error::Error {
    tiberius::error::Error::Conversion(expected.into())
}

impl MssqlScalar for String {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
    where
        I: MssqlColumnIndex,
    {
        decode_column(row, index, decode_string_column).map_err(|source| {
            sqlx::Error::ColumnDecode {
                index: index.display(),
                source: source.into(),
            }
        })
    }
}

fn decode_string_column(value: &ColumnData<'static>) -> tiberius::Result<Option<String>> {
    use tiberius::FromSql;

    Ok(match value {
        ColumnData::String(value) => value.as_deref().map(str::to_owned),
        // Offset-free SQL Server date-times use the ORM's UTC convention.
        ColumnData::DateTime(legacy) => legacy
            .map(legacy_datetime)
            .transpose()?
            .map(|value| temporal::mssql_rfc3339(value.assume_utc())),
        ColumnData::SmallDateTime(_) | ColumnData::DateTime2(_) => {
            PrimitiveDateTime::from_sql(value)?
                .map(|value| temporal::mssql_rfc3339(value.assume_utc()))
        }
        ColumnData::DateTimeOffset(_) => {
            OffsetDateTime::from_sql(value)?.map(temporal::mssql_rfc3339)
        }
        ColumnData::Date(_) => Date::from_sql(value)?.map(temporal::display_date),
        ColumnData::Time(_) => Time::from_sql(value)?.map(temporal::clock_text),
        ColumnData::Guid(value) => value.map(|value| value.to_string()),
        _ => {
            return Err(incompatible_column(
                "could not decode SQL Server column as String",
            ));
        }
    })
}

macro_rules! impl_mssql_temporal_scalar {
    ($($ty:ty),+ $(,)?) => {$(
        impl MssqlScalar for $ty {
            fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
            where
                I: MssqlColumnIndex,
            {
                index.try_get_raw::<Self>(&row.inner).map_err(map_tiberius_error)
            }
        }
    )+};
}

impl_mssql_temporal_scalar!(
    chrono::NaiveDate,
    chrono::NaiveDateTime,
    chrono::DateTime<chrono::FixedOffset>,
);

impl MssqlScalar for chrono::DateTime<chrono::Utc> {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
    where
        I: MssqlColumnIndex,
    {
        use tiberius::FromSql;

        decode_column(row, index, |value| match value {
            ColumnData::DateTime(_) | ColumnData::SmallDateTime(_) | ColumnData::DateTime2(_) => {
                Ok(chrono::NaiveDateTime::from_sql(value)?.map(|value| value.and_utc()))
            }
            ColumnData::DateTimeOffset(_) => {
                Ok(chrono::DateTime::<chrono::FixedOffset>::from_sql(value)?
                    .map(|value| value.to_utc()))
            }
            _ => Err(incompatible_column(
                "expected a SQL Server date-time column",
            )),
        })
        .map_err(map_tiberius_error)
    }
}

impl MssqlScalar for serde_json::Value {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
    where
        I: MssqlColumnIndex,
    {
        let display = index.display();
        index
            .try_get_raw::<&str>(&row.inner)
            .map_err(map_tiberius_error)?
            .map(|value| {
                serde_json::from_str(value).map_err(|error| sqlx::Error::ColumnDecode {
                    index: display.clone(),
                    source: error.into(),
                })
            })
            .transpose()
    }
}

impl MssqlScalar for Vec<u8> {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
    where
        I: MssqlColumnIndex,
    {
        index
            .try_get_raw::<&[u8]>(&row.inner)
            .map(|value| value.map(<[u8]>::to_vec))
            .map_err(map_tiberius_error)
    }
}

impl MssqlScalar for uuid::Uuid {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
    where
        I: MssqlColumnIndex,
    {
        index
            .try_get_raw::<uuid::Uuid>(&row.inner)
            .map_err(map_tiberius_error)
    }
}

impl MssqlScalar for bool {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
    where
        I: MssqlColumnIndex,
    {
        decode_column(row, index, |value| match value {
            ColumnData::Bit(value) => Ok(*value),
            ColumnData::I32(value) => Ok(value.map(|value| value != 0)),
            ColumnData::I16(value) => Ok(value.map(|value| value != 0)),
            // The driver uses U8(None) for some null integer wire types.
            ColumnData::U8(None) => Ok(None),
            _ => Err(incompatible_column(
                "expected a SQL Server bit, int, or smallint column",
            )),
        })
        .map_err(map_tiberius_error)
    }
}

impl MssqlScalar for i32 {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
    where
        I: MssqlColumnIndex,
    {
        let value = <i64 as MssqlScalar>::try_get_optional(row, index)?;
        value
            .map(|value| {
                i32::try_from(value).map_err(|error| sqlx::Error::ColumnDecode {
                    index: index.display(),
                    source: error.into(),
                })
            })
            .transpose()
    }
}

impl MssqlScalar for i64 {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
    where
        I: MssqlColumnIndex,
    {
        decode_column(row, index, |value| match value {
            ColumnData::I64(value) => Ok(*value),
            ColumnData::I32(value) => Ok(value.map(i64::from)),
            ColumnData::I16(value) => Ok(value.map(i64::from)),
            ColumnData::U8(value) => Ok(value.map(i64::from)),
            _ => Err(incompatible_column("expected a SQL Server integer column")),
        })
        .map_err(map_tiberius_error)
    }
}

macro_rules! impl_mssql_int_scalar {
    ($ty:ty) => {
        impl MssqlScalar for $ty {
            fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
            where
                I: MssqlColumnIndex,
            {
                <i64 as MssqlScalar>::try_get_optional(row, index)?
                    .map(|value| {
                        <$ty>::try_from(value).map_err(|error| sqlx::Error::ColumnDecode {
                            index: index.display(),
                            source: error.into(),
                        })
                    })
                    .transpose()
            }
        }
    };
}

impl_mssql_int_scalar!(i8);
impl_mssql_int_scalar!(i16);
impl_mssql_int_scalar!(isize);
impl_mssql_int_scalar!(u8);
impl_mssql_int_scalar!(u16);
impl_mssql_int_scalar!(u32);
impl_mssql_int_scalar!(u64);
impl_mssql_int_scalar!(usize);

impl MssqlScalar for f32 {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
    where
        I: MssqlColumnIndex,
    {
        index
            .try_get_raw::<f32>(&row.inner)
            .map_err(map_tiberius_error)
    }
}

impl MssqlScalar for f64 {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
    where
        I: MssqlColumnIndex,
    {
        decode_column(row, index, |value| match value {
            ColumnData::F64(value) => Ok(*value),
            ColumnData::F32(value) => Ok(value.map(f64::from)),
            _ => Err(incompatible_column(
                "expected a SQL Server floating-point column",
            )),
        })
        .map_err(map_tiberius_error)
    }
}

impl MssqlScalar for rust_decimal::Decimal {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
    where
        I: MssqlColumnIndex,
    {
        // The driver's own `Decimal` conversion panics beyond 28 decimal
        // places or a 96-bit mantissa, which `DECIMAL(38, s)` columns of an
        // externally managed table can hold.
        index
            .try_get_raw::<tiberius::numeric::Numeric>(&row.inner)
            .map_err(map_tiberius_error)?
            .map(|numeric| {
                decimal_from_numeric(numeric.value(), numeric.scale()).map_err(|source| {
                    sqlx::Error::ColumnDecode {
                        index: index.display(),
                        source: source.into(),
                    }
                })
            })
            .transpose()
    }
}

/// Decode legacy datetime with signed days (the driver's time codec uses unsigned days).
/// Round the 1/300-second ticks to the milliseconds that SQL Server displays.
fn legacy_datetime(value: tiberius::time::DateTime) -> tiberius::Result<PrimitiveDateTime> {
    let epoch = time::macros::datetime!(1900-01-01 0:00);
    let millis = (i64::from(value.seconds_fragments()) * 1_000 + 150) / 300;
    epoch
        .checked_add(
            time::Duration::days(i64::from(value.days())) + time::Duration::milliseconds(millis),
        )
        .ok_or_else(|| incompatible_column("SQL Server datetime is outside the supported range"))
}

/// Convert a TDS numeric to `Decimal`, refusing values it cannot represent.
fn decimal_from_numeric(value: i128, scale: u8) -> Result<rust_decimal::Decimal, String> {
    rust_decimal::Decimal::try_from_i128_with_scale(value, u32::from(scale)).map_err(|error| {
        format!("SQL Server DECIMAL value is outside the supported decimal range: {error}")
    })
}

impl<'a> tiberius::IntoSql<'a> for MssqlParamValue {
    fn into_sql(self) -> ColumnData<'a> {
        match self {
            Self::String(value) => ColumnData::String(Some(Cow::Owned(value))),
            Self::Bytes(value) => ColumnData::Binary(Some(Cow::Owned(value))),
            Self::Uuid(value) => ColumnData::Guid(Some(value)),
            Self::Int(value) => ColumnData::I64(Some(value)),
            Self::Float(value) => ColumnData::F64(Some(value)),
            Self::Bool(value) => ColumnData::Bit(Some(value)),
            Self::Decimal(value) => match tiberius::ToSql::to_sql(&value) {
                ColumnData::Numeric(value) => ColumnData::Numeric(value),
                _ => unreachable!("rust_decimal always maps to TDS numeric"),
            },
            Self::Native(value) => value,
            Self::NullString => ColumnData::String(None),
            Self::NullBytes => ColumnData::Binary(None),
            Self::NullUuid => ColumnData::Guid(None),
            Self::NullInt => ColumnData::I64(None),
            Self::NullFloat => ColumnData::F64(None),
            Self::NullBool => ColumnData::Bit(None),
            Self::NullDecimal => ColumnData::Numeric(None),
        }
    }
}

/// Run one exchange with the driver, turning a panic inside it into an error.
///
/// The driver asserts on some values it cannot represent while it decodes a
/// row, before the ORM sees the value: a non-NULL value in a column declared
/// with scale 38 (`DECIMAL(38,38)`, #452) is one. A panic would unwind
/// through the caller's task. Here it becomes a decode error instead. The
/// stream is then in an unknown state, so the connection is never reused,
/// and inside a transaction the failure ends the transaction. The process's
/// panic hook still reports the assertion. A binary built with
/// `panic = "abort"` cannot catch it and aborts.
pub(super) async fn catch_driver_panic<T, E>(
    exchange: impl std::future::Future<Output = Result<T, E>>,
    into_error: impl FnOnce(sqlx::Error) -> E,
) -> Result<T, E> {
    use futures::FutureExt;
    match std::panic::AssertUnwindSafe(exchange).catch_unwind().await {
        Ok(result) => result,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|detail| (*detail).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "no detail".to_string());
            Err(into_error(sqlx::Error::Decode(
                format!(
                    "the SQL Server driver could not decode a value in the result ({detail}); \
                     the connection was closed"
                )
                .into(),
            )))
        }
    }
}

/// Tiberius transports strings as NVARCHAR. Convert the parameter expression
/// on the server so SQL Server can compare a VARCHAR index key as VARCHAR.
fn render_varchar_parameters(sql: &str, values: &[SqlValue]) -> Option<String> {
    let varchar_length = |value: &SqlValue| {
        let SqlValue::MssqlTyped(typed) = value else {
            return None;
        };
        let MssqlParameterType::VarChar(length) = typed.parameter_type() else {
            return None;
        };
        let SqlValue::String(value) = typed.value() else {
            return None;
        };
        // A comparison can have an operand longer than the column. Use a
        // wide enough conversion to keep it from truncating into a match.
        let bytes = value.len();
        Some(
            length
                .map(|length| {
                    if bytes <= usize::from(length) {
                        length
                    } else {
                        8000
                    }
                })
                .filter(|_| bytes <= 8000),
        )
    };
    let mut lengths = values.iter().map(varchar_length);
    let first = lengths.position(|length| length.is_some())?;
    let mut converted = String::with_capacity(sql.len());
    let mut copied = 0;
    scan_sql(DatabaseBackend::Mssql, sql, |token| {
        if token.kind != SqlTokenKind::Placeholder {
            return;
        }
        let placeholder = &sql[token.start..token.end];
        let Some(index) = placeholder
            .strip_prefix("@P")
            .or_else(|| placeholder.strip_prefix("@p"))
            .and_then(|part| part.parse::<usize>().ok())
            .and_then(|index| index.checked_sub(1))
            .filter(|index| *index >= first)
        else {
            return;
        };
        if let Some(Some(length)) = values.get(index).map(varchar_length) {
            use std::fmt::Write as _;
            converted.push_str(&sql[copied..token.start]);
            converted.push_str("CONVERT(varchar(");
            match length {
                Some(length) => write!(converted, "{length}").expect("writing to a string"),
                None => converted.push_str("max"),
            }
            converted.push_str("), ");
            converted.push_str(placeholder);
            converted.push(')');
            copied = token.end;
        }
    })
    .ok()?;
    if copied == 0 {
        return None;
    }
    converted.push_str(&sql[copied..]);
    Some(converted)
}

fn bind_query(
    sql: &str,
    values: &[SqlValue],
    options: &MssqlPoolOptions,
    allow_varchar_comparisons: bool,
) -> Result<Query<'static>, StatementFailure> {
    let varchar_sql = (options.typed_parameters && allow_varchar_comparisons)
        .then(|| render_varchar_parameters(sql, values))
        .flatten();
    let mut query = Query::new(varchar_sql.unwrap_or_else(|| sql.to_owned()));
    for value in values {
        query.bind(
            MssqlParamValue::from_value(value, options).map_err(|error| StatementFailure {
                error,
                reusable: true,
            })?,
        );
    }
    Ok(query)
}

async fn fetch_rows_with_client(
    client: &mut MssqlClient,
    sql: &str,
    values: &[SqlValue],
    options: &MssqlPoolOptions,
    allow_varchar_comparisons: bool,
) -> Result<Vec<MssqlRow>, StatementFailure> {
    catch_driver_panic(
        async {
            let query = bind_query(sql, values, options, allow_varchar_comparisons)?;
            let stream = query.query(client).await.map_err(statement_failure)?;
            let rows = stream
                .into_first_result()
                .await
                .map_err(statement_failure)?;
            Ok(rows.into_iter().map(MssqlRow::new).collect())
        },
        StatementFailure::from,
    )
    .await
}

async fn execute_with_client(
    client: &mut MssqlClient,
    sql: &str,
    values: &[SqlValue],
    options: &MssqlPoolOptions,
) -> Result<MssqlWriteResult, StatementFailure> {
    catch_driver_panic(
        async {
            let query = bind_query(sql, values, options, false)?;
            let result = query.execute(client).await.map_err(statement_failure)?;
            Ok(MssqlWriteResult {
                rows_affected: statement_rows_affected(result.rows_affected()),
            })
        },
        StatementFailure::from,
    )
    .await
}

/// The row count of the executed statement itself.
///
/// SQL Server reports a count for every statement it runs, including the
/// statements inside triggers that the DML fires, unless those triggers set
/// `NOCOUNT`. A trigger finishes before the statement that fired it, so its
/// counts arrive first and the statement's own count arrives last. Summing
/// them would let an audit trigger make a one-row update look like several
/// rows, or a conditional insert that matched nothing look like one row.
/// For a batch of several statements this is the final statement's count.
fn statement_rows_affected(counts: &[u64]) -> u64 {
    counts.last().copied().unwrap_or(0)
}

async fn execute_control_statement(
    client: &mut MssqlClient,
    sql: &str,
) -> Result<(), StatementFailure> {
    catch_driver_panic(
        async {
            client
                .simple_query(sql)
                .await
                .map_err(statement_failure)?
                .into_results()
                .await
                .map_err(statement_failure)?;
            Ok(())
        },
        StatementFailure::from,
    )
    .await
}

/// Classify a driver error. Only a non-fatal error the server reported leaves
/// the connection in a known state: the driver drains the rest of the response
/// before its next request. SQL Server closes the session after an error of
/// severity 20 or higher, and transport, TLS and protocol errors leave the
/// stream in an unknown state.
fn statement_failure(error: tiberius::error::Error) -> StatementFailure {
    let reusable = matches!(&error, tiberius::error::Error::Server(token) if token.class() < 20);
    StatementFailure {
        error: map_tiberius_error(error),
        reusable,
    }
}

impl MssqlParamValue {
    fn from_value(value: &SqlValue, options: &MssqlPoolOptions) -> crate::Result<Self> {
        Ok(match value {
            SqlValue::String(value) => Self::String(value.clone()),
            SqlValue::Temporal(value) if options.typed_temporal_parameters || value.is_chrono() => {
                Self::Native(encode_temporal(value)?)
            }
            SqlValue::Temporal(value) => Self::String(value.as_text().to_owned()),
            SqlValue::StringNull => Self::NullString,
            SqlValue::Bytes(value) => Self::Bytes(value.clone()),
            SqlValue::BytesNull => Self::NullBytes,
            SqlValue::Json(value) => Self::String(value.to_string()),
            SqlValue::JsonNull => Self::NullString,
            SqlValue::Uuid(value) => Self::Uuid(*value),
            SqlValue::UuidNull => Self::NullUuid,
            SqlValue::Int(value) => Self::Int(*value),
            SqlValue::IntNull => Self::NullInt,
            SqlValue::Float(value) => Self::Float(*value),
            SqlValue::FloatNull => Self::NullFloat,
            // The writable MSSQL capability replaces this read-path-compatible
            // textual bind with Tiberius' native DECIMAL transport.
            SqlValue::Decimal(value) => Self::Decimal(value.value()),
            SqlValue::DecimalNull(_) => Self::NullDecimal,
            SqlValue::Bool(value) => Self::Bool(*value),
            SqlValue::BoolNull => Self::NullBool,
            SqlValue::Null => Self::NullString,
            SqlValue::MssqlTyped(typed) => {
                if typed.parameter_type() == MssqlParameterType::UnsignedOverflow {
                    use crate::graphql::errors::{
                        OrmErrorCode, OrmPublicError, sqlx_error_from_public,
                    };
                    return Err(sqlx_error_from_public(OrmPublicError::with_message(
                        OrmErrorCode::InvalidInput,
                        "integer exceeds the declared SQL Server range",
                    )));
                }
                let value = typed.value();
                if options.typed_parameters {
                    let integer = match value {
                        SqlValue::Int(value) => Some(*value),
                        _ => None,
                    };
                    // Use the legacy wide bind when a comparison operand
                    // is out of range for the column. It can still be a
                    // legitimate bound in a range predicate.
                    let narrowed = match typed.parameter_type() {
                        MssqlParameterType::Int => integer
                            .map(i32::try_from)
                            .transpose()
                            .ok()
                            .map(ColumnData::I32),
                        MssqlParameterType::SmallInt => integer
                            .map(i16::try_from)
                            .transpose()
                            .ok()
                            .map(ColumnData::I16),
                        MssqlParameterType::TinyInt => integer
                            .map(u8::try_from)
                            .transpose()
                            .ok()
                            .map(ColumnData::U8),
                        MssqlParameterType::VarChar(_) | MssqlParameterType::UnsignedOverflow => {
                            None
                        }
                    };
                    if let Some(narrowed) = narrowed {
                        return Ok(Self::Native(narrowed));
                    }
                }
                return Self::from_value(value, options);
            }
        })
    }
}

fn encode_temporal(value: &TemporalValue) -> crate::Result<ColumnData<'static>> {
    use tiberius::ToSql;

    let valid_date = |date: Date| (1..=9999).contains(&date.year());
    let date_error =
        || sqlx::Error::Encode("value is outside the SQL Server date or datetime2 range".into());
    match *value.kind() {
        TemporalKind::Date(date) => temporal::from_chrono_date(date)
            .filter(|date| valid_date(*date))
            .map(|date| owned_temporal_column(date.to_sql()))
            .ok_or_else(date_error),
        TemporalKind::DateTime(time) => temporal::from_chrono_naive(time)
            .filter(|(value, leap)| valid_date(value.date()) && !leap)
            .map(|(value, _)| owned_temporal_column(value.to_sql()))
            .ok_or_else(date_error),
        TemporalKind::DateTimeOffset(time) => {
            let seconds = time.offset().local_minus_utc();
            if seconds % 60 == 0
                && seconds.abs() <= 14 * 3600
                && let temporal::Timestamp::Time { utc, offset, leap_second: false } =
                    temporal::Timestamp::from_chrono(time)
                && valid_date(utc.date())
                && let Some(local) = utc.checked_add(time::Duration::seconds(i64::from(seconds)))
                && valid_date(local.date())
            {
                Ok(owned_temporal_column(local.assume_offset(offset).to_sql()))
            } else {
                Err(sqlx::Error::Encode(
                    "timestamp is outside the SQL Server datetimeoffset range".into(),
                ))
            }
        }
        TemporalKind::Unparsed => Err(sqlx::Error::Encode(
            "SQL Server native temporal parameters require an ISO date or an ISO timestamp with seconds".into(),
        )),
    }
}

// The driver's time codecs expose ToSql rather than IntoSql. All three
// temporal variants contain only owned data.
fn owned_temporal_column(data: ColumnData<'_>) -> ColumnData<'static> {
    match data {
        ColumnData::Date(value) => ColumnData::Date(value),
        ColumnData::DateTime2(value) => ColumnData::DateTime2(value),
        ColumnData::DateTimeOffset(value) => ColumnData::DateTimeOffset(value),
        _ => unreachable!("time codecs return native temporal data"),
    }
}

/// Whether `config` names an instance and no TCP port, which is the one case
/// that needs SQL Server Browser.
///
/// The driver exposes neither field. It reports port 1434 for an instance
/// without a port, and also for an explicit `,1434`, so the address alone
/// cannot tell them apart; the port's presence is read from the leading
/// fields of the configuration's `Debug` form (`Config { host: .., port: ..`),
/// which hides the password. When that form is not recognised the answer is
/// `false` and the connection is direct, as before named instances were
/// supported.
fn needs_instance_lookup(config: &Config) -> bool {
    // Without a port the driver reports 1434 only when an instance is named.
    if !config.get_addr().ends_with(":1434") {
        return false;
    }
    let debug = format!("{config:?}");
    let Some(rest) = debug.strip_prefix("Config { host: ") else {
        return false;
    };
    // Skip the host, a `None` or a `Some("...")` with `Debug` escapes.
    let rest = if let Some(rest) = rest.strip_prefix("None") {
        rest
    } else if let Some(quoted) = rest.strip_prefix("Some(\"") {
        let mut end = None;
        let mut escaped = false;
        for (index, character) in quoted.char_indices() {
            match character {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => {
                    end = Some(index);
                    break;
                }
                _ => {}
            }
        }
        match end.and_then(|end| quoted[end..].strip_prefix("\")")) {
            Some(rest) => rest,
            None => return false,
        }
    } else {
        return false;
    };
    rest.starts_with(", port: None,")
}

/// Whether `config` asks for `EncryptionLevel::On`, "encrypt if the server
/// allows it".
///
/// The driver panics during prelogin when such a configuration meets a
/// server that declines encryption, so the ORM refuses it before connecting.
/// Only a caller-built `Config` under
/// [`MssqlTransportPolicy::insecure_allow_login_only_encryption`] can carry
/// it: every other policy raises the level to `Required`, and connection
/// strings never produce `On`. The driver has no getter for the level, so it
/// is read from the configuration's `Debug` form outside quoted values,
/// which hides the password.
fn requests_optional_encryption(config: &Config) -> bool {
    let debug = format!("{config:?}");
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in debug.char_indices() {
        match character {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted && debug[index..].starts_with(", encryption: On,") => return true,
            _ => {}
        }
    }
    false
}

/// Open the TCP stream for `config`.
///
/// A named instance given without a port (`server=host\\INSTANCE`) has no
/// TCP port of its own: SQL Server Browser on the host answers a UDP request
/// on port 1434 with the instance's port, and the stream connects there. A
/// configuration that names a port, 1434 included, connects to that port
/// directly and ignores an instance name, as Microsoft's clients do. A
/// routing redirect names its own TCP port and is always direct.
async fn open_tcp(config: &Config, redirected: bool) -> crate::Result<TcpStream> {
    use tiberius::SqlBrowser;
    if !redirected && needs_instance_lookup(config) {
        return TcpStream::connect_named(config)
            .await
            .map_err(map_tiberius_error);
    }
    TcpStream::connect(config.get_addr())
        .await
        .map_err(sqlx::Error::Io)
}

/// Open one TDS connection, following at most one routing redirect.
///
/// Under Azure SQL's Redirect connection policy the gateway answers a
/// successful login with the address of the node that hosts the database and
/// the client must connect there. The redirect arrives after the gateway's
/// TLS session and login, and the second connection is validated under the
/// same transport policy against the redirected host name. A second redirect
/// is an error.
async fn open_client(mut config: Config) -> crate::Result<MssqlClient> {
    if requests_optional_encryption(&config) {
        return Err(sqlx::Error::Configuration(
            "SQL Server EncryptionLevel::On is not supported: the driver panics when the \
             server declines encryption. Use EncryptionLevel::Required, or Off with the \
             login-only opt-in"
                .into(),
        ));
    }
    let mut redirected = false;
    loop {
        let tcp = open_tcp(&config, redirected).await?;
        tcp.set_nodelay(true).map_err(sqlx::Error::Io)?;
        match tiberius::Client::connect(config.clone(), tcp.compat_write()).await {
            Ok(client) => return Ok(client),
            Err(tiberius::error::Error::Routing { host, port }) if !redirected => {
                redirected = true;
                follow_route(&mut config, &host, port);
            }
            // The bound that keeps a routing loop from reconnecting forever.
            Err(tiberius::error::Error::Routing { host, port }) => {
                return Err(sqlx::Error::Protocol(format!(
                    "SQL Server redirected the connection a second time, to {host}:{port}; \
                     only one routing redirect is followed"
                )));
            }
            Err(error) => return Err(map_tiberius_error(error)),
        }
    }
}

/// Point `config` at the node a routing redirect named.
fn follow_route(config: &mut Config, host: &str, port: u16) {
    config.host(host);
    config.port(port);
}

/// Keep the driver error's class: a server error stays a database error with
/// its number, and I/O and TLS failures stay I/O and TLS failures.
fn map_tiberius_error(error: tiberius::error::Error) -> sqlx::Error {
    match error {
        tiberius::error::Error::Server(token) => sqlx::Error::Database(Box::new(
            MssqlServerError::new(token.code(), token.state(), token.class(), token.message()),
        )),
        tiberius::error::Error::Io { kind, message } => {
            sqlx::Error::Io(std::io::Error::new(kind, message))
        }
        tiberius::error::Error::Tls(message) => sqlx::Error::Tls(message.into()),
        other => sqlx::Error::Protocol(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config::from_ado_string(
            "server=tcp:127.0.0.1,1433;database=test;user id=test;password=test;TrustServerCertificate=true",
        )
        .expect("test ADO configuration should parse")
    }

    #[test]
    fn compatibility_constructors_remain_physically_read_only() {
        let pool = MssqlPool::new(config());
        assert_eq!(pool.access_mode(), MssqlAccessMode::ReadOnly);
        let error = futures::executor::block_on(
            pool.execute("UPDATE [records] SET [value] = @P1", &[SqlValue::Int(1)]),
        )
        .expect_err("a read-only pool must reject DML before connecting");
        assert!(error.to_string().contains("physically read-only"));
    }

    #[test]
    fn snapshot_start_classifies_server_error_3952() {
        use crate::graphql::orm::ReadSnapshotError;

        let server_error = |number| {
            sqlx::Error::Database(Box::new(MssqlServerError::new(
                number,
                1,
                16,
                "server error",
            )))
        };
        assert!(matches!(
            classify_read_snapshot_start_error(server_error(3952)),
            ReadSnapshotError::MssqlSnapshotIsolationRequired,
        ));
        assert!(matches!(
            classify_read_snapshot_start_error(server_error(3953)),
            ReadSnapshotError::Transaction(_),
        ));
    }

    #[tokio::test]
    #[ignore = "requires Docker for an owned SQL Server"]
    async fn snapshot_start_preserves_3952_from_restore_gate() {
        use crate::graphql::orm::ReadSnapshotError;

        let mut owned = owned_mssql::OwnedSqlServer::start().await.unwrap();
        let pool = MssqlPool::connect_ado(&owned.connection_string)
            .await
            .unwrap();
        let lease = pool.acquire().await.unwrap();
        // Model a successful preflight followed by the database being OFF
        // when the begin batch runs, without racing an ALTER DATABASE.
        let Err(error) = pool.begin_transaction_on_lease(lease, "SNAPSHOT").await else {
            panic!("snapshot began while snapshot isolation was disabled");
        };
        assert!(matches!(
            classify_read_snapshot_start_error(error),
            ReadSnapshotError::MssqlSnapshotIsolationRequired,
        ));
        owned.cleanup().unwrap();
    }

    #[test]
    fn server_errors_keep_their_number_and_classification() {
        use sqlx::error::ErrorKind;
        let kind = |number, message: &str| {
            let error =
                sqlx::Error::Database(Box::new(MssqlServerError::new(number, 1, 14, message)));
            let database = error.as_database_error().expect("database error");
            assert_eq!(
                database.code().as_deref(),
                Some(number.to_string().as_str())
            );
            database.kind()
        };
        assert!(matches!(
            kind(2627, "PRIMARY KEY"),
            ErrorKind::UniqueViolation
        ));
        assert!(matches!(
            kind(2601, "unique index"),
            ErrorKind::UniqueViolation
        ));
        assert!(matches!(kind(515, "NULL"), ErrorKind::NotNullViolation));
        assert!(matches!(
            kind(547, "conflicted with the FOREIGN KEY constraint"),
            ErrorKind::ForeignKeyViolation
        ));
        assert!(matches!(
            kind(547, "conflicted with the REFERENCE constraint"),
            ErrorKind::ForeignKeyViolation
        ));
        assert!(matches!(
            kind(547, "conflicted with the CHECK constraint"),
            ErrorKind::CheckViolation
        ));
        assert!(matches!(kind(8152, "truncated"), ErrorKind::Other));

        for number in [1205, 1222, 3960, 41302, 40501, 40197, 40613] {
            assert!(MssqlServerError::new(number, 1, 13, "").is_retryable());
        }
        // A message that merely contains a retryable number is not retryable.
        for number in [2627, 2601, 547, 515, 8152, 2628, 262, 517] {
            assert!(!MssqlServerError::new(number, 1, 16, "row 1205 of 3960").is_retryable());
        }
    }

    #[test]
    fn public_errors_classify_server_errors_by_number() {
        use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
        let public = |number, message: &str| {
            OrmPublicError::from_sqlx(&sqlx::Error::Database(Box::new(MssqlServerError::new(
                number, 1, 14, message,
            ))))
        };
        assert_eq!(
            public(2627, "duplicate").code,
            OrmErrorCode::ConstraintViolation
        );
        assert_eq!(
            public(547, "CHECK constraint").code,
            OrmErrorCode::ConstraintViolation
        );
        let deadlock = public(1205, "deadlock victim");
        assert_eq!(deadlock.code, OrmErrorCode::ServiceUnavailable);
        assert!(deadlock.is_retryable());
        // 262 and 517 are SQLite's retryable result codes but ordinary SQL
        // Server errors, and 5 and 6 are not retryable there either.
        for number in [5, 6, 261, 262, 517] {
            let error = public(number, "not transient");
            assert_eq!(error.code, OrmErrorCode::InternalError);
            assert!(!error.is_retryable());
        }
    }

    #[test]
    fn driver_errors_keep_their_class() {
        let io = map_tiberius_error(tiberius::error::Error::Io {
            kind: std::io::ErrorKind::ConnectionReset,
            message: "reset".into(),
        });
        assert!(
            matches!(&io, sqlx::Error::Io(error) if error.kind() == std::io::ErrorKind::ConnectionReset)
        );
        assert!(matches!(
            map_tiberius_error(tiberius::error::Error::Tls("handshake".into())),
            sqlx::Error::Tls(_)
        ));
        assert!(matches!(
            map_tiberius_error(tiberius::error::Error::Protocol("bad token".into())),
            sqlx::Error::Protocol(_)
        ));
    }

    fn refusal(message: &str) -> sqlx::Error {
        sqlx::Error::Database(Box::new(MssqlServerError::new(50000, 1, 16, message)))
    }

    #[test]
    fn the_gate_refusal_tells_unavailable_state_from_a_restore() {
        use crate::graphql::orm::RestoreStateUnavailableReason as Reason;
        let unavailable = |number: u32| {
            restore_refusal(&refusal(&format!(
                "{RESTORE_UNAVAILABLE}: marker read failed with error {number}"
            )))
        };
        assert!(matches!(
            unavailable(208),
            Some(RestoreRefusal::Unavailable(Reason::MarkerNotInstalled))
        ));
        for number in [229, 230, 262, 297, 300] {
            assert!(matches!(
                unavailable(number),
                Some(RestoreRefusal::Unavailable(Reason::PermissionDenied))
            ));
        }
        assert!(matches!(
            unavailable(207),
            Some(RestoreRefusal::Unavailable(Reason::Other))
        ));
        for message in [
            "the restore lock is held",
            "a restore marker is present",
            "open transactions or another restore hold the restore lock",
        ] {
            assert!(
                matches!(
                    restore_refusal(&refusal(&format!("{RESTORE_REFUSAL}: {message}"))),
                    Some(RestoreRefusal::InProgress)
                ),
                "{message}"
            );
        }
        assert!(restore_refusal(&refusal("something else")).is_none());
        assert!(restore_refusal(&sqlx::Error::PoolTimedOut).is_none());
    }

    #[test]
    fn marker_read_failures_are_classified_without_claiming_a_restore() {
        use crate::graphql::orm::RestoreStateUnavailableReason as Reason;
        let server = |number| {
            sqlx::Error::Database(Box::new(MssqlServerError::new(number, 1, 16, "denied")))
        };
        assert_eq!(
            classify_marker_error(&server(208)),
            Reason::MarkerNotInstalled
        );
        assert_eq!(
            classify_marker_error(&server(229)),
            Reason::PermissionDenied
        );
        assert_eq!(classify_marker_error(&server(4060)), Reason::Other);
        for error in [
            timed_out("SQL Server statement exceeded the configured query timeout"),
            sqlx::Error::PoolTimedOut,
            sqlx::Error::Tls("handshake".into()),
            sqlx::Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
        ] {
            assert_eq!(classify_marker_error(&error), Reason::ConnectionFailed);
        }
        // The server's text is kept as the source, never put in the message.
        let wrapped = restore_state_unavailable(
            Reason::PermissionDenied,
            sqlx::Error::Database(Box::new(MssqlServerError::new(
                229,
                1,
                14,
                "SELECT permission was denied on the object 'x'",
            ))),
        );
        let message = wrapped.to_string();
        assert!(!message.contains("was denied on the object"), "{message}");
        assert!(message.contains("grant SELECT"), "{message}");
        let unavailable =
            crate::graphql::orm::RestoreStateUnavailable::from_error(&wrapped).expect("typed");
        assert!(std::error::Error::source(unavailable).is_some());
        assert!(crate::graphql::orm::RestoreInProgress::from_error(&wrapped).is_none());
        // A claim failure other than a missing table or a denial stays as it was.
        assert!(matches!(
            marker_access_error(sqlx::Error::PoolTimedOut),
            sqlx::Error::PoolTimedOut
        ));
        assert!(
            crate::graphql::orm::RestoreStateUnavailable::from_error(&marker_access_error(server(
                208
            )))
            .is_some()
        );
    }

    #[test]
    fn a_routing_redirect_changes_only_the_address() {
        let mut config = config();
        follow_route(&mut config, "node7.worker.database.example", 11042);
        assert_eq!(config.get_addr(), "node7.worker.database.example:11042");
    }

    /// The driver panics when `EncryptionLevel::On` meets a server that
    /// declines encryption; the pool refuses that configuration before it
    /// opens a connection.
    #[tokio::test]
    async fn optional_encryption_is_refused_before_connecting() {
        use std::sync::atomic::Ordering;
        let (port, logins, task) = fake_tds_endpoint(FakeLogin::Accept).await;
        let mut config = Config::new();
        config.host("127.0.0.1");
        config.port(port);
        config.authentication(tiberius::AuthMethod::sql_server("owned", "owned"));
        for (level, refused) in [
            (tiberius::EncryptionLevel::On, true),
            (tiberius::EncryptionLevel::Off, false),
            (tiberius::EncryptionLevel::NotSupported, false),
            (tiberius::EncryptionLevel::Required, false),
        ] {
            config.encryption(level);
            assert_eq!(requests_optional_encryption(&config), refused, "{level:?}");
        }
        // A value cannot imitate the field.
        let mut odd = config.clone();
        odd.host("a\", encryption: On, \"");
        odd.encryption(tiberius::EncryptionLevel::Required);
        assert!(!requests_optional_encryption(&odd));

        config.encryption(tiberius::EncryptionLevel::On);
        let pool = MssqlPool::with_options(
            config,
            MssqlPoolOptions::new()
                .transport(MssqlTransportPolicy::verified().insecure_allow_login_only_encryption()),
        );
        let error = pool
            .fetch_rows("SELECT 1 AS one", &[])
            .await
            .err()
            .expect("refused");
        assert!(
            matches!(&error, sqlx::Error::Configuration(message) if message.to_string().contains("EncryptionLevel::On")),
            "{error:?}"
        );
        assert_eq!(logins.load(Ordering::SeqCst), 0, "nothing was sent");
        task.abort();
    }

    /// What a fake TDS endpoint answers a login with.
    #[derive(Clone)]
    enum FakeLogin {
        /// Log the client in, then answer every request with an empty result.
        Accept,
        /// Send the client to this port on loopback, as an Azure SQL gateway
        /// under the Redirect connection policy does.
        Redirect(u16),
    }

    type FakeStream = tokio_util::compat::Compat<TcpStream>;

    /// Read one TDS message: packets up to the one with the end-of-message bit.
    async fn read_tds_message(stream: &mut FakeStream) -> std::io::Result<u8> {
        use futures::io::AsyncReadExt;
        loop {
            let mut header = [0u8; 8];
            stream.read_exact(&mut header).await?;
            let length = usize::from(u16::from_be_bytes([header[2], header[3]]));
            let mut body = vec![0u8; length.saturating_sub(8)];
            stream.read_exact(&mut body).await?;
            if header[1] & 0x01 != 0 {
                return Ok(header[0]);
            }
        }
    }

    async fn write_tds_message(
        stream: &mut FakeStream,
        packet_type: u8,
        body: &[u8],
    ) -> std::io::Result<()> {
        use futures::io::AsyncWriteExt;
        let length = u16::try_from(body.len() + 8).expect("one small packet");
        let mut packet = vec![packet_type, 0x01];
        packet.extend_from_slice(&length.to_be_bytes());
        packet.extend_from_slice(&[0, 0, 1, 0]);
        packet.extend_from_slice(body);
        stream.write_all(&packet).await
    }

    /// A final `DONE` token: status, current command and a TDS 7.2+ row count.
    fn tds_done() -> Vec<u8> {
        let mut token = vec![0xFD, 0, 0, 0, 0];
        token.extend_from_slice(&0u64.to_le_bytes());
        token
    }

    /// An `ENVCHANGE` routing token (type 20) naming `host:port`.
    fn tds_routing(host: &str, port: u16) -> Vec<u8> {
        let name = host.encode_utf16().collect::<Vec<_>>();
        let mut value = vec![0u8]; // TCP
        value.extend_from_slice(&port.to_le_bytes());
        value.extend_from_slice(&u16::try_from(name.len()).unwrap().to_le_bytes());
        for unit in &name {
            value.extend_from_slice(&unit.to_le_bytes());
        }
        let mut body = vec![20u8];
        body.extend_from_slice(&u16::try_from(value.len()).unwrap().to_le_bytes());
        body.extend_from_slice(&value);
        body.extend_from_slice(&0u16.to_le_bytes()); // no old value
        let mut token = vec![0xE3];
        token.extend_from_slice(&u16::try_from(body.len()).unwrap().to_le_bytes());
        token.extend_from_slice(&body);
        token
    }

    /// A loopback TDS endpoint without TLS that counts the logins it sees.
    async fn fake_tds_endpoint(
        answer: FakeLogin,
    ) -> (
        u16,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        use std::sync::atomic::Ordering;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let logins = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = logins.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let mut stream = tokio_util::compat::TokioAsyncReadCompatExt::compat(stream);
                let answer = answer.clone();
                let seen = seen.clone();
                tokio::spawn(async move {
                    // PRELOGIN: version 16.0, encryption not supported.
                    assert_eq!(read_tds_message(&mut stream).await?, 0x12);
                    let prelogin = [
                        0x00, 0x00, 0x0B, 0x00, 0x06, // VERSION at 11, 6 bytes
                        0x01, 0x00, 0x11, 0x00, 0x01, // ENCRYPTION at 17, 1 byte
                        0xFF, 16, 0, 0, 0, 0, 0,    // version, sub-build
                        0x02, // ENCRYPT_NOT_SUP
                    ];
                    write_tds_message(&mut stream, 0x04, &prelogin).await?;
                    // LOGIN7.
                    assert_eq!(read_tds_message(&mut stream).await?, 0x10);
                    seen.fetch_add(1, Ordering::SeqCst);
                    match answer {
                        FakeLogin::Redirect(port) => {
                            let mut tokens = tds_routing("127.0.0.1", port);
                            tokens.extend(tds_done());
                            write_tds_message(&mut stream, 0x04, &tokens).await?;
                        }
                        FakeLogin::Accept => {
                            write_tds_message(&mut stream, 0x04, &tds_done()).await?;
                            while read_tds_message(&mut stream).await.is_ok() {
                                write_tds_message(&mut stream, 0x04, &tds_done()).await?;
                            }
                        }
                    }
                    std::io::Result::Ok(())
                });
            }
        });
        (port, logins, task)
    }

    /// Under Azure SQL's Redirect connection policy the gateway logs the
    /// client in and names the node to connect to (#518). The pool follows
    /// that one redirect and runs its statements on the node; a second
    /// redirect is refused rather than followed.
    #[tokio::test]
    async fn the_pool_follows_one_routing_redirect_to_the_node() {
        use std::sync::atomic::Ordering;
        // No TLS: the fake endpoints speak plain TDS. That needs both the
        // code opt-in and a caller-built configuration.
        let pool = |port: u16| {
            let mut config = Config::new();
            config.host("127.0.0.1");
            config.port(port);
            config.authentication(tiberius::AuthMethod::sql_server("owned", "owned"));
            config.encryption(tiberius::EncryptionLevel::NotSupported);
            MssqlPool::with_options(
                config,
                MssqlPoolOptions::new()
                    .connect_timeout(Duration::from_secs(5))
                    .transport(
                        MssqlTransportPolicy::verified().insecure_allow_login_only_encryption(),
                    ),
            )
        };

        let (node, node_logins, node_task) = fake_tds_endpoint(FakeLogin::Accept).await;
        let (gateway, gateway_logins, gateway_task) =
            fake_tds_endpoint(FakeLogin::Redirect(node)).await;
        let rows = pool(gateway)
            .fetch_rows("SELECT 1 AS one", &[])
            .await
            .expect("the redirected connection serves the statement");
        assert!(rows.is_empty());
        assert_eq!(gateway_logins.load(Ordering::SeqCst), 1);
        assert_eq!(node_logins.load(Ordering::SeqCst), 1);

        // A node that redirects again is not followed: the redirect bound is
        // one hop, so a routing loop cannot keep the pool connecting.
        let (second, second_logins, second_task) =
            fake_tds_endpoint(FakeLogin::Redirect(node)).await;
        let (first, first_logins, first_task) =
            fake_tds_endpoint(FakeLogin::Redirect(second)).await;
        let error = pool(first)
            .fetch_rows("SELECT 1 AS one", &[])
            .await
            .err()
            .expect("a second redirect is refused");
        assert!(
            error
                .to_string()
                .contains("only one routing redirect is followed"),
            "{error}"
        );
        assert_eq!(first_logins.load(Ordering::SeqCst), 1);
        assert_eq!(second_logins.load(Ordering::SeqCst), 1);
        assert_eq!(node_logins.load(Ordering::SeqCst), 1, "never reached");
        for task in [node_task, gateway_task, second_task, first_task] {
            task.abort();
        }
    }

    /// A named instance without a port is resolved through SQL Server
    /// Browser (#518): the stream connects to the TCP port the browser
    /// reports. A configuration that names a port, 1434 included, connects
    /// to it directly and sends the browser nothing.
    #[tokio::test]
    async fn only_an_instance_without_a_port_asks_sql_browser() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let ado = |server: &str| {
            Config::from_ado_string(&format!(
                "server=tcp:{server};database=test;user id=test;password=test;TrustServerCertificate=true"
            ))
            .unwrap()
        };
        // The decision itself, for every shape of server address.
        for (server, lookup) in [
            ("127.0.0.1\\REPORTING", true),
            ("127.0.0.1\\REPORTING,1434", false),
            ("127.0.0.1\\REPORTING,1500", false),
            ("127.0.0.1,1434", false),
            ("127.0.0.1,1433", false),
            ("127.0.0.1", false),
        ] {
            assert_eq!(needs_instance_lookup(&ado(server)), lookup, "{server}");
        }
        // A host name cannot imitate the fields that follow it.
        let mut odd = ado("127.0.0.1\\REPORTING");
        odd.host("a\", port: None, x\\");
        assert!(needs_instance_lookup(&odd));
        odd.port(1434);
        assert!(!needs_instance_lookup(&odd));
        // A redirect sets a port, so the redirected address is direct too.
        let mut routed = ado("127.0.0.1\\REPORTING");
        follow_route(&mut routed, "127.0.0.1", 1434);
        assert!(!needs_instance_lookup(&routed));

        // SQL Server Browser has one well-known port, and this test needs
        // both the UDP and the TCP side of 1434 on loopback to itself.
        let browser = tokio::net::UdpSocket::bind("127.0.0.1:1434")
            .await
            .expect("UDP 1434 on loopback must be free for this test");
        let explicit = tokio::net::TcpListener::bind("127.0.0.1:1434")
            .await
            .expect("TCP 1434 on loopback must be free for this test");
        let instance = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = instance.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let answer = tokio::spawn(async move {
            loop {
                let mut request = [0u8; 64];
                let (length, client) = browser.recv_from(&mut request).await.unwrap();
                seen.fetch_add(1, Ordering::SeqCst);
                // CLNT_UCAST_INST: 0x04 followed by the instance name.
                assert_eq!(&request[..length], b"\x04REPORTING");
                let body = format!(
                    "ServerName;HOST;InstanceName;REPORTING;IsClustered;No;Version;16.0.1000.6;tcp;{port};;"
                );
                // SVR_RESP: 0x05, a little-endian length, then the description.
                let mut reply = vec![0x05, body.len() as u8, (body.len() >> 8) as u8];
                reply.extend_from_slice(body.as_bytes());
                browser.send_to(&reply, client).await.unwrap();
            }
        });

        // Explicit TCP 1434, with and without an instance name, and after a
        // redirect: each connects to TCP 1434 and asks the browser nothing.
        for (config, redirected) in [
            (ado("127.0.0.1\\REPORTING,1434"), false),
            (ado("127.0.0.1,1434"), false),
            (routed, true),
        ] {
            let stream = open_tcp(&config, redirected).await.unwrap();
            let (_accepted, peer) = explicit.accept().await.unwrap();
            assert_eq!(stream.local_addr().unwrap(), peer);
        }
        // Another explicit port connects there.
        let stream = open_tcp(&ado(&format!("127.0.0.1\\REPORTING,{port}")), false)
            .await
            .unwrap();
        let (_accepted, peer) = instance.accept().await.unwrap();
        assert_eq!(stream.local_addr().unwrap(), peer);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(requests.load(Ordering::SeqCst), 0, "no discovery request");

        // An instance without a port asks the browser once and connects to
        // the port it reports, not to 1434.
        let named = ado("127.0.0.1\\REPORTING");
        assert_eq!(named.get_addr(), "127.0.0.1:1434");
        let stream = open_tcp(&named, false).await.unwrap();
        let (_accepted, peer) = instance.accept().await.unwrap();
        assert_eq!(stream.local_addr().unwrap(), peer);
        assert_eq!(stream.peer_addr().unwrap().port(), port);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        answer.abort();
    }

    fn chrono_temporal_text(column: &ColumnData<'static>) -> Option<String> {
        use chrono::{SecondsFormat, Timelike};
        use tiberius::FromSql;

        match column {
            ColumnData::Date(_) => chrono::NaiveDate::from_sql(column)
                .unwrap()
                .map(|v| v.to_string()),
            ColumnData::Time(_) => chrono::NaiveTime::from_sql(column)
                .unwrap()
                .map(|v| v.to_string()),
            ColumnData::DateTimeOffset(_) => {
                chrono::DateTime::<chrono::FixedOffset>::from_sql(column)
                    .unwrap()
                    .map(|v| v.to_rfc3339_opts(SecondsFormat::AutoSi, true))
            }
            _ => chrono::NaiveDateTime::from_sql(column)
                .unwrap()
                .map(|value| {
                    let nanos = value.nanosecond();
                    let value = if nanos.is_multiple_of(100) {
                        value
                    } else {
                        value
                            .with_nanosecond((nanos + 500_000) / 1_000_000 * 1_000_000)
                            .unwrap()
                    };
                    value.and_utc().to_rfc3339_opts(SecondsFormat::AutoSi, true)
                }),
        }
    }

    #[test]
    fn temporal_column_text_matches_chrono() {
        use tiberius::time as tds;

        let date = tds::Date::new(3_652_058);
        let time = tds::Time::new(863_999_999_999, 7);
        let datetime2 = tds::DateTime2::new(tds::Date::new(0), tds::Time::new(123_456_700, 7));
        for column in [
            ColumnData::Date(Some(date)),
            ColumnData::Date(Some(tds::Date::new(0))),
            ColumnData::Date(None),
            ColumnData::Time(Some(time)),
            ColumnData::Time(Some(tds::Time::new(45_296_123, 3))),
            ColumnData::Time(Some(tds::Time::new(0, 7))),
            ColumnData::Time(None),
            ColumnData::SmallDateTime(Some(tds::SmallDateTime::new(65_535, 1439))),
            ColumnData::SmallDateTime(None),
            ColumnData::DateTime2(Some(tds::DateTime2::new(date, time))),
            ColumnData::DateTime2(None),
            ColumnData::DateTimeOffset(Some(tds::DateTimeOffset::new(datetime2, -30))),
            ColumnData::DateTimeOffset(Some(tds::DateTimeOffset::new(datetime2, 840))),
            ColumnData::DateTimeOffset(Some(tds::DateTimeOffset::new(datetime2, 0))),
            ColumnData::DateTimeOffset(None),
            ColumnData::DateTime(None),
        ] {
            assert_eq!(
                decode_string_column(&column).unwrap(),
                chrono_temporal_text(&column),
                "{column:?}"
            );
        }
        for scale in 0..=6 {
            let ticks = 452_961_234_567 / 10u64.pow(7 - u32::from(scale));
            let clock = tds::Time::new(ticks, scale);
            let column = ColumnData::DateTime2(Some(tds::DateTime2::new(date, clock)));
            assert_eq!(
                decode_string_column(&column).unwrap(),
                chrono_temporal_text(&column),
                "datetime2({scale})"
            );
        }
        for day in [-53_690, -1, 0, 1] {
            for tick in 0..300 {
                let column =
                    ColumnData::DateTime(Some(tds::DateTime::new(day, 12_345 * 300 + tick)));
                assert_eq!(
                    decode_string_column(&column).unwrap(),
                    chrono_temporal_text(&column),
                    "{column:?}"
                );
            }
        }
    }

    #[test]
    fn native_temporal_parameter_bytes_match_chrono_codecs() {
        use tiberius::IntoSql;

        for text in [
            "0001-01-01",
            "9999-12-31",
            "1970-01-01 00:00:00",
            "2000-02-29T23:59:59.999999999",
            "1970-01-01T00:00:00.123456789Z",
            "2000-02-29T23:59:59.123456789-12:30",
            "9999-12-31T23:59:59.999999999+14:00",
        ] {
            let SqlValue::Temporal(value) = SqlValue::temporal_text(text.to_owned()) else {
                unreachable!();
            };
            let expected = match *value.kind() {
                TemporalKind::Date(date) => date.into_sql(),
                TemporalKind::DateTime(date) => date.into_sql(),
                TemporalKind::DateTimeOffset(date) => date.into_sql(),
                TemporalKind::Unparsed => panic!("could not parse {text}"),
            };
            assert_eq!(encode_temporal(&value).unwrap(), expected, "{text}");
        }

        use chrono::{Datelike, Timelike};

        for year in [
            "-262144", "-262143", "-262142", "-10000", "-9999", "9999", "+9999", "+10000",
            "+262142", "+262143",
        ] {
            for suffix in [
                "-01-01",
                "-12-31T23:59:59.123456789",
                "-12-31T23:59:59.123456789-00:01",
                "-12-31T23:59:59.123456789+00:01",
            ] {
                let text = format!("{year}{suffix}");
                let SqlValue::Temporal(value) = SqlValue::temporal_text(text.clone()) else {
                    unreachable!();
                };
                let valid_date = |date: chrono::NaiveDate| (1..=9999).contains(&date.year());
                let valid_time = |stamp: chrono::NaiveDateTime| {
                    valid_date(stamp.date()) && stamp.nanosecond() < 1_000_000_000
                };
                let expected = match *value.kind() {
                    TemporalKind::Date(date) if valid_date(date) => Some(date.into_sql()),
                    TemporalKind::DateTime(stamp) if valid_time(stamp) => Some(stamp.into_sql()),
                    TemporalKind::DateTimeOffset(stamp) if valid_time(stamp.naive_utc()) => stamp
                        .naive_utc()
                        .checked_add_signed(chrono::TimeDelta::seconds(i64::from(
                            stamp.offset().local_minus_utc(),
                        )))
                        .filter(|stamp| valid_time(*stamp))
                        .map(|_| stamp.into_sql()),
                    _ => None,
                };
                assert_eq!(encode_temporal(&value).ok(), expected, "{text}");
            }
        }
    }

    #[test]
    fn native_temporal_parameter_rejections_match_chrono_bounds() {
        use chrono::{Datelike, Timelike};

        let leap = "2016-12-31T23:59:60.5"
            .parse::<chrono::NaiveDateTime>()
            .unwrap();
        let stamp = "2024-02-29T12:34:56Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap();
        let valid_date = |date: chrono::NaiveDate| (1..=9999).contains(&date.year());
        let valid_time = |time: chrono::NaiveDateTime| {
            valid_date(time.date()) && time.nanosecond() < 1_000_000_000
        };
        for parameter in [
            SqlValue::from("0000-01-01".parse::<chrono::NaiveDate>().unwrap()),
            SqlValue::from("+10000-01-01".parse::<chrono::NaiveDate>().unwrap()),
            SqlValue::from(
                "+10000-01-01T00:00:00"
                    .parse::<chrono::NaiveDateTime>()
                    .unwrap(),
            ),
            SqlValue::from(leap),
            SqlValue::from(leap.and_utc().fixed_offset()),
            SqlValue::from(
                stamp.with_timezone(&chrono::FixedOffset::east_opt(14 * 3600 + 60).unwrap()),
            ),
            SqlValue::from(stamp.with_timezone(&chrono::FixedOffset::east_opt(30).unwrap())),
        ] {
            let SqlValue::Temporal(value) = parameter else {
                unreachable!();
            };
            let expected = match *value.kind() {
                TemporalKind::Date(date) => valid_date(date),
                TemporalKind::DateTime(time) => valid_time(time),
                TemporalKind::DateTimeOffset(time) => {
                    let offset = time.offset().local_minus_utc();
                    offset % 60 == 0
                        && offset.abs() <= 14 * 3600
                        && valid_time(time.naive_utc())
                        && valid_time(time.naive_local())
                }
                TemporalKind::Unparsed => unreachable!(),
            };
            assert!(
                !expected,
                "expected an out-of-range case: {}",
                value.as_text()
            );
            assert!(
                matches!(encode_temporal(&value), Err(sqlx::Error::Encode(_))),
                "{}",
                value.as_text()
            );
        }
    }

    #[test]
    fn out_of_range_decimals_are_decode_errors_not_panics() {
        assert_eq!(
            decimal_from_numeric(12345, 2).unwrap().to_string(),
            "123.45"
        );
        // Scale beyond 28 and a mantissa beyond 96 bits both fit DECIMAL(38, s).
        assert!(decimal_from_numeric(1, 30).is_err());
        assert!(decimal_from_numeric(10i128.pow(37), 0).is_err());
    }

    #[test]
    fn writable_mode_requires_the_explicit_constructor() {
        let pool = MssqlPool::new_external_writable(config());
        assert_eq!(pool.access_mode(), MssqlAccessMode::ExternalWritable);
    }
}
