//! External source-compatibility contract for the existing migration namespace.

use graphql_orm::graphql::orm::{
    ColumnModel, DatabaseBackend, IndexDef, MigrationPlan, MigrationStep, PlannedMigration,
    PlannedMigrationStep, SchemaDiff, SchemaManager, SchemaModel, TableModel, build_migration_plan,
    classify_migration_step, diff_schema_models, render_migration_step,
};

fn table() -> TableModel {
    TableModel {
        entity_name: "Example".into(),
        table_name: "examples".into(),
        primary_key: "id".into(),
        primary_keys: vec!["id".into()],
        default_sort: "id ASC".into(),
        columns: vec![ColumnModel {
            name: "id".into(),
            sql_type: "BIGINT".into(),
            spatial: None,
            nullable: false,
            is_primary_key: true,
            is_unique: false,
            default: None,
        }],
        indexes: vec![IndexDef::new("idx_examples_id", &["id"])],
        composite_unique_indexes: vec![],
        foreign_keys: vec![],
        search_indexes: vec![],
        append_only: false,
        retention_purge: false,
        check_constraints: vec![],
    }
}

mod variant_import {
    use super::{MigrationStep, TableModel};
    use MigrationStep::CreateTable;

    pub fn constructor() -> fn(TableModel) -> MigrationStep {
        CreateTable
    }
}

mod glob_import {
    use super::MigrationStep;
    use MigrationStep::*;

    pub fn classify(step: &MigrationStep) -> bool {
        match step {
            CreateTable(_) | CreateIndex { .. } => true,
            EnableExtension { .. }
            | DropTable { .. }
            | AddColumn { .. }
            | DropColumn { .. }
            | AlterColumn { .. }
            | DropIndex { .. }
            | CreateSearchIndex { .. }
            | DropSearchIndex { .. }
            | AlterSearchIndex { .. }
            | AddForeignKey { .. }
            | DropForeignKey { .. }
            | SetAppendOnly { .. }
            | SetCheckConstraints { .. } => false,
        }
    }
}

/// Exercises unannotated literals, constructors, namespaces and function pointers.
pub fn verify_legacy_surface() {
    let empty = SchemaModel {
        extensions: vec![],
        tables: vec![],
    };
    let target = SchemaModel {
        extensions: vec![],
        tables: vec![table()],
    };
    let qualified = MigrationStep::CreateTable(table());
    let imported = variant_import::constructor()(table());
    assert_eq!(qualified, imported);
    assert!(glob_import::classify(&qualified));

    let diff: fn(&SchemaModel, &SchemaModel) -> SchemaDiff = diff_schema_models;
    let render: fn(DatabaseBackend, &MigrationStep) -> Vec<String> = render_migration_step;
    let classify: fn(&MigrationStep) -> PlannedMigrationStep = classify_migration_step;
    let build: fn(DatabaseBackend, &SchemaModel, &SchemaModel) -> MigrationPlan =
        build_migration_plan;
    let hash: fn(&SchemaModel) -> String = SchemaModel::stable_hash;
    assert!(!diff(&empty, &target).steps.is_empty());
    assert!(!render(DatabaseBackend::Sqlite, &qualified).is_empty());
    assert_eq!(classify(&qualified).step, qualified);
    assert!(
        !build(DatabaseBackend::Sqlite, &empty, &target)
            .steps
            .is_empty()
    );
    assert!(!hash(&target).is_empty());

    let unannotated_diff = SchemaDiff { steps: vec![] };
    let unannotated_plan = MigrationPlan {
        backend: DatabaseBackend::Sqlite,
        steps: vec![],
        statements: vec![],
    };
    let planned = PlannedMigration {
        version: "example".into(),
        description: "compatibility".into(),
        backend: "sqlite",
        source_schema_hash: None,
        target_schema_hash: hash(&target),
        plan_hash: "example".into(),
        steps: vec![],
        statements: vec![],
    };
    assert!(unannotated_diff.steps.is_empty());
    assert!(unannotated_plan.steps.is_empty());
    assert!(planned.steps.is_empty());
}

/// Preserves the existing synchronous manager method pointer signatures.
pub fn manager_method_pointers<'db, B: graphql_orm::graphql::orm::OrmBackend>(
    manager: &SchemaManager<'db, B>,
    current: &SchemaModel,
    target: &SchemaModel,
) {
    use graphql_orm::graphql::orm::{SchemaPolicy, SchemaValidationReport};
    let policy: fn(&SchemaManager<'db, B>) -> SchemaPolicy = SchemaManager::policy;
    let validate: fn(
        &SchemaManager<'db, B>,
        &SchemaModel,
        &SchemaModel,
    ) -> graphql_orm::Result<SchemaValidationReport> = SchemaManager::validate;
    let _ = policy(manager);
    let _ = validate(manager, current, target);
}

#[cfg(feature = "sqlite")]
type ApplyBackend = graphql_orm::graphql::orm::SqliteBackend;
#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
type ApplyBackend = graphql_orm::graphql::orm::PostgresBackend;

/// Preserves the existing async apply method pointer on both migration backends.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub fn apply_method_pointer<'db, 'call>(
    manager: &'call SchemaManager<'db, ApplyBackend>,
    plan: &'call PlannedMigration,
) {
    use graphql_orm::graphql::orm::ApplyOptions;
    let apply: fn(
        &'call SchemaManager<'db, ApplyBackend>,
        &'call PlannedMigration,
        ApplyOptions,
    ) -> _ = SchemaManager::<ApplyBackend>::apply_migration;
    // Construct and drop an unpolled future: this fixture performs no database I/O.
    drop(apply(manager, plan, ApplyOptions::default()));
}

#[test]
fn existing_migration_api_is_source_compatible() {
    verify_legacy_surface();
}
