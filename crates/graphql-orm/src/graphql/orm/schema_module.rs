//! Dependency-owned schema modules.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::{
    DatabaseBackend, EntityBackupDescriptor, EntityMetadata, SchemaModel,
    backup_descriptors_from_entities, stable_schema_hash,
};

/// Stable metadata declaring ownership of a dependency-managed schema module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaModuleDescriptor {
    /// Globally stable module identifier, such as `com.example.feature`.
    pub module_id: &'static str,
    /// Semantic module schema version.
    pub version: &'static str,
    /// Reserved table-name prefix including its trailing separator.
    pub table_namespace: &'static str,
    /// Optional source-controlled expected fingerprint.
    pub expected_fingerprint: Option<&'static str>,
}

impl SchemaModuleDescriptor {
    /// Creates a descriptor.
    pub const fn new(
        module_id: &'static str,
        version: &'static str,
        table_namespace: &'static str,
    ) -> Self {
        Self {
            module_id,
            version,
            table_namespace,
            expected_fingerprint: None,
        }
    }

    /// Requires the computed fingerprint to match a source-controlled value.
    pub const fn with_expected_fingerprint(mut self, fingerprint: &'static str) -> Self {
        self.expected_fingerprint = Some(fingerprint);
        self
    }
}

/// Restore lifecycle phase contributed by a schema module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum SchemaModuleRestorePhase {
    /// Validate a backup and deployment before importing rows.
    Preflight,
    /// Reconcile restored runtime state without external side effects.
    Reconcile,
    /// Validate repaired state.
    Validate,
    /// Decide whether the module runtime may start.
    Readiness,
}

/// Stable declaration of a module-owned restore hook.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaModuleRestoreHook {
    /// Stable hook identifier within the module.
    pub hook_id: &'static str,
    /// Lifecycle phase.
    pub phase: SchemaModuleRestorePhase,
}

/// Dependency contract for migration-only internal entities and lifecycle
/// metadata.
pub trait OrmSchemaModule: Send + Sync {
    /// Returns stable ownership/version metadata.
    fn descriptor(&self) -> &SchemaModuleDescriptor;

    /// Returns all entities owned by this module.
    fn entities(&self) -> &[&'static EntityMetadata];

    /// Returns restore hooks owned by this module.
    fn restore_hooks(&self) -> &[SchemaModuleRestoreHook] {
        &[]
    }
}

/// Serializable module metadata included in backup/schema snapshots.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaModuleSnapshot {
    /// Stable module identifier.
    pub module_id: String,
    /// Module schema version.
    pub version: String,
    /// Reserved table namespace.
    pub table_namespace: String,
    /// Computed descriptor fingerprint.
    pub fingerprint: String,
    /// Owned table names.
    pub tables: Vec<String>,
    /// Declared restore phases and hook IDs.
    pub restore_hooks: Vec<SchemaModuleRestoreHookSnapshot>,
}

/// Serializable restore hook metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaModuleRestoreHookSnapshot {
    /// Stable hook identifier.
    pub hook_id: String,
    /// Lifecycle phase.
    pub phase: SchemaModuleRestorePhase,
}

/// Validated composition of dependency-owned schema modules.
#[derive(Clone, Debug)]
pub struct SchemaModuleCatalog {
    modules: Vec<SchemaModuleSnapshot>,
    entities: Vec<&'static EntityMetadata>,
}

