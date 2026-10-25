//! Backup descriptors of the ORM-owned link tables of many-to-many runtime relations.
//!
//! A link table holds one row per link: the source collection's primary key and the target
//! collection's primary key, which together form its composite primary key. Both sides reference
//! their collection with `ON DELETE CASCADE`, so the table restores after both endpoints.
use std::collections::BTreeMap;
use std::fmt;

use super::*;

/// Why a link table cannot be described for backup.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeLinkBackupError {
    /// A join relation's endpoint collection has no backup order from the host.
    UnorderedEndpoint(CollectionId),
    /// A link column's key kind has no portable backup representation.
    UnsupportedKeyKind { table: String, column: String },
}
impl fmt::Display for RuntimeLinkBackupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnorderedEndpoint(collection) => {
                write!(f, "link table endpoint {collection} has no backup order")
            }
            Self::UnsupportedKeyKind { table, column } => {
                write!(
                    f,
                    "link column {table}.{column} has no portable backup kind"
                )
            }
        }
    }
}
impl std::error::Error for RuntimeLinkBackupError {}

fn backup_kind(kind: RuntimeValueKind) -> Option<(BackupValueKind, Option<DecimalDef>)> {
    Some(match kind {
        RuntimeValueKind::Boolean => (BackupValueKind::Bool, None),
        RuntimeValueKind::Integer => (BackupValueKind::Integer, None),
        RuntimeValueKind::Float => (BackupValueKind::Float, None),
        RuntimeValueKind::String => (BackupValueKind::String, None),
        RuntimeValueKind::Uuid => (BackupValueKind::Uuid, None),
        RuntimeValueKind::Json => (BackupValueKind::Json, None),
        RuntimeValueKind::Bytes => (BackupValueKind::Bytes, None),
        RuntimeValueKind::Decimal(definition) => (BackupValueKind::Decimal, Some(definition)),
        RuntimeValueKind::DateTime => return None,
    })
}

impl ValidatedRuntimeSchema {
    /// One backup descriptor per ORM-owned join table, in table-name order. Exact inverse
    /// relations share one table and yield one descriptor.
    ///
    /// The key is composite (source key columns, then target key columns). Each column depends on
    /// its endpoint's key column with `Cascade`. Export and restore order follow both endpoints:
    /// one more than the larger of `order_of(source)` and `order_of(target)`, the host's orders
    /// for the endpoint collections' own descriptors.
    ///
    /// # Errors
    ///
    /// [`RuntimeLinkBackupError::UnorderedEndpoint`] when `order_of` has no order for an endpoint,
    /// and [`RuntimeLinkBackupError::UnsupportedKeyKind`] for a `DateTime` key.
    pub fn join_table_backup_descriptors(
        &self,
        order_of: impl Fn(&CollectionId) -> Option<i32>,
    ) -> Result<Vec<EntityBackupDescriptor>, RuntimeLinkBackupError> {
        let schema = self.schema();
        let collection = |id: &CollectionId| schema.collections.iter().find(|c| &c.id == id);
        let mut tables = BTreeMap::new();
        for source in &schema.collections {
            for relation in &source.relations {
                let Some(join) = &relation.join else {
                    continue;
                };
                if tables.contains_key(&join.physical_table) {
                    continue;
                }
                // Validation guarantees both endpoints and every referenced key field exist.
                let target = collection(&relation.target).ok_or_else(|| {
                    RuntimeLinkBackupError::UnorderedEndpoint(relation.target.clone())
                })?;
                let mut columns = Vec::new();
                let mut dependencies = Vec::new();
                for (endpoint, side) in [(source, &join.source), (target, &join.target)] {
                    for member in side {
                        let field = endpoint
                            .fields
                            .iter()
                            .find(|f| f.id == member.field)
                            .ok_or_else(|| {
                                RuntimeLinkBackupError::UnorderedEndpoint(endpoint.id.clone())
                            })?;
                        let (logical_type, decimal) =
                            backup_kind(field.value_kind).ok_or_else(|| {
                                RuntimeLinkBackupError::UnsupportedKeyKind {
                                    table: join.physical_table.clone(),
                                    column: member.column.clone(),
                                }
                            })?;
                        columns.push(ColumnBackupDescriptor {
                            column_name: member.column.clone(),
                            rust_field_name: member.column.clone(),
                            logical_type,
                            decimal,
                            nullable: false,
                            is_primary_key: true,
                            is_generated: false,
                            backup_policy: ColumnBackupPolicy::Include,
                        });
                        dependencies.push(EntityDependencyDescriptor {
                            entity_name: endpoint.api_type_name.clone(),
                            table_name: endpoint.physical_table.clone(),
                            source_column: member.column.clone(),
                            target_column: field.physical_column.clone(),
                            nullable: false,
                            on_delete: DeletePolicy::Cascade,
                        });
                    }
                }
                let order = |id: &CollectionId| {
                    order_of(id)
                        .ok_or_else(|| RuntimeLinkBackupError::UnorderedEndpoint(id.clone()))
                };
                let order = order(&source.id)?.max(order(&target.id)?).saturating_add(1);
                let keys = columns
                    .iter()
                    .map(|c| c.column_name.clone())
                    .collect::<Vec<_>>();
                tables.insert(
                    join.physical_table.clone(),
                    EntityBackupDescriptor {
                        entity_name: join.physical_table.clone(),
                        table_name: join.physical_table.clone(),
                        primary_key_column: keys[0].clone(),
                        primary_key_columns: keys,
                        retention_purge: false,
                        export_order: order,
                        restore_order: order,
                        columns,
                        dependencies,
                    },
                );
            }
        }
        Ok(tables.into_values().collect())
    }
}
