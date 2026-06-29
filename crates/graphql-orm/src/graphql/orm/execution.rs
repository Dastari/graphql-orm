use super::core::{
    AppliedMigrationRecord, DbAuthContext, MigrationApplicationMetadata, PlannedSchemaStage,
    SchemaPolicy, SchemaStage, SqlValue, record_executed_query,
};
use super::migrations::build_migration_plan;
use super::{MigrationBackend, OrmBackend, SqlxBackend};
use std::collections::HashSet;

pub const MIGRATION_HISTORY_TABLE: &str = "__graphql_orm_migrations";

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

impl<B> MigrationRunner for crate::db::Database<B>
where
    B: MigrationBackend,
{
    async fn apply_migrations(&self, migrations: &[Migration]) -> Result<(), sqlx::Error> {
        ensure_managed_policy(self.schema_policy(), "apply legacy migrations")?;
        B::prepare_migration_runtime(self.pool()).await?;
        let mut applied_versions = applied_version_set::<B>(self.pool()).await?;
        for migration in migrations {
            if applied_versions.contains(migration.version) {
                continue;
            }
            B::apply_migration_statements_transactionally(
                self.pool(),
                migration.version,
                migration.description,
                migration.statements,
                None,
                true,
            )
            .await?;
            applied_versions.insert(migration.version.to_string());
        }
        Ok(())
    }
}

impl<B> SchemaStageRunner for crate::db::Database<B>
where
    B: MigrationBackend,
{
    async fn plan_schema_stages(
        &self,
        stages: &[SchemaStage],
    ) -> Result<Vec<PlannedSchemaStage>, sqlx::Error> {
        ensure_planning_policy(self.schema_policy(), "plan schema stages")?;
        B::prepare_migration_runtime(self.pool()).await?;
        validate_schema_stages(stages)?;

        let applied_versions = applied_version_set::<B>(self.pool()).await?;
        ensure_applied_stages_form_prefix(stages, &applied_versions)?;

        let mut current_schema = B::introspect_schema(self.pool()).await?;
        let mut planned = Vec::new();

        for stage in stages {
            if applied_versions.contains(&stage.version) {
                current_schema = stage.target_schema.clone();
                continue;
            }

            let plan = build_migration_plan(B::DIALECT, &current_schema, &stage.target_schema);
            planned.push(PlannedSchemaStage {
                version: stage.version.clone(),
                description: stage.description.clone(),
                target_schema_hash: stage.target_schema_hash.clone(),
                plan,
            });
            current_schema = stage.target_schema.clone();
        }

        Ok(planned)
    }

    async fn apply_schema_stages(&self, stages: &[SchemaStage]) -> Result<(), sqlx::Error> {
        ensure_managed_policy(self.schema_policy(), "apply schema stages")?;
        let planned = self.plan_schema_stages(stages).await?;
        for stage in planned {
            let metadata = MigrationApplicationMetadata {
                backend: B::DIALECT.name(),
                graphql_orm_version: env!("CARGO_PKG_VERSION"),
                source_schema_hash: None,
                target_schema_hash: stage.target_schema_hash.clone(),
                plan_hash: stage.plan.stable_hash(),
                policy: self.schema_policy(),
            };
            B::apply_migration_statements_transactionally(
                self.pool(),
                &stage.version,
                &stage.description,
                &stage.plan.statements,
                Some(&metadata),
                true,
            )
            .await?;
        }
        Ok(())
    }
}

pub(crate) fn ensure_managed_policy(policy: SchemaPolicy, action: &str) -> Result<(), sqlx::Error> {
    if policy.allows_application() {
        Ok(())
    } else {
        Err(sqlx::Error::Protocol(format!(
            "graphql-orm schema policy {policy} does not allow {action}"
        )))
    }
}

pub(crate) fn ensure_planning_policy(
    policy: SchemaPolicy,
    action: &str,
) -> Result<(), sqlx::Error> {
    if policy.allows_planning() {
        Ok(())
    } else {
        Err(sqlx::Error::Protocol(format!(
            "graphql-orm schema policy {policy} does not allow {action}"
        )))
    }
}

