mod notifications;
mod transport;
pub use notifications::{
    MssqlBrokerQueue, MssqlQueryNotificationConnection, MssqlQueryNotificationKind,
    MssqlQueryNotificationMessage,
};
pub use transport::MssqlTransportPolicy;

use crate::graphql::orm::SqlValue;
use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tiberius::{ColumnData, Config, Query};
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
    SET IMPLICIT_TRANSACTIONS OFF;";

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
pub struct MssqlTransaction {
    pool: MssqlPool,
    lease: Option<Lease>,
}

#[derive(Clone)]
pub struct MssqlPool {
    inner: Arc<MssqlPoolInner>,
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

    pub async fn fetch_rows(&self, sql: &str, values: &[SqlValue]) -> crate::Result<Vec<MssqlRow>> {
        let mut lease = self.acquire().await?;
        let result = self
            .bounded(fetch_rows_with_client(&mut lease.client, sql, values))
            .await;
        self.finish(lease, result).await
    }

    pub(crate) async fn execute(
        &self,
        sql: &str,
        values: &[SqlValue],
    ) -> crate::Result<MssqlWriteResult> {
        self.require_external_writable()?;
        let mut lease = self.acquire().await?;
        let result = self
            .bounded(execute_with_client(&mut lease.client, sql, values))
            .await;
        self.finish(lease, result).await
    }

    pub(crate) async fn begin_transaction(
        &self,
        mode: crate::graphql::orm::TransactionMode,
    ) -> crate::Result<MssqlTransaction> {
        self.require_external_writable()?;
        let mut lease = self.acquire().await?;
        let isolation = match mode {
            crate::graphql::orm::TransactionMode::Default => "READ COMMITTED",
            crate::graphql::orm::TransactionMode::StateMachine => "SERIALIZABLE",
        };
        // Never nest inside a transaction raw SQL left open on this session:
        // its COMMIT would only decrement the count and nothing would be durable.
        let begin = format!(
            "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION; \
             SET TRANSACTION ISOLATION LEVEL {isolation}; BEGIN TRANSACTION;"
        );
        // A failed begin may have changed the isolation level; never reuse it.
        self.bounded(execute_control_statement(&mut lease.client, &begin))
            .await
            .map_err(|failure| failure.error)?;
        Ok(MssqlTransaction {
            pool: self.clone(),
            lease: Some(lease),
        })
    }

