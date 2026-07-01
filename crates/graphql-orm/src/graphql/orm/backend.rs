use super::core::{
    AppliedMigrationRecord, DbAuthContext, MigrationApplicationMetadata, SchemaModel, SqlValue,
};
use super::dialect::{DatabaseBackend, SqlDialect};
use super::rls::LiveRlsTable;
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

    fn fetch_rows_with_auth<'a>(
        pool: &'a Self::Pool,
        sql: &'a str,
        values: &'a [SqlValue],
        _auth: Option<&'a DbAuthContext>,
    ) -> BoxFuture<'a, Result<Vec<Self::Row>, sqlx::Error>> {
        Self::fetch_rows(pool, sql, values)
    }

    fn fetch_rows_pair_with_auth<'a>(
        pool: &'a Self::Pool,
        first_sql: &'a str,
        first_values: &'a [SqlValue],
        second_sql: &'a str,
        second_values: &'a [SqlValue],
        _auth: Option<&'a DbAuthContext>,
    ) -> BoxFuture<'a, Result<(Vec<Self::Row>, Vec<Self::Row>), sqlx::Error>> {
        Box::pin(async move {
            let first = Self::fetch_rows(pool, first_sql, first_values).await?;
            let second = Self::fetch_rows(pool, second_sql, second_values).await?;
            Ok((first, second))
        })
    }

    fn try_get_i64(row: &Self::Row, column: &str) -> Result<i64, sqlx::Error>;
    fn try_get_f64(row: &Self::Row, column: &str) -> Result<f64, sqlx::Error>;
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

    fn apply_auth_context_to_transaction<'a>(
        _tx: &'a mut sqlx::Transaction<'_, Self::Database>,
        _auth: Option<&'a DbAuthContext>,
    ) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async { Ok(()) })
    }
}

pub trait WriteBackend: SqlxBackend {}
#[allow(async_fn_in_trait)]
/// Backend capability for read-only live schema inspection.
///
/// Backends that implement this trait can power validation against an existing
/// database without applying schema changes.
pub trait IntrospectionBackend: OrmBackend {
    /// Return a structured schema model for the live database.
    async fn introspect_schema(pool: &Self::Pool) -> Result<SchemaModel, sqlx::Error>;
}

#[allow(async_fn_in_trait)]
/// Backend capability for introspecting PostgreSQL RLS state.
///
/// Non-Postgres backends use the default empty implementation so full schema
/// target validation can remain backend-generic.
pub trait RlsIntrospectionBackend: OrmBackend {
    /// Return live table RLS flags and policies known to this backend.
    async fn introspect_rls(_pool: &Self::Pool) -> Result<Vec<LiveRlsTable>, sqlx::Error> {
        Ok(Vec::new())
    }
}

#[allow(async_fn_in_trait)]
/// Backend capability for applying explicit schema migrations.
///
/// SQLite and Postgres implement this trait. MSSQL intentionally does not,
/// which keeps migration application unavailable for the read-only SQL Server
/// backend at compile time in generic APIs.
pub trait MigrationBackend: IntrospectionBackend + WriteBackend {
    /// Prepare backend-owned migration infrastructure such as history tables.
    async fn prepare_migration_runtime(pool: &Self::Pool) -> Result<(), sqlx::Error>;

    /// Load applied migration records from the backend's history table.
    async fn load_applied_migrations(
        pool: &Self::Pool,
    ) -> Result<Vec<AppliedMigrationRecord>, sqlx::Error>;

