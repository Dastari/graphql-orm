//! Planning a target whose entity tables live outside the connection's
//! current schema (#621).
//!
//! PostgreSQL introspection reads the current schema. A table an entity
//! declares with a schema (`table = "qualified_audit.logs"`) is elsewhere, so
//! a second plan of the same target used to create it again and fail to apply.
//!
//! Container-backed tests are ignored by default and run only against the
//! test-owned disposable PostgreSQL container.
#![cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]

use super::owned_postgres;

use graphql_orm::graphql::orm::{
    ApplyOptions, ColumnModel, DeletePolicy, Entity, ForeignKeyModel, IndexDef, MigrationStep,
    PlanOptions, SchemaTarget,
};
use graphql_orm::prelude::*;

/// An entity table in a schema that is not on the search path.
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "qualified_audit.logs",
    plural = "QualifiedLogEntries"
)]
#[allow(dead_code)]
struct QualifiedLog {
    #[primary_key]
    id: String,
    actor_id: String,
    body: String,
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "qualified_actors",
    plural = "QualifiedActors"
)]
#[allow(dead_code)]
struct QualifiedActor {
    #[primary_key]
    id: String,
    name: String,
}

/// A table of the current schema, declared with its schema.
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "public.qualified_items",
    plural = "QualifiedItems"
)]
#[allow(dead_code)]
struct QualifiedItem {
    #[primary_key]
    id: String,
    log_id: Option<String>,
}

const LOGS: &str = "qualified_audit.logs";

fn target() -> SchemaTarget {
    let mut target = SchemaTarget::from_entities(
        &[
            <QualifiedLog as Entity>::metadata(),
            <QualifiedActor as Entity>::metadata(),
            <QualifiedItem as Entity>::metadata(),
        ],
        &[],
    );
    for table in &mut target.schema.tables {
        match table.table_name.as_str() {
            LOGS => {
                // An index, an FK into the current schema and append-only
                // enforcement, each read back from the other schema.
                table.indexes = vec![IndexDef::new("qualified_logs_body_idx", &["body"])];
                table.foreign_keys = vec![ForeignKeyModel::single(
                    "actor_id",
                    "qualified_actors",
                    "id",
                    false,
                    DeletePolicy::Restrict,
                )];
                table.append_only = true;
            }
            "public.qualified_items" => {
                // An FK from the current schema into the other one.
                table.foreign_keys = vec![ForeignKeyModel::single(
                    "log_id",
                    LOGS,
                    "id",
                    false,
                    DeletePolicy::SetNull,
                )];
            }
            _ => {}
        }
    }
    target
}

fn step_tables(steps: &[graphql_orm::graphql::orm::PlannedMigrationStep]) -> Vec<String> {
    steps
        .iter()
        .map(|step| format!("{:?}", step.step))
        .collect()
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn schema_qualified_entity_tables_replan_and_reapply()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = owned_postgres::OwnedPostgres::pool("qualified-tables", 5).await?;
    sqlx::query("CREATE SCHEMA qualified_audit")
        .execute(&pool)
        .await?;
    let database = graphql_orm::db::Database::new(pool.clone());
    let target = target();

    let first = database
        .schema()
        .plan_schema_target("qualified-1", "create", &target)
        .await?;
    assert!(
        first
            .migration
            .steps
            .iter()
            .any(|step| matches!(&step.step, MigrationStep::CreateTable(table) if table.table_name == LOGS)),
        "{:?}",
        step_tables(&first.migration.steps)
    );
    database
        .schema()
        .apply_schema_target(&first, ApplyOptions::default())
        .await?;
    for statement in [
        "INSERT INTO qualified_actors VALUES ('actor', 'name')",
        "INSERT INTO qualified_audit.logs VALUES ('log', 'actor', 'body')",
        "INSERT INTO public.qualified_items VALUES ('item', 'log')",
    ] {
        sqlx::query(statement).execute(&pool).await?;
    }

    // The same target plans to nothing: the tables that live in
    // `qualified_audit` and the one declared as `public.qualified_items` are
    // read back under their declared paths, with their index, both foreign
    // keys and append-only enforcement.
    let second = database
        .schema()
        .plan_schema_target("qualified-2", "unchanged", &target)
        .await?;
    assert!(
        second.migration.steps.is_empty(),
        "{:?}",
        step_tables(&second.migration.steps)
    );
    database
        .schema()
        .apply_schema_target(&second, ApplyOptions::default())
        .await?;
    let validation = database.schema().validate_target(&target).await?;
    assert!(!validation.has_errors(), "{validation:?}");

    // A table of the other schema that the target does not name is never
    // read, so even a strict plan cannot drop it.
    sqlx::query("CREATE TABLE qualified_audit.host_only (id text PRIMARY KEY)")
        .execute(&pool)
        .await?;
    let strict = database
        .schema()
        .plan_schema_target_with_options("qualified-3", "strict", &target, PlanOptions::strict())
        .await?;
    assert!(
        strict.migration.steps.is_empty(),
        "{:?}",
        step_tables(&strict.migration.steps)
    );

    // A later change to the qualified table is one step on that table.
    let mut changed = target.clone();
    changed
        .schema
        .tables
        .iter_mut()
        .find(|table| table.table_name == LOGS)
        .unwrap()
        .columns
        .push(ColumnModel {
            name: "source".into(),
            sql_type: "text".into(),
            spatial: None,
            nullable: true,
            is_primary_key: false,
            is_unique: false,
            default: None,
        });
    let third = database
        .schema()
        .plan_schema_target("qualified-4", "add a column", &changed)
        .await?;
    assert!(
        matches!(
            third.migration.steps.as_slice(),
            [step] if matches!(&step.step, MigrationStep::AddColumn { table_name, .. } if table_name == LOGS)
        ),
        "{:?}",
        step_tables(&third.migration.steps)
    );
    database
        .schema()
        .apply_schema_target(&third, ApplyOptions::default())
        .await?;
    let fourth = database
        .schema()
        .plan_schema_target("qualified-5", "unchanged", &changed)
        .await?;
    assert!(
        fourth.migration.steps.is_empty(),
        "{:?}",
        step_tables(&fourth.migration.steps)
    );
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT body, source FROM qualified_audit.logs")
            .fetch_all(&pool)
            .await?;
    assert_eq!(rows, [("body".into(), None)]);
    // Append-only enforcement on the qualified table survived the replans.
    assert!(
        sqlx::query("UPDATE qualified_audit.logs SET body = 'changed'")
            .execute(&pool)
            .await
            .is_err()
    );

    // A schema-qualified table that leaves the target is no longer read, so
    // it is not dropped; tables of the current schema still are.
    let mut smaller = changed.clone();
    smaller
        .schema
        .tables
        .retain(|table| table.table_name == "qualified_actors");
    let removal = database
        .schema()
        .plan_schema_target_with_options("qualified-6", "smaller", &smaller, PlanOptions::strict())
        .await?;
    let dropped = removal
        .migration
        .steps
        .iter()
        .filter_map(|step| match &step.step {
            MigrationStep::DropTable { table_name } => Some(table_name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(dropped, ["qualified_items"]);
    Ok(())
}
