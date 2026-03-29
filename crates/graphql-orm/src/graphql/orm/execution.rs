use super::core::{PlannedSchemaStage, SchemaStage, SqlValue, record_executed_query};
use super::dialect::current_backend;
use super::migrations::{build_migration_plan, introspect_schema};
use crate::{DbPool, DbRow};
use sqlx::Row;
use std::collections::HashSet;

const MIGRATION_HISTORY_TABLE: &str = "__graphql_orm_migrations";

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

#[allow(async_fn_in_trait)]
pub trait SchemaStageRunner {
    async fn plan_schema_stages(
        &self,
        stages: &[SchemaStage],
    ) -> Result<Vec<PlannedSchemaStage>, sqlx::Error>;

    async fn apply_schema_stages(&self, stages: &[SchemaStage]) -> Result<(), sqlx::Error>;
}

impl MigrationRunner for crate::db::Database {
    async fn apply_migrations(&self, migrations: &[Migration]) -> Result<(), sqlx::Error> {
        prepare_migration_runtime(self.pool()).await?;
        let mut applied_versions = load_applied_migration_versions(self.pool()).await?;
        for migration in migrations {
            if applied_versions.contains(migration.version) {
                continue;
            }
            apply_migration_statements_transactionally(
                self.pool(),
                migration.version,
                migration.description,
                migration.statements,
            )
            .await?;
            applied_versions.insert(migration.version.to_string());
        }
        Ok(())
    }
}

impl SchemaStageRunner for crate::db::Database {
    async fn plan_schema_stages(
        &self,
        stages: &[SchemaStage],
    ) -> Result<Vec<PlannedSchemaStage>, sqlx::Error> {
        prepare_migration_runtime(self.pool()).await?;
        validate_schema_stages(stages)?;

        let applied_versions = load_applied_migration_versions(self.pool()).await?;
        ensure_applied_stages_form_prefix(stages, &applied_versions)?;

        let mut current_schema = introspect_schema(self.pool()).await?;
        let mut planned = Vec::new();

        for stage in stages {
            if applied_versions.contains(&stage.version) {
                continue;
            }

            let plan =
                build_migration_plan(current_backend(), &current_schema, &stage.target_schema);
            planned.push(PlannedSchemaStage {
                version: stage.version.clone(),
                description: stage.description.clone(),
                plan,
            });
            current_schema = stage.target_schema.clone();
        }

        Ok(planned)
    }

    async fn apply_schema_stages(&self, stages: &[SchemaStage]) -> Result<(), sqlx::Error> {
        let planned = self.plan_schema_stages(stages).await?;
        for stage in planned {
            apply_migration_statements_transactionally(
                self.pool(),
                &stage.version,
                &stage.description,
                &stage.plan.statements,
            )
            .await?;
        }
        Ok(())
    }
}

async fn prepare_migration_runtime(pool: &DbPool) -> Result<(), sqlx::Error> {
    ensure_migration_history_table(pool).await?;
    #[cfg(feature = "sqlite")]
    cleanup_stale_sqlite_rewrite_tables(pool).await?;
    Ok(())
}

fn validate_schema_stages(stages: &[SchemaStage]) -> Result<(), sqlx::Error> {
    let mut seen = HashSet::new();
    for stage in stages {
        if stage.version.trim().is_empty() {
            return Err(sqlx::Error::Protocol(
                "Schema stage version must not be empty".to_string(),
            ));
        }
        if stage.description.trim().is_empty() {
            return Err(sqlx::Error::Protocol(
                "Schema stage description must not be empty".to_string(),
            ));
        }
        if !seen.insert(stage.version.clone()) {
            return Err(sqlx::Error::Protocol(format!(
                "Duplicate schema stage version: {}",
                stage.version
            )));
        }
    }
    Ok(())
}