    fn require_external_writable(&self) -> crate::Result<()> {
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
            let tcp = TcpStream::connect(config.get_addr())
                .await
                .map_err(sqlx::Error::Io)?;
            tcp.set_nodelay(true).map_err(sqlx::Error::Io)?;
            let mut client = tiberius::Client::connect(config, tcp.compat_write())
                .await
                .map_err(map_tiberius_error)?;
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

impl MssqlTransaction {
    pub(crate) async fn fetch_rows(
        &mut self,
        sql: &str,
        values: &[SqlValue],
    ) -> crate::Result<Vec<MssqlRow>> {
        let lease = self.lease.as_mut().ok_or_else(inactive_transaction)?;
        let result = self
            .pool
            .bounded(fetch_rows_with_client(&mut lease.client, sql, values))
            .await;
        result.map_err(|failure| self.poison(failure))
    }

    pub(crate) async fn execute(
        &mut self,
        sql: &str,
        values: &[SqlValue],
    ) -> crate::Result<MssqlWriteResult> {
        let lease = self.lease.as_mut().ok_or_else(inactive_transaction)?;
        let result = self
            .pool
            .bounded(execute_with_client(&mut lease.client, sql, values))
            .await;
        result.map_err(|failure| self.poison(failure))
    }

    pub(crate) async fn commit(self) -> crate::Result<()> {
        self.finish(COMMIT_TRANSACTION).await
    }

    pub(crate) async fn rollback(self) -> crate::Result<()> {
        self.finish("ROLLBACK TRANSACTION;").await
    }

    /// End the transaction and reset the session in one batch, then return
    /// the connection to the pool. On any failure the connection is closed.
    async fn finish(mut self, end: &str) -> crate::Result<()> {
        let mut lease = self.lease.take().ok_or_else(inactive_transaction)?;
        let batch = format!("{end} {SESSION_RESET}");
        self.pool
            .bounded(execute_control_statement(&mut lease.client, &batch))
            .await
            .map_err(|failure| failure.error)?;
        self.pool.release(lease);
        Ok(())
    }

    fn poison(&mut self, failure: StatementFailure) -> sqlx::Error {
        self.lease.take();
        failure.error
    }
}

fn inactive_transaction() -> sqlx::Error {
    sqlx::Error::Protocol("SQL Server transaction is no longer active".to_string())
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

impl MssqlScalar for String {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
    where
        I: MssqlColumnIndex,
    {
        if let Ok(value) = index.try_get_raw::<&str>(&row.inner) {
            return Ok(value.map(str::to_owned));
        }
        if let Ok(value) = index.try_get_raw::<tiberius::time::chrono::NaiveDateTime>(&row.inner) {
            return Ok(value.map(|value| value.to_string()));
        }
        if let Ok(value) = index.try_get_raw::<tiberius::time::chrono::NaiveDate>(&row.inner) {
            return Ok(value.map(|value| value.to_string()));
        }
        if let Ok(value) = index.try_get_raw::<tiberius::time::chrono::NaiveTime>(&row.inner) {
            return Ok(value.map(|value| value.to_string()));
        }
        if let Ok(value) = index.try_get_raw::<uuid::Uuid>(&row.inner) {
            return Ok(value.map(|value| value.to_string()));
        }

        Err(sqlx::Error::ColumnDecode {
            index: index.display(),
            source: "could not decode SQL Server column as String".into(),
        })
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
        if let Ok(value) = index.try_get_raw::<bool>(&row.inner) {
            return Ok(value);
        }
        Ok(index
            .try_get_raw::<i32>(&row.inner)
            .map_err(map_tiberius_error)?
            .map(|value| value != 0))
    }
}

impl MssqlScalar for i32 {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
    where
        I: MssqlColumnIndex,
    {
        if let Ok(value) = index.try_get_raw::<i32>(&row.inner) {
            return Ok(value);
        }
        if let Ok(value) = index.try_get_raw::<i16>(&row.inner) {
            return Ok(value.map(i32::from));
        }
        if let Ok(value) = index.try_get_raw::<u8>(&row.inner) {
            return Ok(value.map(i32::from));
        }
        let value = index
            .try_get_raw::<i64>(&row.inner)
            .map_err(map_tiberius_error)?;
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
        if let Ok(value) = index.try_get_raw::<i64>(&row.inner) {
            return Ok(value);
        }
        if let Ok(value) = index.try_get_raw::<i32>(&row.inner) {
            return Ok(value.map(i64::from));
        }
        if let Ok(value) = index.try_get_raw::<i16>(&row.inner) {
            return Ok(value.map(i64::from));
        }
        Ok(index
            .try_get_raw::<u8>(&row.inner)
            .map_err(map_tiberius_error)?
            .map(i64::from))
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
        if let Ok(value) = index.try_get_raw::<f64>(&row.inner) {
            return Ok(value);
        }
        Ok(index
            .try_get_raw::<f32>(&row.inner)
            .map_err(map_tiberius_error)?
            .map(f64::from))
    }
}

impl MssqlScalar for rust_decimal::Decimal {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> crate::Result<Option<Self>>
    where
        I: MssqlColumnIndex,
    {
        index
            .try_get_raw::<rust_decimal::Decimal>(&row.inner)
            .map_err(map_tiberius_error)
    }
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

async fn fetch_rows_with_client(
    client: &mut MssqlClient,
    sql: &str,
    values: &[SqlValue],
) -> Result<Vec<MssqlRow>, StatementFailure> {
    let mut query = Query::new(sql.to_string());
    for value in values.iter().map(MssqlParamValue::from) {
        query.bind(value);
    }
    let stream = query.query(client).await.map_err(statement_failure)?;
    let rows = stream
        .into_first_result()
        .await
        .map_err(statement_failure)?;
    Ok(rows.into_iter().map(MssqlRow::new).collect())
}

async fn execute_with_client(
    client: &mut MssqlClient,
    sql: &str,
    values: &[SqlValue],
) -> Result<MssqlWriteResult, StatementFailure> {
    let mut query = Query::new(sql.to_string());
    for value in values.iter().map(MssqlParamValue::from) {
        query.bind(value);
    }
    let result = query.execute(client).await.map_err(statement_failure)?;
    Ok(MssqlWriteResult {
        rows_affected: statement_rows_affected(result.rows_affected()),
    })
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
    client
        .simple_query(sql)
        .await
        .map_err(statement_failure)?
        .into_results()
        .await
        .map_err(statement_failure)?;
    Ok(())
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

impl From<&SqlValue> for MssqlParamValue {
    fn from(value: &SqlValue) -> Self {
        match value {
            SqlValue::String(value) => Self::String(value.clone()),
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
        }
    }
}

fn map_tiberius_error(error: tiberius::error::Error) -> sqlx::Error {
    sqlx::Error::Protocol(error.to_string())
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
    fn writable_mode_requires_the_explicit_constructor() {
        let pool = MssqlPool::new_external_writable(config());
        assert_eq!(pool.access_mode(), MssqlAccessMode::ExternalWritable);
    }
}
