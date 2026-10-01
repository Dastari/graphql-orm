//! Bounded runtime conversion and owned canonical migration plans.
//!
//! Planning reads catalog state only. The host supplies persistent table ownership;
//! applying is a separate managed-policy operation and never reconciles host RLS.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use super::core::*;
use super::dialect::{DatabaseBackend, SqlDialect};
use super::migrations::{
    build_migration_plan_views, classify_migration_step_view, migration_step_table_name,
};
use super::owned_schema::{
    IndexModel, OwnedMigrationStep, OwnedSchemaModel, OwnedTableModel, StepRef,
    add_owned_generated_index,
};
use super::runtime_schema::*;
use super::schema_manager::{
    SchemaManager, reject_migration_risks, stable_plan_hash_views, validate_schema_views,
};
use super::{NoDefaultBackend, OrmBackend};

/// Stable classification of a runtime physical conversion/planning failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeMigrationDiagnosticCode {
    /// This backend has not opted into runtime migration execution.
    UnsupportedBackend,
    /// No supported generator is defined for the field.
    UnsupportedGeneration,
    /// Default semantics cannot be represented faithfully.
    UnsupportedDefault,
    /// Invalid or unsupported canonical physical definition.
    InvalidPhysicalContract,
    /// Physical or diagnostic names collide across composed targets.
    TargetCollision,
    /// The target requests tables outside explicit host ownership.
    OwnershipMismatch,
    /// A change would affect an external dependent object/table.
    UnmanagedDependency,
    /// A configured resource bound was exceeded.
    LimitExceeded,
    /// Schema policy refuses the requested operation.
    PolicyDenied,
    /// The reviewed live baseline or plan integrity no longer matches.
    BaselineMismatch,
}

/// A structured diagnostic identifies schema members without including default values or SQL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeMigrationDiagnostic {
    /// Stable machine-readable classification.
    pub code: RuntimeMigrationDiagnosticCode,
    /// Stable collection ID, when conversion identified a collection.
    pub collection: Option<CollectionId>,
    /// Stable field/index/relation ID, when known.
    pub subject: Option<String>,
}

/// Complete structured conversion diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeMigrationDiagnostics(pub Vec<RuntimeMigrationDiagnostic>);
impl fmt::Display for RuntimeMigrationDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "runtime migration rejected ({} diagnostics)",
            self.0.len()
        )
    }
}
impl std::error::Error for RuntimeMigrationDiagnostics {}

/// Runtime migration failure. Database sources remain available to the trusted host.
#[derive(Debug)]
pub enum RuntimeMigrationError {
    /// Structured capability, scope, bounds or integrity rejection.
    Diagnostics(RuntimeMigrationDiagnostics),
    /// Backend execution failure.
    Database(sqlx::Error),
}
impl fmt::Display for RuntimeMigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Diagnostics(d) => d.fmt(f),
            Self::Database(_) => f.write_str("runtime migration database failure"),
        }
    }
}
impl std::error::Error for RuntimeMigrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Diagnostics(d) => Some(d),
            Self::Database(e) => Some(e),
        }
    }
}
impl From<sqlx::Error> for RuntimeMigrationError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
impl From<RuntimeMigrationDiagnostics> for RuntimeMigrationError {
    fn from(diagnostics: RuntimeMigrationDiagnostics) -> Self {
        Self::Diagnostics(diagnostics)
    }
}
pub(super) fn rejection(code: RuntimeMigrationDiagnosticCode) -> RuntimeMigrationDiagnostics {
    RuntimeMigrationDiagnostics(vec![RuntimeMigrationDiagnostic {
        code,
        collection: None,
        subject: None,
    }])
}

/// Bounds applied before conversion allocations and to the completed canonical target/plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeMigrationLimits {
    /// Maximum collections/tables.
    pub max_collections: usize,
    /// Maximum fields in one collection/table.
    pub max_fields: usize,
    /// Maximum indexes on one collection/table.
    pub max_indexes: usize,
    /// Maximum relations/FKs on one collection/table.
    pub max_relations: usize,
    /// Maximum ordered key/index arity.
    pub max_key_members: usize,
    /// Maximum literal/default expression bytes per field.
    pub max_default_bytes: usize,
    /// Maximum aggregate serialized definition bytes.
    pub max_target_bytes: usize,
    /// Maximum migration steps.
    pub max_plan_steps: usize,
    /// Maximum rendered statements.
    pub max_plan_statements: usize,
    /// Maximum aggregate rendered statement bytes.
    pub max_plan_bytes: usize,
}
impl Default for RuntimeMigrationLimits {
    fn default() -> Self {
        Self {
            max_collections: 512,
            max_fields: 256,
            max_indexes: 128,
            max_relations: 128,
            max_key_members: 32,
            max_default_bytes: 65536,
            max_target_bytes: 8 * 1024 * 1024,
            max_plan_steps: 16384,
            max_plan_statements: 32768,
            max_plan_bytes: 8 * 1024 * 1024,
        }
    }
}