fn ensure_applied_stages_form_prefix(
    stages: &[SchemaStage],
    applied_versions: &HashSet<String>,
) -> Result<(), sqlx::Error> {
    let mut seen_missing = false;
    for stage in stages {
        if applied_versions.contains(&stage.version) {
            if seen_missing {
                return Err(sqlx::Error::Protocol(format!(
                    "Applied schema stage {} appears after an unapplied earlier stage; staged migrations must be a contiguous prefix",
                    stage.version
                )));
            }
        } else {
            seen_missing = true;
        }
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn ensure_migration_history_table(pool: &DbPool) -> Result<(), sqlx::Error> {
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {} (
            version TEXT PRIMARY KEY,
            description TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
        MIGRATION_HISTORY_TABLE
    ))
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn ensure_migration_history_table(pool: &DbPool) -> Result<(), sqlx::Error> {
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {} (
            version TEXT PRIMARY KEY,
            description TEXT NOT NULL,
            applied_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
        MIGRATION_HISTORY_TABLE
    ))
    .execute(pool)
    .await?;
    Ok(())
}

async fn load_applied_migration_versions(pool: &DbPool) -> Result<HashSet<String>, sqlx::Error> {
    let rows = fetch_rows(
        pool,
        &format!(
            "SELECT version FROM {} ORDER BY version",
            MIGRATION_HISTORY_TABLE
        ),
        &[],
    )
    .await?;
    rows.into_iter()
        .map(|row| row.try_get::<String, _>("version"))
        .collect()
}

#[cfg(feature = "sqlite")]
async fn cleanup_stale_sqlite_rewrite_tables(pool: &DbPool) -> Result<(), sqlx::Error> {
    let rows = sqlx::query(
        "SELECT name
         FROM sqlite_master
         WHERE type = 'table'
           AND name LIKE '__graphql_orm\\_%\\_new' ESCAPE '\\'
         ORDER BY name",
    )
    .fetch_all(pool)
    .await?;

    for row in rows {
        let table_name: String = row.try_get("name")?;
        let escaped = table_name.replace('"', "\"\"");
        sqlx::query(&format!("DROP TABLE IF EXISTS \"{}\"", escaped))
            .execute(pool)
            .await?;
    }

    Ok(())
}

#[cfg(feature = "sqlite")]
async fn apply_migration_statements_transactionally<S>(
    pool: &DbPool,
    version: &str,
    description: &str,
    statements: &[S],
) -> Result<(), sqlx::Error>
where
    S: AsRef<str>,
{
    let mut tx = pool.begin().await?;
    for statement in statements {
        let statement = statement.as_ref().trim();
        if statement.is_empty()
            || statement == "PRAGMA foreign_keys = OFF"
            || statement == "PRAGMA foreign_keys = ON"
        {
            continue;
        }
        sqlx::query(statement).execute(&mut *tx).await?;
    }

    sqlx::query(&format!(
        "INSERT INTO {} (version, description) VALUES (?, ?)",
        MIGRATION_HISTORY_TABLE
    ))
    .bind(version)
    .bind(description)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn apply_migration_statements_transactionally<S>(
    pool: &DbPool,
    version: &str,
    description: &str,
    statements: &[S],
) -> Result<(), sqlx::Error>
where
    S: AsRef<str>,
{
    let mut tx = pool.begin().await?;
    for statement in statements {
        let statement = statement.as_ref().trim();
        if statement.is_empty() {
            continue;
        }
        sqlx::query(statement).execute(&mut *tx).await?;
    }

    sqlx::query(&format!(
        "INSERT INTO {} (version, description) VALUES ($1, $2)",
        MIGRATION_HISTORY_TABLE
    ))
    .bind(version)
    .bind(description)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
pub async fn execute_with_binds(
    sql: &str,
    values: &[SqlValue],
    pool: &DbPool,
) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
    record_executed_query();
    execute_with_binds_on(pool, sql, values).await
}

#[cfg(feature = "postgres")]
pub async fn execute_with_binds(
    sql: &str,
    values: &[SqlValue],
    pool: &DbPool,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    record_executed_query();
    execute_with_binds_on(pool, sql, values).await
}

pub async fn fetch_rows(
    pool: &DbPool,
    sql: &str,
    values: &[SqlValue],
) -> Result<Vec<DbRow>, sqlx::Error> {
    record_executed_query();
    fetch_rows_on(pool, sql, values).await
}

#[cfg(feature = "sqlite")]
pub async fn execute_with_binds_on<'e, E>(
    executor: E,
    sql: &str,
    values: &[SqlValue],
) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let mut query = sqlx::query(sql);
    for value in values {
        query = match value {
            SqlValue::String(value) => query.bind(value),
            SqlValue::Json(value) => query.bind(value.to_string()),
            SqlValue::JsonNull => query.bind(Option::<String>::None),
            SqlValue::Uuid(value) => query.bind(crate::db::sqlite_helpers::uuid_to_string(value)),
            SqlValue::Int(value) => query.bind(*value),
            SqlValue::Float(value) => query.bind(*value),
            SqlValue::Bool(value) => query.bind(*value),
            SqlValue::Null => query.bind(Option::<String>::None),
        };
    }
    query.execute(executor).await
}