impl SchemaModuleCatalog {
    /// Validates and composes schema modules.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid identifiers/namespaces, duplicate module or
    /// table ownership, namespace overlap, an out-of-namespace entity, or an
    /// expected fingerprint mismatch.
    pub fn compose(modules: &[&dyn OrmSchemaModule]) -> Result<Self, SchemaModuleError> {
        let mut modules = modules.to_vec();
        modules.sort_by_key(|module| module.descriptor().module_id);
        let mut module_ids = BTreeSet::new();
        let mut namespaces = BTreeSet::new();
        let mut table_owners = BTreeMap::new();
        let mut snapshots = Vec::with_capacity(modules.len());
        let mut entities = Vec::new();

        for module in modules {
            let descriptor = module.descriptor();
            let module_entities = module.entities();
            let module_restore_hooks = module.restore_hooks();
            validate_descriptor(descriptor)?;
            validate_restore_hooks(descriptor.module_id, module_restore_hooks)?;

            if !module_ids.insert(descriptor.module_id) {
                return Err(SchemaModuleError::DuplicateModuleId(
                    descriptor.module_id.to_owned(),
                ));
            }
            if namespaces.iter().any(|existing: &&str| {
                existing.starts_with(descriptor.table_namespace)
                    || descriptor.table_namespace.starts_with(*existing)
            }) {
                return Err(SchemaModuleError::OverlappingNamespace(
                    descriptor.table_namespace.to_owned(),
                ));
            }
            namespaces.insert(descriptor.table_namespace);

            let mut tables = Vec::with_capacity(module_entities.len());
            for entity in module_entities {
                let Some(table_name) = table_basename(entity.table_name) else {
                    return Err(SchemaModuleError::TableOutsideNamespace {
                        module_id: descriptor.module_id.to_owned(),
                        table_name: entity.table_name.to_owned(),
                        namespace: descriptor.table_namespace.to_owned(),
                    });
                };
                if !table_name.starts_with(descriptor.table_namespace) {
                    return Err(SchemaModuleError::TableOutsideNamespace {
                        module_id: descriptor.module_id.to_owned(),
                        table_name: entity.table_name.to_owned(),
                        namespace: descriptor.table_namespace.to_owned(),
                    });
                }
                if let Some(owner) = table_owners.insert(table_name.clone(), descriptor.module_id) {
                    return Err(SchemaModuleError::DuplicateTableOwnership {
                        table_name,
                        first_module: owner.to_owned(),
                        second_module: descriptor.module_id.to_owned(),
                    });
                }
                tables.push(table_name);
                entities.push(*entity);
            }
            tables.sort();

            let fingerprint = stable_schema_module_fingerprint(
                descriptor,
                &backup_descriptors_from_entities(module_entities),
                module_restore_hooks,
            );
            if let Some(expected) = descriptor.expected_fingerprint {
                if expected != fingerprint {
                    return Err(SchemaModuleError::FingerprintMismatch {
                        module_id: descriptor.module_id.to_owned(),
                        expected: expected.to_owned(),
                        actual: fingerprint,
                    });
                }
            }

            let mut restore_hooks = module_restore_hooks
                .iter()
                .map(|hook| SchemaModuleRestoreHookSnapshot {
                    hook_id: hook.hook_id.to_owned(),
                    phase: hook.phase,
                })
                .collect::<Vec<_>>();
            restore_hooks.sort_by_key(|hook| (hook.phase as u8, hook.hook_id.clone()));
            snapshots.push(SchemaModuleSnapshot {
                module_id: descriptor.module_id.to_owned(),
                version: descriptor.version.to_owned(),
                table_namespace: descriptor.table_namespace.to_owned(),
                fingerprint,
                tables,
                restore_hooks,
            });
        }

        snapshots.sort_by(|left, right| left.module_id.cmp(&right.module_id));
        entities.sort_by_key(|entity| entity.table_name);
        Ok(Self {
            modules: snapshots,
            entities,
        })
    }

    /// Returns validated module snapshots in stable module-ID order.
    pub fn modules(&self) -> &[SchemaModuleSnapshot] {
        &self.modules
    }

    /// Returns all owned entity metadata.
    pub fn entities(&self) -> &[&'static EntityMetadata] {
        &self.entities
    }

    /// Builds the migration target for all module-owned entities.
    pub fn schema_model(&self) -> SchemaModel {
        SchemaModel::from_entities(&self.entities)
    }

    /// Returns backup descriptors for module-owned entities.
    pub fn backup_descriptors(&self) -> Vec<EntityBackupDescriptor> {
        let mut descriptors = backup_descriptors_from_entities(&self.entities);
        descriptors.sort_by(|left, right| left.table_name.cmp(&right.table_name));
        descriptors
    }
}

