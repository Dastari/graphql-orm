use super::core::{
    AppliedMigrationReport, AppliedSchemaUpgrade, ApplyOptions, EntityMetadata,
    MigrationApplicationMetadata, MigrationRisk, PlannedMigration, PlannedMigrationStep,
    PlannedSchemaTarget, PlannedSchemaUpgrade, SchemaAbi, SchemaDiagnostic, SchemaDiagnosticKind,
    SchemaDiagnosticSeverity, SchemaModel, SchemaPolicy, SchemaTarget,
    SchemaTargetValidationReport, SchemaValidationReport,
};
use super::execution::{applied_migration_records, ensure_managed_policy, ensure_planning_policy};
use super::migrations::{build_migration_plan, classify_migration_steps};
use super::rls::{build_rls_policy_plan, validate_rls_models};
use super::{IntrospectionBackend, MigrationBackend, OrmBackend, RlsIntrospectionBackend};
use crate::db::Database;

/// Explicit schema validation, planning, and migration API for a [`Database`].
///
/// A `SchemaManager` is created with [`Database::schema`](crate::db::Database::schema).
/// Its methods honor the database's [`SchemaPolicy`]. Validation never mutates
/// the database; migration application requires a backend implementing
/// [`MigrationBackend`].
pub struct SchemaManager<'db, B: OrmBackend> {
    database: &'db Database<B>,
}

impl<'db, B: OrmBackend> SchemaManager<'db, B> {
    pub(crate) fn new(database: &'db Database<B>) -> Self {
        Self { database }
    }

    /// Return the schema policy currently attached to the database handle.
    pub fn policy(&self) -> SchemaPolicy {
        self.database.schema_policy()
    }

    /// Validate an already-built schema model against a target model.
    ///
    /// This is a pure in-memory comparison and does not introspect or mutate a
    /// live database.
    pub fn validate(
        &self,
        current: &SchemaModel,
        target: &SchemaModel,
    ) -> Result<SchemaValidationReport, sqlx::Error> {
        if !self.policy().allows_validation() {
            return Err(sqlx::Error::Protocol(format!(
                "graphql-orm schema policy {} does not allow schema validation",
                self.policy()
            )));
        }

        Ok(validate_schema_models(
            B::DIALECT.name(),
            self.policy(),
            current,
            target,
        ))
    }