pub(crate) fn validate_schema_stages(stages: &[SchemaStage]) -> Result<(), sqlx::Error> {
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

pub(crate) fn ensure_applied_stages_form_prefix(
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

pub(crate) async fn applied_migration_records<B: MigrationBackend>(
    pool: &B::Pool,
) -> Result<Vec<AppliedMigrationRecord>, sqlx::Error> {
    B::prepare_migration_runtime(pool).await?;
    B::load_applied_migrations(pool).await
}

pub(crate) async fn applied_version_set<B: MigrationBackend>(
    pool: &B::Pool,
) -> Result<HashSet<String>, sqlx::Error> {
    Ok(applied_migration_records::<B>(pool)
        .await?
        .into_iter()
        .map(|record| record.version)
        .collect())
}

#[cfg(feature = "sqlite")]
impl MigrationBackend for super::SqliteBackend {
    async fn prepare_migration_runtime(pool: &Self::Pool) -> Result<(), sqlx::Error> {
        ensure_sqlite_migration_history_table(pool).await?;
        cleanup_stale_sqlite_rewrite_tables(pool).await?;
        Ok(())
    }

    async fn load_applied_migrations(
        pool: &Self::Pool,
    ) -> Result<Vec<AppliedMigrationRecord>, sqlx::Error> {
        load_sqlite_applied_migrations(pool).await
    }

    async fn apply_migration_statements_transactionally<S>(
        pool: &Self::Pool,
        version: &str,
        description: &str,
        statements: &[S],
        metadata: Option<&MigrationApplicationMetadata>,
        record_history: bool,
    ) -> Result<(), sqlx::Error>
    where
        S: AsRef<str> + Send + Sync,
    {
        apply_sqlite_migration_statements_transactionally(
            pool,
            version,
            description,
            statements,
            metadata,
            record_history,
        )
        .await
    }
}

#[cfg(feature = "postgres")]
impl MigrationBackend for super::PostgresBackend {
    async fn prepare_migration_runtime(pool: &Self::Pool) -> Result<(), sqlx::Error> {
        ensure_postgres_migration_history_table(pool).await
    }

    async fn load_applied_migrations(
        pool: &Self::Pool,
    ) -> Result<Vec<AppliedMigrationRecord>, sqlx::Error> {
        load_postgres_applied_migrations(pool).await
    }

    async fn apply_migration_statements_transactionally<S>(
        pool: &Self::Pool,
        version: &str,
        description: &str,
        statements: &[S],
        metadata: Option<&MigrationApplicationMetadata>,
        record_history: bool,
    ) -> Result<(), sqlx::Error>
    where
        S: AsRef<str> + Send + Sync,
    {
        apply_postgres_migration_statements_transactionally(
            pool,
            version,
            description,
            statements,
            metadata,
            record_history,
        )
        .await
    }
}

#[cfg(feature = "sqlite")]
async fn ensure_sqlite_migration_history_table(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
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

    let existing = sqlx::query(&format!("PRAGMA table_info({})", MIGRATION_HISTORY_TABLE))
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| sqlx::Row::try_get::<String, _>(&row, "name"))
        .collect::<Result<HashSet<_>, _>>()?;
    for (column, sql_type) in [
        ("backend", "TEXT"),
        ("graphql_orm_version", "TEXT"),
        ("source_schema_hash", "TEXT"),
        ("target_schema_hash", "TEXT"),
        ("plan_hash", "TEXT"),
        ("policy", "TEXT"),
    ] {
        if !existing.contains(column) {
            sqlx::query(&format!(
                "ALTER TABLE {} ADD COLUMN {} {}",
                MIGRATION_HISTORY_TABLE, column, sql_type
            ))
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn ensure_postgres_migration_history_table(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {} (
            version TEXT PRIMARY KEY,
            description TEXT NOT NULL,
            applied_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
            backend TEXT,
            graphql_orm_version TEXT,
            source_schema_hash TEXT,
            target_schema_hash TEXT,
            plan_hash TEXT,
            policy TEXT
        )",
        MIGRATION_HISTORY_TABLE
    ))
    .execute(pool)
    .await?;

    for column in [
        "backend",
        "graphql_orm_version",
        "source_schema_hash",
        "target_schema_hash",
        "plan_hash",
        "policy",
    ] {
        sqlx::query(&format!(
            "ALTER TABLE {} ADD COLUMN IF NOT EXISTS {} TEXT",
            MIGRATION_HISTORY_TABLE, column
        ))
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn load_sqlite_applied_migrations(
    pool: &sqlx::SqlitePool,
) -> Result<Vec<AppliedMigrationRecord>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "SELECT version, description, applied_at, backend, graphql_orm_version,
                source_schema_hash, target_schema_hash, plan_hash, policy
         FROM {}
         ORDER BY version",
        MIGRATION_HISTORY_TABLE
    ))
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(AppliedMigrationRecord {
                version: sqlx::Row::try_get(&row, "version")?,
                description: sqlx::Row::try_get(&row, "description")?,
                applied_at: sqlx::Row::try_get(&row, "applied_at")?,
                backend: sqlx::Row::try_get(&row, "backend")?,
                graphql_orm_version: sqlx::Row::try_get(&row, "graphql_orm_version")?,
                source_schema_hash: sqlx::Row::try_get(&row, "source_schema_hash")?,
                target_schema_hash: sqlx::Row::try_get(&row, "target_schema_hash")?,
                plan_hash: sqlx::Row::try_get(&row, "plan_hash")?,
                policy: sqlx::Row::try_get(&row, "policy")?,
            })
        })
        .collect()
}

