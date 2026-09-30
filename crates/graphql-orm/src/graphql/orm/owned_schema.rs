//! Owned storage and borrowed views of the canonical physical schema contract.
//!
//! Legacy enums and struct literals retain their original public definitions.
//! Both storage forms use the same validation, hashing, diffing and SQL renderer.
use super::core::*;
use super::dialect::DatabaseBackend;
use std::fmt;

/// Owned closed-set partial-index predicate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexPredicateModel {
    /// Physical column.
    pub column: String,
    /// Literal values, quoted by the database dialect.
    pub values: Vec<String>,
}

/// Owned storage form of [`IndexDef`]; it is the same physical contract.
#[derive(Clone, Debug)]
pub struct IndexModel {
    /// Physical index name.
    pub name: String,
    /// Ordered physical columns.
    pub columns: Vec<String>,
    /// Empty means ascending, as for [`IndexDef`].
    pub column_directions: Vec<IndexDirection>,
    /// Whether the index enforces uniqueness.
    pub is_unique: bool,
    /// Backend index method.
    pub method: IndexMethod,
    /// Whether this is a spatial index.
    pub is_spatial: bool,
    /// Optional closed-set predicate.
    pub predicate: Option<IndexPredicateModel>,
}
impl IndexModel {
    /// Construct an ordinary all-ascending index.
    pub fn new(name: impl Into<String>, columns: Vec<String>) -> Self {
        Self {
            name: name.into(),
            columns,
            column_directions: vec![],
            is_unique: false,
            method: IndexMethod::Default,
            is_spatial: false,
            predicate: None,
        }
    }
    /// Effective direction, preserving the legacy empty-slice convention.
    pub fn direction_at(&self, index: usize) -> IndexDirection {
        self.column_directions
            .get(index)
            .copied()
            .unwrap_or(IndexDirection::Asc)
    }
    pub(super) fn borrowed(&self) -> IndexRef<'_> {
        IndexRef {
            name: &self.name,
            columns: self.columns.iter().map(String::as_str).collect(),
            column_directions: &self.column_directions,
            is_unique: self.is_unique,
            method: self.method,
            is_spatial: self.is_spatial,
            predicate: self.predicate.as_ref().map(|p| IndexPredicateRef {
                column: &p.column,
                values: p.values.iter().map(String::as_str).collect(),
            }),
        }
    }
    // Only compatibility APIs with legacy &'static storage call this adapter.
    // The owned runtime conversion/introspection/planning/apply paths never do.
    pub(super) fn into_legacy(self) -> IndexDef {
        IndexDef {
            name: Box::leak(self.name.into_boxed_str()),
            columns: Box::leak(
                self.columns
                    .into_iter()
                    .map(|c| &*Box::leak(c.into_boxed_str()))
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
            column_directions: Box::leak(self.column_directions.into_boxed_slice()),
            is_unique: self.is_unique,
            method: self.method,
            is_spatial: self.is_spatial,
            predicate: self.predicate.map(|p| IndexPredicateDef {
                column: Box::leak(p.column.into_boxed_str()),
                values: Box::leak(
                    p.values
                        .into_iter()
                        .map(|v| &*Box::leak(v.into_boxed_str()))
                        .collect::<Vec<_>>()
                        .into_boxed_slice(),
                ),
            }),
        }
    }
}
impl From<&IndexDef> for IndexModel {
    fn from(value: &IndexDef) -> Self {
        IndexRef::from(value).to_owned()
    }
}
impl PartialEq for IndexModel {
    fn eq(&self, other: &Self) -> bool {
        self.borrowed() == other.borrowed()
    }
}

/// Owned canonical physical table. Inspection does not transfer ownership.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnedTableModel {
    pub(super) entity_name: String,
    pub(super) table_name: String,
    pub(super) primary_key: String,
    pub(super) primary_keys: Vec<String>,
    pub(super) default_sort: String,
    pub(super) columns: Vec<ColumnModel>,
    pub(super) indexes: Vec<IndexModel>,
    pub(super) composite_unique_indexes: Vec<Vec<String>>,
    pub(super) foreign_keys: Vec<ForeignKeyModel>,
    pub(super) search_indexes: Vec<SearchIndexModel>,
    pub(super) append_only: bool,
    pub(super) retention_purge: bool,
    pub(super) check_constraints: Vec<CheckConstraintModel>,
}
impl OwnedTableModel {
    /// Inspect the canonical `entity_name` component.
    pub fn entity_name(&self) -> &str {
        &self.entity_name
    }
    /// Inspect the canonical `table_name` component.
    pub fn table_name(&self) -> &str {
        &self.table_name
    }
    /// Inspect the canonical `primary_key` component.
    pub fn primary_key(&self) -> &str {
        &self.primary_key
    }
    /// Inspect the canonical `primary_keys` component.
    pub fn primary_keys(&self) -> &[String] {
        &self.primary_keys
    }
    /// Inspect the canonical `default_sort` component.
    pub fn default_sort(&self) -> &str {
        &self.default_sort
    }
    /// Inspect the canonical `columns` component.
    pub fn columns(&self) -> &[ColumnModel] {
        &self.columns
    }
    /// Inspect the canonical `indexes` component.
    pub fn indexes(&self) -> &[IndexModel] {
        &self.indexes
    }
    /// Inspect the canonical `composite_unique_indexes` component.
    pub fn composite_unique_indexes(&self) -> &[Vec<String>] {
        &self.composite_unique_indexes
    }
    /// Inspect the canonical `foreign_keys` component.
    pub fn foreign_keys(&self) -> &[ForeignKeyModel] {
        &self.foreign_keys
    }
    /// Inspect the canonical `search_indexes` component.
    pub fn search_indexes(&self) -> &[SearchIndexModel] {
        &self.search_indexes
    }
    /// Inspect the canonical `append_only` component.
    pub fn append_only(&self) -> bool {
        self.append_only
    }
    /// Inspect the canonical `retention_purge` component.
    pub fn retention_purge(&self) -> bool {
        self.retention_purge
    }
    /// Inspect the canonical `check_constraints` component.
    pub fn check_constraints(&self) -> &[CheckConstraintModel] {
        &self.check_constraints
    }
    pub(super) fn borrowed(&self) -> TableRef<'_> {
        TableRef {
            entity_name: &self.entity_name,
            table_name: &self.table_name,
            primary_key: &self.primary_key,
            primary_keys: &self.primary_keys,
            default_sort: &self.default_sort,
            columns: &self.columns,
            indexes: self.indexes.iter().map(IndexModel::borrowed).collect(),
            composite_unique_indexes: &self.composite_unique_indexes,
            foreign_keys: &self.foreign_keys,
            search_indexes: &self.search_indexes,
            append_only: self.append_only,
            retention_purge: self.retention_purge,
            check_constraints: &self.check_constraints,
        }
    }
    pub(super) fn into_legacy(self) -> TableModel {
        TableModel {
            entity_name: self.entity_name,
            table_name: self.table_name,
            primary_key: self.primary_key,
            primary_keys: self.primary_keys,
            default_sort: self.default_sort,
            columns: self.columns,
            indexes: self
                .indexes
                .into_iter()
                .map(IndexModel::into_legacy)
                .collect(),
            composite_unique_indexes: self.composite_unique_indexes,
            foreign_keys: self.foreign_keys,
            search_indexes: self.search_indexes,
            append_only: self.append_only,
            retention_purge: self.retention_purge,
            check_constraints: self.check_constraints,
        }
    }
}
impl From<&TableModel> for OwnedTableModel {
    fn from(value: &TableModel) -> Self {
        TableRef::from(value).to_owned()
    }
}

