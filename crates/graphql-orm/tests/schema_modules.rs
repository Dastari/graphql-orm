use std::sync::OnceLock;

use graphql_orm::graphql::orm::{
    DatabaseBackend, Entity, OrmSchemaModule, SchemaModuleCatalog, SchemaModuleDescriptor,
    SchemaModuleError, SchemaModuleRestoreHook, SchemaModuleRestorePhase, SchemaModulesSnapshot,
};
use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(table = "example_feature_items", plural = "ExampleFeatureItems")]
struct FeatureItem {
    #[primary_key]
    id: String,
    value: String,
}

static FEATURE_DESCRIPTOR: SchemaModuleDescriptor =
    SchemaModuleDescriptor::new("com.example.feature", "1.0.0", "example_feature_");
static FEATURE_HOOKS: [SchemaModuleRestoreHook; 2] = [
    SchemaModuleRestoreHook {
        hook_id: "reconcile-runtime",
        phase: SchemaModuleRestorePhase::Reconcile,
    },
    SchemaModuleRestoreHook {
        hook_id: "runtime-readiness",
        phase: SchemaModuleRestorePhase::Readiness,
    },
];

struct FeatureModule;

impl OrmSchemaModule for FeatureModule {
    fn descriptor(&self) -> &SchemaModuleDescriptor {
        &FEATURE_DESCRIPTOR
    }

    fn entities(&self) -> &[&'static graphql_orm::graphql::orm::EntityMetadata] {
        static ENTITIES: OnceLock<Vec<&'static graphql_orm::graphql::orm::EntityMetadata>> =
            OnceLock::new();
        ENTITIES.get_or_init(|| vec![FeatureItem::metadata()])
    }

    fn restore_hooks(&self) -> &[SchemaModuleRestoreHook] {
        &FEATURE_HOOKS
    }
}

#[test]
fn schema_module_catalog_tracks_owner_version_tables_and_restore_hooks() {
    let module = FeatureModule;
    let catalog = SchemaModuleCatalog::compose(&[&module]).expect("module should compose");
    let snapshot = SchemaModulesSnapshot::from_catalog(DatabaseBackend::Sqlite, "app-1", &catalog);

    assert_eq!(catalog.schema_model().tables.len(), 1);
    assert_eq!(catalog.backup_descriptors().len(), 1);
    assert_eq!(snapshot.modules[0].module_id, "com.example.feature");
    assert_eq!(snapshot.modules[0].version, "1.0.0");
    assert_eq!(snapshot.modules[0].tables, ["example_feature_items"]);
    assert_eq!(snapshot.modules[0].restore_hooks.len(), 2);
    assert!(!snapshot.modules[0].fingerprint.is_empty());
    assert!(!snapshot.schema_hash.is_empty());
}

static BAD_NAMESPACE_DESCRIPTOR: SchemaModuleDescriptor =
    SchemaModuleDescriptor::new("com.example.bad", "1.0.0", "bad_");

struct BadNamespaceModule;

impl OrmSchemaModule for BadNamespaceModule {
    fn descriptor(&self) -> &SchemaModuleDescriptor {
        &BAD_NAMESPACE_DESCRIPTOR
    }

    fn entities(&self) -> &[&'static graphql_orm::graphql::orm::EntityMetadata] {
        static ENTITIES: OnceLock<Vec<&'static graphql_orm::graphql::orm::EntityMetadata>> =
            OnceLock::new();
        ENTITIES.get_or_init(|| vec![FeatureItem::metadata()])
    }
}

#[test]
fn schema_module_rejects_tables_outside_owned_namespace() {
    let module = BadNamespaceModule;
    let result = SchemaModuleCatalog::compose(&[&module]);

    assert!(matches!(
        result,
        Err(SchemaModuleError::TableOutsideNamespace { .. })
    ));
}

static BAD_FINGERPRINT_DESCRIPTOR: SchemaModuleDescriptor =
    SchemaModuleDescriptor::new("com.example.fingerprint", "1.0.0", "example_feature_")
        .with_expected_fingerprint("not-the-computed-fingerprint");

struct BadFingerprintModule;

impl OrmSchemaModule for BadFingerprintModule {
    fn descriptor(&self) -> &SchemaModuleDescriptor {
        &BAD_FINGERPRINT_DESCRIPTOR
    }

    fn entities(&self) -> &[&'static graphql_orm::graphql::orm::EntityMetadata] {
        static ENTITIES: OnceLock<Vec<&'static graphql_orm::graphql::orm::EntityMetadata>> =
            OnceLock::new();
        ENTITIES.get_or_init(|| vec![FeatureItem::metadata()])
    }
}

#[test]
fn schema_module_fingerprint_mismatch_fails_closed() {
    let module = BadFingerprintModule;
    let result = SchemaModuleCatalog::compose(&[&module]);

    assert!(matches!(
        result,
        Err(SchemaModuleError::FingerprintMismatch { .. })
    ));
}

static DUPLICATE_HOOK_DESCRIPTOR: SchemaModuleDescriptor =
    SchemaModuleDescriptor::new("com.example.duplicate-hook", "1.0.0", "example_feature_");
static DUPLICATE_HOOKS: [SchemaModuleRestoreHook; 2] = [
    SchemaModuleRestoreHook {
        hook_id: "reconcile-runtime",
        phase: SchemaModuleRestorePhase::Reconcile,
    },
    SchemaModuleRestoreHook {
        hook_id: "reconcile-runtime",
        phase: SchemaModuleRestorePhase::Validate,
    },
];

struct DuplicateHookModule;

impl OrmSchemaModule for DuplicateHookModule {
    fn descriptor(&self) -> &SchemaModuleDescriptor {
        &DUPLICATE_HOOK_DESCRIPTOR
    }

    fn entities(&self) -> &[&'static graphql_orm::graphql::orm::EntityMetadata] {
        &[]
    }

    fn restore_hooks(&self) -> &[SchemaModuleRestoreHook] {
        &DUPLICATE_HOOKS
    }
}

#[test]
fn schema_module_rejects_duplicate_restore_hook_ids() {
    let module = DuplicateHookModule;
    assert!(matches!(
        SchemaModuleCatalog::compose(&[&module]),
        Err(SchemaModuleError::DuplicateRestoreHookId { .. })
    ));
}

static INVALID_ID_DESCRIPTOR: SchemaModuleDescriptor =
    SchemaModuleDescriptor::new("com..example", "1.0.0", "invalid_id_");

struct InvalidIdModule;

impl OrmSchemaModule for InvalidIdModule {
    fn descriptor(&self) -> &SchemaModuleDescriptor {
        &INVALID_ID_DESCRIPTOR
    }

    fn entities(&self) -> &[&'static graphql_orm::graphql::orm::EntityMetadata] {
        &[]
    }
}

#[test]
fn schema_module_rejects_empty_module_id_components() {
    let module = InvalidIdModule;
    assert!(matches!(
        SchemaModuleCatalog::compose(&[&module]),
        Err(SchemaModuleError::InvalidModuleId(_))
    ));
}
