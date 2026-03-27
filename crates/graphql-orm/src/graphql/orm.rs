use super::pagination::{Connection, Edge, PageInfo, encode_cursor};
use super::{DbPool, DbRow, PhantomData};
use sqlx::Row;
use std::sync::atomic::{AtomicUsize, Ordering};

static QUERY_COUNT: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, PartialEq)]
pub enum SqlValue {
    String(String),
    Uuid(uuid::Uuid),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MutationPhase {
    Before,
    After,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MutationFieldValue {
    pub field: String,
    pub value: SqlValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MutationEvent {
    pub phase: MutationPhase,
    pub action: ChangeAction,
    pub entity_name: &'static str,
    pub table_name: &'static str,
    pub id: String,
    pub changes: Vec<MutationFieldValue>,
}

pub trait MutationHook: Send + Sync {
    fn on_mutation<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        db: &'a crate::db::Database,
        event: &'a MutationEvent,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<()>>;
}

pub trait FieldPolicy: Send + Sync {
    fn can_read_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        db: &'a crate::db::Database,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>>;

    fn can_write_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        db: &'a crate::db::Database,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&'a (dyn std::any::Any + Send + Sync)>,
        value: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>>;
}

pub fn mutation_changes(fields: &[&str], values: &[SqlValue]) -> Vec<MutationFieldValue> {
    fields
        .iter()
        .zip(values.iter())
        .map(|(field, value)| MutationFieldValue {
            field: (*field).to_string(),
            value: value.clone(),
        })
        .collect()
}

pub fn reset_query_count() {
    QUERY_COUNT.store(0, Ordering::SeqCst);
}

pub fn query_count() -> usize {
    QUERY_COUNT.load(Ordering::SeqCst)
}

fn record_query() {
    QUERY_COUNT.fetch_add(1, Ordering::SeqCst);
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColumnDef {
    pub name: &'static str,
    pub sql_type: &'static str,
    pub nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub default: Option<&'static str>,
    pub references: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldMetadata {
    pub name: &'static str,
    pub sql_type: &'static str,
    pub nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub default: Option<&'static str>,
    pub references: Option<&'static str>,
}

impl From<&ColumnDef> for FieldMetadata {
    fn from(value: &ColumnDef) -> Self {
        Self {
            name: value.name,
            sql_type: value.sql_type,
            nullable: value.nullable,
            is_primary_key: value.is_primary_key,
            is_unique: value.is_unique,
            default: value.default,
            references: value.references,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexDef {
    pub name: &'static str,
    pub columns: &'static [&'static str],
    pub is_unique: bool,
}

impl IndexDef {
    pub fn new(name: &'static str, columns: &'static [&'static str]) -> Self {
        Self {
            name,
            columns,
            is_unique: false,
        }
    }

    pub fn unique(mut self) -> Self {
        self.is_unique = true;
        self
    }
}

pub type IndexMetadata = IndexDef;

#[derive(Clone, Debug, PartialEq)]
pub struct RelationMetadata {
    pub field_name: &'static str,
    pub target_type: &'static str,
    pub source_column: &'static str,
    pub target_column: &'static str,
    pub is_multiple: bool,
}

#[derive(Clone, Debug)]
pub struct EntityMetadata {
    pub entity_name: &'static str,
    pub table_name: &'static str,
    pub plural_name: &'static str,
    pub primary_key: &'static str,
    pub default_sort: &'static str,
    pub fields: Box<[FieldMetadata]>,
    pub indexes: Box<[IndexMetadata]>,
    pub composite_unique_indexes: Box<[Box<[&'static str]>]>,
    pub relations: Box<[RelationMetadata]>,
}

impl EntityMetadata {
    pub fn from_schema<T>(entity_name: &'static str) -> Self
    where
        T: DatabaseEntity + DatabaseSchema + EntityRelations,
    {
        Self {
            entity_name,
            table_name: T::TABLE_NAME,
            plural_name: T::PLURAL_NAME,
            primary_key: T::PRIMARY_KEY,
            default_sort: T::DEFAULT_SORT,
            fields: T::columns()
                .iter()
                .map(FieldMetadata::from)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            indexes: T::indexes().to_vec().into_boxed_slice(),
            composite_unique_indexes: T::composite_unique_indexes()
                .iter()
                .map(|columns| columns.to_vec().into_boxed_slice())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            relations: T::relation_metadata().to_vec().into_boxed_slice(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColumnModel {
    pub name: String,
    pub sql_type: String,
    pub nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ForeignKeyModel {
    pub source_column: String,
    pub target_table: String,
    pub target_column: String,
    pub is_multiple: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableModel {
    pub entity_name: String,
    pub table_name: String,
    pub primary_key: String,
    pub default_sort: String,
    pub columns: Vec<ColumnModel>,
    pub indexes: Vec<IndexMetadata>,
    pub composite_unique_indexes: Vec<Vec<String>>,
    pub foreign_keys: Vec<ForeignKeyModel>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SchemaModel {
    pub tables: Vec<TableModel>,
}

impl From<&EntityMetadata> for TableModel {
    fn from(value: &EntityMetadata) -> Self {
        Self {
            entity_name: value.entity_name.to_string(),
            table_name: value.table_name.to_string(),
            primary_key: value.primary_key.to_string(),
            default_sort: value.default_sort.to_string(),
            columns: value
                .fields
                .iter()
                .map(|field| ColumnModel {
                    name: field.name.to_string(),
                    sql_type: field.sql_type.to_string(),
                    nullable: field.nullable,
                    is_primary_key: field.is_primary_key,
                    is_unique: field.is_unique,
                    default: field.default.map(str::to_string),
                })
                .collect(),
            indexes: value.indexes.iter().cloned().collect(),
            composite_unique_indexes: value
                .composite_unique_indexes
                .iter()
                .map(|columns| columns.iter().map(|column| (*column).to_string()).collect())
                .collect(),
            foreign_keys: value
                .relations
                .iter()
                .map(|relation| ForeignKeyModel {
                    source_column: relation.source_column.to_string(),
                    target_table: relation.target_type.to_string(),
                    target_column: relation.target_column.to_string(),
                    is_multiple: relation.is_multiple,
                })
                .collect(),
        }
    }
}

impl SchemaModel {
    pub fn from_entities(entities: &[&EntityMetadata]) -> Self {
        Self {
            tables: entities
                .iter()
                .map(|entity| TableModel::from(*entity))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MigrationStep {
    CreateTable(TableModel),
    DropTable {
        table_name: String,
    },
    AddColumn {
        table_name: String,
        column: ColumnModel,
    },
    DropColumn {
        table_name: String,
        column_name: String,
    },
    AlterColumn {
        table_name: String,
        before: ColumnModel,
        after: ColumnModel,
    },
    CreateIndex {
        table_name: String,
        index: IndexMetadata,
    },
    DropIndex {
        table_name: String,
        index_name: String,
    },
    AddForeignKey {
        table_name: String,
        foreign_key: ForeignKeyModel,
    },
    DropForeignKey {
        table_name: String,
        foreign_key: ForeignKeyModel,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SchemaDiff {
    pub steps: Vec<MigrationStep>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MigrationPlan {
    pub backend: DatabaseBackend,
    pub steps: Vec<MigrationStep>,
    pub statements: Vec<String>,
}

fn render_column_definition(column: &ColumnModel) -> String {
    let mut parts = vec![format!("{} {}", column.name, column.sql_type)];
    if !column.nullable {
        parts.push("NOT NULL".to_string());
    }
    if column.is_primary_key {
        parts.push("PRIMARY KEY".to_string());
    }
    if column.is_unique && !column.is_primary_key {
        parts.push("UNIQUE".to_string());
    }
    if let Some(default) = &column.default {
        parts.push(format!("DEFAULT {}", default));
    }
    parts.join(" ")
}

fn render_create_table_statement(table: &TableModel) -> String {
    render_create_table_statement_for_name(table, &table.table_name)
}

fn render_create_table_statement_for_name(table: &TableModel, table_name: &str) -> String {
    let mut parts = table
        .columns
        .iter()
        .map(render_column_definition)
        .collect::<Vec<_>>();
    parts.extend(table.foreign_keys.iter().map(|foreign_key| {
        format!(
            "FOREIGN KEY ({}) REFERENCES {}({})",
            foreign_key.source_column, foreign_key.target_table, foreign_key.target_column
        )
    }));
    format!("CREATE TABLE {} ({})", table_name, parts.join(", "))
}

fn column_changed(before: &ColumnModel, after: &ColumnModel) -> bool {
    before != after
}

pub fn diff_schema_models(current: &SchemaModel, target: &SchemaModel) -> SchemaDiff {
    let current_tables = current
        .tables
        .iter()
        .map(|table| (table.table_name.clone(), table))
        .collect::<std::collections::BTreeMap<_, _>>();
    let target_tables = target
        .tables
        .iter()
        .map(|table| (table.table_name.clone(), table))
        .collect::<std::collections::BTreeMap<_, _>>();

    let mut steps = Vec::new();

    for (table_name, table) in &target_tables {
        if !current_tables.contains_key(table_name) {
            steps.push(MigrationStep::CreateTable((*table).clone()));
        }
    }

    for (table_name, table) in &current_tables {
        if !target_tables.contains_key(table_name) {
            steps.push(MigrationStep::DropTable {
                table_name: table_name.clone(),
            });
            continue;
        }

        let target_table = target_tables[table_name];
        let current_columns = table
            .columns
            .iter()
            .map(|column| (column.name.clone(), column))
            .collect::<std::collections::BTreeMap<_, _>>();
        let target_columns = target_table
            .columns
            .iter()
            .map(|column| (column.name.clone(), column))
            .collect::<std::collections::BTreeMap<_, _>>();

        for (column_name, column) in &target_columns {
            if !current_columns.contains_key(column_name) {
                steps.push(MigrationStep::AddColumn {
                    table_name: table_name.clone(),
                    column: (*column).clone(),
                });
            }
        }

        for (column_name, column) in &current_columns {
            if !target_columns.contains_key(column_name) {
                steps.push(MigrationStep::DropColumn {
                    table_name: table_name.clone(),
                    column_name: column_name.clone(),
                });
                continue;
            }

            let target_column = target_columns[column_name];
            if column_changed(column, target_column) {
                steps.push(MigrationStep::AlterColumn {
                    table_name: table_name.clone(),
                    before: (*column).clone(),
                    after: (*target_column).clone(),
                });
            }
        }

        let current_indexes = table
            .indexes
            .iter()
            .map(|index| (index.name.to_string(), index))
            .collect::<std::collections::BTreeMap<_, _>>();
        let target_indexes = target_table
            .indexes
            .iter()
            .map(|index| (index.name.to_string(), index))
            .collect::<std::collections::BTreeMap<_, _>>();

        for (index_name, index) in &target_indexes {
            if !current_indexes.contains_key(index_name) {
                steps.push(MigrationStep::CreateIndex {
                    table_name: table_name.clone(),
                    index: (*index).clone(),
                });
            }
        }

        for index_name in current_indexes.keys() {
            if !target_indexes.contains_key(index_name) {
                steps.push(MigrationStep::DropIndex {
                    table_name: table_name.clone(),
                    index_name: index_name.clone(),
                });
            }
        }

        let current_foreign_keys = table
            .foreign_keys
            .iter()
            .map(|foreign_key| {
                (
                    (
                        foreign_key.source_column.clone(),
                        foreign_key.target_table.clone(),
                        foreign_key.target_column.clone(),
                    ),
                    foreign_key,
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let target_foreign_keys = target_table
            .foreign_keys
            .iter()
            .map(|foreign_key| {
                (
                    (
                        foreign_key.source_column.clone(),
                        foreign_key.target_table.clone(),
                        foreign_key.target_column.clone(),
                    ),
                    foreign_key,
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();

        for (key, foreign_key) in &target_foreign_keys {
            if !current_foreign_keys.contains_key(key) {
                steps.push(MigrationStep::AddForeignKey {
                    table_name: table_name.clone(),
                    foreign_key: (*foreign_key).clone(),
                });
            }
        }

        for (key, foreign_key) in &current_foreign_keys {
            if !target_foreign_keys.contains_key(key) {
                steps.push(MigrationStep::DropForeignKey {
                    table_name: table_name.clone(),
                    foreign_key: (*foreign_key).clone(),
                });
            }
        }
    }

    SchemaDiff { steps }
}

fn foreign_key_constraint_name(table_name: &str, foreign_key: &ForeignKeyModel) -> String {
    format!(
        "fk_{}_{}_{}_{}",
        table_name, foreign_key.source_column, foreign_key.target_table, foreign_key.target_column
    )
}

fn migration_step_table_name(step: &MigrationStep) -> Option<&str> {
    match step {
        MigrationStep::CreateTable(table) => Some(&table.table_name),
        MigrationStep::DropTable { table_name } => Some(table_name),
        MigrationStep::AddColumn { table_name, .. } => Some(table_name),
        MigrationStep::DropColumn { table_name, .. } => Some(table_name),
        MigrationStep::AlterColumn { table_name, .. } => Some(table_name),
        MigrationStep::CreateIndex { table_name, .. } => Some(table_name),
        MigrationStep::DropIndex { table_name, .. } => Some(table_name),
        MigrationStep::AddForeignKey { table_name, .. } => Some(table_name),
        MigrationStep::DropForeignKey { table_name, .. } => Some(table_name),
    }
}

fn sqlite_requires_table_rebuild(step: &MigrationStep) -> bool {
    matches!(
        step,
        MigrationStep::DropColumn { .. }
            | MigrationStep::AlterColumn { .. }
            | MigrationStep::AddForeignKey { .. }
            | MigrationStep::DropForeignKey { .. }
    )
}

fn render_sqlite_table_rebuild_statements(
    current_table: &TableModel,
    target_table: &TableModel,
) -> Vec<String> {
    let temp_table_name = format!("__graphql_orm_{}_new", target_table.table_name);
    let target_table_sql = render_create_table_statement_for_name(target_table, &temp_table_name);
    let common_columns = target_table
        .columns
        .iter()
        .filter(|target_column| {
            current_table
                .columns
                .iter()
                .any(|current_column| current_column.name == target_column.name)
        })
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();

    let mut statements = vec!["PRAGMA foreign_keys = OFF".to_string(), target_table_sql];
    if !common_columns.is_empty() {
        let columns = common_columns.join(", ");
        statements.push(format!(
            "INSERT INTO {} ({}) SELECT {} FROM {}",
            temp_table_name, columns, columns, current_table.table_name
        ));
    }
    statements.push(format!("DROP TABLE {}", current_table.table_name));
    statements.push(format!(
        "ALTER TABLE {} RENAME TO {}",
        temp_table_name, target_table.table_name
    ));
    statements.extend(target_table.indexes.iter().map(|index| {
        let unique = if index.is_unique { "UNIQUE " } else { "" };
        format!(
            "CREATE {}INDEX {} ON {} ({})",
            unique,
            index.name,
            target_table.table_name,
            index.columns.join(", ")
        )
    }));
    statements.push("PRAGMA foreign_keys = ON".to_string());
    statements
}

pub fn render_migration_step(backend: DatabaseBackend, step: &MigrationStep) -> Vec<String> {
    match step {
        MigrationStep::CreateTable(table) => {
            let mut statements = vec![render_create_table_statement(table)];
            statements.extend(table.indexes.iter().map(|index| {
                let unique = if index.is_unique { "UNIQUE " } else { "" };
                format!(
                    "CREATE {}INDEX {} ON {} ({})",
                    unique,
                    index.name,
                    table.table_name,
                    index.columns.join(", ")
                )
            }));
            statements
        }
        MigrationStep::DropTable { table_name } => {
            vec![format!("DROP TABLE {}", table_name)]
        }
        MigrationStep::AddColumn { table_name, column } => vec![format!(
            "ALTER TABLE {} ADD COLUMN {}",
            table_name,
            render_column_definition(column)
        )],
        MigrationStep::DropColumn {
            table_name,
            column_name,
        } => match backend {
            DatabaseBackend::Postgres => vec![format!(
                "ALTER TABLE {} DROP COLUMN {}",
                table_name, column_name
            )],
            DatabaseBackend::Sqlite => vec![format!(
                "-- sqlite requires table rebuild to drop column {} from {}",
                column_name, table_name
            )],
            DatabaseBackend::Mysql | DatabaseBackend::Mssql => vec![format!(
                "ALTER TABLE {} DROP COLUMN {}",
                table_name, column_name
            )],
        },
        MigrationStep::AlterColumn {
            table_name,
            before: _,
            after,
        } => match backend {
            DatabaseBackend::Postgres => vec![
                format!(
                    "ALTER TABLE {} ALTER COLUMN {} TYPE {}",
                    table_name, after.name, after.sql_type
                ),
                if after.nullable {
                    format!(
                        "ALTER TABLE {} ALTER COLUMN {} DROP NOT NULL",
                        table_name, after.name
                    )
                } else {
                    format!(
                        "ALTER TABLE {} ALTER COLUMN {} SET NOT NULL",
                        table_name, after.name
                    )
                },
            ],
            DatabaseBackend::Sqlite => vec![format!(
                "-- sqlite requires table rebuild to alter column {} on {}",
                after.name, table_name
            )],
            DatabaseBackend::Mysql | DatabaseBackend::Mssql => vec![format!(
                "ALTER TABLE {} ALTER COLUMN {} {}",
                table_name, after.name, after.sql_type
            )],
        },
        MigrationStep::CreateIndex { table_name, index } => {
            let unique = if index.is_unique { "UNIQUE " } else { "" };
            vec![format!(
                "CREATE {}INDEX {} ON {} ({})",
                unique,
                index.name,
                table_name,
                index.columns.join(", ")
            )]
        }
        MigrationStep::DropIndex {
            table_name,
            index_name,
        } => match backend {
            DatabaseBackend::Sqlite => {
                vec![format!("DROP INDEX {}", index_name)]
            }
            DatabaseBackend::Postgres => {
                vec![format!("DROP INDEX {}", index_name)]
            }
            DatabaseBackend::Mysql => {
                vec![format!("DROP INDEX {} ON {}", index_name, table_name)]
            }
            DatabaseBackend::Mssql => {
                vec![format!("DROP INDEX {} ON {}", index_name, table_name)]
            }
        },
        MigrationStep::AddForeignKey {
            table_name,
            foreign_key,
        } => {
            let constraint_name = foreign_key_constraint_name(table_name, foreign_key);
            match backend {
                DatabaseBackend::Postgres => vec![format!(
                    "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {}({})",
                    table_name,
                    constraint_name,
                    foreign_key.source_column,
                    foreign_key.target_table,
                    foreign_key.target_column
                )],
                DatabaseBackend::Sqlite => vec![format!(
                    "-- sqlite requires table rebuild to add foreign key {} on {}",
                    constraint_name, table_name
                )],
                DatabaseBackend::Mysql | DatabaseBackend::Mssql => vec![format!(
                    "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {}({})",
                    table_name,
                    constraint_name,
                    foreign_key.source_column,
                    foreign_key.target_table,
                    foreign_key.target_column
                )],
            }
        }
        MigrationStep::DropForeignKey {
            table_name,
            foreign_key,
        } => {
            let constraint_name = foreign_key_constraint_name(table_name, foreign_key);
            match backend {
                DatabaseBackend::Postgres => vec![format!(
                    "ALTER TABLE {} DROP CONSTRAINT {}",
                    table_name, constraint_name
                )],
                DatabaseBackend::Sqlite => vec![format!(
                    "-- sqlite requires table rebuild to drop foreign key {} on {}",
                    constraint_name, table_name
                )],
                DatabaseBackend::Mysql => vec![format!(
                    "ALTER TABLE {} DROP FOREIGN KEY {}",
                    table_name, constraint_name
                )],
                DatabaseBackend::Mssql => vec![format!(
                    "ALTER TABLE {} DROP CONSTRAINT {}",
                    table_name, constraint_name
                )],
            }
        }
    }
}

pub fn build_migration_plan(
    backend: DatabaseBackend,
    current: &SchemaModel,
    target: &SchemaModel,
) -> MigrationPlan {
    let diff = diff_schema_models(current, target);
    let statements = match backend {
        DatabaseBackend::Sqlite => {
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
            let rebuild_tables = diff
                .steps
                .iter()
                .filter(|step| sqlite_requires_table_rebuild(step))
                .filter_map(migration_step_table_name)
                .collect::<std::collections::BTreeSet<_>>();
            let mut statements = Vec::new();
            let mut rebuilt_tables = std::collections::BTreeSet::new();

            for step in &diff.steps {
                let table_name = migration_step_table_name(step);
                if let Some(table_name) = table_name {
                    if rebuild_tables.contains(table_name) {
                        if rebuilt_tables.insert(table_name.to_string()) {
                            if let (Some(current_table), Some(target_table)) = (
                                current_tables.get(table_name),
                                target_tables.get(table_name),
                            ) {
                                statements.extend(render_sqlite_table_rebuild_statements(
                                    current_table,
                                    target_table,
                                ));
                            }
                        }
                        continue;
                    }
                }
                statements.extend(render_migration_step(backend, step));
            }

            statements
        }
        _ => diff
            .steps
            .iter()
            .flat_map(|step| render_migration_step(backend, step))
            .collect::<Vec<_>>(),
    };

    MigrationPlan {
        backend,
        steps: diff.steps,
        statements,
    }
}

pub fn migration_filename(version: &str, description: &str) -> String {
    let normalized = description
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    format!("{}_{}.sql", version, normalized)
}

pub fn render_migration_file(plan: &MigrationPlan, version: &str, description: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("-- version: {}\n", version));
    out.push_str(&format!("-- description: {}\n", description));
    out.push_str(&format!("-- backend: {:?}\n\n", plan.backend));
    for statement in &plan.statements {
        out.push_str(statement);
        if !statement.trim_end().ends_with(';') {
            out.push(';');
        }
        out.push('\n');
    }
    out
}

pub fn write_migration_file(
    directory: impl AsRef<std::path::Path>,
    plan: &MigrationPlan,
    version: &str,
    description: &str,
) -> Result<std::path::PathBuf, std::io::Error> {
    let path = directory
        .as_ref()
        .join(migration_filename(version, description));
    std::fs::create_dir_all(directory.as_ref())?;
    std::fs::write(&path, render_migration_file(plan, version, description))?;
    Ok(path)
}

#[cfg(feature = "sqlite")]
pub async fn introspect_schema(provider: &impl PoolProvider) -> Result<SchemaModel, sqlx::Error> {
    let pool = provider.pool();
    let table_rows = sqlx::query(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(pool)
    .await?;

    let mut tables = Vec::new();
    for row in table_rows {
        let table_name: String = row.try_get("name")?;

        let pragma_table_info = format!("PRAGMA table_info({})", table_name);
        let column_rows = sqlx::query(&pragma_table_info).fetch_all(pool).await?;
        let columns = column_rows
            .into_iter()
            .map(|row| {
                let name: String = row.try_get("name")?;
                let sql_type: String = row.try_get("type")?;
                let nullable = row.try_get::<i64, _>("notnull")? == 0;
                let default = row.try_get::<Option<String>, _>("dflt_value")?;
                let is_primary_key = row.try_get::<i64, _>("pk")? > 0;
                Ok(ColumnModel {
                    name,
                    sql_type,
                    nullable,
                    is_primary_key,
                    is_unique: false,
                    default,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        let primary_key = columns
            .iter()
            .find(|column| column.is_primary_key)
            .map(|column| column.name.clone())
            .unwrap_or_else(|| "id".to_string());

        let pragma_index_list = format!("PRAGMA index_list({})", table_name);
        let index_rows = sqlx::query(&pragma_index_list).fetch_all(pool).await?;
        let mut indexes = Vec::new();
        for row in index_rows {
            let index_name: String = row.try_get("name")?;
            let unique = row.try_get::<i64, _>("unique")? != 0;
            let pragma_index_info = format!("PRAGMA index_info({})", index_name);
            let index_info_rows = sqlx::query(&pragma_index_info).fetch_all(pool).await?;
            let column_names = index_info_rows
                .into_iter()
                .map(|index_row| index_row.try_get::<String, _>("name"))
                .collect::<Result<Vec<_>, _>>()?;
            let leaked_name: &'static str = Box::leak(index_name.into_boxed_str());
            let leaked_columns: &'static [&'static str] = Box::leak(
                column_names
                    .into_iter()
                    .map(|column| Box::leak(column.into_boxed_str()) as &'static str)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            indexes.push(IndexDef {
                name: leaked_name,
                columns: leaked_columns,
                is_unique: unique,
            });
        }

        let pragma_fk_list = format!("PRAGMA foreign_key_list({})", table_name);
        let foreign_key_rows = sqlx::query(&pragma_fk_list).fetch_all(pool).await?;
        let foreign_keys = foreign_key_rows
            .into_iter()
            .map(|row| {
                Ok(ForeignKeyModel {
                    source_column: row.try_get("from")?,
                    target_table: row.try_get("table")?,
                    target_column: row.try_get("to")?,
                    is_multiple: false,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;

        tables.push(TableModel {
            entity_name: table_name.clone(),
            table_name,
            primary_key: primary_key.clone(),
            default_sort: primary_key.clone(),
            columns,
            indexes,
            composite_unique_indexes: Vec::new(),
            foreign_keys,
        });
    }

    Ok(SchemaModel { tables })
}

#[cfg(feature = "postgres")]
pub async fn introspect_schema(provider: &impl PoolProvider) -> Result<SchemaModel, sqlx::Error> {
    let pool = provider.pool();
    let table_rows = sqlx::query(
        "SELECT table_name
         FROM information_schema.tables
         WHERE table_schema = 'public' AND table_type = 'BASE TABLE'
         ORDER BY table_name",
    )
    .fetch_all(pool)
    .await?;

    let mut tables = Vec::new();
    for row in table_rows {
        let table_name: String = row.try_get("table_name")?;
        let column_rows = sqlx::query(
            "SELECT column_name, data_type, is_nullable, column_default
             FROM information_schema.columns
             WHERE table_schema = 'public' AND table_name = $1
             ORDER BY ordinal_position",
        )
        .bind(&table_name)
        .fetch_all(pool)
        .await?;

        let primary_key_rows = sqlx::query(
            "SELECT kcu.column_name
             FROM information_schema.table_constraints tc
             JOIN information_schema.key_column_usage kcu
               ON tc.constraint_name = kcu.constraint_name
              AND tc.table_schema = kcu.table_schema
             WHERE tc.table_schema = 'public'
               AND tc.table_name = $1
               AND tc.constraint_type = 'PRIMARY KEY'",
        )
        .bind(&table_name)
        .fetch_all(pool)
        .await?;
        let primary_key_columns = primary_key_rows
            .into_iter()
            .map(|row| row.try_get::<String, _>("column_name"))
            .collect::<Result<Vec<_>, _>>()?;

        let unique_rows = sqlx::query(
            "SELECT kcu.column_name
             FROM information_schema.table_constraints tc
             JOIN information_schema.key_column_usage kcu
               ON tc.constraint_name = kcu.constraint_name
              AND tc.table_schema = kcu.table_schema
             WHERE tc.table_schema = 'public'
               AND tc.table_name = $1
               AND tc.constraint_type = 'UNIQUE'",
        )
        .bind(&table_name)
        .fetch_all(pool)
        .await?;
        let unique_columns = unique_rows
            .into_iter()
            .map(|row| row.try_get::<String, _>("column_name"))
            .collect::<Result<std::collections::HashSet<_>, _>>()?;

        let columns = column_rows
            .into_iter()
            .map(|row| {
                let name: String = row.try_get("column_name")?;
                Ok(ColumnModel {
                    is_primary_key: primary_key_columns.iter().any(|column| column == &name),
                    is_unique: unique_columns.contains(&name),
                    name,
                    sql_type: row.try_get::<String, _>("data_type")?,
                    nullable: row.try_get::<String, _>("is_nullable")? == "YES",
                    default: row.try_get::<Option<String>, _>("column_default")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;

        let primary_key = primary_key_columns
            .first()
            .cloned()
            .unwrap_or_else(|| "id".to_string());

        let index_rows = sqlx::query(
            "SELECT indexname, indexdef
             FROM pg_indexes
             WHERE schemaname = 'public' AND tablename = $1",
        )
        .bind(&table_name)
        .fetch_all(pool)
        .await?;
        let mut indexes = Vec::new();
        for row in index_rows {
            let index_name: String = row.try_get("indexname")?;
            let indexdef: String = row.try_get("indexdef")?;
            let unique = indexdef.contains("CREATE UNIQUE INDEX");
            let columns_segment = indexdef
                .split('(')
                .nth(1)
                .and_then(|segment| segment.split(')').next())
                .unwrap_or("");
            let column_names = columns_segment
                .split(',')
                .map(str::trim)
                .filter(|column| !column.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            let leaked_name: &'static str = Box::leak(index_name.into_boxed_str());
            let leaked_columns: &'static [&'static str] = Box::leak(
                column_names
                    .into_iter()
                    .map(|column| Box::leak(column.into_boxed_str()) as &'static str)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            indexes.push(IndexDef {
                name: leaked_name,
                columns: leaked_columns,
                is_unique: unique,
            });
        }

        let foreign_key_rows = sqlx::query(
            "SELECT
                kcu.column_name AS source_column,
                ccu.table_name AS target_table,
                ccu.column_name AS target_column
             FROM information_schema.table_constraints tc
             JOIN information_schema.key_column_usage kcu
               ON tc.constraint_name = kcu.constraint_name
              AND tc.table_schema = kcu.table_schema
             JOIN information_schema.constraint_column_usage ccu
               ON ccu.constraint_name = tc.constraint_name
              AND ccu.table_schema = tc.table_schema
             WHERE tc.table_schema = 'public'
               AND tc.table_name = $1
               AND tc.constraint_type = 'FOREIGN KEY'",
        )
        .bind(&table_name)
        .fetch_all(pool)
        .await?;
        let foreign_keys = foreign_key_rows
            .into_iter()
            .map(|row| {
                Ok(ForeignKeyModel {
                    source_column: row.try_get("source_column")?,
                    target_table: row.try_get("target_table")?,
                    target_column: row.try_get("target_column")?,
                    is_multiple: false,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;

        tables.push(TableModel {
            entity_name: table_name.clone(),
            table_name,
            primary_key: primary_key.clone(),
            default_sort: primary_key,
            columns,
            indexes,
            composite_unique_indexes: Vec::new(),
            foreign_keys,
        });
    }

    Ok(SchemaModel { tables })
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DatabaseBackend {
    Sqlite,
    Postgres,
    Mysql,
    Mssql,
}

pub trait SqlDialect {
    fn backend(&self) -> DatabaseBackend;
    fn placeholder(&self, index: usize) -> String;
    fn normalize_sql(&self, sql: &str, start_index: usize) -> String;
    fn current_epoch_expr(&self) -> &'static str;
    fn current_date_expr(&self) -> &'static str;
    fn ci_like(&self, column: &str, placeholder: &str) -> String;
    fn days_ago_expr(&self, days: i64) -> String;
    fn days_ahead_expr(&self, days: i64) -> String;
}

impl SqlDialect for DatabaseBackend {
    fn backend(&self) -> DatabaseBackend {
        *self
    }

    fn placeholder(&self, index: usize) -> String {
        match self {
            DatabaseBackend::Postgres => format!("${index}"),
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql | DatabaseBackend::Mssql => {
                "?".to_string()
            }
        }
    }

    fn normalize_sql(&self, sql: &str, start_index: usize) -> String {
        if *self != DatabaseBackend::Postgres {
            return sql.to_string();
        }

        let chars: Vec<char> = sql.chars().collect();
        let mut out = String::with_capacity(sql.len() + 16);
        let mut i = 0usize;
        let mut next = start_index;
        while i < chars.len() {
            if chars[i] == '?' || chars[i] == '$' {
                out.push_str(&self.placeholder(next));
                next += 1;
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            } else {
                out.push(chars[i]);
                i += 1;
            }
        }
        out
    }

    fn current_epoch_expr(&self) -> &'static str {
        match self {
            DatabaseBackend::Postgres => "(EXTRACT(EPOCH FROM NOW())::bigint)",
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql | DatabaseBackend::Mssql => {
                "(unixepoch())"
            }
        }
    }

    fn current_date_expr(&self) -> &'static str {
        match self {
            DatabaseBackend::Postgres => "CURRENT_DATE",
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql | DatabaseBackend::Mssql => {
                "date('now')"
            }
        }
    }

    fn ci_like(&self, column: &str, placeholder: &str) -> String {
        match self {
            DatabaseBackend::Postgres => format!("{column} ILIKE {placeholder}"),
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql | DatabaseBackend::Mssql => {
                format!("LOWER({column}) LIKE LOWER({placeholder})")
            }
        }
    }

    fn days_ago_expr(&self, days: i64) -> String {
        match self {
            DatabaseBackend::Postgres => {
                format!("CURRENT_DATE - INTERVAL '{days} days'")
            }
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql | DatabaseBackend::Mssql => {
                format!("date('now', '-{days} days')")
            }
        }
    }

    fn days_ahead_expr(&self, days: i64) -> String {
        match self {
            DatabaseBackend::Postgres => {
                format!("CURRENT_DATE + INTERVAL '{days} days'")
            }
            DatabaseBackend::Sqlite | DatabaseBackend::Mysql | DatabaseBackend::Mssql => {
                format!("date('now', '+{days} days')")
            }
        }
    }
}

pub fn current_backend() -> DatabaseBackend {
    #[cfg(feature = "sqlite")]
    {
        DatabaseBackend::Sqlite
    }
    #[cfg(feature = "postgres")]
    {
        DatabaseBackend::Postgres
    }
}

pub trait DatabaseEntity {
    const TABLE_NAME: &'static str;
    const PLURAL_NAME: &'static str;
    const PRIMARY_KEY: &'static str;
    const DEFAULT_SORT: &'static str;

    fn column_names() -> &'static [&'static str];
}

pub trait DatabaseSchema {
    fn columns() -> &'static [ColumnDef];
    fn indexes() -> &'static [IndexDef];
    fn composite_unique_indexes() -> &'static [&'static [&'static str]];
}

pub trait EntityRelations {
    fn relation_metadata() -> &'static [RelationMetadata] {
        &[]
    }
}

pub trait Entity: DatabaseEntity + DatabaseSchema + EntityRelations {
    fn entity_name() -> &'static str;
    fn metadata() -> &'static EntityMetadata;
}

pub trait FromSqlRow: Sized {
    fn from_row(row: &DbRow) -> Result<Self, sqlx::Error>;
}

pub trait DatabaseFilter {
    fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>);
    fn is_empty(&self) -> bool;

    fn to_filter_expression(&self) -> Option<FilterExpression> {
        let (conditions, values) = self.to_sql_conditions();
        filter_expression_from_raw_parts(&conditions, &values)
    }
}

pub trait DatabaseOrderBy {
    fn to_sql_order(&self) -> Option<String>;

    fn to_sort_expression(&self) -> Option<SortExpression> {
        self.to_sql_order().map(|clause| SortExpression { clause })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FilterExpression {
    Raw {
        clause: String,
        values: Vec<SqlValue>,
    },
    And(Vec<FilterExpression>),
    Or(Vec<FilterExpression>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SortExpression {
    pub clause: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderedQuery {
    pub sql: String,
    pub values: Vec<SqlValue>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectQuery {
    pub table: &'static str,
    pub columns: Vec<String>,
    pub filter: Option<FilterExpression>,
    pub sorts: Vec<SortExpression>,
    pub pagination: Option<PaginationRequest>,
    pub count_only: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeleteQuery {
    pub table: &'static str,
    pub filter: Option<FilterExpression>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaginationRequest {
    pub limit: Option<i64>,
    pub offset: i64,
}

impl From<&PageInput> for PaginationRequest {
    fn from(value: &PageInput) -> Self {
        Self {
            limit: value.limit(),
            offset: value.offset(),
        }
    }
}

#[derive(async_graphql::Enum, Copy, Clone, Debug, Eq, PartialEq)]
pub enum OrderDirection {
    Asc,
    Desc,
}

impl OrderDirection {
    pub fn to_sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

impl DatabaseFilter for () {
    fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>) {
        (Vec::new(), Vec::new())
    }

    fn is_empty(&self) -> bool {
        true
    }
}

impl DatabaseOrderBy for () {
    fn to_sql_order(&self) -> Option<String> {
        None
    }
}

#[derive(
    async_graphql::Enum, serde::Serialize, serde::Deserialize, Copy, Clone, Debug, Eq, PartialEq,
)]
pub enum ChangeAction {
    Created,
    Updated,
    Deleted,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
pub struct SubscriptionFilterInput {
    #[graphql(name = "Dummy")]
    pub dummy: Option<bool>,
}

#[derive(async_graphql::InputObject, Clone, Debug, Default)]
pub struct PageInput {
    #[graphql(name = "Limit")]
    pub limit: Option<i64>,
    #[graphql(name = "Offset")]
    pub offset: Option<i64>,
}

impl PageInput {
    pub fn offset(&self) -> i64 {
        self.offset.unwrap_or(0)
    }

    pub fn limit(&self) -> Option<i64> {
        self.limit
    }
}

pub trait PoolProvider {
    fn pool(&self) -> &DbPool;
}

pub trait DatabaseExecutor: PoolProvider {
    fn backend(&self) -> DatabaseBackend {
        current_backend()
    }
}

impl PoolProvider for DbPool {
    fn pool(&self) -> &DbPool {
        self
    }
}

impl DatabaseExecutor for DbPool {}

impl PoolProvider for crate::db::Database {
    fn pool(&self) -> &DbPool {
        self.pool()
    }
}

impl DatabaseExecutor for crate::db::Database {}

#[allow(async_fn_in_trait)]
pub trait RelationLoader {
    async fn load_relations(
        &mut self,
        pool: &DbPool,
        selection: &[async_graphql::context::SelectionField<'_>],
    ) -> Result<(), sqlx::Error>;

    async fn bulk_load_relations(
        entities: &mut [Self],
        pool: &DbPool,
        selection: &[async_graphql::context::SelectionField<'_>],
    ) -> Result<(), sqlx::Error>
    where
        Self: Sized;
}

pub struct FuzzyMatcher {
    query: String,
    threshold: f64,
}

#[derive(Clone, Debug)]
pub struct MatchResult<T> {
    pub entity: T,
    pub score: f64,
}

impl FuzzyMatcher {
    pub fn new(query: &str) -> Self {
        Self {
            query: query.to_lowercase(),
            threshold: 0.0,
        }
    }

    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold;
        self
    }

    pub fn filter_and_score<T, F>(&self, items: Vec<T>, extract: F) -> Vec<MatchResult<T>>
    where
        F: Fn(&T) -> Option<&str>,
    {
        let mut out = Vec::new();
        for item in items {
            let score = extract(&item)
                .map(|candidate| {
                    if candidate.to_lowercase().contains(&self.query) {
                        1.0
                    } else {
                        0.0
                    }
                })
                .unwrap_or(0.0);

            if score >= self.threshold {
                out.push(MatchResult {
                    entity: item,
                    score,
                });
            }
        }
        out
    }
}

pub fn generate_candidate_pattern(value: &str) -> String {
    format!("%{}%", value)
}

fn filter_expression_from_raw_parts(
    conditions: &[String],
    values: &[SqlValue],
) -> Option<FilterExpression> {
    if conditions.is_empty() {
        return None;
    }

    let mut value_iter = values.iter().cloned();
    let filters = conditions
        .iter()
        .map(|clause| {
            let placeholder_count = count_placeholders(clause);
            let clause_values = value_iter
                .by_ref()
                .take(placeholder_count)
                .collect::<Vec<_>>();
            FilterExpression::Raw {
                clause: clause.clone(),
                values: clause_values,
            }
        })
        .collect::<Vec<_>>();

    if filters.len() == 1 {
        filters.into_iter().next()
    } else {
        Some(FilterExpression::And(filters))
    }
}

fn count_placeholders(clause: &str) -> usize {
    let chars: Vec<char> = clause.chars().collect();
    let mut count = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        match chars[i] {
            '?' => {
                count += 1;
                i += 1;
            }
            '$' => {
                let mut j = i + 1;
                let mut saw_digit = false;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    saw_digit = true;
                    j += 1;
                }
                if saw_digit {
                    count += 1;
                    i = j;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    count
}

fn render_filter_expression(
    dialect: DatabaseBackend,
    filter: &FilterExpression,
    next_index: &mut usize,
    bind_values: &mut Vec<SqlValue>,
) -> String {
    match filter {
        FilterExpression::Raw { clause, values } => {
            let rendered = dialect.normalize_sql(clause, *next_index);
            *next_index += values.len();
            bind_values.extend(values.iter().cloned());
            rendered
        }
        FilterExpression::And(filters) => filters
            .iter()
            .map(|filter| render_filter_expression(dialect, filter, next_index, bind_values))
            .filter(|sql| !sql.is_empty())
            .map(|sql| format!("({sql})"))
            .collect::<Vec<_>>()
            .join(" AND "),
        FilterExpression::Or(filters) => filters
            .iter()
            .map(|filter| render_filter_expression(dialect, filter, next_index, bind_values))
            .filter(|sql| !sql.is_empty())
            .map(|sql| format!("({sql})"))
            .collect::<Vec<_>>()
            .join(" OR "),
    }
}

pub fn render_select_query(dialect: DatabaseBackend, query: &SelectQuery) -> RenderedQuery {
    let projection = if query.count_only {
        "COUNT(*) AS count".to_string()
    } else {
        query.columns.join(", ")
    };
    let mut sql = format!("SELECT {} FROM {}", projection, query.table);
    let mut values = Vec::new();
    let mut next_index = 1usize;

    if let Some(filter) = &query.filter {
        let where_sql = render_filter_expression(dialect, filter, &mut next_index, &mut values);
        if !where_sql.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&where_sql);
        }
    }

    if !query.count_only && !query.sorts.is_empty() {
        sql.push_str(" ORDER BY ");
        sql.push_str(
            &query
                .sorts
                .iter()
                .map(|sort| sort.clause.clone())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }

    if !query.count_only {
        if let Some(page) = &query.pagination {
            if let Some(limit) = page.limit {
                sql.push_str(&format!(" LIMIT {}", limit));
            }
            if page.offset > 0 {
                sql.push_str(&format!(" OFFSET {}", page.offset));
            }
        }
    }

    RenderedQuery { sql, values }
}

pub fn render_delete_query(dialect: DatabaseBackend, query: &DeleteQuery) -> RenderedQuery {
    let mut sql = format!("DELETE FROM {}", query.table);
    let mut values = Vec::new();
    let mut next_index = 1usize;

    if let Some(filter) = &query.filter {
        let where_sql = render_filter_expression(dialect, filter, &mut next_index, &mut values);
        if !where_sql.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&where_sql);
        }
    }

    RenderedQuery { sql, values }
}

pub fn backend_placeholder(index: usize) -> String {
    current_backend().placeholder(index)
}

pub fn normalize_sql(sql: &str, start_index: usize) -> String {
    current_backend().normalize_sql(sql, start_index)
}

#[derive(Clone, Debug, PartialEq)]
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
    record_query();
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
    record_query();
    let sql = normalize_sql(sql, 1);
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
    record_query();
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
        let sql = normalize_sql(sql, 1);
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

pub struct EntityQuery<T> {
    pub where_clauses: Vec<String>,
    pub values: Vec<SqlValue>,
    pub order_clauses: Vec<String>,
    pub page: Option<PageInput>,
    _marker: PhantomData<T>,
}

impl<T> Clone for EntityQuery<T> {
    fn clone(&self) -> Self {
        Self {
            where_clauses: self.where_clauses.clone(),
            values: self.values.clone(),
            order_clauses: self.order_clauses.clone(),
            page: self.page.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T> EntityQuery<T>
where
    T: DatabaseEntity + FromSqlRow + Clone + Send + Sync,
{
    pub fn new() -> Self {
        Self {
            where_clauses: Vec::new(),
            values: Vec::new(),
            order_clauses: Vec::new(),
            page: None,
            _marker: PhantomData,
        }
    }

    pub fn where_clause(mut self, clause: &str, value: SqlValue) -> Self {
        self.where_clauses.push(clause.to_string());
        self.values.push(value);
        self
    }

    pub fn filter<F>(mut self, filter: &F) -> Self
    where
        F: DatabaseFilter,
    {
        let (conds, values) = filter.to_sql_conditions();
        self.where_clauses.extend(conds);
        self.values.extend(values);
        self
    }

    pub fn order_by<O>(mut self, order: &O) -> Self
    where
        O: DatabaseOrderBy,
    {
        if let Some(sort) = order.to_sort_expression() {
            self.order_clauses.push(sort.clause);
        }
        self
    }

    pub fn default_order(mut self) -> Self {
        self.order_clauses.push(T::DEFAULT_SORT.to_string());
        self
    }

    pub fn paginate(mut self, page: &PageInput) -> Self {
        self.page = Some(page.clone());
        self
    }

    fn build_select_query(&self) -> SelectQuery {
        SelectQuery {
            table: T::TABLE_NAME,
            columns: T::column_names()
                .iter()
                .map(|column| (*column).to_string())
                .collect(),
            filter: filter_expression_from_raw_parts(&self.where_clauses, &self.values),
            sorts: self
                .order_clauses
                .iter()
                .cloned()
                .map(|clause| SortExpression { clause })
                .collect(),
            pagination: self.page.as_ref().map(PaginationRequest::from),
            count_only: false,
        }
    }

    pub async fn fetch_all<P>(&self, provider: &P) -> Result<Vec<T>, sqlx::Error>
    where
        P: PoolProvider + ?Sized,
    {
        let rendered = render_select_query(current_backend(), &self.build_select_query());
        let rows = fetch_rows(provider.pool(), &rendered.sql, &rendered.values).await?;
        rows.iter().map(T::from_row).collect()
    }

    pub async fn fetch_one<P>(&self, provider: &P) -> Result<Option<T>, sqlx::Error>
    where
        P: PoolProvider + ?Sized,
    {
        Ok(self.fetch_all(provider).await?.into_iter().next())
    }

    pub async fn count<P>(&self, provider: &P) -> Result<i64, sqlx::Error>
    where
        P: PoolProvider + ?Sized,
    {
        let mut query = self.build_select_query();
        query.count_only = true;
        query.pagination = None;
        query.sorts.clear();
        let rendered = render_select_query(current_backend(), &query);
        let rows = fetch_rows(provider.pool(), &rendered.sql, &rendered.values).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        row.try_get::<i64, _>("count")
    }

    pub fn build_delete_sql(&self) -> (String, Vec<SqlValue>) {
        let rendered = render_delete_query(
            current_backend(),
            &DeleteQuery {
                table: T::TABLE_NAME,
                filter: filter_expression_from_raw_parts(&self.where_clauses, &self.values),
            },
        );
        (rendered.sql, rendered.values)
    }

    pub async fn fetch_connection<P>(&self, provider: &P) -> Result<Connection<T>, sqlx::Error>
    where
        P: PoolProvider + ?Sized,
    {
        let total = self.count(provider).await?;
        let offset = self.page.as_ref().map(|p| p.offset()).unwrap_or(0) as usize;
        let nodes = self.fetch_all(provider).await?;
        let edges = nodes
            .into_iter()
            .enumerate()
            .map(|(index, node)| Edge {
                node,
                cursor: encode_cursor((offset + index) as i64),
            })
            .collect::<Vec<_>>();

        Ok(Connection {
            page_info: PageInfo {
                has_next_page: false,
                has_previous_page: offset > 0,
                start_cursor: edges.first().map(|edge| edge.cursor.clone()),
                end_cursor: edges.last().map(|edge| edge.cursor.clone()),
                total_count: Some(total),
            },
            edges,
        })
    }
}

pub struct FindQuery<'a, T, W, O>
where
    T: DatabaseEntity + FromSqlRow + Clone + Send + Sync,
{
    pool: &'a DbPool,
    query: EntityQuery<T>,
    _marker: PhantomData<(W, O)>,
}

impl<'a, T, W, O> FindQuery<'a, T, W, O>
where
    T: DatabaseEntity + FromSqlRow + Clone + Send + Sync,
{
    pub fn new(pool: &'a DbPool) -> Self {
        Self {
            pool,
            query: EntityQuery::new(),
            _marker: PhantomData,
        }
    }

    pub fn filter(mut self, filter: W) -> Self
    where
        W: DatabaseFilter,
    {
        self.query = self.query.filter(&filter);
        self
    }

    pub fn order_by(mut self, order: O) -> Self
    where
        O: DatabaseOrderBy,
    {
        self.query = self.query.order_by(&order);
        self
    }

    pub fn limit(mut self, limit: i64) -> Self {
        self.query.page = Some(PageInput {
            limit: Some(limit),
            offset: Some(0),
        });
        self
    }

    pub async fn fetch_all(self) -> Result<Vec<T>, sqlx::Error> {
        self.query.fetch_all(self.pool).await
    }
}

pub struct CountQuery<'a, W> {
    pool: &'a DbPool,
    table: &'static str,
    filters: Vec<String>,
    values: Vec<SqlValue>,
    _marker: PhantomData<W>,
}

impl<'a, W> CountQuery<'a, W>
where
    W: DatabaseFilter,
{
    pub fn new(pool: &'a DbPool, table: &'static str) -> Self {
        Self {
            pool,
            table,
            filters: Vec::new(),
            values: Vec::new(),
            _marker: PhantomData,
        }
    }

    pub fn filter(mut self, filter: &W) -> Self {
        let (conds, values) = filter.to_sql_conditions();
        self.filters.extend(conds);
        self.values.extend(values);
        self
    }

    pub async fn count(self) -> Result<i64, sqlx::Error> {
        let mut sql = format!("SELECT COUNT(*) AS count FROM {}", self.table);
        if !self.filters.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.filters.join(" AND "));
        }
        let rows = fetch_rows(self.pool, &sql, &self.values).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        row.try_get::<i64, _>("count")
    }
}