/// Owned storage form of the canonical [`SchemaModel`], including owned indexes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OwnedSchemaModel {
    pub(super) extensions: Vec<String>,
    pub(super) tables: Vec<OwnedTableModel>,
    pub(super) limits: super::runtime_migration::RuntimeMigrationLimits,
}
impl OwnedSchemaModel {
    /// Inspect the physical target tables.
    pub fn tables(&self) -> &[OwnedTableModel] {
        &self.tables
    }
    /// Inspect the required extensions.
    pub fn extensions(&self) -> &[String] {
        &self.extensions
    }
    /// Existing physical hash algorithm, independent of owned/static storage.
    pub fn stable_hash(&self) -> String {
        self.borrowed().stable_hash()
    }
    /// Validate the existing canonical physical contract.
    pub fn validate_physical_contract(&self) -> crate::Result<()> {
        self.borrowed().validate_physical_contract()
    }
    pub(super) fn borrowed(&self) -> SchemaRef<'_> {
        SchemaRef {
            extensions: &self.extensions,
            tables: self.tables.iter().map(OwnedTableModel::borrowed).collect(),
        }
    }
    pub(super) fn into_legacy(self) -> SchemaModel {
        SchemaModel {
            extensions: self.extensions,
            tables: self
                .tables
                .into_iter()
                .map(OwnedTableModel::into_legacy)
                .collect(),
        }
    }
}
impl From<&SchemaModel> for OwnedSchemaModel {
    fn from(value: &SchemaModel) -> Self {
        Self {
            extensions: value.extensions.clone(),
            tables: value.tables.iter().map(OwnedTableModel::from).collect(),
            limits: Default::default(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct IndexPredicateRef<'a> {
    pub column: &'a str,
    pub values: Vec<&'a str>,
}
impl fmt::Debug for IndexPredicateRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IndexPredicateDef")
            .field("column", &self.column)
            .field("values", &self.values)
            .finish()
    }
}
#[derive(Clone)]
pub(super) struct IndexRef<'a> {
    pub name: &'a str,
    pub columns: Vec<&'a str>,
    pub column_directions: &'a [IndexDirection],
    pub is_unique: bool,
    pub method: IndexMethod,
    pub is_spatial: bool,
    pub predicate: Option<IndexPredicateRef<'a>>,
}
impl IndexRef<'_> {
    pub fn direction_at(&self, index: usize) -> IndexDirection {
        self.column_directions
            .get(index)
            .copied()
            .unwrap_or(IndexDirection::Asc)
    }
    pub fn to_owned(&self) -> IndexModel {
        IndexModel {
            name: self.name.into(),
            columns: self.columns.iter().map(|c| (*c).into()).collect(),
            column_directions: self.column_directions.into(),
            is_unique: self.is_unique,
            method: self.method,
            is_spatial: self.is_spatial,
            predicate: self.predicate.as_ref().map(|p| IndexPredicateModel {
                column: p.column.into(),
                values: p.values.iter().map(|v| (*v).into()).collect(),
            }),
        }
    }
}
impl PartialEq for IndexRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.columns == other.columns
            && (self.column_directions.is_empty()
                || self.column_directions.len() == self.columns.len())
            && (other.column_directions.is_empty()
                || other.column_directions.len() == other.columns.len())
            && self.is_unique == other.is_unique
            && self.method == other.method
            && self.is_spatial == other.is_spatial
            && self.predicate == other.predicate
            && (0..self.columns.len()).all(|i| self.direction_at(i) == other.direction_at(i))
    }
}
impl<'a> From<&'a IndexDef> for IndexRef<'a> {
    fn from(i: &'a IndexDef) -> Self {
        Self {
            name: i.name,
            columns: i.columns.to_vec(),
            column_directions: i.column_directions,
            is_unique: i.is_unique,
            method: i.method,
            is_spatial: i.is_spatial,
            predicate: i.predicate.as_ref().map(|p| IndexPredicateRef {
                column: p.column,
                values: p.values.to_vec(),
            }),
        }
    }
}
impl fmt::Debug for IndexRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IndexDef")
            .field("name", &self.name)
            .field("columns", &self.columns)
            .field("column_directions", &self.column_directions)
            .field("is_unique", &self.is_unique)
            .field("method", &self.method)
            .field("is_spatial", &self.is_spatial)
            .field("predicate", &self.predicate)
            .finish()
    }
}
#[derive(Clone)]
pub(super) struct TableRef<'a> {
    pub entity_name: &'a String,
    pub table_name: &'a String,
    pub primary_key: &'a String,
    pub primary_keys: &'a Vec<String>,
    pub default_sort: &'a String,
    pub columns: &'a Vec<ColumnModel>,
    pub indexes: Vec<IndexRef<'a>>,
    pub composite_unique_indexes: &'a Vec<Vec<String>>,
    pub foreign_keys: &'a Vec<ForeignKeyModel>,
    pub search_indexes: &'a Vec<SearchIndexModel>,
    pub append_only: bool,
    pub retention_purge: bool,
    pub check_constraints: &'a Vec<CheckConstraintModel>,
}
impl TableRef<'_> {
    pub fn primary_keys(&self) -> &[String] {
        if self.primary_keys.is_empty() {
            std::slice::from_ref(self.primary_key)
        } else {
            self.primary_keys
        }
    }
    pub fn to_owned(&self) -> OwnedTableModel {
        OwnedTableModel {
            entity_name: self.entity_name.clone(),
            table_name: self.table_name.clone(),
            primary_key: self.primary_key.clone(),
            primary_keys: self.primary_keys.clone(),
            default_sort: self.default_sort.clone(),
            columns: self.columns.clone(),
            indexes: self.indexes.iter().map(IndexRef::to_owned).collect(),
            composite_unique_indexes: self.composite_unique_indexes.clone(),
            foreign_keys: self.foreign_keys.clone(),
            search_indexes: self.search_indexes.clone(),
            append_only: self.append_only,
            retention_purge: self.retention_purge,
            check_constraints: self.check_constraints.clone(),
        }
    }
}
impl<'a> From<&'a TableModel> for TableRef<'a> {
    fn from(value: &'a TableModel) -> Self {
        Self {
            entity_name: &value.entity_name,
            table_name: &value.table_name,
            primary_key: &value.primary_key,
            primary_keys: &value.primary_keys,
            default_sort: &value.default_sort,
            columns: &value.columns,
            indexes: value.indexes.iter().map(IndexRef::from).collect(),
            composite_unique_indexes: &value.composite_unique_indexes,
            foreign_keys: &value.foreign_keys,
            search_indexes: &value.search_indexes,
            append_only: value.append_only,
            retention_purge: value.retention_purge,
            check_constraints: &value.check_constraints,
        }
    }
}
impl fmt::Debug for TableRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TableModel")
            .field("entity_name", &self.entity_name)
            .field("table_name", &self.table_name)
            .field("primary_key", &self.primary_key)
            .field("primary_keys", &self.primary_keys)
            .field("default_sort", &self.default_sort)
            .field("columns", &self.columns)
            .field("indexes", &self.indexes)
            .field("composite_unique_indexes", &self.composite_unique_indexes)
            .field("foreign_keys", &self.foreign_keys)
            .field("search_indexes", &self.search_indexes)
            .field("append_only", &self.append_only)
            .field("retention_purge", &self.retention_purge)
            .field("check_constraints", &self.check_constraints)
            .finish()
    }
}
#[derive(Clone, Debug)]
pub(super) struct SchemaRef<'a> {
    pub extensions: &'a Vec<String>,
    pub tables: Vec<TableRef<'a>>,
}
impl<'a> From<&'a SchemaModel> for SchemaRef<'a> {
    fn from(s: &'a SchemaModel) -> Self {
        Self {
            extensions: &s.extensions,
            tables: s.tables.iter().map(TableRef::from).collect(),
        }
    }
}
impl SchemaRef<'_> {
    pub(super) fn validate_physical_contract(&self) -> crate::Result<()> {
        let tables = self
            .tables
            .iter()
            .map(|table| (table.table_name.as_str(), table))
            .collect::<std::collections::BTreeMap<_, _>>();
        for table in &self.tables {
            for index in &table.indexes {
                if index.name.trim().is_empty()
                    || index.columns.is_empty()
                    || (!index.column_directions.is_empty()
                        && index.column_directions.len() != index.columns.len())
                {
                    return Err(sqlx::Error::Protocol(format!(
                        "invalid index column metadata for {}.{}",
                        table.table_name, index.name
                    )));
                }
                let mut index_members = std::collections::BTreeSet::new();
                for column_name in &index.columns {
                    if !index_members.insert(*column_name)
                        || !table
                            .columns
                            .iter()
                            .any(|column| column.name == *column_name)
                    {
                        return Err(sqlx::Error::Protocol(format!(
                            "index {}.{} contains a duplicate or missing column {}",
                            table.table_name, index.name, column_name
                        )));
                    }
                }
            }
            let mut foreign_key_identities = std::collections::BTreeSet::new();
            for foreign_key in table.foreign_keys {
                if foreign_key
                    .constraint_name
                    .as_deref()
                    .is_some_and(|name| name.trim().is_empty())
                    || foreign_key.column_pairs.is_empty()
                {
                    return Err(sqlx::Error::Protocol(format!(
                        "foreign key on {} has invalid constraint identity or no column pairs",
                        table.table_name
                    )));
                }
                if !foreign_key_identities.insert((
                    foreign_key.column_pairs.clone(),
                    foreign_key.target_table.clone(),
                    foreign_key.on_delete.clone(),
                )) {
                    return Err(sqlx::Error::Protocol(format!(
                        "table {} contains duplicate semantic foreign keys",
                        table.table_name
                    )));
                }
                let Some(target_table) = tables.get(foreign_key.target_table.as_str()) else {
                    return Err(sqlx::Error::Protocol(format!(
                        "foreign key on {} references unmanaged target table {}",
                        table.table_name, foreign_key.target_table
                    )));
                };
                let mut source_members = std::collections::BTreeSet::new();
                let mut target_members = std::collections::BTreeSet::new();
                for pair in &foreign_key.column_pairs {
                    if !source_members.insert(pair.source_column.as_str())
                        || !target_members.insert(pair.target_column.as_str())
                    {
                        return Err(sqlx::Error::Protocol(format!(
                            "foreign key on {} contains duplicate column members",
                            table.table_name
                        )));
                    }
                    let source = table
                        .columns
                        .iter()
                        .find(|column| column.name == pair.source_column)
                        .ok_or_else(|| {
                            sqlx::Error::Protocol(format!(
                                "foreign key on {} references missing source column {}",
                                table.table_name, pair.source_column
                            ))
                        })?;
                    let target = target_table
                        .columns
                        .iter()
                        .find(|column| column.name == pair.target_column)
                        .ok_or_else(|| {
                            sqlx::Error::Protocol(format!(
                                "foreign key on {} references missing target column {}.{}",
                                table.table_name, target_table.table_name, pair.target_column
                            ))
                        })?;
                    if !source.sql_type.eq_ignore_ascii_case(&target.sql_type) {
                        return Err(sqlx::Error::Protocol(format!(
                            "foreign key type mismatch between {}.{} ({}) and {}.{} ({})",
                            table.table_name,
                            pair.source_column,
                            source.sql_type,
                            target_table.table_name,
                            pair.target_column,
                            target.sql_type
                        )));
                    }
                    if foreign_key.on_delete == DeletePolicy::SetNull && !source.nullable {
                        return Err(sqlx::Error::Protocol(format!(
                            "SET NULL foreign key source {}.{} is not nullable",
                            table.table_name, pair.source_column
                        )));
                    }
                }
                let target_columns = foreign_key.target_columns().collect::<Vec<_>>();
                let target_is_unique = target_table
                    .primary_keys()
                    .iter()
                    .map(String::as_str)
                    .eq(target_columns.iter().copied())
                    || (target_columns.len() == 1
                        && target_table.columns.iter().any(|column| {
                            column.name == target_columns[0]
                                && (column.is_primary_key || column.is_unique)
                        }))
                    || target_table.composite_unique_indexes.iter().any(|columns| {
                        columns.iter().map(String::as_str).collect::<Vec<_>>() == target_columns
                    })
                    || target_table.indexes.iter().any(|index| {
                        index.is_unique
                            && index.predicate.is_none()
                            && index.columns == target_columns.as_slice()
                    });
                if !target_is_unique {
                    return Err(sqlx::Error::Protocol(format!(
                        "foreign key on {} does not reference an exact unique key on {}",
                        table.table_name, target_table.table_name
                    )));
                }
            }
        }
        Ok(())
    }
}
pub(super) fn stable_schema_view_hash(schema: &SchemaRef<'_>) -> String {
    let mut canonical = String::new();
    let mut extensions = schema.extensions.iter().collect::<Vec<_>>();
    extensions.sort();
    for extension in extensions {
        canonical.push_str("extension:");
        canonical.push_str(extension);
        canonical.push('\n');
    }

    let mut tables = schema.tables.iter().collect::<Vec<_>>();
    tables.sort_by(|left, right| left.table_name.cmp(&right.table_name));

    for table in tables {
        canonical.push_str("table:");
        canonical.push_str(&table.entity_name);
        canonical.push('|');
        canonical.push_str(&table.table_name);
        canonical.push('|');
        canonical.push_str(&table.primary_key);
        canonical.push('|');
        canonical.push_str(&table.primary_keys().join(","));
        canonical.push('|');
        canonical.push_str(&table.default_sort);
        canonical.push('|');
        canonical.push_str(if table.append_only {
            "append_only"
        } else {
            "mutable"
        });
        if table.retention_purge {
            canonical.push_str("|retention_purge");
        }
        canonical.push('\n');

        let mut constraints = table
            .check_constraints
            .iter()
            .map(|constraint| {
                super::migrations::canonical_check_tokens(&constraint.expression)
                    .map(|tokens| tokens.join("|"))
                    .unwrap_or_else(|| constraint.expression.clone())
            })
            .collect::<Vec<_>>();
        constraints.sort();
        for constraint in constraints {
            canonical.push_str("check:");
            canonical.push_str(&constraint);
            canonical.push('\n');
        }

        let mut columns = table.columns.iter().collect::<Vec<_>>();
        columns.sort_by(|left, right| left.name.cmp(&right.name));
        for column in columns {
            canonical.push_str("column:");
            canonical.push_str(&column.name);
            canonical.push('|');
            canonical.push_str(&column.sql_type);
            canonical.push('|');
            canonical.push_str(if column.nullable {
                "nullable"
            } else {
                "not_null"
            });
            canonical.push('|');
            canonical.push_str(if column.is_primary_key {
                "pk"
            } else {
                "not_pk"
            });
            canonical.push('|');
            canonical.push_str(if column.is_unique {
                "unique"
            } else {
                "not_unique"
            });
            canonical.push('|');
            canonical.push_str(
                &column
                    .default
                    .as_deref()
                    .map(super::dialect::canonicalize_column_default_expression)
                    .unwrap_or_default(),
            );
            canonical.push('|');
            if let Some(spatial) = column.spatial {
                canonical.push_str(spatial.kind.as_sql());
                canonical.push(':');
                canonical.push_str(spatial.geometry_type.as_sql());
                canonical.push(':');
                canonical.push_str(&spatial.srid.to_string());
            }
            canonical.push('\n');
        }

        let mut indexes = table.indexes.iter().collect::<Vec<_>>();
        indexes.sort_by(|left, right| left.name.cmp(right.name));
        for index in indexes {
            canonical.push_str("index:");
            canonical.push_str(index.name);
            canonical.push('|');
            canonical.push_str(&index.columns.join(","));
            canonical.push('|');
            for position in 0..index.columns.len() {
                canonical.push_str(index.direction_at(position).as_sql());
                canonical.push(',');
            }
            canonical.push('|');
            canonical.push_str(if index.is_unique { "unique" } else { "index" });
            canonical.push('|');
            canonical.push_str(match index.method {
                IndexMethod::Default => "default",
                IndexMethod::Gist => "gist",
            });
            canonical.push('|');
            canonical.push_str(if index.is_spatial {
                "spatial"
            } else {
                "not_spatial"
            });
            canonical.push('|');
            if let Some(predicate) = &index.predicate {
                canonical.push_str("where_in:");
                canonical.push_str(predicate.column);
                canonical.push(':');
                canonical.push_str(&predicate.values.join(","));
            } else {
                canonical.push_str("no_predicate");
            }
            canonical.push('\n');
        }

        let mut search_indexes = table.search_indexes.iter().collect::<Vec<_>>();
        search_indexes.sort_by(|left, right| left.name.cmp(&right.name));
        for index in search_indexes {
            canonical.push_str("search_index:");
            canonical.push_str(&index.name);
            canonical.push('|');
            canonical.push_str(index.strategy.as_str());
            canonical.push('|');
            canonical.push_str(&index.language);
            canonical.push('|');
            canonical.push_str(&index.tokenizer);
            canonical.push('|');
            canonical.push_str(&index.min_token_len.to_string());
            canonical.push('|');
            canonical.push_str(if index.fallback_enabled {
                "fallback"
            } else {
                "native_only"
            });
            canonical.push('\n');

            let mut fields = index.fields.iter().collect::<Vec<_>>();
            fields.sort_by(|left, right| left.field_name.cmp(&right.field_name));
            for field in fields {
                canonical.push_str("search_field:");
                canonical.push_str(&field.field_name);
                canonical.push('|');
                canonical.push_str(&field.column_name);
                canonical.push('|');
                canonical.push_str(field.weight.as_str());
                canonical.push('|');
                canonical.push_str(field.alias.as_deref().unwrap_or(""));
                canonical.push('|');
                canonical.push_str(field.policy.as_deref().unwrap_or(""));
                canonical.push('\n');
            }

            let mut json_paths = index.json_paths.iter().collect::<Vec<_>>();
            json_paths.sort_by(|left, right| {
                left.field_name
                    .cmp(&right.field_name)
                    .then_with(|| left.path.cmp(&right.path))
            });
            for json_path in json_paths {
                canonical.push_str("search_json_path:");
                canonical.push_str(&json_path.field_name);
                canonical.push('|');
                canonical.push_str(&json_path.column_name);
                canonical.push('|');
                canonical.push_str(&json_path.path);
                canonical.push('|');
                canonical.push_str(json_path.weight.as_str());
                canonical.push('|');
                canonical.push_str(json_path.policy.as_deref().unwrap_or(""));
                canonical.push('\n');
            }

            let mut relations = index.relations.iter().collect::<Vec<_>>();
            relations.sort_by(|left, right| left.relation_field.cmp(&right.relation_field));
            for relation in relations {
                canonical.push_str("search_relation:");
                canonical.push_str(&relation.relation_field);
                canonical.push('|');
                canonical.push_str(&relation.target_type);
                canonical.push('|');
                canonical.push_str(&relation.fields.join(","));
                canonical.push('|');
                canonical.push_str(relation.weight.as_str());
                canonical.push('|');
                canonical.push_str(&relation.max_items.to_string());
                canonical.push('|');
                canonical.push_str(relation.policy.as_deref().unwrap_or(""));
                canonical.push('\n');
            }
        }

        let mut foreign_keys = table.foreign_keys.iter().collect::<Vec<_>>();
        foreign_keys.sort_by(|left, right| {
            (&left.column_pairs, &left.target_table, &left.on_delete).cmp(&(
                &right.column_pairs,
                &right.target_table,
                &right.on_delete,
            ))
        });
        for foreign_key in foreign_keys {
            canonical.push_str("foreign_key:");
            canonical.push_str(&foreign_key.target_table);
            canonical.push('|');
            for pair in &foreign_key.column_pairs {
                canonical.push_str(&pair.source_column);
                canonical.push_str("->");
                canonical.push_str(&pair.target_column);
                canonical.push(',');
            }
            canonical.push_str(foreign_key.on_delete.as_sql());
            canonical.push('\n');
        }
    }

    format!("{:016x}", fnv1a64(canonical.as_bytes()))
}
impl SchemaRef<'_> {
    pub fn stable_hash(&self) -> String {
        stable_schema_view_hash(self)
    }
}
impl From<&EntityMetadata> for OwnedTableModel {
    fn from(value: &EntityMetadata) -> Self {
        let primary_keys = value
            .primary_keys
            .iter()
            .map(|column| (*column).to_string())
            .collect::<Vec<_>>();
        let mut table = Self {
            entity_name: value.entity_name.to_string(),
            table_name: value.table_name.to_string(),
            primary_key: value.primary_key.to_string(),
            primary_keys,
            default_sort: value.default_sort.to_string(),
            columns: value
                .fields
                .iter()
                .map(|field| ColumnModel {
                    name: field.name.to_string(),
                    sql_type: field.sql_type.to_string(),
                    spatial: field.spatial,
                    nullable: field.nullable,
                    is_primary_key: field.is_primary_key,
                    is_unique: field.is_unique,
                    default: field
                        .default
                        .map(super::dialect::canonicalize_column_default_expression),
                })
                .collect(),
            indexes: value.indexes.iter().map(IndexModel::from).collect(),
            composite_unique_indexes: value
                .composite_unique_indexes
                .iter()
                .map(|columns| columns.iter().map(|column| (*column).to_string()).collect())
                .collect(),
            foreign_keys: value
                .relations
                .iter()
                .filter(|relation| relation.emit_foreign_key)
                .map(|relation| {
                    let column_pairs = relation
                        .source_columns
                        .iter()
                        .zip(relation.target_columns.iter())
                        .map(|(source_field, target_column)| {
                            let source_column = value
                                .fields
                                .iter()
                                .find(|field| field.rust_name == *source_field)
                                .map_or(*source_field, |field| field.name);
                            ForeignKeyColumnPairModel {
                                source_column: source_column.to_string(),
                                target_column: (*target_column).to_string(),
                            }
                        })
                        .collect();
                    ForeignKeyModel {
                        constraint_name: None,
                        column_pairs,
                        target_table: relation.target_type.to_string(),
                        is_multiple: relation.is_multiple,
                        on_delete: relation.on_delete.clone(),
                    }
                })
                .collect(),
            search_indexes: value
                .search
                .filter(|index| index.enabled)
                .map(|index| {
                    vec![SearchIndexModel {
                        name: index.index_name.to_string(),
                        table_name: index.table_name.to_string(),
                        entity_name: index.entity_name.to_string(),
                        primary_key: index.primary_key.to_string(),
                        strategy: index.strategy,
                        language: index.language.to_string(),
                        tokenizer: index.tokenizer.to_string(),
                        min_token_len: index.min_token_len,
                        fallback_enabled: index.fallback_enabled,
                        fields: index
                            .fields
                            .iter()
                            .map(|field| SearchFieldModel {
                                field_name: field.field_name.to_string(),
                                column_name: field.column_name.to_string(),
                                weight: field.weight,
                                alias: field.alias.map(str::to_string),
                                policy: field.policy.map(str::to_string),
                            })
                            .collect(),
                        json_paths: index
                            .json_paths
                            .iter()
                            .map(|json_path| SearchJsonPathModel {
                                field_name: json_path.field_name.to_string(),
                                column_name: json_path.column_name.to_string(),
                                path: json_path.path.to_string(),
                                weight: json_path.weight,
                                policy: json_path.policy.map(str::to_string),
                            })
                            .collect(),
                        relations: index
                            .relations
                            .iter()
                            .map(|relation| SearchRelationFieldModel {
                                relation_field: relation.relation_field.to_string(),
                                target_type: relation.target_type.to_string(),
                                fields: relation
                                    .fields
                                    .iter()
                                    .map(|field| (*field).to_string())
                                    .collect(),
                                weight: relation.weight,
                                max_items: relation.max_items,
                                policy: relation.policy.map(str::to_string),
                            })
                            .collect(),
                    }]
                })
                .unwrap_or_default(),
            append_only: value.append_only,
            retention_purge: value.retention_policy.is_some(),
            check_constraints: value
                .check_constraints
                .iter()
                .map(|constraint| CheckConstraintModel {
                    name: constraint.name.to_string(),
                    expression: constraint.expression.to_string(),
                })
                .collect(),
        };
        table.check_constraints.sort();

        for field in &value.fields {
            if field.is_filterable && field.spatial.is_none() {
                add_owned_generated_index(&mut table, &[field.name]);
            }
        }

        table
    }
}
impl OwnedSchemaModel {
    pub(super) fn from_entities(entities: &[&EntityMetadata]) -> Self {
        let entity_table_names = entities
            .iter()
            .map(|entity| (entity.entity_name, entity.table_name))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut extensions = Vec::new();
        if entities
            .iter()
            .flat_map(|entity| entity.fields.iter())
            .any(|field| field.spatial.is_some())
        {
            extensions.push("postgis".to_string());
        }

        let mut tables = entities
            .iter()
            .map(|entity| {
                let mut table = OwnedTableModel::from(*entity);
                for foreign_key in &mut table.foreign_keys {
                    if let Some(table_name) =
                        entity_table_names.get(foreign_key.target_table.as_str())
                    {
                        foreign_key.target_table = (*table_name).to_string();
                    }
                }
                table
            })
            .collect::<Vec<_>>();

        let table_positions = tables
            .iter()
            .enumerate()
            .map(|(index, table)| (table.table_name.clone(), index))
            .collect::<std::collections::BTreeMap<_, _>>();

        for entity in entities {
            for relation in &entity.relations {
                let (table_name, columns) = if relation.is_multiple {
                    let Some(target_table) = entity_table_names.get(relation.target_type) else {
                        continue;
                    };
                    (*target_table, relation.target_columns.to_vec())
                } else {
                    let source_columns = relation
                        .source_columns
                        .iter()
                        .map(|source_field| {
                            entity
                                .fields
                                .iter()
                                .find(|field| field.rust_name == *source_field)
                                .map_or(*source_field, |field| field.name)
                        })
                        .collect::<Vec<_>>();
                    (entity.table_name, source_columns)
                };

                let Some(table_index) = table_positions.get(table_name) else {
                    continue;
                };
                add_owned_generated_index(&mut tables[*table_index], &columns);
            }
        }

        Self {
            extensions,
            tables,
            limits: Default::default(),
        }
    }
}
fn index_columns_match(left: &[&str], right: &[&str]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right.iter())
            .all(|(left, right)| left == right)
}
fn table_has_index_for_columns(table: &TableRef<'_>, columns: &[&str]) -> bool {
    if columns.is_empty() {
        return true;
    }

    if table.primary_keys().len() == columns.len()
        && table
            .primary_keys()
            .iter()
            .zip(columns.iter())
            .all(|(left, right)| left == right)
    {
        return true;
    }

    if columns.len() == 1
        && table
            .columns
            .iter()
            .any(|column| column.name == columns[0] && (column.is_primary_key || column.is_unique))
    {
        return true;
    }

    if table.composite_unique_indexes.iter().any(|index| {
        index_columns_match(
            &index.iter().map(String::as_str).collect::<Vec<_>>(),
            columns,
        )
    }) {
        return true;
    }

    table
        .indexes
        .iter()
        .any(|index| index_columns_match(&index.columns, columns))
}
pub(super) fn add_owned_generated_index(table: &mut OwnedTableModel, columns: &[&str]) {
    if table_has_index_for_columns(&table.borrowed(), columns) {
        return;
    }

    let name = generated_index_name(&table.table_name, columns);
    if table.indexes.iter().any(|index| index.name == name) {
        return;
    }

    table.indexes.push(IndexModel::new(
        name,
        columns.iter().map(|c| (*c).into()).collect(),
    ));
}
#[derive(Clone, Debug, PartialEq)]
pub(super) enum OwnedMigrationStep {
    EnableExtension {
        name: String,
    },
    CreateTable(OwnedTableModel),
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
        index: IndexModel,
    },
    DropIndex {
        table_name: String,
        index_name: String,
    },
    CreateSearchIndex {
        table_name: String,
        index: SearchIndexModel,
    },
    DropSearchIndex {
        table_name: String,
        index_name: String,
    },
    AlterSearchIndex {
        table_name: String,
        before: SearchIndexModel,
        after: SearchIndexModel,
    },
    AddForeignKey {
        table_name: String,
        foreign_key: ForeignKeyModel,
    },
    DropForeignKey {
        table_name: String,
        foreign_key: ForeignKeyModel,
    },
    SetAppendOnly {
        /// Managed physical table whose enforcement contract changes.
        table_name: String,
        /// Whether append-only enforcement is enabled after this step.
        enabled: bool,
        /// Whether enforcement admits exact ORM bounded-retention context.
        retention_purge: bool,
    },
    SetCheckConstraints {
        table_name: String,
        before: Vec<CheckConstraintModel>,
        after: Vec<CheckConstraintModel>,
    },
}
#[derive(Clone, Debug)]
pub(super) enum StepRef<'a> {
    EnableExtension {
        name: &'a String,
    },
    CreateTable(TableRef<'a>),
    DropTable {
        table_name: &'a String,
    },
    AddColumn {
        table_name: &'a String,
        column: &'a ColumnModel,
    },
    DropColumn {
        table_name: &'a String,
        column_name: &'a String,
    },
    AlterColumn {
        table_name: &'a String,
        before: &'a ColumnModel,
        after: &'a ColumnModel,
    },
    CreateIndex {
        table_name: &'a String,
        index: IndexRef<'a>,
    },
    DropIndex {
        table_name: &'a String,
        index_name: &'a String,
    },
    CreateSearchIndex {
        table_name: &'a String,
        index: &'a SearchIndexModel,
    },
    DropSearchIndex {
        table_name: &'a String,
        index_name: &'a String,
    },
    AlterSearchIndex {
        table_name: &'a String,
        before: &'a SearchIndexModel,
        after: &'a SearchIndexModel,
    },
    AddForeignKey {
        table_name: &'a String,
        foreign_key: &'a ForeignKeyModel,
    },
    DropForeignKey {
        table_name: &'a String,
        foreign_key: &'a ForeignKeyModel,
    },
    SetAppendOnly {
        /// Managed physical table whose enforcement contract changes.
        table_name: &'a String,
        /// Whether append-only enforcement is enabled after this step.
        enabled: bool,
        /// Whether enforcement admits exact ORM bounded-retention context.
        retention_purge: bool,
    },
    SetCheckConstraints {
        table_name: &'a String,
        before: &'a Vec<CheckConstraintModel>,
        after: &'a Vec<CheckConstraintModel>,
    },
}
impl<'a> From<&'a MigrationStep> for StepRef<'a> {
    fn from(step: &'a MigrationStep) -> Self {
        match step {
            MigrationStep::EnableExtension { name } => Self::EnableExtension { name: name },
            MigrationStep::CreateTable(value) => Self::CreateTable(TableRef::from(value)),
            MigrationStep::DropTable { table_name } => Self::DropTable {
                table_name: table_name,
            },
            MigrationStep::AddColumn { table_name, column } => Self::AddColumn {
                table_name: table_name,
                column: column,
            },
            MigrationStep::DropColumn {
                table_name,
                column_name,
            } => Self::DropColumn {
                table_name: table_name,
                column_name: column_name,
            },
            MigrationStep::AlterColumn {
                table_name,
                before,
                after,
            } => Self::AlterColumn {
                table_name: table_name,
                before: before,
                after: after,
            },
            MigrationStep::CreateIndex { table_name, index } => Self::CreateIndex {
                table_name: table_name,
                index: IndexRef::from(index),
            },
            MigrationStep::DropIndex {
                table_name,
                index_name,
            } => Self::DropIndex {
                table_name: table_name,
                index_name: index_name,
            },
            MigrationStep::CreateSearchIndex { table_name, index } => Self::CreateSearchIndex {
                table_name: table_name,
                index: index,
            },
            MigrationStep::DropSearchIndex {
                table_name,
                index_name,
            } => Self::DropSearchIndex {
                table_name: table_name,
                index_name: index_name,
            },
            MigrationStep::AlterSearchIndex {
                table_name,
                before,
                after,
            } => Self::AlterSearchIndex {
                table_name: table_name,
                before: before,
                after: after,
            },
            MigrationStep::AddForeignKey {
                table_name,
                foreign_key,
            } => Self::AddForeignKey {
                table_name: table_name,
                foreign_key: foreign_key,
            },
            MigrationStep::DropForeignKey {
                table_name,
                foreign_key,
            } => Self::DropForeignKey {
                table_name: table_name,
                foreign_key: foreign_key,
            },
            MigrationStep::SetAppendOnly {
                table_name,
                enabled,
                retention_purge,
            } => Self::SetAppendOnly {
                table_name: table_name,
                enabled: *enabled,
                retention_purge: *retention_purge,
            },
            MigrationStep::SetCheckConstraints {
                table_name,
                before,
                after,
            } => Self::SetCheckConstraints {
                table_name: table_name,
                before: before,
                after: after,
            },
        }
    }
}
impl<'a> From<&'a OwnedMigrationStep> for StepRef<'a> {
    fn from(step: &'a OwnedMigrationStep) -> Self {
        match step {
            OwnedMigrationStep::EnableExtension { name } => Self::EnableExtension { name: name },
            OwnedMigrationStep::CreateTable(value) => Self::CreateTable(value.borrowed()),
            OwnedMigrationStep::DropTable { table_name } => Self::DropTable {
                table_name: table_name,
            },
            OwnedMigrationStep::AddColumn { table_name, column } => Self::AddColumn {
                table_name: table_name,
                column: column,
            },
            OwnedMigrationStep::DropColumn {
                table_name,
                column_name,
            } => Self::DropColumn {
                table_name: table_name,
                column_name: column_name,
            },
            OwnedMigrationStep::AlterColumn {
                table_name,
                before,
                after,
            } => Self::AlterColumn {
                table_name: table_name,
                before: before,
                after: after,
            },
            OwnedMigrationStep::CreateIndex { table_name, index } => Self::CreateIndex {
                table_name: table_name,
                index: index.borrowed(),
            },
            OwnedMigrationStep::DropIndex {
                table_name,
                index_name,
            } => Self::DropIndex {
                table_name: table_name,
                index_name: index_name,
            },
            OwnedMigrationStep::CreateSearchIndex { table_name, index } => {
                Self::CreateSearchIndex {
                    table_name: table_name,
                    index: index,
                }
            }
            OwnedMigrationStep::DropSearchIndex {
                table_name,
                index_name,
            } => Self::DropSearchIndex {
                table_name: table_name,
                index_name: index_name,
            },
            OwnedMigrationStep::AlterSearchIndex {
                table_name,
                before,
                after,
            } => Self::AlterSearchIndex {
                table_name: table_name,
                before: before,
                after: after,
            },
            OwnedMigrationStep::AddForeignKey {
                table_name,
                foreign_key,
            } => Self::AddForeignKey {
                table_name: table_name,
                foreign_key: foreign_key,
            },
            OwnedMigrationStep::DropForeignKey {
                table_name,
                foreign_key,
            } => Self::DropForeignKey {
                table_name: table_name,
                foreign_key: foreign_key,
            },
            OwnedMigrationStep::SetAppendOnly {
                table_name,
                enabled,
                retention_purge,
            } => Self::SetAppendOnly {
                table_name: table_name,
                enabled: *enabled,
                retention_purge: *retention_purge,
            },
            OwnedMigrationStep::SetCheckConstraints {
                table_name,
                before,
                after,
            } => Self::SetCheckConstraints {
                table_name: table_name,
                before: before,
                after: after,
            },
        }
    }
}

