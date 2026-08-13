use crate::graphql::orm::SqlValue;
use std::borrow::Cow;
use std::sync::Arc;
use tiberius::{ColumnData, Config, Query};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
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

/// One connection-pinned SQL Server transaction.
///
/// The client is returned to the pool only after a successful explicit commit
/// or rollback. Cancellation, protocol errors, and drop discard the socket.
pub struct MssqlTransaction {
    pool: MssqlPool,
    client: Option<MssqlClient>,
    permit: Option<OwnedSemaphorePermit>,
}

#[derive(Clone)]
pub struct MssqlPool {
    inner: Arc<MssqlPoolInner>,
}

struct MssqlPoolInner {
    config: Config,
    access_mode: MssqlAccessMode,
    idle: Mutex<Vec<MssqlClient>>,
    permits: Arc<Semaphore>,
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
    pub async fn connect_ado(connection_string: &str) -> crate::Result<Self> {
        let mut config = Config::from_ado_string(connection_string).map_err(map_tiberius_error)?;
        config.readonly(true);
        Ok(Self::new(config))
    }

    /// Open a pool that deliberately permits DML against an externally managed schema.
    pub async fn connect_ado_external_writable(connection_string: &str) -> crate::Result<Self> {
        let mut config = Config::from_ado_string(connection_string).map_err(map_tiberius_error)?;
        config.readonly(false);
        Ok(Self::new_external_writable(config))
    }

    pub fn new(config: Config) -> Self {
        Self::with_max_connections(config, 5)
    }

    pub fn with_max_connections(config: Config, max_connections: usize) -> Self {
        Self::with_access_mode(config, max_connections, MssqlAccessMode::ReadOnly)
    }

    /// Construct a deliberately writable pool for an externally managed schema.
    pub fn new_external_writable(config: Config) -> Self {
        Self::with_max_connections_external_writable(config, 5)
    }

    /// Construct a deliberately writable pool with a bounded connection count.
    pub fn with_max_connections_external_writable(config: Config, max_connections: usize) -> Self {
        Self::with_access_mode(config, max_connections, MssqlAccessMode::ExternalWritable)
    }

    fn with_access_mode(
        mut config: Config,
        max_connections: usize,
        access_mode: MssqlAccessMode,
    ) -> Self {
        config.readonly(matches!(access_mode, MssqlAccessMode::ReadOnly));
        Self {
            inner: Arc::new(MssqlPoolInner {
                config,
                access_mode,
                idle: Mutex::new(Vec::new()),
                permits: Arc::new(Semaphore::new(max_connections.max(1))),
            }),
        }
    }

    /// Report the immutable physical access mode.
    pub fn access_mode(&self) -> MssqlAccessMode {
        self.inner.access_mode
    }

    pub async fn fetch_rows(&self, sql: &str, values: &[SqlValue]) -> crate::Result<Vec<MssqlRow>> {
        let (mut client, permit) = self.acquire_client().await?;
        let result = fetch_rows_with_client(&mut client, sql, values).await;
        if result.is_ok() {
            self.release_client(client).await;
        }
        drop(permit);
        result
    }

    pub(crate) async fn execute(
        &self,
        sql: &str,
        values: &[SqlValue],
    ) -> crate::Result<MssqlWriteResult> {
        self.require_external_writable()?;
        let (mut client, permit) = self.acquire_client().await?;
        let result = execute_with_client(&mut client, sql, values).await;
        if result.is_ok() {
            self.release_client(client).await;
        }
        drop(permit);
        result
    }