/// Explicit trusted managed-table ownership, independent of the current target.
///
/// Retain intentionally removed tables in this set to review guarded drops.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedTableSet(BTreeSet<String>);
impl ManagedTableSet {
    /// Validate portable trusted table identifiers, rejecting ORM-reserved infrastructure.
    pub fn new(
        tables: impl IntoIterator<Item = String>,
    ) -> Result<Self, RuntimeMigrationDiagnostics> {
        let mut set = BTreeSet::new();
        for table in tables {
            if !portable_identifier(&table)
                || table.starts_with("__graphql_orm_")
                || !set.insert(table)
            {
                return Err(rejection(RuntimeMigrationDiagnosticCode::OwnershipMismatch));
            }
            if set.len() > RuntimeMigrationLimits::default().max_collections {
                return Err(rejection(RuntimeMigrationDiagnosticCode::LimitExceeded));
            }
        }
        Ok(Self(set))
    }
    /// Owned physical identifiers in deterministic order.
    pub fn tables(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
    /// Whether the trusted set owns this exact physical table.
    pub fn contains(&self, table: &str) -> bool {
        self.0.contains(table)
    }
}
fn portable_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    !value.is_empty()
        && value.len() <= MAX_PHYSICAL_IDENTIFIER_LEN
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl ValidatedRuntimeSchema {
    /// Convert the existing runtime IR into the canonical physical target without I/O or leaks.
    pub fn physical_schema<B: RuntimeMigrationBackend>(
        &self,
        limits: RuntimeMigrationLimits,
    ) -> Result<OwnedSchemaModel, RuntimeMigrationDiagnostics> {
        if !B::RUNTIME_MIGRATIONS_SUPPORTED {
            return Err(rejection(
                RuntimeMigrationDiagnosticCode::UnsupportedBackend,
            ));
        }
        let mut diagnostics = Vec::new();
        let schema = self.schema();
        if schema.collections.len() > limits.max_collections {
            return Err(rejection(RuntimeMigrationDiagnosticCode::LimitExceeded));
        }
        let mut definition_bytes = BoundedDefinitionWriter::new(limits.max_target_bytes);
        for collection in &schema.collections {
            let mut scoped = |code, subject: Option<String>| {
                diagnostics.push(RuntimeMigrationDiagnostic {
                    code,
                    collection: Some(collection.id.clone()),
                    subject,
                })
            };
            if collection.fields.len() > limits.max_fields
                || collection.indexes.len() > limits.max_indexes
                || collection.relations.len() > limits.max_relations
                || collection.primary_key.len() > limits.max_key_members
                || collection
                    .composite_unique
                    .iter()
                    .any(|key| key.len() > limits.max_key_members)
                || collection
                    .indexes
                    .iter()
                    .any(|index| index.fields.len() > limits.max_key_members)
                || collection
                    .relations
                    .iter()
                    .any(|relation| relation.key_pairs.len() > limits.max_key_members)
            {
                scoped(RuntimeMigrationDiagnosticCode::LimitExceeded, None);
            }
            if collection.physical_table.starts_with("__graphql_orm_") {
                scoped(
                    RuntimeMigrationDiagnosticCode::InvalidPhysicalContract,
                    None,
                );
            }
            // Count without allocating a serialized catalog copy.
            serde_json::to_writer(&mut definition_bytes, collection)
                .map_err(|_| rejection(RuntimeMigrationDiagnosticCode::LimitExceeded))?;
            for field in &collection.fields {
                if field.generated
                    && field.default.is_none()
                    && field.value_kind != RuntimeValueKind::Uuid
                {
                    scoped(
                        RuntimeMigrationDiagnosticCode::UnsupportedGeneration,
                        Some(field.id.as_str().into()),
                    );
                }
                if matches!(&field.default, Some(RuntimeDefault::Literal(value)) if value.len() > limits.max_default_bytes)
                {
                    scoped(
                        RuntimeMigrationDiagnosticCode::LimitExceeded,
                        Some(field.id.as_str().into()),
                    );
                }
            }
        }
        if definition_bytes.size > limits.max_target_bytes {
            diagnostics.push(RuntimeMigrationDiagnostic {
                code: RuntimeMigrationDiagnosticCode::LimitExceeded,
                collection: None,
                subject: None,
            });
        }
        if !diagnostics.is_empty() {
            return Err(RuntimeMigrationDiagnostics(diagnostics));
        }
        let by_id = schema
            .collections
            .iter()
            .map(|c| (&c.id, c))
            .collect::<BTreeMap<_, _>>();
        let mut tables = Vec::with_capacity(schema.collections.len());
        for collection in &schema.collections {
            let fields = collection
                .fields
                .iter()
                .map(|f| (&f.id, f))
                .collect::<BTreeMap<_, _>>();
            let column_name = |id: &FieldId| fields[id].physical_column.clone();
            let keys = collection
                .primary_key
                .iter()
                .map(column_name)
                .collect::<Vec<_>>();
            let mut table = OwnedTableModel {
                entity_name: collection.api_type_name.clone(),
                table_name: collection.physical_table.clone(),
                primary_key: keys[0].clone(),
                primary_keys: keys,
                default_sort: collection
                    .default_order
                    .iter()
                    .map(|term| {
                        format!(
                            "{} {}",
                            fields[&term.field].physical_column,
                            match term.direction {
                                RuntimeOrderDirection::Asc => "ASC",
                                RuntimeOrderDirection::Desc => "DESC",
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
                columns: collection
                    .fields
                    .iter()
                    .map(|field| {
                        Ok(ColumnModel {
                            name: field.physical_column.clone(),
                            sql_type: runtime_sql_type(B::DIALECT, field.value_kind).into(),
                            spatial: None,
                            nullable: field.nullable,
                            is_primary_key: collection.primary_key.contains(&field.id),
                            is_unique: field.unique,
                            default: runtime_default(B::DIALECT, field)?.map(|v| {
                                super::dialect::canonicalize_column_default_expression(&v)
                            }),
                        })
                    })
                    .collect::<Result<Vec<_>, RuntimeMigrationDiagnostics>>()?,
                indexes: collection
                    .indexes
                    .iter()
                    .map(|index| {
                        let mut owned = IndexModel::new(
                            index.name.clone(),
                            index.fields.iter().map(column_name).collect(),
                        );
                        owned.is_unique = index.unique;
                        owned
                    })
                    .collect(),
                composite_unique_indexes: collection
                    .composite_unique
                    .iter()
                    .map(|key| key.iter().map(column_name).collect())
                    .collect(),
                foreign_keys: collection
                    .relations
                    .iter()
                    .filter(|r| r.enforce_foreign_key)
                    .map(|relation| {
                        let target = by_id[&relation.target];
                        ForeignKeyModel {
                            constraint_name: None,
                            column_pairs: relation
                                .key_pairs
                                .iter()
                                .map(|pair| ForeignKeyColumnPairModel {
                                    source_column: column_name(&pair.source),
                                    target_column: target
                                        .fields
                                        .iter()
                                        .find(|field| field.id == pair.target)
                                        .expect("validated relation target member")
                                        .physical_column
                                        .clone(),
                                })
                                .collect(),
                            target_table: target.physical_table.clone(),
                            is_multiple: relation.cardinality == RelationCardinality::Many,
                            on_delete: relation
                                .on_delete
                                .clone()
                                .expect("validated enforcing relation delete action"),
                        }
                    })
                    .collect(),
                search_indexes: vec![],
                append_only: collection.append_only,
                retention_purge: collection.retention_purge,
                check_constraints: vec![],
            };
            for field in &collection.fields {
                if field.filterable {
                    add_owned_generated_index(&mut table, &[&field.physical_column]);
                }
            }
            for relation in collection
                .relations
                .iter()
                .filter(|r| r.enforce_foreign_key)
            {
                let columns = relation
                    .key_pairs
                    .iter()
                    .map(|pair| fields[&pair.source].physical_column.as_str())
                    .collect::<Vec<_>>();
                add_owned_generated_index(&mut table, &columns);
            }
            tables.push(table);
        }
        // The same generated-index rule serves forward and inverse relations,
        // including non-enforcing logical relations on target columns.
        for collection in &schema.collections {
            for relation in &collection.relations {
                let (table_name, columns) = if relation.cardinality == RelationCardinality::Many {
                    let target = by_id[&relation.target];
                    (
                        &target.physical_table,
                        relation
                            .key_pairs
                            .iter()
                            .map(|pair| {
                                target
                                    .fields
                                    .iter()
                                    .find(|field| field.id == pair.target)
                                    .expect("validated member")
                                    .physical_column
                                    .as_str()
                            })
                            .collect::<Vec<_>>(),
                    )
                } else {
                    (
                        &collection.physical_table,
                        relation
                            .key_pairs
                            .iter()
                            .map(|pair| {
                                collection
                                    .fields
                                    .iter()
                                    .find(|field| field.id == pair.source)
                                    .expect("validated member")
                                    .physical_column
                                    .as_str()
                            })
                            .collect::<Vec<_>>(),
                    )
                };
                let table = tables
                    .iter_mut()
                    .find(|table| &table.table_name == table_name)
                    .expect("validated target");
                add_owned_generated_index(table, &columns);
            }
        }
        let target = OwnedSchemaModel {
            extensions: vec![],
            tables,
            limits,
        };
        target
            .validate_physical_contract()
            .map_err(|_| rejection(RuntimeMigrationDiagnosticCode::InvalidPhysicalContract))?;
        target.check_limits(limits)?;
        Ok(target)
    }
}
fn runtime_sql_type(backend: DatabaseBackend, kind: RuntimeValueKind) -> &'static str {
    match (backend, kind) {
        (DatabaseBackend::Postgres, RuntimeValueKind::Boolean) => "BOOLEAN",
        (_, RuntimeValueKind::Boolean | RuntimeValueKind::Integer)
            if backend == DatabaseBackend::Sqlite =>
        {
            "INTEGER"
        }
        (_, RuntimeValueKind::Integer) => "BIGINT",
        (DatabaseBackend::Postgres, RuntimeValueKind::Float) => "DOUBLE PRECISION",
        (_, RuntimeValueKind::Float) => "REAL",
        (DatabaseBackend::Postgres, RuntimeValueKind::Uuid) => "UUID",
        (DatabaseBackend::Postgres, RuntimeValueKind::Json) => "JSONB",
        (DatabaseBackend::Postgres, RuntimeValueKind::Bytes) => "BYTEA",
        (_, RuntimeValueKind::Bytes) => "BLOB",
        (DatabaseBackend::Postgres, RuntimeValueKind::DateTime) => "TIMESTAMPTZ",
        _ => "TEXT",
    }
}
fn runtime_default(
    backend: DatabaseBackend,
    field: &RuntimeField,
) -> Result<Option<String>, RuntimeMigrationDiagnostics> {
    match &field.default {
        None => Ok(None),
        Some(RuntimeDefault::CurrentTimestamp) => match field.value_kind {
            RuntimeValueKind::Integer => Ok(Some(backend.current_epoch_expr().into())),
            RuntimeValueKind::DateTime if backend == DatabaseBackend::Sqlite => {
                Ok(Some("strftime('%Y-%m-%dT%H:%M:%f000Z', 'now')".into()))
            }
            RuntimeValueKind::DateTime => Ok(Some("CURRENT_TIMESTAMP".into())),
            _ => Err(rejection(
                RuntimeMigrationDiagnosticCode::UnsupportedDefault,
            )),
        },
        Some(RuntimeDefault::Literal(value)) => Ok(Some(match field.value_kind {
            RuntimeValueKind::String | RuntimeValueKind::Uuid => {
                format!("'{}'", value.replace('\'', "''"))
            }
            RuntimeValueKind::Boolean | RuntimeValueKind::Integer | RuntimeValueKind::Float => {
                value.clone()
            }
            _ => {
                return Err(rejection(
                    RuntimeMigrationDiagnosticCode::UnsupportedDefault,
                ));
            }
        })),
    }
}

impl OwnedSchemaModel {
    /// Compose host-owned static/system entities, preserving all physical policy metadata.
    /// RLS declarations remain on the separate existing [`SchemaTarget`] path.
    pub fn with_static_entities(
        mut self,
        entities: &[&EntityMetadata],
    ) -> Result<Self, RuntimeMigrationDiagnostics> {
        let other = Self::from_entities(entities);
        self.tables.extend(other.tables);
        self.extensions.extend(other.extensions);
        self.extensions.sort();
        self.extensions.dedup();
        let mut names = BTreeSet::new();
        let mut labels = BTreeSet::new();
        if self.tables.iter().any(|table| {
            !names.insert(table.table_name.clone()) || !labels.insert(table.entity_name.clone())
        }) {
            return Err(rejection(RuntimeMigrationDiagnosticCode::TargetCollision));
        }
        let physical = self
            .tables
            .iter()
            .map(|t| (t.entity_name.clone(), t.table_name.clone()))
            .collect::<BTreeMap<_, _>>();
        for table in &mut self.tables {
            for foreign_key in &mut table.foreign_keys {
                if let Some(name) = physical.get(&foreign_key.target_table) {
                    foreign_key.target_table.clone_from(name);
                }
            }
        }
        self.check_limits(self.limits)?;
        self.validate_physical_contract()
            .map_err(|_| rejection(RuntimeMigrationDiagnosticCode::InvalidPhysicalContract))?;
        Ok(self)
    }
    fn check_limits(
        &self,
        limits: RuntimeMigrationLimits,
    ) -> Result<(), RuntimeMigrationDiagnostics> {
        if self.tables.len() > limits.max_collections
            || self.tables.iter().any(|table| {
                table.columns.len() > limits.max_fields
                    || table.indexes.len() > limits.max_indexes
                    || table.foreign_keys.len() > limits.max_relations
                    || table.primary_keys().len() > limits.max_key_members
                    || table.columns.iter().any(|column| {
                        column
                            .default
                            .as_ref()
                            .is_some_and(|value| value.len() > limits.max_default_bytes)
                    })
                    || table
                        .indexes
                        .iter()
                        .any(|index| index.columns.len() > limits.max_key_members)
                    || table
                        .composite_unique_indexes
                        .iter()
                        .any(|key| key.len() > limits.max_key_members)
                    || table
                        .foreign_keys
                        .iter()
                        .any(|fk| fk.column_pairs.len() > limits.max_key_members)
            })
        {
            return Err(rejection(RuntimeMigrationDiagnosticCode::LimitExceeded));
        }
        // The canonical Debug view includes every stored string, including checks,
        // predicates, FK members and search metadata; writing counts, never copies.
        let mut writer = BoundedDefinitionWriter::new(limits.max_target_bytes);
        std::fmt::write(&mut writer, format_args!("{:?}", self.borrowed()))
            .map_err(|_| rejection(RuntimeMigrationDiagnosticCode::LimitExceeded))
    }
}

/// Read-only inspection of one classified owned canonical step.
#[derive(Clone, Debug)]
pub struct OwnedPlannedMigrationStep {
    step: OwnedMigrationStep,
    risk: MigrationRisk,
    reason: String,
}
impl OwnedPlannedMigrationStep {
    /// Existing canonical risk classification.
    pub fn risk(&self) -> MigrationRisk {
        self.risk
    }
    /// Existing classification explanation.
    pub fn reason(&self) -> &str {
        &self.reason
    }
    /// Physical table affected, if any.
    pub fn table_name(&self) -> Option<&str> {
        migration_step_table_name(&StepRef::from(&self.step))
    }
}

/// Immutable owned plan bound to explicit ownership, backend and introspected baseline.
#[derive(Clone, Debug)]
pub struct OwnedPlannedMigration {
    version: String,
    description: String,
    backend: &'static str,
    source_schema_hash: String,
    target_schema_hash: String,
    plan_hash: String,
    ownership: ManagedTableSet,
    steps: Vec<OwnedPlannedMigrationStep>,
    statements: Vec<String>,
    target: OwnedSchemaModel,
}
impl OwnedPlannedMigration {
    /// Host version identifier.
    pub fn version(&self) -> &str {
        &self.version
    }
    /// Human description.
    pub fn description(&self) -> &str {
        &self.description
    }
    /// Selected backend.
    pub fn backend(&self) -> &'static str {
        self.backend
    }
    /// Complete introspected physical baseline hash.
    pub fn source_schema_hash(&self) -> &str {
        &self.source_schema_hash
    }
    /// Existing canonical physical target hash.
    pub fn target_schema_hash(&self) -> &str {
        &self.target_schema_hash
    }
    /// Plan integrity bound to the explicit managed set.
    pub fn plan_hash(&self) -> &str {
        &self.plan_hash
    }
    /// Trusted owned tables.
    pub fn ownership(&self) -> &ManagedTableSet {
        &self.ownership
    }
    /// Classified immutable canonical steps.
    pub fn steps(&self) -> &[OwnedPlannedMigrationStep] {
        &self.steps
    }
    /// Exact statements reviewed by the host.
    pub fn statements(&self) -> &[String] {
        &self.statements
    }
    fn integrity_hash(&self) -> String {
        let semantic = stable_plan_hash_views(
            self.backend,
            self.steps
                .iter()
                .map(|step| (step.risk, StepRef::from(&step.step), step.reason.as_str())),
            &self.statements,
        );
        let canonical = format!(
            "owned-migration-v1|{}|{}|{}|{}|{}|{}",
            self.version,
            self.description,
            self.source_schema_hash,
            self.target_schema_hash,
            self.ownership.tables().collect::<Vec<_>>().join(","),
            semantic
        );
        format!("{:016x}", fnv1a64(canonical.as_bytes()))
    }
    pub(super) fn validate_apply(
        &self,
        policy: SchemaPolicy,
        backend: &'static str,
        options: &ApplyOptions,
    ) -> Result<(), RuntimeMigrationError> {
        if !policy.allows_application() {
            return Err(rejection(RuntimeMigrationDiagnosticCode::PolicyDenied).into());
        }
        if self.backend != backend
            || self.plan_hash != self.integrity_hash()
            || self.target_schema_hash != self.target.stable_hash()
            || self
                .steps
                .iter()
                .filter_map(OwnedPlannedMigrationStep::table_name)
                .any(|table| !self.ownership.contains(table))
        {
            return Err(rejection(RuntimeMigrationDiagnosticCode::BaselineMismatch).into());
        }
        if options
            .expected_current_schema_hash
            .as_deref()
            .is_some_and(|expected| expected != self.source_schema_hash)
        {
            return Err(rejection(RuntimeMigrationDiagnosticCode::BaselineMismatch).into());
        }
        reject_migration_risks(
            &self.version,
            self.steps
                .iter()
                .map(|step| (step.risk, StepRef::from(&step.step))),
            options,
        )?;
        Ok(())
    }
    #[cfg(feature = "sqlite")]
    pub(super) fn rebuilt_sqlite_tables(&self) -> impl Iterator<Item = &str> {
        self.ownership.tables().filter(|table| {
            let temporary = format!("__graphql_orm_{table}_new");
            self.statements
                .iter()
                .any(|sql| sql.starts_with(&format!("CREATE TABLE {temporary} ")))
        })
    }
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(super) fn application_metadata(&self) -> MigrationApplicationMetadata {
        MigrationApplicationMetadata {
            backend: self.backend,
            graphql_orm_version: env!("CARGO_PKG_VERSION"),
            source_schema_hash: Some(self.source_schema_hash.clone()),
            target_schema_hash: self.target_schema_hash.clone(),
            plan_hash: self.plan_hash.clone(),
            policy: SchemaPolicy::Managed,
        }
    }
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(super) fn recorded_version_report(
        &self,
        recorded: bool,
    ) -> crate::Result<Option<AppliedMigrationReport>> {
        super::schema_manager::recorded_version_report(
            &self.version,
            super::schema_manager::RemainingPlanWork {
                migration_steps: self.steps.len(),
                migration_statements: self.statements.len(),
                rls_statements: 0,
                combined_statements: self.statements.len(),
            },
            recorded,
        )
    }
    #[cfg(feature = "postgres")]
    pub(super) fn changes_protected_rls_structure(&self, table: &str) -> bool {
        self.steps.iter().any(|step| match &step.step {
            OwnedMigrationStep::DropTable { table_name }
            | OwnedMigrationStep::DropColumn { table_name, .. }
            | OwnedMigrationStep::AlterColumn { table_name, .. } => table_name == table,
            _ => false,
        })
    }
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(super) fn check_baseline(
        &self,
        current: &OwnedSchemaModel,
    ) -> Result<(), RuntimeMigrationError> {
        if current.stable_hash() != self.source_schema_hash {
            return Err(rejection(RuntimeMigrationDiagnosticCode::BaselineMismatch).into());
        }
        validate_owned_effects(
            if self.backend == "postgres" {
                DatabaseBackend::Postgres
            } else {
                DatabaseBackend::Sqlite
            },
            current,
            &self.target,
            &self.ownership,
        )
        .map_err(Into::into)
    }
}

/// Additional capability; existing backend trait implementations acquire no new requirements.
#[allow(async_fn_in_trait)]
pub trait RuntimeMigrationBackend: OrmBackend {
    /// True only for implemented owned migration lanes.
    const RUNTIME_MIGRATIONS_SUPPORTED: bool = false;
    /// Read the existing canonical physical schema using owned index storage.
    #[doc(hidden)]
    async fn introspect_owned(
        pool: &Self::Pool,
    ) -> Result<OwnedSchemaModel, RuntimeMigrationError> {
        let _ = pool;
        Err(rejection(RuntimeMigrationDiagnosticCode::UnsupportedBackend).into())
    }
    /// Read all physical dependency sources, including ORM infrastructure, without leaks.
    #[doc(hidden)]
    async fn introspect_dependencies(
        pool: &Self::Pool,
    ) -> Result<OwnedSchemaModel, RuntimeMigrationError> {
        let _ = pool;
        Err(rejection(RuntimeMigrationDiagnosticCode::UnsupportedBackend).into())
    }
    /// Reject live semantics that cannot be safely preserved by owned application.
    #[doc(hidden)]
    async fn validate_owned_plan(
        pool: &Self::Pool,
        plan: &OwnedPlannedMigration,
    ) -> Result<(), RuntimeMigrationError> {
        let _ = (pool, plan);
        Err(rejection(RuntimeMigrationDiagnosticCode::UnsupportedBackend).into())
    }
    /// Guarded application through the existing migration transaction/history engine.
    #[doc(hidden)]
    async fn apply_owned(
        pool: &Self::Pool,
        plan: &OwnedPlannedMigration,
        options: &ApplyOptions,
    ) -> Result<AppliedMigrationReport, RuntimeMigrationError> {
        let _ = (pool, plan, options);
        Err(rejection(RuntimeMigrationDiagnosticCode::UnsupportedBackend).into())
    }
}
#[cfg(feature = "mssql")]
impl RuntimeMigrationBackend for super::MssqlBackend {}
impl RuntimeMigrationBackend for NoDefaultBackend {}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
fn owned_introspection_error(error: sqlx::Error) -> RuntimeMigrationError {
    match error {
        // Catalog semantics rejected by the canonical parser are capability
        // diagnostics; transport/database errors retain their original source.
        sqlx::Error::Protocol(_) | sqlx::Error::ColumnDecode { .. } => {
            rejection(RuntimeMigrationDiagnosticCode::InvalidPhysicalContract).into()
        }
        other => RuntimeMigrationError::Database(other),
    }
}

#[cfg(feature = "sqlite")]
impl RuntimeMigrationBackend for super::SqliteBackend {
    const RUNTIME_MIGRATIONS_SUPPORTED: bool = true;
    async fn introspect_owned(
        pool: &Self::Pool,
    ) -> Result<OwnedSchemaModel, RuntimeMigrationError> {
        super::migrations::introspect_owned_sqlite_schema(pool)
            .await
            .map_err(owned_introspection_error)
    }
    async fn introspect_dependencies(
        pool: &Self::Pool,
    ) -> Result<OwnedSchemaModel, RuntimeMigrationError> {
        let mut connection = pool.acquire().await?;
        Ok(
            super::migrations::introspect_owned_sqlite_connection_with_internal(
                &mut connection,
                true,
            )
            .await?,
        )
    }
    async fn validate_owned_plan(
        pool: &Self::Pool,
        plan: &OwnedPlannedMigration,
    ) -> Result<(), RuntimeMigrationError> {
        let mut connection = pool.acquire().await?;
        super::execution::validate_owned_sqlite_plan(&mut connection, plan).await
    }
    async fn apply_owned(
        pool: &Self::Pool,
        plan: &OwnedPlannedMigration,
        options: &ApplyOptions,
    ) -> Result<AppliedMigrationReport, RuntimeMigrationError> {
        super::execution::apply_owned_sqlite_migration(pool, plan, options).await
    }
}
#[cfg(feature = "postgres")]
impl RuntimeMigrationBackend for super::PostgresBackend {
    const RUNTIME_MIGRATIONS_SUPPORTED: bool = true;
    async fn introspect_owned(
        pool: &Self::Pool,
    ) -> Result<OwnedSchemaModel, RuntimeMigrationError> {
        super::migrations::introspect_owned_postgres_schema(pool)
            .await
            .map_err(owned_introspection_error)
    }
    async fn introspect_dependencies(
        pool: &Self::Pool,
    ) -> Result<OwnedSchemaModel, RuntimeMigrationError> {
        let mut connection = pool.acquire().await?;
        Ok(
            super::migrations::introspect_owned_postgres_connection_with_internal(
                &mut connection,
                true,
            )
            .await?,
        )
    }
    async fn validate_owned_plan(
        pool: &Self::Pool,
        plan: &OwnedPlannedMigration,
    ) -> Result<(), RuntimeMigrationError> {
        let mut connection = pool.acquire().await?;
        super::execution::validate_owned_postgres_plan(&mut connection, plan).await
    }
    async fn apply_owned(
        pool: &Self::Pool,
        plan: &OwnedPlannedMigration,
        options: &ApplyOptions,
    ) -> Result<AppliedMigrationReport, RuntimeMigrationError> {
        super::execution::apply_owned_postgres_migration(pool, plan, options).await
    }
}

impl<'db, B: RuntimeMigrationBackend> SchemaManager<'db, B> {
    /// Read-only live validation within explicit table ownership.
    pub async fn validate_owned_schema(
        &self,
        target: &OwnedSchemaModel,
        ownership: &ManagedTableSet,
    ) -> Result<SchemaValidationReport, RuntimeMigrationError> {
        supported::<B>()?;
        if !self.policy().allows_validation() {
            return Err(rejection(RuntimeMigrationDiagnosticCode::PolicyDenied).into());
        }
        target.check_limits(target.limits)?;
        target
            .validate_physical_contract()
            .map_err(|_| rejection(RuntimeMigrationDiagnosticCode::InvalidPhysicalContract))?;
        let current = B::introspect_owned(self.database.pool()).await?;
        validate_owned_effects(B::DIALECT, &current, target, ownership)?;
        let scoped = scoped_current(&current, ownership, PlanOptions::strict(), target);
        Ok(validate_schema_views(
            B::DIALECT.name(),
            self.policy(),
            &scoped.borrowed(),
            &target.borrowed(),
        ))
    }
    /// Read-only bounded planning, never creating history tables or changing RLS.
    pub async fn plan_owned_migration(
        &self,
        version: impl Into<String>,
        description: impl Into<String>,
        target: &OwnedSchemaModel,
        ownership: &ManagedTableSet,
        options: PlanOptions,
    ) -> Result<OwnedPlannedMigration, RuntimeMigrationError> {
        supported::<B>()?;
        if !self.policy().allows_planning() {
            return Err(rejection(RuntimeMigrationDiagnosticCode::PolicyDenied).into());
        }
        target.check_limits(target.limits)?;
        target
            .validate_physical_contract()
            .map_err(|_| rejection(RuntimeMigrationDiagnosticCode::InvalidPhysicalContract))?;
        let version = version.into();
        let description = description.into();
        if version.trim().is_empty() || version.len() > 256 || description.len() > 4096 {
            return Err(rejection(RuntimeMigrationDiagnosticCode::LimitExceeded).into());
        }
        let current = B::introspect_owned(self.database.pool()).await?;
        validate_owned_effects(B::DIALECT, &current, target, ownership)?;
        let scoped = scoped_current(&current, ownership, options, target);
        let physical =
            build_migration_plan_views(B::DIALECT, &scoped.borrowed(), &target.borrowed());
        let limits = target.limits;
        if physical.steps.len() > limits.max_plan_steps
            || physical.statements.len() > limits.max_plan_statements
            || physical
                .statements
                .iter()
                .try_fold(0usize, |sum, sql| sum.checked_add(sql.len()))
                .is_none_or(|size| size > limits.max_plan_bytes)
        {
            return Err(rejection(RuntimeMigrationDiagnosticCode::LimitExceeded).into());
        }
        let steps = physical
            .steps
            .into_iter()
            .map(|step| {
                let (risk, reason) = classify_migration_step_view(&StepRef::from(&step));
                OwnedPlannedMigrationStep {
                    step,
                    risk,
                    reason: reason.into(),
                }
            })
            .collect();
        let mut plan = OwnedPlannedMigration {
            version,
            description,
            backend: B::DIALECT.name(),
            source_schema_hash: current.stable_hash(),
            target_schema_hash: target.stable_hash(),
            plan_hash: String::new(),
            ownership: ownership.clone(),
            steps,
            statements: physical.statements,
            target: target.clone(),
        };
        plan.plan_hash = plan.integrity_hash();
        B::validate_owned_plan(self.database.pool(), &plan).await?;
        Ok(plan)
    }
    /// Separate guarded application of an immutable owned plan; never reconciles RLS.
    pub async fn apply_owned_migration(
        &self,
        plan: &OwnedPlannedMigration,
        options: ApplyOptions,
    ) -> Result<AppliedMigrationReport, RuntimeMigrationError> {
        supported::<B>()?;
        plan.validate_apply(self.policy(), B::DIALECT.name(), &options)?;
        if options.dry_run {
            return Ok(AppliedMigrationReport {
                version: plan.version.clone(),
                dry_run: true,
                statements_applied: 0,
                already_applied: false,
            });
        }
        B::apply_owned(self.database.pool(), plan, &options).await
    }
    /// Verify complete composed incoming dependencies for later single-record runtime writes.
    pub async fn runtime_mutation_environment(
        &self,
        schema: Arc<ValidatedRuntimeSchema>,
        target: &OwnedSchemaModel,
        ownership: &ManagedTableSet,
    ) -> Result<RuntimeMutationEnvironment, RuntimeMigrationError> {
        supported::<B>()?;
        target
            .validate_physical_contract()
            .map_err(|_| rejection(RuntimeMigrationDiagnosticCode::InvalidPhysicalContract))?;
        let current = B::introspect_owned(self.database.pool()).await?;
        validate_owned_effects(B::DIALECT, &current, target, ownership)?;
        let scoped = scoped_current(&current, ownership, PlanOptions::strict(), target);
        if !build_migration_plan_views(B::DIALECT, &scoped.borrowed(), &target.borrowed())
            .steps
            .is_empty()
        {
            return Err(rejection(RuntimeMigrationDiagnosticCode::BaselineMismatch).into());
        }
        let runtime = schema.physical_schema::<B>(RuntimeMigrationLimits::default())?;
        let relevant = OwnedSchemaModel {
            limits: target.limits,
            extensions: runtime.extensions.clone(),
            tables: target
                .tables
                .iter()
                .filter(|table| {
                    runtime
                        .tables
                        .iter()
                        .any(|r| r.table_name == table.table_name)
                })
                .cloned()
                .collect(),
        };
        if relevant.tables.len() != runtime.tables.len()
            || !build_migration_plan_views(B::DIALECT, &runtime.borrowed(), &relevant.borrowed())
                .steps
                .is_empty()
            || !build_migration_plan_views(B::DIALECT, &relevant.borrowed(), &runtime.borrowed())
                .steps
                .is_empty()
        {
            return Err(rejection(RuntimeMigrationDiagnosticCode::InvalidPhysicalContract).into());
        }
        let mut incoming = BTreeMap::<String, Vec<VerifiedRuntimeDependency>>::new();
        // Live dependencies include every system/external source, even reserved infrastructure.
        let dependencies = B::introspect_dependencies(self.database.pool()).await?;
        for table in &dependencies.tables {
            for foreign_key in &table.foreign_keys {
                incoming
                    .entry(foreign_key.target_table.clone())
                    .or_default()
                    .push(VerifiedRuntimeDependency {
                        source_table: table.table_name.clone(),
                        target_table: foreign_key.target_table.clone(),
                        column_pairs: foreign_key.column_pairs.clone(),
                        on_delete: foreign_key.on_delete.clone(),
                    });
            }
        }
        Ok(RuntimeMutationEnvironment {
            schema,
            physical_baseline_hash: current.stable_hash(),
            ownership: ownership.clone(),
            incoming,
        })
    }
}
fn supported<B: RuntimeMigrationBackend>() -> Result<(), RuntimeMigrationError> {
    if B::RUNTIME_MIGRATIONS_SUPPORTED {
        Ok(())
    } else {
        Err(rejection(RuntimeMigrationDiagnosticCode::UnsupportedBackend).into())
    }
}
fn scoped_current(
    current: &OwnedSchemaModel,
    ownership: &ManagedTableSet,
    options: PlanOptions,
    target: &OwnedSchemaModel,
) -> OwnedSchemaModel {
    OwnedSchemaModel {
        limits: target.limits,
        extensions: current.extensions.clone(),
        tables: current
            .tables
            .iter()
            .filter(|table| {
                ownership.contains(&table.table_name)
                    && (!options.ignore_unmanaged_tables
                        || target
                            .tables
                            .iter()
                            .any(|t| t.table_name == table.table_name))
            })
            .cloned()
            .collect(),
    }
}
fn validate_owned_effects(
    backend: DatabaseBackend,
    current: &OwnedSchemaModel,
    target: &OwnedSchemaModel,
    ownership: &ManagedTableSet,
) -> Result<(), RuntimeMigrationDiagnostics> {
    if target
        .tables
        .iter()
        .any(|table| !ownership.contains(&table.table_name))
    {
        return Err(rejection(RuntimeMigrationDiagnosticCode::OwnershipMismatch));
    }
    for table in &current.tables {
        if let Some(wanted) = target
            .tables
            .iter()
            .find(|t| t.table_name == table.table_name)
        {
            if table.primary_keys != wanted.primary_keys {
                return Err(rejection(
                    RuntimeMigrationDiagnosticCode::InvalidPhysicalContract,
                ));
            }
        }
        // Unmanaged incoming dependencies prevent destructive/structural target changes.
        if !ownership.contains(&table.table_name) {
            for foreign_key in &table.foreign_keys {
                if ownership.contains(&foreign_key.target_table) {
                    let unchanged = current
                        .tables
                        .iter()
                        .find(|t| t.table_name == foreign_key.target_table)
                        .zip(
                            target
                                .tables
                                .iter()
                                .find(|t| t.table_name == foreign_key.target_table),
                        )
                        .is_some_and(|(before, after)| {
                            before.primary_keys == after.primary_keys
                                && before.columns.len() == after.columns.len()
                                && before.columns.iter().zip(&after.columns).all(
                                    |(before, after)| {
                                        before.name == after.name
                                            && !super::migrations::column_changed_for_backend(
                                                backend, before, after,
                                            )
                                    },
                                )
                        });
                    if !unchanged {
                        return Err(rejection(
                            RuntimeMigrationDiagnosticCode::UnmanagedDependency,
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// One verified incoming physical FK, including host system tables.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedRuntimeDependency {
    /// Owning source table.
    pub source_table: String,
    /// Referenced physical table.
    pub target_table: String,
    /// Ordered source/target members.
    pub column_pairs: Vec<ForeignKeyColumnPairModel>,
    /// Incoming action; mutation engines reject modifying cascades initially.
    pub on_delete: DeletePolicy,
}
/// Immutable schema-bound physical dependency certificate, not another schema IR.
#[derive(Clone, Debug)]
pub struct RuntimeMutationEnvironment {
    schema: Arc<ValidatedRuntimeSchema>,
    physical_baseline_hash: String,
    ownership: ManagedTableSet,
    incoming: BTreeMap<String, Vec<VerifiedRuntimeDependency>>,
}
impl RuntimeMutationEnvironment {
    /// Validated schema pinned by the host generation.
    pub fn schema(&self) -> &Arc<ValidatedRuntimeSchema> {
        &self.schema
    }
    /// Live introspection identity; revalidate if external DDL can bypass the host fence.
    pub fn physical_baseline_hash(&self) -> &str {
        &self.physical_baseline_hash
    }
    /// Explicit ownership supporting this certificate.
    pub fn ownership(&self) -> &ManagedTableSet {
        &self.ownership
    }
    /// Incoming physical FKs, including sources absent from the runtime schema.
    pub fn incoming_dependencies(&self, table: &str) -> &[VerifiedRuntimeDependency] {
        self.incoming.get(table).map(Vec::as_slice).unwrap_or(&[])
    }
}

struct BoundedDefinitionWriter {
    size: usize,
    maximum: usize,
}
impl BoundedDefinitionWriter {
    fn new(maximum: usize) -> Self {
        Self { size: 0, maximum }
    }
    fn count(&mut self, size: usize) -> bool {
        if let Some(total) = self
            .size
            .checked_add(size)
            .filter(|total| *total <= self.maximum)
        {
            self.size = total;
            true
        } else {
            false
        }
    }
}
impl std::io::Write for BoundedDefinitionWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.count(bytes.len()) {
            Ok(bytes.len())
        } else {
            Err(std::io::Error::other("definition limit exceeded"))
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl fmt::Write for BoundedDefinitionWriter {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self.count(value.len()) {
            Ok(())
        } else {
            Err(fmt::Error)
        }
    }
}
