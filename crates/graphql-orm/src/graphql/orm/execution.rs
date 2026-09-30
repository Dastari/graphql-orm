use super::core::{
    AppliedMigrationRecord, DbAuthContext, MigrationApplicationMetadata, PlannedSchemaStage,
    SchemaPolicy, SchemaStage, SqlValue, record_executed_query,
};
use super::migrations::build_migration_plan;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use super::runtime_migration::{OwnedPlannedMigration, RuntimeMigrationError};
use super::{MigrationBackend, OrmBackend, SqlxBackend, WriteBackend};
use std::collections::HashSet;

pub const MIGRATION_HISTORY_TABLE: &str = "__graphql_orm_migrations";
#[cfg(any(feature = "sqlite", feature = "postgres"))]
const MIGRATION_METADATA_COLUMNS: [(&str, &str); 6] = [
    ("backend", "TEXT"),
    ("graphql_orm_version", "TEXT"),
    ("source_schema_hash", "TEXT"),
    ("target_schema_hash", "TEXT"),
    ("plan_hash", "TEXT"),
    ("policy", "TEXT"),
];

#[cfg(any(feature = "sqlite", feature = "postgres"))]
fn invalid_migration_history(message: impl Into<String>) -> sqlx::Error {
    sqlx::Error::Configuration(Box::new(std::io::Error::other(format!(
        "unsafe {MIGRATION_HISTORY_TABLE} schema: {}",
        message.into()
    ))))
}

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
    async fn apply_migrations(&self, migrations: &[Migration]) -> crate::Result<()>;
}

#[allow(async_fn_in_trait)]
pub trait SchemaStageRunner {
    async fn plan_schema_stages(
        &self,
        stages: &[SchemaStage],
    ) -> crate::Result<Vec<PlannedSchemaStage>>;

    async fn apply_schema_stages(&self, stages: &[SchemaStage]) -> crate::Result<()>;
}

impl<B> MigrationRunner for crate::db::Database<B>
where
    B: MigrationBackend,
{
    async fn apply_migrations(&self, migrations: &[Migration]) -> crate::Result<()> {
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
    ) -> crate::Result<Vec<PlannedSchemaStage>> {
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

    async fn apply_schema_stages(&self, stages: &[SchemaStage]) -> crate::Result<()> {
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

pub(crate) fn ensure_managed_policy(policy: SchemaPolicy, action: &str) -> crate::Result<()> {
    if policy.allows_application() {
        Ok(())
    } else {
        Err(sqlx::Error::Protocol(format!(
            "graphql-orm schema policy {policy} does not allow {action}"
        )))
    }
}

pub(crate) fn ensure_planning_policy(policy: SchemaPolicy, action: &str) -> crate::Result<()> {
    if policy.allows_planning() {
        Ok(())
    } else {
        Err(sqlx::Error::Protocol(format!(
            "graphql-orm schema policy {policy} does not allow {action}"
        )))
    }
}

pub(crate) fn validate_schema_stages(stages: &[SchemaStage]) -> crate::Result<()> {
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
) -> crate::Result<()> {
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
) -> crate::Result<Vec<AppliedMigrationRecord>> {
    B::prepare_migration_runtime(pool).await?;
    B::load_applied_migrations(pool).await
}

pub(crate) async fn applied_version_set<B: MigrationBackend>(
    pool: &B::Pool,
) -> crate::Result<HashSet<String>> {
    Ok(applied_migration_records::<B>(pool)
        .await?
        .into_iter()
        .map(|record| record.version)
        .collect())
}

#[cfg(feature = "sqlite")]
impl MigrationBackend for super::SqliteBackend {
    async fn prepare_migration_runtime(pool: &Self::Pool) -> crate::Result<()> {
        ensure_sqlite_migration_history_table(pool).await?;
        cleanup_stale_sqlite_rewrite_tables(pool).await?;
        Ok(())
    }

    async fn load_applied_migrations(
        pool: &Self::Pool,
    ) -> crate::Result<Vec<AppliedMigrationRecord>> {
        load_sqlite_applied_migrations(pool).await
    }

    async fn apply_migration_statements_transactionally<S>(
        pool: &Self::Pool,
        version: &str,
        description: &str,
        statements: &[S],
        metadata: Option<&MigrationApplicationMetadata>,
        record_history: bool,
    ) -> crate::Result<()>
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
            None,
        )
        .await
        .map(|_| ())
        .map_err(runtime_error_to_legacy)
    }
}

#[cfg(feature = "postgres")]
impl MigrationBackend for super::PostgresBackend {
    async fn prepare_migration_runtime(pool: &Self::Pool) -> crate::Result<()> {
        ensure_postgres_migration_history_table(pool).await
    }

    async fn load_applied_migrations(
        pool: &Self::Pool,
    ) -> crate::Result<Vec<AppliedMigrationRecord>> {
        load_postgres_applied_migrations(pool).await
    }

    async fn apply_migration_statements_transactionally<S>(
        pool: &Self::Pool,
        version: &str,
        description: &str,
        statements: &[S],
        metadata: Option<&MigrationApplicationMetadata>,
        record_history: bool,
    ) -> crate::Result<()>
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
            None,
        )
        .await
        .map(|_| ())
        .map_err(runtime_error_to_legacy)
    }
}