/// Stable schema-module validation error.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SchemaModuleError {
    /// Descriptor contains an invalid stable identifier.
    InvalidModuleId(String),
    /// Descriptor version is empty.
    InvalidVersion(String),
    /// Namespace is not a safe table prefix ending in `_`.
    InvalidNamespace(String),
    /// Module ID appears more than once.
    DuplicateModuleId(String),
    /// Two namespaces overlap.
    OverlappingNamespace(String),
    /// An entity table is outside its owner's namespace.
    TableOutsideNamespace {
        /// Module ID.
        module_id: String,
        /// Table name.
        table_name: String,
        /// Required namespace.
        namespace: String,
    },
    /// Two modules claim the same table.
    DuplicateTableOwnership {
        /// Table name.
        table_name: String,
        /// First owner.
        first_module: String,
        /// Second owner.
        second_module: String,
    },
    /// A restore hook identifier is not stable and portable.
    InvalidRestoreHookId {
        /// Module ID.
        module_id: String,
        /// Invalid hook identifier.
        hook_id: String,
    },
    /// A module declares the same restore hook identifier more than once.
    DuplicateRestoreHookId {
        /// Module ID.
        module_id: String,
        /// Duplicate hook identifier.
        hook_id: String,
    },
    /// Source-controlled and computed fingerprints differ.
    FingerprintMismatch {
        /// Module ID.
        module_id: String,
        /// Expected fingerprint.
        expected: String,
        /// Computed fingerprint.
        actual: String,
    },
}

impl fmt::Display for SchemaModuleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidModuleId(value) => write!(formatter, "invalid schema module id {value}"),
            Self::InvalidVersion(module_id) => {
                write!(formatter, "schema module {module_id} has an empty version")
            }
            Self::InvalidNamespace(value) => {
                write!(formatter, "invalid schema module namespace {value}")
            }
            Self::DuplicateModuleId(value) => write!(formatter, "duplicate schema module {value}"),
            Self::OverlappingNamespace(value) => {
                write!(formatter, "overlapping schema module namespace {value}")
            }
            Self::TableOutsideNamespace {
                module_id,
                table_name,
                namespace,
            } => write!(
                formatter,
                "schema module {module_id} table {table_name} is outside namespace {namespace}"
            ),
            Self::DuplicateTableOwnership {
                table_name,
                first_module,
                second_module,
            } => write!(
                formatter,
                "table {table_name} is owned by both {first_module} and {second_module}"
            ),
            Self::InvalidRestoreHookId { module_id, hook_id } => write!(
                formatter,
                "schema module {module_id} has invalid restore hook id {hook_id}"
            ),
            Self::DuplicateRestoreHookId { module_id, hook_id } => write!(
                formatter,
                "schema module {module_id} repeats restore hook id {hook_id}"
            ),
            Self::FingerprintMismatch {
                module_id,
                expected,
                actual,
            } => write!(
                formatter,
                "schema module {module_id} fingerprint mismatch: expected {expected}, actual {actual}"
            ),
        }
    }
}

impl Error for SchemaModuleError {}

/// Computes the stable fingerprint for a module descriptor and owned schema.
pub fn stable_schema_module_fingerprint(
    descriptor: &SchemaModuleDescriptor,
    entities: &[EntityBackupDescriptor],
    restore_hooks: &[SchemaModuleRestoreHook],
) -> String {
    let mut canonical = String::new();
    canonical.push_str(descriptor.module_id);
    canonical.push('|');
    canonical.push_str(descriptor.version);
    canonical.push('|');
    canonical.push_str(descriptor.table_namespace);
    canonical.push('|');
    canonical.push_str(&stable_schema_hash(entities));

    let mut hooks = restore_hooks.iter().collect::<Vec<_>>();
    hooks.sort_by_key(|hook| (hook.phase as u8, hook.hook_id));
    for hook in hooks {
        canonical.push('|');
        canonical.push_str(match hook.phase {
            SchemaModuleRestorePhase::Preflight => "preflight",
            SchemaModuleRestorePhase::Reconcile => "reconcile",
            SchemaModuleRestorePhase::Validate => "validate",
            SchemaModuleRestorePhase::Readiness => "readiness",
        });
        canonical.push(':');
        canonical.push_str(hook.hook_id);
    }

    format!("{:016x}", fnv1a64(canonical.as_bytes()))
}