#[cfg(feature = "postgres")]
async fn load_postgres_applied_migrations(
    pool: &sqlx::PgPool,
) -> Result<Vec<AppliedMigrationRecord>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        "SELECT version, description, applied_at::TEXT AS applied_at, backend, graphql_orm_version,
                source_schema_hash, target_schema_hash, plan_hash, policy
         FROM {}
         ORDER BY version",
        MIGRATION_HISTORY_TABLE
    ))
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(AppliedMigrationRecord {
                version: sqlx::Row::try_get(&row, "version")?,
                description: sqlx::Row::try_get(&row, "description")?,
                applied_at: sqlx::Row::try_get(&row, "applied_at")?,
                backend: sqlx::Row::try_get(&row, "backend")?,
                graphql_orm_version: sqlx::Row::try_get(&row, "graphql_orm_version")?,
                source_schema_hash: sqlx::Row::try_get(&row, "source_schema_hash")?,
                target_schema_hash: sqlx::Row::try_get(&row, "target_schema_hash")?,
                plan_hash: sqlx::Row::try_get(&row, "plan_hash")?,
                policy: sqlx::Row::try_get(&row, "policy")?,
            })
        })
        .collect()
}

#[cfg(feature = "sqlite")]
async fn cleanup_stale_sqlite_rewrite_tables(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
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
        let table_name: String = sqlx::Row::try_get(&row, "name")?;
        let escaped = table_name.replace('"', "\"\"");
        sqlx::query(&format!("DROP TABLE IF EXISTS \"{}\"", escaped))
            .execute(pool)
            .await?;
    }

    Ok(())
}

