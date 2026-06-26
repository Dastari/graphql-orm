use super::core::SqlValue;
use super::dialect::{DatabaseBackend, SqlDialect};
use futures::future::BoxFuture;

pub trait OrmBackend: Copy + Clone + Send + Sync + 'static {
    type Pool: Clone + Send + Sync + 'static;
    type Row: Send + Sync + 'static;

    const DIALECT: DatabaseBackend;
    const READ_ONLY: bool;

    fn fetch_rows<'a>(
        pool: &'a Self::Pool,
        sql: &'a str,
        values: &'a [SqlValue],
    ) -> BoxFuture<'a, Result<Vec<Self::Row>, sqlx::Error>>;

    fn try_get_i64(row: &Self::Row, column: &str) -> Result<i64, sqlx::Error>;
    fn try_get_string(row: &Self::Row, column: &str) -> Result<String, sqlx::Error>;

    fn placeholder(index: usize) -> String {
        Self::DIALECT.placeholder(index)
    }

    fn normalize_sql(sql: &str, start_index: usize) -> String {
        Self::DIALECT.normalize_sql(sql, start_index)
    }
}

pub trait SqlxBackend: OrmBackend {
    type Database: sqlx::Database;
    type QueryResult;

    fn fetch_rows_on<'e, E>(
        executor: E,
        sql: String,
        values: Vec<SqlValue>,
    ) -> BoxFuture<'e, Result<Vec<Self::Row>, sqlx::Error>>
    where
        E: sqlx::Executor<'e, Database = Self::Database> + Send + 'e;

    fn execute_with_binds<'a>(
        pool: &'a Self::Pool,
        sql: &'a str,
        values: &'a [SqlValue],
    ) -> BoxFuture<'a, Result<Self::QueryResult, sqlx::Error>>;

    fn execute_with_binds_on<'e, E>(
        executor: E,
        sql: String,
        values: Vec<SqlValue>,
    ) -> BoxFuture<'e, Result<Self::QueryResult, sqlx::Error>>
    where
        E: sqlx::Executor<'e, Database = Self::Database> + Send + 'e;
}

pub trait WriteBackend: SqlxBackend {}
#[allow(async_fn_in_trait)]
pub trait MigrationBackend: WriteBackend {
    async fn prepare_migration_runtime(pool: &Self::Pool) -> Result<(), sqlx::Error>;

    async fn load_applied_migration_versions(
        pool: &Self::Pool,
    ) -> Result<std::collections::HashSet<String>, sqlx::Error>;

    async fn apply_migration_statements_transactionally<S>(
        pool: &Self::Pool,
        version: &str,
        description: &str,
        statements: &[S],
    ) -> Result<(), sqlx::Error>
    where
        S: AsRef<str> + Send + Sync;
}
pub trait SubscriptionBackend: WriteBackend {}

#[derive(Copy, Clone, Debug)]
pub struct SqliteBackend;

#[derive(Copy, Clone, Debug)]
pub struct PostgresBackend;

#[derive(Copy, Clone, Debug)]
pub struct MssqlBackend;

#[derive(Copy, Clone, Debug)]
pub struct NoDefaultBackend;

#[cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
pub type DefaultBackend = SqliteBackend;

#[cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]
pub type DefaultBackend = PostgresBackend;

#[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
pub type DefaultBackend = MssqlBackend;

#[cfg(not(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))),
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres")))
)))]
pub type DefaultBackend = NoDefaultBackend;

#[cfg(feature = "sqlite")]
pub type DefaultWriteBackend = SqliteBackend;

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
pub type DefaultWriteBackend = PostgresBackend;

#[cfg(not(any(feature = "sqlite", feature = "postgres")))]
pub type DefaultWriteBackend = NoDefaultBackend;

impl OrmBackend for NoDefaultBackend {
    type Pool = ();
    type Row = ();

    const DIALECT: DatabaseBackend = DatabaseBackend::Sqlite;
    const READ_ONLY: bool = true;

    fn fetch_rows<'a>(
        _pool: &'a Self::Pool,
        _sql: &'a str,
        _values: &'a [SqlValue],
    ) -> BoxFuture<'a, Result<Vec<Self::Row>, sqlx::Error>> {
        Box::pin(async {
            Err(sqlx::Error::Protocol(
                "graphql-orm backend is ambiguous; specify an entity/root backend".to_string(),
            ))
        })
    }

    fn try_get_i64(_row: &Self::Row, column: &str) -> Result<i64, sqlx::Error> {
        Err(sqlx::Error::ColumnNotFound(column.to_string()))
    }

    fn try_get_string(_row: &Self::Row, column: &str) -> Result<String, sqlx::Error> {
        Err(sqlx::Error::ColumnNotFound(column.to_string()))
    }
}