impl OwnedMigrationStep {
    pub(super) fn into_legacy(self, current: &SchemaModel, target: &SchemaModel) -> MigrationStep {
        match self {
            Self::EnableExtension { name } => MigrationStep::EnableExtension { name },
            Self::CreateTable(table) => MigrationStep::CreateTable(
                target
                    .tables
                    .iter()
                    .find(|t| t.table_name == table.table_name)
                    .expect("diff create table belongs to target")
                    .clone(),
            ),
            Self::DropTable { table_name } => MigrationStep::DropTable { table_name },
            Self::AddColumn { table_name, column } => {
                MigrationStep::AddColumn { table_name, column }
            }
            Self::DropColumn {
                table_name,
                column_name,
            } => MigrationStep::DropColumn {
                table_name,
                column_name,
            },
            Self::AlterColumn {
                table_name,
                before,
                after,
            } => MigrationStep::AlterColumn {
                table_name,
                before,
                after,
            },
            Self::CreateIndex { table_name, index } => {
                let legacy = target
                    .tables
                    .iter()
                    .chain(&current.tables)
                    .filter(|t| t.table_name == table_name)
                    .flat_map(|t| &t.indexes)
                    .find(|i| IndexRef::from(*i) == index.borrowed())
                    .expect("diff create index belongs to the compared schema")
                    .clone();
                MigrationStep::CreateIndex {
                    table_name,
                    index: legacy,
                }
            }
            Self::DropIndex {
                table_name,
                index_name,
            } => MigrationStep::DropIndex {
                table_name,
                index_name,
            },
            Self::CreateSearchIndex { table_name, index } => {
                MigrationStep::CreateSearchIndex { table_name, index }
            }
            Self::DropSearchIndex {
                table_name,
                index_name,
            } => MigrationStep::DropSearchIndex {
                table_name,
                index_name,
            },
            Self::AlterSearchIndex {
                table_name,
                before,
                after,
            } => MigrationStep::AlterSearchIndex {
                table_name,
                before,
                after,
            },
            Self::AddForeignKey {
                table_name,
                foreign_key,
            } => MigrationStep::AddForeignKey {
                table_name,
                foreign_key,
            },
            Self::DropForeignKey {
                table_name,
                foreign_key,
            } => MigrationStep::DropForeignKey {
                table_name,
                foreign_key,
            },
            Self::SetAppendOnly {
                table_name,
                enabled,
                retention_purge,
            } => MigrationStep::SetAppendOnly {
                table_name,
                enabled,
                retention_purge,
            },
            Self::SetCheckConstraints {
                table_name,
                before,
                after,
            } => MigrationStep::SetCheckConstraints {
                table_name,
                before,
                after,
            },
        }
    }
}
#[derive(Clone, Debug)]
pub(super) struct OwnedMigrationPlan {
    pub backend: DatabaseBackend,
    pub steps: Vec<OwnedMigrationStep>,
    pub statements: Vec<String>,
}
