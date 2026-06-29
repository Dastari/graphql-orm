use crate::graphql::orm::SqlValue;
use std::borrow::Cow;
use std::sync::Arc;
use tiberius::{ColumnData, Config, Query};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

pub type MssqlClient = tiberius::Client<Compat<TcpStream>>;

#[derive(Clone)]
pub struct MssqlPool {
    inner: Arc<MssqlPoolInner>,
}

struct MssqlPoolInner {
    config: Config,
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
    NullString,
    NullBytes,
    NullUuid,
    NullInt,
    NullFloat,
    NullBool,
}

pub trait MssqlColumnIndex: Copy {
    fn try_get_raw<'a, T>(self, row: &'a tiberius::Row) -> tiberius::Result<Option<T>>
    where
        T: tiberius::FromSql<'a>;

    fn display(self) -> String;
}

pub trait MssqlScalar: Sized {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> Result<Option<Self>, sqlx::Error>
    where
        I: MssqlColumnIndex;
}

pub trait MssqlDecode: Sized {
    fn try_get<I>(row: &MssqlRow, index: I) -> Result<Self, sqlx::Error>
    where
        I: MssqlColumnIndex;
}

impl MssqlPool {
    pub async fn connect_ado(connection_string: &str) -> Result<Self, sqlx::Error> {
        let mut config = Config::from_ado_string(connection_string).map_err(map_tiberius_error)?;
        config.readonly(true);
        Ok(Self::new(config))
    }

    pub fn new(config: Config) -> Self {
        Self::with_max_connections(config, 5)
    }

    pub fn with_max_connections(config: Config, max_connections: usize) -> Self {
        Self {
            inner: Arc::new(MssqlPoolInner {
                config,
                idle: Mutex::new(Vec::new()),
                permits: Arc::new(Semaphore::new(max_connections.max(1))),
            }),
        }
    }

    pub async fn fetch_rows(
        &self,
        sql: &str,
        values: &[SqlValue],
    ) -> Result<Vec<MssqlRow>, sqlx::Error> {
        let (mut client, permit) = self.acquire_client().await?;
        let result = fetch_rows_with_client(&mut client, sql, values).await;
        if result.is_ok() {
            self.release_client(client).await;
        }
        drop(permit);
        result
    }

    async fn acquire_client(&self) -> Result<(MssqlClient, OwnedSemaphorePermit), sqlx::Error> {
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
            .map_err(|error| sqlx::Error::Io(error))?;
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

impl std::fmt::Debug for MssqlPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MssqlPool").finish_non_exhaustive()
    }
}

impl MssqlRow {
    pub fn new(inner: tiberius::Row) -> Self {
        Self { inner }
    }

    pub fn try_get<T, I>(&self, index: I) -> Result<T, sqlx::Error>
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
    fn try_get<I>(row: &MssqlRow, index: I) -> Result<Self, sqlx::Error>
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
    fn try_get<I>(row: &MssqlRow, index: I) -> Result<Self, sqlx::Error>
    where
        I: MssqlColumnIndex,
    {
        T::try_get_optional(row, index)
    }
}

impl MssqlScalar for String {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> Result<Option<Self>, sqlx::Error>
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
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> Result<Option<Self>, sqlx::Error>
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
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> Result<Option<Self>, sqlx::Error>
    where
        I: MssqlColumnIndex,
    {
        index
            .try_get_raw::<uuid::Uuid>(&row.inner)
            .map_err(map_tiberius_error)
    }
}

impl MssqlScalar for bool {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> Result<Option<Self>, sqlx::Error>
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
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> Result<Option<Self>, sqlx::Error>
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
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> Result<Option<Self>, sqlx::Error>
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
            fn try_get_optional<I>(row: &MssqlRow, index: I) -> Result<Option<Self>, sqlx::Error>
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
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> Result<Option<Self>, sqlx::Error>
    where
        I: MssqlColumnIndex,
    {
        index
            .try_get_raw::<f32>(&row.inner)
            .map_err(map_tiberius_error)
    }
}

impl MssqlScalar for f64 {
    fn try_get_optional<I>(row: &MssqlRow, index: I) -> Result<Option<Self>, sqlx::Error>
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

impl<'a> tiberius::IntoSql<'a> for MssqlParamValue {
    fn into_sql(self) -> ColumnData<'a> {
        match self {
            Self::String(value) => ColumnData::String(Some(Cow::Owned(value))),
            Self::Bytes(value) => ColumnData::Binary(Some(Cow::Owned(value))),
            Self::Uuid(value) => ColumnData::Guid(Some(value)),
            Self::Int(value) => ColumnData::I64(Some(value)),
            Self::Float(value) => ColumnData::F64(Some(value)),
            Self::Bool(value) => ColumnData::Bit(Some(value)),
            Self::NullString => ColumnData::String(None),
            Self::NullBytes => ColumnData::Binary(None),
            Self::NullUuid => ColumnData::Guid(None),
            Self::NullInt => ColumnData::I64(None),
            Self::NullFloat => ColumnData::F64(None),
            Self::NullBool => ColumnData::Bit(None),
        }
    }
}

async fn fetch_rows_with_client(
    client: &mut MssqlClient,
    sql: &str,
    values: &[SqlValue],
) -> Result<Vec<MssqlRow>, sqlx::Error> {
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
            SqlValue::Bool(value) => Self::Bool(*value),
            SqlValue::BoolNull => Self::NullBool,
            SqlValue::Null => Self::NullString,
        }
    }
}

fn map_tiberius_error(error: tiberius::error::Error) -> sqlx::Error {
    sqlx::Error::Protocol(error.to_string())
}