#[cfg(feature = "sqlite")]
async fn apply_sqlite_migration_statements_transactionally<S>(
    pool: &sqlx::SqlitePool,
    version: &str,
    description: &str,
    statements: &[S],
    metadata: Option<&MigrationApplicationMetadata>,
    record_history: bool,
) -> Result<(), sqlx::Error>
where
    S: AsRef<str>,
{
    use sqlx::Acquire;

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

        if record_history {
            insert_sqlite_history_row(&mut tx, version, description, metadata).await?;
        }

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

#[cfg(feature = "sqlite")]
async fn insert_sqlite_history_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    version: &str,
    description: &str,
    metadata: Option<&MigrationApplicationMetadata>,
) -> Result<(), sqlx::Error> {
    sqlx::query(&format!(
        "INSERT INTO {} (
            version, description, backend, graphql_orm_version, source_schema_hash,
            target_schema_hash, plan_hash, policy
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        MIGRATION_HISTORY_TABLE
    ))
    .bind(version)
    .bind(description)
    .bind(metadata.map(|metadata| metadata.backend))
    .bind(metadata.map(|metadata| metadata.graphql_orm_version))
    .bind(metadata.and_then(|metadata| metadata.source_schema_hash.as_deref()))
    .bind(metadata.map(|metadata| metadata.target_schema_hash.as_str()))
    .bind(metadata.map(|metadata| metadata.plan_hash.as_str()))
    .bind(metadata.map(|metadata| metadata.policy.as_str()))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn apply_postgres_migration_statements_transactionally<S>(
    pool: &sqlx::PgPool,
    version: &str,
    description: &str,
    statements: &[S],
    metadata: Option<&MigrationApplicationMetadata>,
    record_history: bool,
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

    if record_history {
        insert_postgres_history_row(&mut tx, version, description, metadata).await?;
    }

    tx.commit().await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_postgres_history_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    version: &str,
    description: &str,
    metadata: Option<&MigrationApplicationMetadata>,
) -> Result<(), sqlx::Error> {
    sqlx::query(&format!(
        "INSERT INTO {} (
            version, description, backend, graphql_orm_version, source_schema_hash,
            target_schema_hash, plan_hash, policy
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        MIGRATION_HISTORY_TABLE
    ))
    .bind(version)
    .bind(description)
    .bind(metadata.map(|metadata| metadata.backend))
    .bind(metadata.map(|metadata| metadata.graphql_orm_version))
    .bind(metadata.and_then(|metadata| metadata.source_schema_hash.as_deref()))
    .bind(metadata.map(|metadata| metadata.target_schema_hash.as_str()))
    .bind(metadata.map(|metadata| metadata.plan_hash.as_str()))
    .bind(metadata.map(|metadata| metadata.policy.as_str()))
    .execute(&mut **tx)
    .await?;
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

pub async fn fetch_rows_with_auth<B: OrmBackend>(
    pool: &B::Pool,
    sql: &str,
    values: &[SqlValue],
    auth: Option<&DbAuthContext>,
) -> Result<Vec<B::Row>, sqlx::Error> {
    record_executed_query();
    B::fetch_rows_with_auth(pool, sql, values, auth).await
}

pub async fn fetch_rows_pair_with_auth<B: OrmBackend>(
    pool: &B::Pool,
    first_sql: &str,
    first_values: &[SqlValue],
    second_sql: &str,
    second_values: &[SqlValue],
    auth: Option<&DbAuthContext>,
) -> Result<(Vec<B::Row>, Vec<B::Row>), sqlx::Error> {
    record_executed_query();
    record_executed_query();
    B::fetch_rows_pair_with_auth(
        pool,
        first_sql,
        first_values,
        second_sql,
        second_values,
        auth,
    )
    .await
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

pub async fn apply_db_auth_context_to_transaction<B: SqlxBackend>(
    tx: &mut sqlx::Transaction<'_, B::Database>,
    auth: Option<&DbAuthContext>,
) -> Result<(), sqlx::Error> {
    B::apply_auth_context_to_transaction(tx, auth).await
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

trait MigrationPlanHashExt {
    fn stable_hash(&self) -> String;
}

impl MigrationPlanHashExt for super::MigrationPlan {
    fn stable_hash(&self) -> String {
        let mut canonical = format!("{:?}\n", self.backend);
        for step in &self.steps {
            canonical.push_str(&format!("{step:?}\n"));
        }
        for statement in &self.statements {
            canonical.push_str(statement);
            canonical.push('\n');
        }
        format!("{:016x}", fnv1a64(canonical.as_bytes()))
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