#[cfg(feature = "postgres")]
pub async fn execute_with_binds_on<'e, E>(
    executor: E,
    sql: &str,
    values: &[SqlValue],
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let sql = super::query::normalize_sql(sql, 1);
    let mut query = sqlx::query(&sql);
    for value in values {
        query = match value {
            SqlValue::String(value) => query.bind(value),
            SqlValue::Json(value) => query.bind(sqlx::types::Json(value.clone())),
            SqlValue::JsonNull => query.bind(Option::<sqlx::types::Json<serde_json::Value>>::None),
            SqlValue::Uuid(value) => query.bind(*value),
            SqlValue::Int(value) => query.bind(*value),
            SqlValue::Float(value) => query.bind(*value),
            SqlValue::Bool(value) => query.bind(*value),
            SqlValue::Null => query.bind(Option::<String>::None),
        };
    }
    query.execute(executor).await
}

#[cfg(feature = "sqlite")]
pub async fn fetch_rows_on<'e, E>(
    executor: E,
    sql: &str,
    values: &[SqlValue],
) -> Result<Vec<DbRow>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    #[cfg(feature = "sqlite")]
    {
        let mut query = sqlx::query(sql);
        for value in values {
            query = match value {
                SqlValue::String(value) => query.bind(value),
                SqlValue::Json(value) => query.bind(value.to_string()),
                SqlValue::JsonNull => query.bind(Option::<String>::None),
                SqlValue::Uuid(value) => {
                    query.bind(crate::db::sqlite_helpers::uuid_to_string(value))
                }
                SqlValue::Int(value) => query.bind(*value),
                SqlValue::Float(value) => query.bind(*value),
                SqlValue::Bool(value) => query.bind(*value),
                SqlValue::Null => query.bind(Option::<String>::None),
            };
        }
        query.fetch_all(executor).await
    }
}

#[cfg(feature = "postgres")]
pub async fn fetch_rows_on<'e, E>(
    executor: E,
    sql: &str,
    values: &[SqlValue],
) -> Result<Vec<DbRow>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let sql = super::query::normalize_sql(sql, 1);
    let mut query = sqlx::query(&sql);
    for value in values {
        query = match value {
            SqlValue::String(value) => query.bind(value),
            SqlValue::Json(value) => query.bind(sqlx::types::Json(value.clone())),
            SqlValue::JsonNull => query.bind(Option::<sqlx::types::Json<serde_json::Value>>::None),
            SqlValue::Uuid(value) => query.bind(*value),
            SqlValue::Int(value) => query.bind(*value),
            SqlValue::Float(value) => query.bind(*value),
            SqlValue::Bool(value) => query.bind(*value),
            SqlValue::Null => query.bind(Option::<String>::None),
        };
    }
    query.fetch_all(executor).await
}
