use super::dialect::DatabaseBackend;
use super::query::{ChangeAction, DatabaseEntity, DatabaseSchema, EntityRelations};
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
        ctx: Option<&'a async_graphql::Context<'_>>,
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

pub(crate) fn record_executed_query() {
    record_query();
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