    /// Apply rendered SQL statements transactionally and optionally record history.
    async fn apply_migration_statements_transactionally<S>(
        pool: &Self::Pool,
        version: &str,
        description: &str,
        statements: &[S],
        metadata: Option<&MigrationApplicationMetadata>,
        record_history: bool,
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

    fn try_get_f64(_row: &Self::Row, column: &str) -> Result<f64, sqlx::Error> {
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

    fn try_get_f64(row: &Self::Row, column: &str) -> Result<f64, sqlx::Error> {
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

    fn fetch_rows_with_auth<'a>(
        pool: &'a Self::Pool,
        sql: &'a str,
        values: &'a [SqlValue],
        auth: Option<&'a DbAuthContext>,
    ) -> BoxFuture<'a, Result<Vec<Self::Row>, sqlx::Error>> {
        Box::pin(async move {
            let Some(auth) = auth else {
                return Self::fetch_rows(pool, sql, values).await;
            };

            let mut tx = pool.begin().await?;
            apply_postgres_auth_context(&mut tx, auth).await?;
            let sql = Self::normalize_sql(sql, 1);
            let mut query = sqlx::query(&sql);
            for value in values {
                query = bind_postgres_value(query, value);
            }
            let rows = query.fetch_all(&mut *tx).await?;
            tx.commit().await?;
            Ok(rows)
        })
    }

    fn fetch_rows_pair_with_auth<'a>(
        pool: &'a Self::Pool,
        first_sql: &'a str,
        first_values: &'a [SqlValue],
        second_sql: &'a str,
        second_values: &'a [SqlValue],
        auth: Option<&'a DbAuthContext>,
    ) -> BoxFuture<'a, Result<(Vec<Self::Row>, Vec<Self::Row>), sqlx::Error>> {
        Box::pin(async move {
            let mut tx = pool.begin().await?;
            if let Some(auth) = auth {
                apply_postgres_auth_context(&mut tx, auth).await?;
            }

            let first_sql = Self::normalize_sql(first_sql, 1);
            let mut first_query = sqlx::query(&first_sql);
            for value in first_values {
                first_query = bind_postgres_value(first_query, value);
            }
            let first = first_query.fetch_all(&mut *tx).await?;

            let second_sql = Self::normalize_sql(second_sql, 1);
            let mut second_query = sqlx::query(&second_sql);
            for value in second_values {
                second_query = bind_postgres_value(second_query, value);
            }
            let second = second_query.fetch_all(&mut *tx).await?;

            tx.commit().await?;
            Ok((first, second))
        })
    }

    fn try_get_i64(row: &Self::Row, column: &str) -> Result<i64, sqlx::Error> {
        use sqlx::Row;
        row.try_get(column)
    }

    fn try_get_f64(row: &Self::Row, column: &str) -> Result<f64, sqlx::Error> {
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

    fn apply_auth_context_to_transaction<'a>(
        tx: &'a mut sqlx::Transaction<'_, Self::Database>,
        auth: Option<&'a DbAuthContext>,
    ) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async move {
            if let Some(auth) = auth {
                apply_postgres_auth_context(tx, auth).await?;
            }
            Ok(())
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

    fn try_get_f64(row: &Self::Row, column: &str) -> Result<f64, sqlx::Error> {
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
        SqlValue::StringNull => query.bind(Option::<String>::None),
        SqlValue::Bytes(value) => query.bind(value),
        SqlValue::BytesNull => query.bind(Option::<Vec<u8>>::None),
        SqlValue::Json(value) => query.bind(value.to_string()),
        SqlValue::JsonNull => query.bind(Option::<String>::None),
        SqlValue::Uuid(value) => query.bind(crate::db::sqlite_helpers::uuid_to_string(value)),
        SqlValue::UuidNull => query.bind(Option::<String>::None),
        SqlValue::Int(value) => query.bind(*value),
        SqlValue::IntNull => query.bind(Option::<i64>::None),
        SqlValue::Float(value) => query.bind(*value),
        SqlValue::FloatNull => query.bind(Option::<f64>::None),
        SqlValue::Bool(value) => query.bind(*value),
        SqlValue::BoolNull => query.bind(Option::<bool>::None),
        SqlValue::Null => query.bind(Option::<String>::None),
    }
}

#[cfg(feature = "postgres")]
async fn apply_postgres_auth_context(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    auth: &DbAuthContext,
) -> Result<(), sqlx::Error> {
    let settings = auth.postgres_settings()?;
    if settings.is_empty() {
        return Ok(());
    }
    let projections = settings
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let base = index * 2 + 1;
            format!("set_config(${base}, ${}, true)", base + 1)
        })
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("SELECT {projections}");
    let mut query = sqlx::query(&sql);
    for (setting, value) in &settings {
        query = query.bind(*setting).bind(value);
    }
    query.execute(&mut **tx).await?;
    Ok(())
}

#[cfg(feature = "postgres")]
fn bind_postgres_value<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    value: &'q SqlValue,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match value {
        SqlValue::String(value) => query.bind(value),
        SqlValue::StringNull => query.bind(Option::<String>::None),
        SqlValue::Bytes(value) => query.bind(value),
        SqlValue::BytesNull => query.bind(Option::<Vec<u8>>::None),
        SqlValue::Json(value) => query.bind(sqlx::types::Json(value.clone())),
        SqlValue::JsonNull => query.bind(Option::<sqlx::types::Json<serde_json::Value>>::None),
        SqlValue::Uuid(value) => query.bind(*value),
        SqlValue::UuidNull => query.bind(Option::<uuid::Uuid>::None),
        SqlValue::Int(value) => query.bind(*value),
        SqlValue::IntNull => query.bind(Option::<i64>::None),
        SqlValue::Float(value) => query.bind(*value),
        SqlValue::FloatNull => query.bind(Option::<f64>::None),
        SqlValue::Bool(value) => query.bind(*value),
        SqlValue::BoolNull => query.bind(Option::<bool>::None),
        SqlValue::Null => query.bind(Option::<String>::None),
    }
}