#[cfg(feature = "sqlite")]
async fn ensure_sqlite_migration_history_table(pool: &sqlx::SqlitePool) -> crate::Result<()> {
    let mut transaction = pool.begin().await?;
    let exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(MIGRATION_HISTORY_TABLE)
            .fetch_one(&mut *transaction)
            .await?;
    if exists == 0 {
        sqlx::query(&format!(
            "CREATE TABLE IF NOT EXISTS {} (
            version TEXT PRIMARY KEY,
            description TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            backend TEXT,
            graphql_orm_version TEXT,
            source_schema_hash TEXT,
            target_schema_hash TEXT,
            plan_hash TEXT,
            policy TEXT
        )",
            MIGRATION_HISTORY_TABLE
        ))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        return Ok(());
    }

    let rows = sqlx::query(&format!("PRAGMA table_info({})", MIGRATION_HISTORY_TABLE))
        .fetch_all(&mut *transaction)
        .await?;
    let allowed = ["version", "description", "applied_at"]
        .into_iter()
        .chain(MIGRATION_METADATA_COLUMNS.iter().map(|(name, _)| *name))
        .collect::<HashSet<_>>();
    let mut columns = std::collections::HashMap::new();
    for row in rows {
        let name: String = sqlx::Row::try_get(&row, "name")?;
        if !allowed.contains(name.as_str()) {
            return Err(invalid_migration_history(format!(
                "unrecognized column `{name}`"
            )));
        }
        columns.insert(
            name,
            (
                sqlx::Row::try_get::<String, _>(&row, "type")?,
                sqlx::Row::try_get::<i64, _>(&row, "notnull")? != 0,
                sqlx::Row::try_get::<i64, _>(&row, "pk")?,
            ),
        );
    }
    let Some((version_type, _, version_pk)) = columns.get("version") else {
        return Err(invalid_migration_history(
            "missing required `version` column",
        ));
    };
    let Some((applied_type, applied_not_null, applied_pk)) = columns.get("applied_at") else {
        return Err(invalid_migration_history(
            "missing required `applied_at` column",
        ));
    };
    if !version_type.eq_ignore_ascii_case("TEXT") || *version_pk != 1 {
        return Err(invalid_migration_history(
            "`version` must be the sole textual primary-key identity",
        ));
    }
    if !applied_type.eq_ignore_ascii_case("TEXT") || !applied_not_null || *applied_pk != 0 {
        return Err(invalid_migration_history(
            "`applied_at` must be a non-null textual timestamp",
        ));
    }
    if columns.values().filter(|(_, _, pk)| *pk > 0).count() != 1 {
        return Err(invalid_migration_history(
            "migration history must have exactly one primary-key column",
        ));
    }
    for (column, _) in MIGRATION_METADATA_COLUMNS {
        if let Some((kind, _, pk)) = columns.get(column) {
            if !kind.eq_ignore_ascii_case("TEXT") || *pk != 0 {
                return Err(invalid_migration_history(format!(
                    "`{column}` metadata must be text"
                )));
            }
        }
    }
    let invalid_rows: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM {} WHERE version IS NULL OR version = ''
         OR typeof(version) != 'text' OR applied_at IS NULL OR typeof(applied_at) != 'text'",
        MIGRATION_HISTORY_TABLE
    ))
    .fetch_one(&mut *transaction)
    .await?;
    if invalid_rows != 0 {
        return Err(invalid_migration_history(
            "legacy rows contain an invalid version identity or timestamp",
        ));
    }

    if let Some((description_type, description_not_null, description_pk)) =
        columns.get("description")
    {
        if !description_type.eq_ignore_ascii_case("TEXT")
            || !description_not_null
            || *description_pk != 0
        {
            return Err(invalid_migration_history(
                "`description` must be non-null text",
            ));
        }
        let invalid_descriptions: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {} WHERE description IS NULL OR typeof(description) != 'text'",
            MIGRATION_HISTORY_TABLE
        ))
        .fetch_one(&mut *transaction)
        .await?;
        if invalid_descriptions != 0 {
            return Err(invalid_migration_history(
                "current-format rows contain an invalid description",
            ));
        }
    } else {
        let upgrade_table = "__graphql_orm_migrations_legacy_upgrade";
        sqlx::query(&format!(
            "CREATE TABLE {} (
                version TEXT PRIMARY KEY,
                description TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                backend TEXT,
                graphql_orm_version TEXT,
                source_schema_hash TEXT,
                target_schema_hash TEXT,
                plan_hash TEXT,
                policy TEXT
            )",
            upgrade_table
        ))
        .execute(&mut *transaction)
        .await?;
        let metadata_select = MIGRATION_METADATA_COLUMNS
            .iter()
            .map(|(name, _)| {
                if columns.contains_key(*name) {
                    (*name).to_string()
                } else {
                    "NULL".to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        sqlx::query(&format!(
            "INSERT INTO {upgrade_table}
             (version, description, applied_at, backend, graphql_orm_version,
              source_schema_hash, target_schema_hash, plan_hash, policy)
             SELECT version, 'Legacy migration ' || version, applied_at, {metadata_select}
             FROM {MIGRATION_HISTORY_TABLE}"
        ))
        .execute(&mut *transaction)
        .await?;
        sqlx::query(&format!("DROP TABLE {MIGRATION_HISTORY_TABLE}"))
            .execute(&mut *transaction)
            .await?;
        sqlx::query(&format!(
            "ALTER TABLE {upgrade_table} RENAME TO {MIGRATION_HISTORY_TABLE}"
        ))
        .execute(&mut *transaction)
        .await?;
        columns.extend(
            MIGRATION_METADATA_COLUMNS
                .iter()
                .map(|(name, sql_type)| ((*name).to_string(), ((*sql_type).to_string(), false, 0))),
        );
    }

    for (column, sql_type) in MIGRATION_METADATA_COLUMNS {
        if !columns.contains_key(column) {
            sqlx::query(&format!(
                "ALTER TABLE {} ADD COLUMN {} {}",
                MIGRATION_HISTORY_TABLE, column, sql_type
            ))
            .execute(&mut *transaction)
            .await?;
        }
    }
    transaction.commit().await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn ensure_postgres_migration_history_table(pool: &sqlx::PgPool) -> crate::Result<()> {
    let mut transaction = pool.begin().await?;
    let schema_name: String = sqlx::query_scalar("SELECT current_schema()")
        .fetch_one(&mut *transaction)
        .await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.tables
            WHERE table_schema = $1 AND table_name = $2
        )",
    )
    .bind(&schema_name)
    .bind(MIGRATION_HISTORY_TABLE)
    .fetch_one(&mut *transaction)
    .await?;
    if !exists {
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
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        return Ok(());
    }

    sqlx::query(&format!(
        "LOCK TABLE {} IN ACCESS EXCLUSIVE MODE",
        MIGRATION_HISTORY_TABLE
    ))
    .execute(&mut *transaction)
    .await?;
    let rows = sqlx::query(
        "SELECT column_name, data_type, is_nullable
         FROM information_schema.columns
         WHERE table_schema = $1 AND table_name = $2",
    )
    .bind(&schema_name)
    .bind(MIGRATION_HISTORY_TABLE)
    .fetch_all(&mut *transaction)
    .await?;
    let allowed = ["version", "description", "applied_at"]
        .into_iter()
        .chain(MIGRATION_METADATA_COLUMNS.iter().map(|(name, _)| *name))
        .collect::<HashSet<_>>();
    let mut columns = std::collections::HashMap::new();
    for row in rows {
        let name: String = sqlx::Row::try_get(&row, "column_name")?;
        if !allowed.contains(name.as_str()) {
            return Err(invalid_migration_history(format!(
                "unrecognized column `{name}`"
            )));
        }
        columns.insert(
            name,
            (
                sqlx::Row::try_get::<String, _>(&row, "data_type")?,
                sqlx::Row::try_get::<String, _>(&row, "is_nullable")? == "NO",
            ),
        );
    }
    let primary_keys: Vec<String> = sqlx::query_scalar(
        "SELECT a.attname
         FROM pg_index i
         JOIN pg_class t ON t.oid = i.indrelid
         JOIN pg_namespace n ON n.oid = t.relnamespace
         JOIN LATERAL unnest(i.indkey) AS key(attnum) ON true
         JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = key.attnum
         WHERE n.nspname = $1 AND t.relname = $2 AND i.indisprimary",
    )
    .bind(&schema_name)
    .bind(MIGRATION_HISTORY_TABLE)
    .fetch_all(&mut *transaction)
    .await?;
    if primary_keys != ["version"] {
        return Err(invalid_migration_history(
            "`version` must be the sole primary-key identity",
        ));
    }
    if !matches!(columns.get("version"), Some((kind, true)) if kind == "text") {
        return Err(invalid_migration_history("`version` must be non-null text"));
    }
    if !matches!(columns.get("applied_at"), Some((kind, true)) if kind == "timestamp with time zone")
    {
        return Err(invalid_migration_history(
            "`applied_at` must be a non-null timestamp with time zone",
        ));
    }
    for (column, _) in MIGRATION_METADATA_COLUMNS {
        if let Some((kind, _)) = columns.get(column) {
            if kind != "text" {
                return Err(invalid_migration_history(format!(
                    "`{column}` metadata must be text"
                )));
            }
        }
    }
    let invalid_versions: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM {} WHERE version = ''",
        MIGRATION_HISTORY_TABLE
    ))
    .fetch_one(&mut *transaction)
    .await?;
    if invalid_versions != 0 {
        return Err(invalid_migration_history(
            "`version` values must be non-empty text",
        ));
    }
    if let Some((kind, not_null)) = columns.get("description") {
        if kind != "text" || !not_null {
            return Err(invalid_migration_history(
                "`description` must be non-null text",
            ));
        }
    } else {
        sqlx::query(&format!(
            "ALTER TABLE {} ADD COLUMN description TEXT",
            MIGRATION_HISTORY_TABLE
        ))
        .execute(&mut *transaction)
        .await?;
        sqlx::query(&format!(
            "UPDATE {} SET description = 'Legacy migration ' || version",
            MIGRATION_HISTORY_TABLE
        ))
        .execute(&mut *transaction)
        .await?;
        sqlx::query(&format!(
            "ALTER TABLE {} ALTER COLUMN description SET NOT NULL",
            MIGRATION_HISTORY_TABLE
        ))
        .execute(&mut *transaction)
        .await?;
    }

    // Legacy PostgreSQL helpers did not always install the default used when
    // graphql-orm records a subsequently applied managed migration. Existing
    // timestamps remain untouched; this only governs future history rows.
    sqlx::query(&format!(
        "ALTER TABLE {} ALTER COLUMN applied_at SET DEFAULT CURRENT_TIMESTAMP",
        MIGRATION_HISTORY_TABLE
    ))
    .execute(&mut *transaction)
    .await?;

    for (column, _) in MIGRATION_METADATA_COLUMNS {
        sqlx::query(&format!(
            "ALTER TABLE {} ADD COLUMN IF NOT EXISTS {} TEXT",
            MIGRATION_HISTORY_TABLE, column
        ))
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn load_sqlite_applied_migrations(
    pool: &sqlx::SqlitePool,
) -> crate::Result<Vec<AppliedMigrationRecord>> {
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
) -> crate::Result<Vec<AppliedMigrationRecord>> {
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
async fn cleanup_stale_sqlite_rewrite_tables(pool: &sqlx::SqlitePool) -> crate::Result<()> {
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
    owned: Option<&OwnedPlannedMigration>,
) -> Result<Option<super::AppliedMigrationReport>, RuntimeMigrationError>
where
    S: AsRef<str>,
{
    use sqlx::{Acquire, Connection};

    let mut conn = pool.acquire().await?;
    let suspend_foreign_keys = statements.iter().any(|statement| {
        statement
            .as_ref()
            .trim()
            .eq_ignore_ascii_case("PRAGMA foreign_keys = OFF")
    });

    if suspend_foreign_keys {
        if owned.is_some() {
            conn.close_on_drop();
        }
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await?;
    }

    let mut tx = if owned.is_some() {
        conn.begin_with("BEGIN IMMEDIATE").await?
    } else {
        conn.begin().await?
    };
    let migration_result = async {
        if let Some(plan) = owned {
            let current = super::migrations::introspect_owned_sqlite_connection(&mut tx).await?;
            plan.check_baseline(&current)?;
            validate_owned_sqlite_plan(&mut tx, plan).await?;
            let recorded: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {MIGRATION_HISTORY_TABLE} WHERE version = ?"))
                .bind(version).fetch_one(&mut *tx).await?;
            if let Some(report) = plan.recorded_version_report(recorded != 0)? { return Ok(Some(report)); }
            // Controlled rebuilds must not erase host triggers, views, or reserved collision tables.
            if suspend_foreign_keys {
                for table in plan.ownership().tables() {
                    let temp_name = format!("__graphql_orm_{table}_new");
                    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
                        .bind(temp_name).fetch_one(&mut *tx).await?;
                    if exists != 0 { return Err(sqlx::Error::Protocol("owned rebuild temporary table collision".into()).into()); }
                }
            }
        }

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
                )).into());
            }
        }

        if record_history {
            insert_sqlite_history_row(&mut tx, version, description, metadata).await?;
        }

        Ok::<_, RuntimeMigrationError>(None)
    }
    .await;

    let final_result = match migration_result {
        Ok(report) => tx.commit().await.map(|_| report).map_err(Into::into),
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

#[cfg(any(feature = "sqlite", feature = "postgres"))]
fn runtime_error_to_legacy(error: RuntimeMigrationError) -> sqlx::Error {
    match error {
        RuntimeMigrationError::Database(error) => error,
        RuntimeMigrationError::Diagnostics(error) => sqlx::Error::Protocol(error.to_string()),
    }
}

#[cfg(feature = "sqlite")]
pub(super) async fn apply_owned_sqlite_migration(
    pool: &sqlx::SqlitePool,
    plan: &OwnedPlannedMigration,
    options: &super::ApplyOptions,
) -> Result<super::AppliedMigrationReport, RuntimeMigrationError> {
    let current = super::migrations::introspect_owned_sqlite_schema(pool).await?;
    plan.check_baseline(&current)?;
    ensure_sqlite_migration_history_table(pool).await?;
    let metadata = plan.application_metadata();
    if let Some(report) = apply_sqlite_migration_statements_transactionally(
        pool,
        plan.version(),
        plan.description(),
        plan.statements(),
        Some(&metadata),
        options.record_history,
        Some(plan),
    )
    .await?
    {
        return Ok(report);
    }
    Ok(super::AppliedMigrationReport {
        version: plan.version().into(),
        dry_run: false,
        statements_applied: plan.statements().len(),
        already_applied: false,
    })
}

#[cfg(feature = "postgres")]
pub(super) async fn apply_owned_postgres_migration(
    pool: &sqlx::PgPool,
    plan: &OwnedPlannedMigration,
    options: &super::ApplyOptions,
) -> Result<super::AppliedMigrationReport, RuntimeMigrationError> {
    let current = super::migrations::introspect_owned_postgres_schema(pool).await?;
    plan.check_baseline(&current)?;
    ensure_postgres_migration_history_table(pool).await?;
    let metadata = plan.application_metadata();
    if let Some(report) = apply_postgres_migration_statements_transactionally(
        pool,
        plan.version(),
        plan.description(),
        plan.statements(),
        Some(&metadata),
        options.record_history,
        Some(plan),
    )
    .await?
    {
        return Ok(report);
    }
    Ok(super::AppliedMigrationReport {
        version: plan.version().into(),
        dry_run: false,
        statements_applied: plan.statements().len(),
        already_applied: false,
    })
}

#[cfg(feature = "sqlite")]
async fn insert_sqlite_history_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    version: &str,
    description: &str,
    metadata: Option<&MigrationApplicationMetadata>,
) -> crate::Result<()> {
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
    owned: Option<&OwnedPlannedMigration>,
) -> Result<Option<super::AppliedMigrationReport>, RuntimeMigrationError>
where
    S: AsRef<str>,
{
    let mut tx = pool.begin().await?;
    if let Some(plan) = owned {
        // Cooperative migration serialization plus table locks keep catalog validation
        // and DDL on this pinned transaction. Hosts fence non-ORM external DDL.
        sqlx::query("SELECT pg_advisory_xact_lock(718644299020133922)")
            .execute(&mut *tx)
            .await?;
        for table in plan.ownership().tables() {
            let row: Option<(bool,)> = sqlx::query_as("SELECT c.relrowsecurity OR c.relforcerowsecurity FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = current_schema() AND c.relname = $1 AND c.relkind = 'r'")
                .bind(table).fetch_optional(&mut *tx).await?;
            if row.is_some() {
                let identifier = table.replace('"', "\"\"");
                sqlx::query(&format!(
                    "LOCK TABLE \"{identifier}\" IN ACCESS EXCLUSIVE MODE"
                ))
                .execute(&mut *tx)
                .await?;
            }
        }
        let current = super::migrations::introspect_owned_postgres_connection(&mut tx).await?;
        plan.check_baseline(&current)?;
        validate_owned_postgres_plan(&mut tx, plan).await?;
        let recorded: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {MIGRATION_HISTORY_TABLE} WHERE version = $1"
        ))
        .bind(version)
        .fetch_one(&mut *tx)
        .await?;
        if let Some(report) = plan.recorded_version_report(recorded != 0)? {
            tx.commit().await?;
            return Ok(Some(report));
        }
    }

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
    Ok(None)
}

