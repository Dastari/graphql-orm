use super::core::{SqlValue, record_executed_query};
use crate::{DbPool, DbRow};

pub struct Migration {
    pub version: &'static str,
    pub description: &'static str,
    pub statements: &'static [&'static str],
}

pub trait MigrationSource {
    fn migrations() -> &'static [Migration] {
        &[]
    }
}

#[allow(async_fn_in_trait)]
pub trait MigrationRunner {
    async fn apply_migrations(&self, migrations: &[Migration]) -> Result<(), sqlx::Error>;
}

impl MigrationRunner for crate::db::Database {
    async fn apply_migrations(&self, migrations: &[Migration]) -> Result<(), sqlx::Error> {
        for migration in migrations {
            for statement in migration.statements {
                execute_with_binds(statement, &[], self.pool()).await?;
            }
        }
        Ok(())
    }
}

#[cfg(feature = "sqlite")]
pub async fn execute_with_binds(
    sql: &str,
    values: &[SqlValue],
    pool: &DbPool,
) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
    record_executed_query();
    let mut query = sqlx::query(sql);
    for value in values {
        query = match value {
            SqlValue::String(value) => query.bind(value),
            SqlValue::Uuid(value) => query.bind(crate::db::sqlite_helpers::uuid_to_string(value)),
            SqlValue::Int(value) => query.bind(*value),
            SqlValue::Float(value) => query.bind(*value),
            SqlValue::Bool(value) => query.bind(*value),
            SqlValue::Null => query.bind(Option::<String>::None),
        };
    }
    query.execute(pool).await
}

#[cfg(feature = "postgres")]
pub async fn execute_with_binds(
    sql: &str,
    values: &[SqlValue],
    pool: &DbPool,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    record_executed_query();
    let sql = super::query::normalize_sql(sql, 1);
    let mut query = sqlx::query(&sql);
    for value in values {
        query = match value {
            SqlValue::String(value) => query.bind(value),
            SqlValue::Uuid(value) => query.bind(*value),
            SqlValue::Int(value) => query.bind(*value),
            SqlValue::Float(value) => query.bind(*value),
            SqlValue::Bool(value) => query.bind(*value),
            SqlValue::Null => query.bind(Option::<String>::None),
        };
    }
    query.execute(pool).await
}

pub async fn fetch_rows(
    pool: &DbPool,
    sql: &str,
    values: &[SqlValue],
) -> Result<Vec<DbRow>, sqlx::Error> {
    record_executed_query();
    #[cfg(feature = "sqlite")]
    {
        let mut query = sqlx::query(sql);
        for value in values {
            query = match value {
                SqlValue::String(value) => query.bind(value),
                SqlValue::Uuid(value) => {
                    query.bind(crate::db::sqlite_helpers::uuid_to_string(value))
                }
                SqlValue::Int(value) => query.bind(*value),
                SqlValue::Float(value) => query.bind(*value),
                SqlValue::Bool(value) => query.bind(*value),
                SqlValue::Null => query.bind(Option::<String>::None),
            };
        }
        query.fetch_all(pool).await
    }

    #[cfg(feature = "postgres")]
    {
        let sql = super::query::normalize_sql(sql, 1);
        let mut query = sqlx::query(&sql);
        for value in values {
            query = match value {
                SqlValue::String(value) => query.bind(value),
                SqlValue::Uuid(value) => query.bind(*value),
                SqlValue::Int(value) => query.bind(*value),
                SqlValue::Float(value) => query.bind(*value),
                SqlValue::Bool(value) => query.bind(*value),
                SqlValue::Null => query.bind(Option::<String>::None),
            };
        }
        query.fetch_all(pool).await
    }
}