    pub(crate) async fn begin_transaction(
        &self,
        mode: crate::graphql::orm::TransactionMode,
    ) -> crate::Result<MssqlTransaction> {
        self.require_external_writable()?;
        let (mut client, permit) = self.acquire_client().await?;
        let isolation = match mode {
            crate::graphql::orm::TransactionMode::Default => "READ COMMITTED",
            crate::graphql::orm::TransactionMode::StateMachine => "SERIALIZABLE",
        };
        execute_control_statement(
            &mut client,
            &format!("SET TRANSACTION ISOLATION LEVEL {isolation}; BEGIN TRANSACTION;"),
        )
        .await?;
        Ok(MssqlTransaction {
            pool: self.clone(),
            client: Some(client),
            permit: Some(permit),
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

    async fn acquire_client(&self) -> crate::Result<(MssqlClient, OwnedSemaphorePermit)> {
        let permit = self
            .inner
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;

        if let Some(client) = self.inner.idle.lock().await.pop() {
            return Ok((client, permit));
        }

        let tcp = TcpStream::connect(self.inner.config.get_addr())
            .await
            .map_err(sqlx::Error::Io)?;
        tcp.set_nodelay(true).map_err(sqlx::Error::Io)?;
        let client = tiberius::Client::connect(self.inner.config.clone(), tcp.compat_write())
            .await
            .map_err(map_tiberius_error)?;
        Ok((client, permit))
    }

    async fn release_client(&self, client: MssqlClient) {
        self.inner.idle.lock().await.push(client);
    }
}

impl MssqlTransaction {
    pub(crate) async fn fetch_rows(
        &mut self,
        sql: &str,
        values: &[SqlValue],
    ) -> crate::Result<Vec<MssqlRow>> {
        let client = self.client.as_mut().ok_or_else(|| {
            sqlx::Error::Protocol("SQL Server transaction is no longer active".to_string())
        })?;
        let result = fetch_rows_with_client(client, sql, values).await;
        if result.is_err() {
            self.poison();
        }
        result
    }

    pub(crate) async fn execute(
        &mut self,
        sql: &str,
        values: &[SqlValue],
    ) -> crate::Result<MssqlWriteResult> {
        let client = self.client.as_mut().ok_or_else(|| {
            sqlx::Error::Protocol("SQL Server transaction is no longer active".to_string())
        })?;
        let result = execute_with_client(client, sql, values).await;
        if result.is_err() {
            self.poison();
        }
        result
    }

    pub(crate) async fn commit(mut self) -> crate::Result<()> {
        let mut client = self.client.take().ok_or_else(|| {
            sqlx::Error::Protocol("SQL Server transaction is no longer active".to_string())
        })?;
        execute_control_statement(&mut client, "COMMIT TRANSACTION;").await?;
        self.pool.release_client(client).await;
        self.permit.take();
        Ok(())
    }

    pub(crate) async fn rollback(mut self) -> crate::Result<()> {
        let mut client = self.client.take().ok_or_else(|| {
            sqlx::Error::Protocol("SQL Server transaction is no longer active".to_string())
        })?;
        execute_control_statement(&mut client, "ROLLBACK TRANSACTION;").await?;
        self.pool.release_client(client).await;
        self.permit.take();
        Ok(())
    }

    fn poison(&mut self) {
        self.client.take();
        self.permit.take();
    }
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
) -> crate::Result<Vec<MssqlRow>> {
    let mut query = Query::new(sql.to_string());
    for value in values.iter().map(MssqlParamValue::from) {
        query.bind(value);
    }
    let stream = query.query(client).await.map_err(map_tiberius_error)?;
    let rows = stream
        .into_first_result()
        .await
        .map_err(map_tiberius_error)?;
    Ok(rows.into_iter().map(MssqlRow::new).collect())
}

async fn execute_with_client(
    client: &mut MssqlClient,
    sql: &str,
    values: &[SqlValue],
) -> crate::Result<MssqlWriteResult> {
    let mut query = Query::new(sql.to_string());
    for value in values.iter().map(MssqlParamValue::from) {
        query.bind(value);
    }
    let result = query.execute(client).await.map_err(map_tiberius_error)?;
    Ok(MssqlWriteResult {
        rows_affected: result.total(),
    })
}

async fn execute_control_statement(client: &mut MssqlClient, sql: &str) -> crate::Result<()> {
    client
        .simple_query(sql)
        .await
        .map_err(map_tiberius_error)?
        .into_results()
        .await
        .map_err(map_tiberius_error)?;
    Ok(())
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