#[cfg(feature = "postgres")]
async fn insert_postgres_history_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    version: &str,
    description: &str,
    metadata: Option<&MigrationApplicationMetadata>,
) -> crate::Result<()> {
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

pub async fn execute_with_binds<B: WriteBackend>(
    sql: &str,
    values: &[SqlValue],
    pool: &B::Pool,
) -> crate::Result<B::WriteResult> {
    record_executed_query();
    B::execute_with_binds(pool, sql, values).await
}

/// Execute a bound statement through an ORM-owned driver-neutral transaction.
pub async fn execute_in_transaction<B: WriteBackend>(
    transaction: &mut B::Transaction<'_>,
    sql: &str,
    values: &[SqlValue],
) -> crate::Result<B::WriteResult> {
    record_executed_query();
    B::execute_in_transaction(transaction, sql, values).await
}

/// Fetch rows through an ORM-owned driver-neutral transaction.
pub async fn fetch_rows_in_transaction<B: WriteBackend>(
    transaction: &mut B::Transaction<'_>,
    sql: &str,
    values: &[SqlValue],
) -> crate::Result<Vec<B::Row>> {
    record_executed_query();
    B::fetch_rows_in_transaction(transaction, sql, values).await
}

pub async fn fetch_rows<B: OrmBackend>(
    pool: &B::Pool,
    sql: &str,
    values: &[SqlValue],
) -> crate::Result<Vec<B::Row>> {
    record_executed_query();
    B::fetch_rows(pool, sql, values).await
}

pub async fn fetch_rows_with_auth<B: OrmBackend>(
    pool: &B::Pool,
    sql: &str,
    values: &[SqlValue],
    auth: Option<&DbAuthContext>,
) -> crate::Result<Vec<B::Row>> {
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
) -> crate::Result<(Vec<B::Row>, Vec<B::Row>)> {
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
) -> crate::Result<B::QueryResult>
where
    B: SqlxBackend,
    E: sqlx::Executor<'e, Database = B::Database> + Send + 'e,
{
    B::execute_with_binds_on(executor, sql.to_string(), values.to_vec()).await
}

pub async fn apply_db_auth_context_to_transaction<B: SqlxBackend>(
    tx: &mut sqlx::Transaction<'_, B::Database>,
    auth: Option<&DbAuthContext>,
) -> crate::Result<()> {
    B::apply_auth_context_to_transaction(tx, auth).await
}

pub async fn fetch_rows_on<'e, B, E>(
    executor: E,
    sql: &str,
    values: &[SqlValue],
) -> crate::Result<Vec<B::Row>>
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