    /// Introspect the live database and compare it to metadata from generated entities.
    pub async fn validate_against_entities(
        &self,
        entities: &[&'static EntityMetadata],
    ) -> Result<SchemaValidationReport, sqlx::Error>
    where
        B: IntrospectionBackend,
    {
        let current = B::introspect_schema(self.database.pool()).await?;
        let target = SchemaModel::from_entities(entities);
        self.validate(&current, &target)
    }

    /// Introspect the live database and validate it against a full schema target,
    /// including Postgres RLS metadata when the schema policy manages or validates it.
    pub async fn validate_target(
        &self,
        target: &SchemaTarget,
    ) -> Result<SchemaTargetValidationReport, sqlx::Error>
    where
        B: IntrospectionBackend + RlsIntrospectionBackend,
    {
        let current = B::introspect_schema(self.database.pool()).await?;
        let schema = self.validate(&current, &target.schema)?;
        let rls_diagnostics = if matches!(
            self.policy(),
            SchemaPolicy::ExternalReadOnly | SchemaPolicy::ExternalWritable
        ) {
            Vec::new()
        } else {
            let current_rls = B::introspect_rls(self.database.pool()).await?;
            validate_rls_models(B::DIALECT, self.policy(), &current_rls, &target.rls)
        };
        Ok(SchemaTargetValidationReport {
            backend: B::DIALECT.name(),
            policy: self.policy(),
            schema,
            rls_diagnostics,
        })
    }

    /// Build a structured migration plan from one schema model to another.
    pub fn plan_migration(
        &self,
        version: impl Into<String>,
        description: impl Into<String>,
        current: &SchemaModel,
        target: &SchemaModel,
    ) -> Result<PlannedMigration, sqlx::Error> {
        ensure_planning_policy(self.policy(), "plan schema migration")?;
        Ok(plan_migration_for_backend::<B>(
            version.into(),
            description.into(),
            Some(current.stable_hash()),
            target.stable_hash(),
            current,
            target,
        ))
    }

    /// Introspect the live database and plan a migration to generated entity metadata.
    pub async fn plan_migration_to_entities(
        &self,
        version: impl Into<String>,
        description: impl Into<String>,
        entities: &[&'static EntityMetadata],
    ) -> Result<PlannedMigration, sqlx::Error>
    where
        B: IntrospectionBackend,
    {
        let current = B::introspect_schema(self.database.pool()).await?;
        let target = SchemaModel::from_entities(entities);
        self.plan_migration(version, description, &current, &target)
    }

    /// Introspect the live database and plan a full schema target, including
    /// deterministic Postgres RLS statements when policy allows planning them.
    pub async fn plan_schema_target(
        &self,
        version: impl Into<String>,
        description: impl Into<String>,
        target: &SchemaTarget,
    ) -> Result<PlannedSchemaTarget, sqlx::Error>
    where
        B: IntrospectionBackend,
    {
        ensure_planning_policy(self.policy(), "plan schema target")?;
        let version = version.into();
        let description = description.into();
        let current = B::introspect_schema(self.database.pool()).await?;
        let migration = plan_migration_for_backend::<B>(
            version.clone(),
            description.clone(),
            Some(current.stable_hash()),
            target.schema.stable_hash(),
            &current,
            &target.schema,
        );
        let rls = build_rls_policy_plan(B::DIALECT, self.policy(), &target.rls);
        let mut statements = migration.statements.clone();
        statements.extend(rls.statements.clone());
        let target_hash = target.stable_hash();
        let plan_hash = stable_plan_hash(B::DIALECT.name(), &migration.steps, &statements);
        Ok(PlannedSchemaTarget {
            version,
            description,
            backend: B::DIALECT.name(),
            source_schema_hash: migration.source_schema_hash.clone(),
            target_schema_hash: migration.target_schema_hash.clone(),
            target_hash,
            plan_hash,
            migration,
            rls,
            statements,
        })
    }

    /// Apply a planned migration according to [`ApplyOptions`].
    ///
    /// This method is only available for backends that implement
    /// [`MigrationBackend`].
    pub async fn apply_migration(
        &self,
        plan: &PlannedMigration,
        options: ApplyOptions,
    ) -> Result<AppliedMigrationReport, sqlx::Error>
    where
        B: MigrationBackend,
    {
        ensure_managed_policy(self.policy(), "apply schema migration")?;
        reject_disallowed_risks(plan, &options)?;
        if let Some(expected) = &options.expected_current_schema_hash {
            if plan.source_schema_hash.as_ref() != Some(expected) {
                return Err(sqlx::Error::Protocol(format!(
                    "Schema migration baseline hash mismatch: expected {}, planned {:?}",
                    expected, plan.source_schema_hash
                )));
            }
        }

        if options.dry_run {
            return Ok(AppliedMigrationReport {
                version: plan.version.clone(),
                dry_run: true,
                statements_applied: 0,
            });
        }

        let metadata = MigrationApplicationMetadata {
            backend: B::DIALECT.name(),
            graphql_orm_version: env!("CARGO_PKG_VERSION"),
            source_schema_hash: plan.source_schema_hash.clone(),
            target_schema_hash: plan.target_schema_hash.clone(),
            plan_hash: plan.plan_hash.clone(),
            policy: self.policy(),
        };
        B::prepare_migration_runtime(self.database.pool()).await?;
        B::apply_migration_statements_transactionally(
            self.database.pool(),
            &plan.version,
            &plan.description,
            &plan.statements,
            Some(&metadata),
            options.record_history,
        )
        .await?;

        Ok(AppliedMigrationReport {
            version: plan.version.clone(),
            dry_run: false,
            statements_applied: plan.statements.len(),
        })
    }

    /// Apply a planned full schema target transactionally.
    pub async fn apply_schema_target(
        &self,
        plan: &PlannedSchemaTarget,
        options: ApplyOptions,
    ) -> Result<AppliedMigrationReport, sqlx::Error>
    where
        B: MigrationBackend,
    {
        ensure_managed_policy(self.policy(), "apply schema target")?;
        reject_disallowed_risks(&plan.migration, &options)?;
        if let Some(expected) = &options.expected_current_schema_hash {
            if plan.source_schema_hash.as_ref() != Some(expected) {
                return Err(sqlx::Error::Protocol(format!(
                    "Schema target baseline hash mismatch: expected {}, planned {:?}",
                    expected, plan.source_schema_hash
                )));
            }
        }

        if options.dry_run {
            return Ok(AppliedMigrationReport {
                version: plan.version.clone(),
                dry_run: true,
                statements_applied: 0,
            });
        }

        let metadata = MigrationApplicationMetadata {
            backend: B::DIALECT.name(),
            graphql_orm_version: env!("CARGO_PKG_VERSION"),
            source_schema_hash: plan.source_schema_hash.clone(),
            target_schema_hash: plan.target_hash.clone(),
            plan_hash: plan.plan_hash.clone(),
            policy: self.policy(),
        };
        B::prepare_migration_runtime(self.database.pool()).await?;
        B::apply_migration_statements_transactionally(
            self.database.pool(),
            &plan.version,
            &plan.description,
            &plan.statements,
            Some(&metadata),
            options.record_history,
        )
        .await?;

        Ok(AppliedMigrationReport {
            version: plan.version.clone(),
            dry_run: false,
            statements_applied: plan.statements.len(),
        })
    }

    /// Return the latest version recorded in `__graphql_orm_migrations`.
    pub async fn current_version(&self) -> Result<Option<String>, sqlx::Error>
    where
        B: MigrationBackend,
    {
        Ok(applied_migration_records::<B>(self.database.pool())
            .await?
            .last()
            .map(|record| record.version.clone()))
    }

    /// Plan a forward upgrade through a [`SchemaAbi`] to the requested target version.
    pub async fn plan_upgrade(
        &self,
        abi: &SchemaAbi,
        target_version: &str,
    ) -> Result<PlannedSchemaUpgrade, sqlx::Error>
    where
        B: MigrationBackend,
    {
        ensure_planning_policy(self.policy(), "plan schema ABI upgrade")?;
        let current_version = self.current_version().await?;
        let path = abi.path(current_version.as_deref(), target_version)?;
        let mut current_schema = B::introspect_schema(self.database.pool()).await?;
        let mut stages = Vec::new();

        for stage in path {
            let planned = plan_migration_for_backend::<B>(
                stage.version.clone(),
                stage.description.clone(),
                Some(current_schema.stable_hash()),
                stage.target_schema_hash.clone(),
                &current_schema,
                &stage.target_schema,
            );
            current_schema = stage.target_schema.clone();
            stages.push(planned);
        }

        Ok(PlannedSchemaUpgrade { stages })
    }

    /// Apply a forward upgrade through a [`SchemaAbi`] to the requested target version.
    pub async fn apply_upgrade(
        &self,
        abi: &SchemaAbi,
        target_version: &str,
        options: ApplyOptions,
    ) -> Result<AppliedSchemaUpgrade, sqlx::Error>
    where
        B: MigrationBackend,
    {
        ensure_managed_policy(self.policy(), "apply schema ABI upgrade")?;
        if options.require_clean_schema {
            self.validate_current_baseline(abi).await?;
        }
        let planned = self.plan_upgrade(abi, target_version).await?;
        let mut applied = Vec::new();
        for plan in &planned.stages {
            applied.push(self.apply_migration(plan, options.clone()).await?);
        }
        Ok(AppliedSchemaUpgrade { applied })
    }

    async fn validate_current_baseline(&self, abi: &SchemaAbi) -> Result<(), sqlx::Error>
    where
        B: MigrationBackend,
    {
        let Some(current_version) = self.current_version().await? else {
            return Ok(());
        };
        let Some(current_stage) = abi.stage(&current_version) else {
            return Err(sqlx::Error::Protocol(format!(
                "Current schema ABI version {current_version} is not present in the ABI"
            )));
        };
        let current_schema = B::introspect_schema(self.database.pool()).await?;
        let report = self.validate(&current_schema, &current_stage.target_schema)?;
        if report.has_errors() {
            return Err(sqlx::Error::Protocol(format!(
                "Current database schema does not match ABI version {current_version}; validation produced {} diagnostics",
                report.diagnostics.len()
            )));
        }
        Ok(())
    }
}

pub fn validate_schema_models(
    backend: &'static str,
    policy: SchemaPolicy,
    current: &SchemaModel,
    target: &SchemaModel,
) -> SchemaValidationReport {
    let current_tables = current
        .tables
        .iter()
        .map(|table| (table.table_name.as_str(), table))
        .collect::<std::collections::BTreeMap<_, _>>();
    let target_tables = target
        .tables
        .iter()
        .map(|table| (table.table_name.as_str(), table))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut diagnostics = Vec::new();

    if backend == "postgres" {
        let current_extensions = current
            .extensions
            .iter()
            .map(|extension| extension.to_ascii_lowercase())
            .collect::<std::collections::BTreeSet<_>>();
        for extension in &target.extensions {
            if !current_extensions.contains(&extension.to_ascii_lowercase()) {
                diagnostics.push(diagnostic(
                    SchemaDiagnosticSeverity::Error,
                    SchemaDiagnosticKind::UnsupportedBackendCapability,
                    None,
                    None,
                    format!("Missing database extension {extension}"),
                ));
            }
        }
    }

    for (table_name, target_table) in &target_tables {
        let Some(current_table) = current_tables.get(table_name) else {
            diagnostics.push(diagnostic(
                SchemaDiagnosticSeverity::Error,
                SchemaDiagnosticKind::MissingTable,
                Some((*table_name).to_string()),
                None,
                format!("Missing table {table_name}"),
            ));
            continue;
        };

        if current_table.primary_keys() != target_table.primary_keys() {
            diagnostics.push(diagnostic(
                SchemaDiagnosticSeverity::Error,
                SchemaDiagnosticKind::PrimaryKeyMismatch,
                Some((*table_name).to_string()),
                None,
                format!(
                    "Primary key mismatch on {table_name}: current ({}) target ({})",
                    current_table.primary_keys().join(", "),
                    target_table.primary_keys().join(", ")
                ),
            ));
        }

        let current_columns = current_table
            .columns
            .iter()
            .map(|column| (column.name.as_str(), column))
            .collect::<std::collections::BTreeMap<_, _>>();
        let target_columns = target_table
            .columns
            .iter()
            .map(|column| (column.name.as_str(), column))
            .collect::<std::collections::BTreeMap<_, _>>();

        for (column_name, target_column) in &target_columns {
            let Some(current_column) = current_columns.get(column_name) else {
                diagnostics.push(diagnostic(
                    SchemaDiagnosticSeverity::Error,
                    SchemaDiagnosticKind::MissingColumn,
                    Some((*table_name).to_string()),
                    Some((*column_name).to_string()),
                    format!("Missing column {table_name}.{column_name}"),
                ));
                continue;
            };
            if !current_column
                .sql_type
                .eq_ignore_ascii_case(&target_column.sql_type)
            {
                diagnostics.push(diagnostic(
                    SchemaDiagnosticSeverity::Error,
                    SchemaDiagnosticKind::TypeMismatch,
                    Some((*table_name).to_string()),
                    Some((*column_name).to_string()),
                    format!(
                        "Type mismatch on {table_name}.{column_name}: current {} target {}",
                        current_column.sql_type, target_column.sql_type
                    ),
                ));
            }
            if current_column.nullable != target_column.nullable {
                diagnostics.push(diagnostic(
                    SchemaDiagnosticSeverity::Error,
                    SchemaDiagnosticKind::NullabilityMismatch,
                    Some((*table_name).to_string()),
                    Some((*column_name).to_string()),
                    format!("Nullability mismatch on {table_name}.{column_name}"),
                ));
            }
        }

        for column_name in current_columns.keys() {
            if !target_columns.contains_key(column_name) {
                diagnostics.push(diagnostic(
                    SchemaDiagnosticSeverity::Warning,
                    SchemaDiagnosticKind::ExtraColumn,
                    Some((*table_name).to_string()),
                    Some((*column_name).to_string()),
                    format!("Extra column {table_name}.{column_name}"),
                ));
            }
        }

        let current_indexes = current_table
            .indexes
            .iter()
            .map(|index| (index.name, index))
            .collect::<std::collections::BTreeMap<_, _>>();
        for target_index in &target_table.indexes {
            let Some(current_index) = current_indexes.get(target_index.name) else {
                diagnostics.push(diagnostic(
                    SchemaDiagnosticSeverity::Error,
                    SchemaDiagnosticKind::IndexMismatch,
                    Some((*table_name).to_string()),
                    None,
                    format!("Missing index {} on {table_name}", target_index.name),
                ));
                continue;
            };
            if *current_index != target_index {
                diagnostics.push(diagnostic(
                    SchemaDiagnosticSeverity::Error,
                    SchemaDiagnosticKind::IndexMismatch,
                    Some((*table_name).to_string()),
                    None,
                    format!("Index mismatch on {table_name}.{}", target_index.name),
                ));
            }
        }
    }

    for table_name in current_tables.keys() {
        if !target_tables.contains_key(table_name) {
            diagnostics.push(diagnostic(
                SchemaDiagnosticSeverity::Warning,
                SchemaDiagnosticKind::ExtraTable,
                Some((*table_name).to_string()),
                None,
                format!("Extra table {table_name}"),
            ));
        }
    }

    if !policy.allows_entity_writes() {
        for table in &target.tables {
            if table.columns.iter().any(|column| !column.is_primary_key) {
                diagnostics.push(diagnostic(
                    SchemaDiagnosticSeverity::Info,
                    SchemaDiagnosticKind::WriteFieldOnReadOnlyBackend,
                    Some(table.table_name.clone()),
                    None,
                    format!(
                        "Schema policy {policy} is read-only; generated write paths must remain unavailable for {}",
                        table.table_name
                    ),
                ));
            }
        }
    }

    SchemaValidationReport {
        backend,
        policy,
        current_schema_hash: Some(current.stable_hash()),
        target_schema_hash: target.stable_hash(),
        diagnostics,
    }
}

fn diagnostic(
    severity: SchemaDiagnosticSeverity,
    kind: SchemaDiagnosticKind,
    table: Option<String>,
    column: Option<String>,
    message: String,
) -> SchemaDiagnostic {
    SchemaDiagnostic {
        severity,
        kind,
        table,
        column,
        message,
    }
}

fn plan_migration_for_backend<B: OrmBackend>(
    version: String,
    description: String,
    source_schema_hash: Option<String>,
    target_schema_hash: String,
    current: &SchemaModel,
    target: &SchemaModel,
) -> PlannedMigration {
    let plan = build_migration_plan(B::DIALECT, current, target);
    let steps = classify_migration_steps(&plan.steps);
    let plan_hash = stable_plan_hash(B::DIALECT.name(), &steps, &plan.statements);
    PlannedMigration {
        version,
        description,
        backend: B::DIALECT.name(),
        source_schema_hash,
        target_schema_hash,
        plan_hash,
        steps,
        statements: plan.statements,
    }
}

fn reject_disallowed_risks(
    plan: &PlannedMigration,
    options: &ApplyOptions,
) -> Result<(), sqlx::Error> {
    if options.allow_destructive {
        return Ok(());
    }
    if let Some(step) = plan
        .steps
        .iter()
        .find(|step| step.risk == MigrationRisk::Destructive)
    {
        return Err(sqlx::Error::Protocol(format!(
            "Migration {} contains destructive step {:?}; set allow_destructive to apply it",
            plan.version, step.step
        )));
    }
    Ok(())
}

fn stable_plan_hash(
    backend: &'static str,
    steps: &[PlannedMigrationStep],
    statements: &[String],
) -> String {
    let mut canonical = backend.to_string();
    canonical.push('\n');
    for step in steps {
        canonical.push_str(&format!(
            "{:?}|{:?}|{}\n",
            step.risk, step.step, step.reason
        ));
    }
    for statement in statements {
        canonical.push_str(statement);
        canonical.push('\n');
    }
    format!("{:016x}", fnv1a64(canonical.as_bytes()))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