fn validate_restore_hooks(
    module_id: &str,
    restore_hooks: &[SchemaModuleRestoreHook],
) -> Result<(), SchemaModuleError> {
    let mut hook_ids = BTreeSet::new();
    for hook in restore_hooks {
        let valid = !hook.hook_id.is_empty()
            && hook.hook_id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            });
        if !valid {
            return Err(SchemaModuleError::InvalidRestoreHookId {
                module_id: module_id.to_owned(),
                hook_id: hook.hook_id.to_owned(),
            });
        }
        if !hook_ids.insert(hook.hook_id) {
            return Err(SchemaModuleError::DuplicateRestoreHookId {
                module_id: module_id.to_owned(),
                hook_id: hook.hook_id.to_owned(),
            });
        }
    }
    Ok(())
}

/// Creates a module-aware backup snapshot using validated ownership metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SchemaModulesSnapshot {
    /// Database backend.
    pub backend: String,
    /// Host migration version.
    pub migration_version: String,
    /// Module metadata.
    pub modules: Vec<SchemaModuleSnapshot>,
    /// Module-owned backup entities.
    pub entities: Vec<EntityBackupDescriptor>,
    /// Stable hash across modules and entities.
    pub schema_hash: String,
}

impl SchemaModulesSnapshot {
    /// Builds a module-aware snapshot from a validated catalog.
    pub fn from_catalog(
        backend: DatabaseBackend,
        migration_version: impl Into<String>,
        catalog: &SchemaModuleCatalog,
    ) -> Self {
        let entities = catalog.backup_descriptors();
        let mut canonical = stable_schema_hash(&entities);
        for module in catalog.modules() {
            canonical.push('|');
            canonical.push_str(&module.module_id);
            canonical.push(':');
            canonical.push_str(&module.version);
            canonical.push(':');
            canonical.push_str(&module.fingerprint);
        }
        let schema_hash = format!("{:016x}", fnv1a64(canonical.as_bytes()));
        Self {
            backend: format!("{backend:?}"),
            migration_version: migration_version.into(),
            modules: catalog.modules().to_vec(),
            entities,
            schema_hash,
        }
    }
}

fn validate_descriptor(descriptor: &SchemaModuleDescriptor) -> Result<(), SchemaModuleError> {
    let valid_id = descriptor.module_id.split('.').all(|component| {
        component
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
            && component
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
            && component
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    });
    if !valid_id {
        return Err(SchemaModuleError::InvalidModuleId(
            descriptor.module_id.to_owned(),
        ));
    }
    if descriptor.version.trim().is_empty() {
        return Err(SchemaModuleError::InvalidVersion(
            descriptor.module_id.to_owned(),
        ));
    }
    let valid_namespace = descriptor
        .table_namespace
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_lowercase)
        && descriptor.table_namespace.ends_with('_')
        && descriptor
            .table_namespace
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    if !valid_namespace {
        return Err(SchemaModuleError::InvalidNamespace(
            descriptor.table_namespace.to_owned(),
        ));
    }
    Ok(())
}

fn table_basename(table_name: &str) -> Option<String> {
    let component = table_name.rsplit('.').next()?.trim();
    if component.is_empty() {
        return None;
    }
    if component.starts_with('[') && component.ends_with(']') {
        return Some(component[1..component.len() - 1].replace("]]", "]"));
    }
    if component.starts_with('"') && component.ends_with('"') {
        return Some(component[1..component.len() - 1].replace("\"\"", "\""));
    }
    Some(component.to_owned())
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::table_basename;

    #[test]
    fn table_basename_normalizes_supported_backend_identifier_paths() {
        assert_eq!(
            table_basename("example_feature_items").as_deref(),
            Some("example_feature_items")
        );
        assert_eq!(
            table_basename("public.example_feature_items").as_deref(),
            Some("example_feature_items")
        );
        assert_eq!(
            table_basename("[dbo].[example_feature_items]").as_deref(),
            Some("example_feature_items")
        );
        assert_eq!(
            table_basename("\"public\".\"example_feature_items\"").as_deref(),
            Some("example_feature_items")
        );
    }
}