#[cfg(feature = "sqlite")]
pub(super) async fn validate_owned_sqlite_plan(
    connection: &mut sqlx::SqliteConnection,
    plan: &OwnedPlannedMigration,
) -> Result<(), RuntimeMigrationError> {
    use super::runtime_migration::{RuntimeMigrationDiagnosticCode, rejection};
    let rebuilt = plan.rebuilt_sqlite_tables().collect::<Vec<_>>();
    if !rebuilt.is_empty() {
        // Without a SQL view-dependency IR, preserving arbitrary view definitions
        // cannot be proven through a drop/rebuild. Reject conservatively.
        let views: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'view'")
                .fetch_one(&mut *connection)
                .await?;
        if views != 0 {
            return Err(rejection(RuntimeMigrationDiagnosticCode::InvalidPhysicalContract).into());
        }
    }
    for table in rebuilt {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = ?",
        )
        .bind(table)
        .fetch_all(&mut *connection)
        .await?;
        let current =
            super::migrations::introspect_owned_sqlite_connection(&mut *connection).await?;
        let canonical_append_only = current
            .tables()
            .iter()
            .find(|t| t.table_name() == table)
            .is_some_and(|t| t.append_only());
        let name = super::migrations::append_only_name(table);
        if rows.iter().any(|(trigger,)| {
            !canonical_append_only
                || (*trigger != format!("{name}_update") && *trigger != format!("{name}_delete"))
        }) {
            return Err(rejection(RuntimeMigrationDiagnosticCode::InvalidPhysicalContract).into());
        }
        let collision: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?")
                .bind(format!("__graphql_orm_{table}_new"))
                .fetch_one(&mut *connection)
                .await?;
        if collision != 0 {
            return Err(rejection(RuntimeMigrationDiagnosticCode::OwnershipMismatch).into());
        }
    }
    Ok(())
}
#[cfg(feature = "postgres")]
pub(super) async fn validate_owned_postgres_plan(
    connection: &mut sqlx::PgConnection,
    plan: &OwnedPlannedMigration,
) -> Result<(), RuntimeMigrationError> {
    use super::runtime_migration::{RuntimeMigrationDiagnosticCode, rejection};
    let protected: Vec<(String,)> = sqlx::query_as("SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=current_schema() AND c.relkind='r' AND (c.relrowsecurity OR c.relforcerowsecurity OR EXISTS(SELECT 1 FROM pg_policy p WHERE p.polrelid=c.oid))")
        .fetch_all(connection).await?;
    if protected
        .iter()
        .any(|(table,)| plan.changes_protected_rls_structure(table))
    {
        return Err(rejection(RuntimeMigrationDiagnosticCode::InvalidPhysicalContract).into());
    }
    Ok(())
}