#[cfg(feature = "sqlite")]
impl OrmBackend for SqliteBackend {
    type Pool = sqlx::SqlitePool;
    type Row = sqlx::sqlite::SqliteRow;

    const DIALECT: DatabaseBackend = DatabaseBackend::Sqlite;
    const READ_ONLY: bool = false;

    fn fetch_rows<'a>(
        pool: &'a Self::Pool,
        sql: &'a str,
        values: &'a [SqlValue],
    ) -> BoxFuture<'a, Result<Vec<Self::Row>, sqlx::Error>> {
        Box::pin(async move {
            let mut query = sqlx::query(sql);
            for value in values {
                query = bind_sqlite_value(query, value);
            }
            query.fetch_all(pool).await
        })
    }

    fn try_get_i64(row: &Self::Row, column: &str) -> Result<i64, sqlx::Error> {
        use sqlx::Row;
        row.try_get(column)
    }

    fn try_get_string(row: &Self::Row, column: &str) -> Result<String, sqlx::Error> {
        use sqlx::Row;
        row.try_get(column)
    }
}

#[cfg(feature = "sqlite")]
impl SqlxBackend for SqliteBackend {
    type Database = sqlx::Sqlite;
    type QueryResult = sqlx::sqlite::SqliteQueryResult;

    fn fetch_rows_on<'e, E>(
        executor: E,
        sql: String,
        values: Vec<SqlValue>,
    ) -> BoxFuture<'e, Result<Vec<Self::Row>, sqlx::Error>>
    where
        E: sqlx::Executor<'e, Database = Self::Database> + Send + 'e,
    {
        Box::pin(async move {
            let mut query = sqlx::query(&sql);
            for value in &values {
                query = bind_sqlite_value(query, value);
            }
            query.fetch_all(executor).await
        })
    }

    fn execute_with_binds<'a>(
        pool: &'a Self::Pool,
        sql: &'a str,
        values: &'a [SqlValue],
    ) -> BoxFuture<'a, Result<Self::QueryResult, sqlx::Error>> {
        Box::pin(async move {
            let mut query = sqlx::query(sql);
            for value in values {
                query = bind_sqlite_value(query, value);
            }
            query.execute(pool).await
        })
    }

    fn execute_with_binds_on<'e, E>(
        executor: E,
        sql: String,
        values: Vec<SqlValue>,
    ) -> BoxFuture<'e, Result<Self::QueryResult, sqlx::Error>>
    where
        E: sqlx::Executor<'e, Database = Self::Database> + Send + 'e,
    {
        Box::pin(async move {
            let mut query = sqlx::query(&sql);
            for value in &values {
                query = bind_sqlite_value(query, value);
            }
            query.execute(executor).await
        })
    }
}

#[cfg(feature = "sqlite")]
impl WriteBackend for SqliteBackend {}
#[cfg(feature = "sqlite")]
impl SubscriptionBackend for SqliteBackend {}

#[cfg(feature = "postgres")]
impl OrmBackend for PostgresBackend {
    type Pool = sqlx::PgPool;
    type Row = sqlx::postgres::PgRow;

    const DIALECT: DatabaseBackend = DatabaseBackend::Postgres;
    const READ_ONLY: bool = false;

    fn fetch_rows<'a>(
        pool: &'a Self::Pool,
        sql: &'a str,
        values: &'a [SqlValue],
    ) -> BoxFuture<'a, Result<Vec<Self::Row>, sqlx::Error>> {
        Box::pin(async move {
            let sql = Self::normalize_sql(sql, 1);
            let mut query = sqlx::query(&sql);
            for value in values {
                query = bind_postgres_value(query, value);
            }
            query.fetch_all(pool).await
        })
    }

    fn try_get_i64(row: &Self::Row, column: &str) -> Result<i64, sqlx::Error> {
        use sqlx::Row;
        row.try_get(column)
    }

    fn try_get_string(row: &Self::Row, column: &str) -> Result<String, sqlx::Error> {
        use sqlx::Row;
        row.try_get(column)
    }
}

#[cfg(feature = "postgres")]
impl SqlxBackend for PostgresBackend {
    type Database = sqlx::Postgres;
    type QueryResult = sqlx::postgres::PgQueryResult;

