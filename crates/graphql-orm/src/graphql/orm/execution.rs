#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use super::DefaultBackend;
#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use super::core::{PlannedSchemaStage, SchemaStage};
use super::core::{SqlValue, record_executed_query};
#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use super::dialect::current_backend;
#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use super::migrations::{build_migration_plan, introspect_schema};
use super::{OrmBackend, SqlxBackend};
#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use crate::DbPool;
#[cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
use sqlx::Acquire;
#[cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
use sqlx::Row;
#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
use std::collections::HashSet;

#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
const MIGRATION_HISTORY_TABLE: &str = "__graphql_orm_migrations";

#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
pub struct Migration {
    pub version: &'static str,
    pub description: &'static str,
    pub statements: &'static [&'static str],
}

#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
pub trait MigrationSource {
    fn migrations() -> &'static [Migration] {
        &[]
    }
}

#[allow(async_fn_in_trait)]
#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
pub trait MigrationRunner {
    async fn apply_migrations(&self, migrations: &[Migration]) -> Result<(), sqlx::Error>;
}

#[allow(async_fn_in_trait)]
#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
pub trait SchemaStageRunner {
    async fn plan_schema_stages(
        &self,
        stages: &[SchemaStage],
    ) -> Result<Vec<PlannedSchemaStage>, sqlx::Error>;

    async fn apply_schema_stages(&self, stages: &[SchemaStage]) -> Result<(), sqlx::Error>;
}

#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
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

#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
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

#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
async fn prepare_migration_runtime(pool: &DbPool) -> Result<(), sqlx::Error> {
    ensure_migration_history_table(pool).await?;
    #[cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
    cleanup_stale_sqlite_rewrite_tables(pool).await?;
    Ok(())
}

#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
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

#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
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

#[cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
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

#[cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]
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

#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]
async fn load_applied_migration_versions(pool: &DbPool) -> Result<HashSet<String>, sqlx::Error> {
    let rows = fetch_rows::<DefaultBackend>(
        pool,
        &format!(
            "SELECT version FROM {} ORDER BY version",
            MIGRATION_HISTORY_TABLE
        ),
        &[],
    )
    .await?;
    rows.into_iter()
        .map(|row| DefaultBackend::try_get_string(&row, "version"))
        .collect()
}

#[cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
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

#[cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
async fn apply_migration_statements_transactionally<S>(
    pool: &DbPool,
    version: &str,
    description: &str,
    statements: &[S],
) -> Result<(), sqlx::Error>
where
    S: AsRef<str>,
{
    let mut conn = pool.acquire().await?;
    let suspend_foreign_keys = statements.iter().any(|statement| {
        statement
            .as_ref()
            .trim()
            .eq_ignore_ascii_case("PRAGMA foreign_keys = OFF")
    });

    if suspend_foreign_keys {
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await?;
    }

    let mut tx = conn.begin().await?;
    let migration_result = async {
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

        if suspend_foreign_keys {
            let violations: Vec<sqlx::sqlite::SqliteRow> = sqlx::query("PRAGMA foreign_key_check")
                .fetch_all(&mut *tx)
                .await?;
            if !violations.is_empty() {
                return Err(sqlx::Error::Protocol(format!(
                    "SQLite foreign_key_check failed after controlled rebuild during migration {version}"
                )));
            }
        }

        sqlx::query(&format!(
            "INSERT INTO {} (version, description) VALUES (?, ?)",
            MIGRATION_HISTORY_TABLE
        ))
        .bind(version)
        .bind(description)
        .execute(&mut *tx)
        .await?;

        Ok::<(), sqlx::Error>(())
    }
    .await;

    let final_result = match migration_result {
        Ok(()) => tx.commit().await,
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    };

    if suspend_foreign_keys {
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&mut *conn)
            .await?;
    }

    final_result
}

#[cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]
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

pub async fn execute_with_binds<B: SqlxBackend>(
    sql: &str,
    values: &[SqlValue],
    pool: &B::Pool,
) -> Result<B::QueryResult, sqlx::Error> {
    record_executed_query();
    B::execute_with_binds(pool, sql, values).await
}

pub async fn fetch_rows<B: OrmBackend>(
    pool: &B::Pool,
    sql: &str,
    values: &[SqlValue],
) -> Result<Vec<B::Row>, sqlx::Error> {
    record_executed_query();
    B::fetch_rows(pool, sql, values).await
}

pub async fn execute_with_binds_on<'e, B, E>(
    executor: E,
    sql: &str,
    values: &[SqlValue],
) -> Result<B::QueryResult, sqlx::Error>
where
    B: SqlxBackend,
    E: sqlx::Executor<'e, Database = B::Database> + Send + 'e,
{
    B::execute_with_binds_on(executor, sql.to_string(), values.to_vec()).await
}

pub async fn fetch_rows_on<'e, B, E>(
    executor: E,
    sql: &str,
    values: &[SqlValue],
) -> Result<Vec<B::Row>, sqlx::Error>
where
    B: SqlxBackend,
    E: sqlx::Executor<'e, Database = B::Database> + Send + 'e,
{
    B::fetch_rows_on(executor, sql.to_string(), values.to_vec()).await
}
