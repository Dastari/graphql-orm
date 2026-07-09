#![cfg(feature = "sqlite")]

//! SQLite column-default idempotency for managed schema replan.
//!
//! File-backed SQLite stores `DEFAULT (unixepoch())` as `dflt_value = unixepoch()`.
//! Generated metadata must compare equal after reopen so additive-only apply
//! does not reject a no-op restart.

use graphql_orm::graphql::orm::{
    ApplyOptions, ColumnModel, DatabaseBackend, MigrationRisk, MigrationStep, SchemaModel,
    SchemaPolicy, TableModel, build_migration_plan, canonicalize_column_default_expression,
    introspect_sqlite_schema,
};
use graphql_orm::prelude::*;
use std::path::PathBuf;

fn temp_db_path(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "graphql-orm-sqlite-default-idempotency-{}-{}.db",
        name,
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn text_column(name: &str, primary_key: bool) -> ColumnModel {
    ColumnModel {
        name: name.to_string(),
        sql_type: "TEXT".to_string(),
        spatial: None,
        nullable: false,
        is_primary_key: primary_key,
        is_unique: false,
        default: None,
    }
}

fn epoch_column(name: &str, default: &str) -> ColumnModel {
    ColumnModel {
        name: name.to_string(),
        sql_type: "INTEGER".to_string(),
        spatial: None,
        nullable: false,
        is_primary_key: false,
        is_unique: false,
        default: Some(default.to_string()),
    }
}

fn timed_notes_schema(created_default: &str, updated_default: &str) -> SchemaModel {
    SchemaModel {
        extensions: Vec::new(),
        tables: vec![TableModel {
            entity_name: "TimedNote".to_string(),
            table_name: "timed_notes".to_string(),
            primary_key: "id".to_string(),
            primary_keys: vec!["id".to_string()],
            default_sort: "created_at ASC".to_string(),
            columns: vec![
                text_column("id", true),
                text_column("title", false),
                epoch_column("created_at", created_default),
                epoch_column("updated_at", updated_default),
            ],
            indexes: vec![],
            composite_unique_indexes: vec![],
            foreign_keys: vec![],
            search_indexes: vec![],
        }],
    }
}

#[test]
fn default_canonicalization_treats_paren_forms_as_equal() {
    assert_eq!(
        canonicalize_column_default_expression("unixepoch()"),
        canonicalize_column_default_expression("(unixepoch())")
    );
    assert_eq!(
        canonicalize_column_default_expression("date('now')"),
        canonicalize_column_default_expression("(date('now'))")
    );
    assert_eq!(
        canonicalize_column_default_expression("CURRENT_TIMESTAMP"),
        canonicalize_column_default_expression("(current_timestamp)")
    );
    assert_eq!(
        canonicalize_column_default_expression("true"),
        canonicalize_column_default_expression("TRUE")
    );
    assert_ne!(
        canonicalize_column_default_expression("unixepoch()"),
        canonicalize_column_default_expression("date('now')")
    );
    // Parentheses that do not wrap the whole expression must remain distinct.
    assert_ne!(
        canonicalize_column_default_expression("(1+2)*3"),
        canonicalize_column_default_expression("1+2*3")
    );
}

#[test]
fn plan_is_empty_for_equivalent_epoch_default_spellings() {
    let cases = [
        ("unixepoch()", "(unixepoch())"),
        ("(unixepoch())", "unixepoch()"),
        ("((unixepoch()))", "unixepoch()"),
        ("date('now')", "(date('now'))"),
        ("CURRENT_TIMESTAMP", "(current_timestamp)"),
    ];
    for (live_default, desired_default) in cases {
        let live = timed_notes_schema(live_default, live_default);
        let desired = timed_notes_schema(desired_default, desired_default);
        let plan = build_migration_plan(DatabaseBackend::Sqlite, &live, &desired);
        assert!(
            plan.steps.is_empty(),
            "expected no steps for {live_default:?} vs {desired_default:?}, got {:?}",
            plan.steps
        );
        assert!(plan.statements.is_empty());
        assert_eq!(
            live.stable_hash(),
            desired.stable_hash(),
            "schema hashes must match for equivalent defaults {live_default:?}/{desired_default:?}"
        );
    }
}

#[test]
fn genuinely_different_defaults_still_plan_alter() {
    let live = timed_notes_schema("unixepoch()", "unixepoch()");
    let desired = timed_notes_schema("date('now')", "unixepoch()");
    let plan = build_migration_plan(DatabaseBackend::Sqlite, &live, &desired);
    assert!(
        plan.steps.iter().any(|step| matches!(
            step,
            MigrationStep::AlterColumn { before, after, .. }
                if before.name == "created_at"
                    && canonicalize_column_default_expression(
                        before.default.as_deref().unwrap_or("")
                    ) == "unixepoch()"
                    && canonicalize_column_default_expression(
                        after.default.as_deref().unwrap_or("")
                    ) == "date('now')"
        )),
        "expected AlterColumn for created_at default change, got {:?}",
        plan.steps
    );
    assert_ne!(live.stable_hash(), desired.stable_hash());
}

#[tokio::test]
async fn file_backed_reopen_and_replan_is_idempotent() -> Result<(), Box<dyn std::error::Error>> {
    let path = temp_db_path("reopen");
    let url = format!("sqlite://{}?mode=rwc", path.display());

    // Apply a schema whose metadata uses the historical parenthesized form.
    let target_paren = timed_notes_schema("(unixepoch())", "(unixepoch())");
    {
        let database = Database::<SqliteBackend>::connect_sqlite(&url)
            .await?
            .with_schema_policy(SchemaPolicy::Managed);
        let empty = SchemaModel {
            extensions: Vec::new(),
            tables: vec![],
        };
        let plan = database.schema().plan_migration(
            "20260710_timed_notes",
            "create timed notes",
            &empty,
            &target_paren,
        )?;
        database
            .schema()
            .apply_migration(
                &plan,
                ApplyOptions {
                    additive_only: true,
                    ..Default::default()
                },
            )
            .await?;
    }

    // Reopen the same file-backed database.
    let database = Database::<SqliteBackend>::connect_sqlite(&url)
        .await?
        .with_schema_policy(SchemaPolicy::Managed);
    let live = introspect_sqlite_schema(&database).await?;

    let created = live
        .tables
        .iter()
        .find(|table| table.table_name == "timed_notes")
        .and_then(|table| {
            table
                .columns
                .iter()
                .find(|column| column.name == "created_at")
        })
        .expect("created_at");
    assert_eq!(
        created.default.as_deref(),
        Some("unixepoch()"),
        "PRAGMA table_info should surface the unwrapped default after canonicalization"
    );

    // Replan with both unwrapped and parenthesized desired forms.
    for desired in [
        timed_notes_schema("unixepoch()", "unixepoch()"),
        timed_notes_schema("(unixepoch())", "(unixepoch())"),
        target_paren.clone(),
    ] {
        let plan = build_migration_plan(DatabaseBackend::Sqlite, &live, &desired);
        assert!(
            !plan
                .steps
                .iter()
                .any(|step| matches!(step, MigrationStep::AlterColumn { .. })),
            "replan must not emit AlterColumn; steps={:?}",
            plan.steps
        );
        assert!(
            plan.steps.is_empty(),
            "identical schema replan should be empty; steps={:?}",
            plan.steps
        );

        let high_level = database.schema().plan_migration(
            "20260710_timed_notes_replan",
            "replan",
            &live,
            &desired,
        )?;
        assert!(
            high_level.steps.is_empty(),
            "high-level replan should be empty; steps={:?}",
            high_level.steps
        );
    }

    // Re-applying the original version with an empty plan must be idempotent
    // (no UNIQUE failure on __graphql_orm_migrations.version).
    let empty_replan = database.schema().plan_migration(
        "20260710_timed_notes",
        "create timed notes",
        &live,
        &target_paren,
    )?;
    assert!(
        empty_replan.statements.is_empty(),
        "restart replan for same schema should have zero statements"
    );
    let reapply = database
        .schema()
        .apply_migration(
            &empty_replan,
            ApplyOptions {
                additive_only: true,
                ..Default::default()
            },
        )
        .await?;
    assert!(reapply.already_applied);
    assert_eq!(reapply.statements_applied, 0);

    let history_count: i64 = {
        let row =
            sqlx::query("SELECT COUNT(*) AS count FROM __graphql_orm_migrations WHERE version = ?")
                .bind("20260710_timed_notes")
                .fetch_one(database.pool())
                .await?;
        sqlx::Row::try_get(&row, "count")?
    };
    assert_eq!(
        history_count, 1,
        "empty re-apply must not insert a second history row"
    );

    // Additive-only must still reject a real non-additive change.
    let changed = timed_notes_schema("date('now')", "unixepoch()");
    let changed_plan = database.schema().plan_migration(
        "20260710_change_default",
        "change default",
        &live,
        &changed,
    )?;
    assert!(
        changed_plan
            .steps
            .iter()
            .any(|step| matches!(step.step, MigrationStep::AlterColumn { .. })),
        "real default change must plan AlterColumn; steps={:?}",
        changed_plan.steps
    );
    let apply_err = database
        .schema()
        .apply_migration(
            &changed_plan,
            ApplyOptions {
                additive_only: true,
                ..Default::default()
            },
        )
        .await;
    assert!(
        apply_err.is_err(),
        "additive_only must reject real AlterColumn plans"
    );
    let message = apply_err.err().expect("error").to_string();
    assert!(
        message.contains("non-additive") || message.contains("AlterColumn"),
        "unexpected rejection message: {message}"
    );

    let _ = std::fs::remove_file(&path);
    Ok(())
}

#[tokio::test]
async fn empty_migration_reapply_is_idempotent_for_recorded_version()
-> Result<(), Box<dyn std::error::Error>> {
    let path = temp_db_path("empty-reapply");
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let version = "20260710_empty_idempotent";
    let target = timed_notes_schema("unixepoch()", "unixepoch()");

    let database = Database::<SqliteBackend>::connect_sqlite(&url)
        .await?
        .with_schema_policy(SchemaPolicy::Managed);
    let empty = SchemaModel {
        extensions: Vec::new(),
        tables: vec![],
    };
    let first = database
        .schema()
        .plan_migration(version, "create", &empty, &target)?;
    assert!(!first.statements.is_empty());
    let first_report = database
        .schema()
        .apply_migration(&first, ApplyOptions::default())
        .await?;
    assert!(!first_report.already_applied);
    assert!(first_report.statements_applied > 0);

    let live = introspect_sqlite_schema(&database).await?;
    let second = database
        .schema()
        .plan_migration(version, "create", &live, &target)?;
    assert!(second.statements.is_empty());
    assert!(second.steps.is_empty());

    // Twice more: must stay idempotent and leave a single history row.
    for _ in 0..2 {
        let report = database
            .schema()
            .apply_migration(&second, ApplyOptions::default())
            .await?;
        assert!(report.already_applied);
        assert_eq!(report.statements_applied, 0);
    }

    let count: i64 = sqlx::Row::try_get(
        &sqlx::query("SELECT COUNT(*) AS count FROM __graphql_orm_migrations WHERE version = ?")
            .bind(version)
            .fetch_one(database.pool())
            .await?,
        "count",
    )?;
    assert_eq!(count, 1);

    let _ = std::fs::remove_file(&path);
    Ok(())
}

#[tokio::test]
async fn recorded_version_with_remaining_work_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let path = temp_db_path("recorded-drift");
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let version = "20260710_recorded_drift";
    let target = timed_notes_schema("unixepoch()", "unixepoch()");

    let database = Database::<SqliteBackend>::connect_sqlite(&url)
        .await?
        .with_schema_policy(SchemaPolicy::Managed);
    let empty = SchemaModel {
        extensions: Vec::new(),
        tables: vec![],
    };
    let first = database
        .schema()
        .plan_migration(version, "create", &empty, &target)?;
    database
        .schema()
        .apply_migration(&first, ApplyOptions::default())
        .await?;

    // Simulate unsafe version reuse / drift: same version, non-empty plan.
    let drifted = timed_notes_schema("date('now')", "unixepoch()");
    let live = introspect_sqlite_schema(&database).await?;
    let bad_plan =
        database
            .schema()
            .plan_migration(version, "reuse version with work", &live, &drifted)?;
    assert!(
        !bad_plan.statements.is_empty() || !bad_plan.steps.is_empty(),
        "expected remaining work for drifted schema"
    );

    let err = database
        .schema()
        .apply_migration(&bad_plan, ApplyOptions::default())
        .await
        .expect_err("recorded version with remaining work must fail closed");
    let message = err.to_string();
    assert!(
        message.contains("already recorded") && message.contains("still has"),
        "unexpected error: {message}"
    );

    // History must still contain exactly one row for the original version.
    let count: i64 = sqlx::Row::try_get(
        &sqlx::query("SELECT COUNT(*) AS count FROM __graphql_orm_migrations WHERE version = ?")
            .bind(version)
            .fetch_one(database.pool())
            .await?,
        "count",
    )?;
    assert_eq!(count, 1);

    let _ = std::fs::remove_file(&path);
    Ok(())
}

#[test]
fn additive_only_classifies_alter_column_as_non_additive() {
    let live = timed_notes_schema("unixepoch()", "unixepoch()");
    let desired = timed_notes_schema("date('now')", "unixepoch()");
    let plan = build_migration_plan(DatabaseBackend::Sqlite, &live, &desired);
    let classified = graphql_orm::graphql::orm::classify_migration_steps(&plan.steps);
    assert!(
        classified
            .iter()
            .any(|step| step.risk != MigrationRisk::Additive
                && matches!(step.step, MigrationStep::AlterColumn { .. })),
        "AlterColumn must not be additive; got {classified:?}"
    );
}