    fn fetch_rows_on<'e, E>(
        executor: E,
        sql: String,
        values: Vec<SqlValue>,
    ) -> BoxFuture<'e, Result<Vec<Self::Row>, sqlx::Error>>
    where
        E: sqlx::Executor<'e, Database = Self::Database> + Send + 'e,
    {
        Box::pin(async move {
            let sql = Self::normalize_sql(&sql, 1);
            let mut query = sqlx::query(&sql);
            for value in &values {
                query = bind_postgres_value(query, value);
            }
            query.fetch_all(executor).await
        })
    }

    fn execute_with_binds<'a>(
        pool: &'a Self::Pool,
        sql: &'a str,
        values: &'a [SqlValue],
    ) -> BoxFuture<'a, Result<Self::QueryResult, sqlx::Error>> {
        Box::pin(async move {
            let sql = Self::normalize_sql(sql, 1);
            let mut query = sqlx::query(&sql);
            for value in values {
                query = bind_postgres_value(query, value);
            }
            query.execute(pool).await
        })
    }

    fn execute_with_binds_on<'e, E>(
        executor: E,
        sql: String,
        values: Vec<SqlValue>,
    ) -> BoxFuture<'e, Result<Self::QueryResult, sqlx::Error>>
    where
        E: sqlx::Executor<'e, Database = Self::Database> + Send + 'e,
    {
        Box::pin(async move {
            let sql = Self::normalize_sql(&sql, 1);
            let mut query = sqlx::query(&sql);
            for value in &values {
                query = bind_postgres_value(query, value);
            }
            query.execute(executor).await
        })
    }
}

#[cfg(feature = "postgres")]
impl WriteBackend for PostgresBackend {}
#[cfg(feature = "postgres")]
impl SubscriptionBackend for PostgresBackend {}

#[cfg(feature = "mssql")]
impl OrmBackend for MssqlBackend {
    type Pool = crate::db::mssql::MssqlPool;
    type Row = crate::db::mssql::MssqlRow;

    const DIALECT: DatabaseBackend = DatabaseBackend::Mssql;
    const READ_ONLY: bool = true;

    fn fetch_rows<'a>(
        pool: &'a Self::Pool,
        sql: &'a str,
        values: &'a [SqlValue],
    ) -> BoxFuture<'a, Result<Vec<Self::Row>, sqlx::Error>> {
        Box::pin(async move { pool.fetch_rows(sql, values).await })
    }

    fn try_get_i64(row: &Self::Row, column: &str) -> Result<i64, sqlx::Error> {
        row.try_get(column)
    }

    fn try_get_string(row: &Self::Row, column: &str) -> Result<String, sqlx::Error> {
        row.try_get(column)
    }
}

#[cfg(feature = "sqlite")]
fn bind_sqlite_value<'q>(
    query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    value: &'q SqlValue,
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    match value {
        SqlValue::String(value) => query.bind(value),
        SqlValue::Bytes(value) => query.bind(value),
        SqlValue::BytesNull => query.bind(Option::<Vec<u8>>::None),
        SqlValue::Json(value) => query.bind(value.to_string()),
        SqlValue::JsonNull => query.bind(Option::<String>::None),
        SqlValue::Uuid(value) => query.bind(crate::db::sqlite_helpers::uuid_to_string(value)),
        SqlValue::Int(value) => query.bind(*value),
        SqlValue::Float(value) => query.bind(*value),
        SqlValue::Bool(value) => query.bind(*value),
        SqlValue::Null => query.bind(Option::<String>::None),
    }
}

#[cfg(feature = "postgres")]
fn bind_postgres_value<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    value: &'q SqlValue,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match value {
        SqlValue::String(value) => query.bind(value),
        SqlValue::Bytes(value) => query.bind(value),
        SqlValue::BytesNull => query.bind(Option::<Vec<u8>>::None),
        SqlValue::Json(value) => query.bind(sqlx::types::Json(value.clone())),
        SqlValue::JsonNull => query.bind(Option::<sqlx::types::Json<serde_json::Value>>::None),
        SqlValue::Uuid(value) => query.bind(*value),
        SqlValue::Int(value) => query.bind(*value),
        SqlValue::Float(value) => query.bind(*value),
        SqlValue::Bool(value) => query.bind(*value),
        SqlValue::Null => query.bind(Option::<String>::None),
    }
}
