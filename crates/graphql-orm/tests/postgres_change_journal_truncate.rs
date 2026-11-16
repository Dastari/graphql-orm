//! The PostgreSQL change journal never loses rows silently: `TRUNCATE` on a
//! journaled table is refused, and so is a schema in which the database
//! changes rows of a table the journal cannot record.
//!
//! Container-backed tests are ignored by default and run only against the
//! test-owned disposable PostgreSQL container.
#![cfg(all(feature = "postgres", feature = "change-journal"))]

#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::graphql::orm::{
    ApplyOptions, BackupChangeAction, ChangeJournalCascadeGap, ChangeWindow, DeletePolicy, Entity,
    SchemaDiagnosticSeverity, change_journal_cascade_gaps,
};
use graphql_orm::prelude::*;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "trunc_parents",
    plural = "TruncParents",
    default_sort = "id ASC"
)]
struct TruncParent {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub id: String,

    pub label: String,
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "trunc_children",
    plural = "TruncChildren",
    default_sort = "id ASC"
)]
struct TruncChild {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub id: String,

    #[sortable]
    #[filterable(type = "string")]
    pub parent_id: String,

    #[relation(
        target = "TruncParent",
        from = "parent_id",
        to = "id",
        on_delete = "cascade"
    )]
    pub parent: Option<TruncParent>,
}

schema_roots! {
    backend: "postgres",
    query_custom_ops: [],
    entities: [TruncParent, TruncChild],
}

async fn journaled(
    name: &str,
) -> Result<(sqlx::PgPool, graphql_orm::db::Database), Box<dyn std::error::Error>> {
    let owner = owned_postgres::OwnedPostgres::pool(name, 5).await?;
    let database = graphql_orm::db::Database::builder(owner.clone())
        .change_journal_enabled(true)
        .build();
    let target = graphql_orm_schema_target();
    let plan = database
        .schema()
        .plan_schema_target("1", "journal", &target)
        .await?;
    database
        .schema()
        .apply_schema_target(&plan, ApplyOptions::default())
        .await?;
    TruncParent::insert(
        &database,
        CreateTruncParentInput {
            id: "parent-1".into(),
            label: "first".into(),
        },
    )
    .await?;
    TruncChild::insert(
        &database,
        CreateTruncChildInput {
            id: "child-1".into(),
            parent_id: "parent-1".into(),
        },
    )
    .await?;
    Ok((owner, database))
}

async fn journal_len(
    database: &graphql_orm::db::Database,
) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(database
        .export_changes(ChangeWindow {
            after_sequence: None,
            until: i64::MAX,
        })
        .await?
        .len())
}

async fn rows(owner: &sqlx::PgPool, table: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(owner)
        .await
}

fn refused_truncate(error: &sqlx::Error) -> bool {
    error.as_database_error().is_some_and(|database| {
        database.code().as_deref() == Some("0A000")
            && database
                .message()
                .contains("graphql-orm change journal refuses TRUNCATE")
    })
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn truncate_is_refused_on_journaled_tables() -> Result<(), Box<dyn std::error::Error>> {
    let (owner, database) = journaled("postgres-journal-truncate").await?;
    let recorded = journal_len(&database).await?;
    assert_eq!(recorded, 2);

    // A plain session, the owner included, cannot truncate a journaled table.
    for statement in [
        "TRUNCATE trunc_children",
        "TRUNCATE trunc_parents CASCADE",
        "TRUNCATE trunc_children, trunc_parents",
    ] {
        let error = sqlx::query(statement)
            .execute(&owner)
            .await
            .expect_err("truncate is refused");
        assert!(refused_truncate(&error), "{statement}: {error}");
    }
    // Neither can a transaction that journals its writes: the journal has no
    // entry for a truncate.
    let mut tx = owner.begin().await?;
    sqlx::query("SET LOCAL graphql_orm.change_journal = 'on'")
        .execute(&mut *tx)
        .await?;
    let error = sqlx::query("TRUNCATE trunc_children")
        .execute(&mut *tx)
        .await
        .expect_err("truncate is refused while journaling");
    assert!(refused_truncate(&error), "{error}");
    tx.rollback().await?;

    // A truncate that reaches a journaled table through a cascade from a table
    // without a recorder is refused by the journaled table's own guard.
    sqlx::raw_sql(
        "CREATE TABLE trunc_host_roots (id text PRIMARY KEY); \
         INSERT INTO trunc_host_roots VALUES ('root'); \
         ALTER TABLE trunc_parents ADD COLUMN host_root text REFERENCES trunc_host_roots(id)",
    )
    .execute(&owner)
    .await?;
    let error = sqlx::query("TRUNCATE trunc_host_roots CASCADE")
        .execute(&owner)
        .await
        .expect_err("the cascade reaches a journaled table");
    assert!(refused_truncate(&error), "{error}");

    assert_eq!(rows(&owner, "trunc_parents").await?, 1);
    assert_eq!(rows(&owner, "trunc_children").await?, 1);
    assert_eq!(rows(&owner, "trunc_host_roots").await?, 1);
    assert_eq!(journal_len(&database).await?, recorded);

    // A delete is journaled, including the rows the database removes through
    // a cascade into a journaled table.
    assert!(TruncParent::delete_by_id(&database, &"parent-1".to_string()).await?);
    let changes = database
        .export_changes(ChangeWindow {
            after_sequence: None,
            until: i64::MAX,
        })
        .await?;
    let deleted = changes
        .iter()
        .filter(|change| change.action == BackupChangeAction::Delete)
        .map(|change| change.table_name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        deleted,
        ["trunc_children", "trunc_parents"].into_iter().collect()
    );

    // A transaction that declares it writes without journaling, as a restore
    // does before it clears its target, may truncate.
    TruncParent::insert(
        &database,
        CreateTruncParentInput {
            id: "parent-2".into(),
            label: "second".into(),
        },
    )
    .await?;
    let before = journal_len(&database).await?;
    let mut tx = owner.begin().await?;
    sqlx::query("SET LOCAL graphql_orm.change_journal = 'off'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("TRUNCATE trunc_children, trunc_parents")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    assert_eq!(rows(&owner, "trunc_parents").await?, 0);
    assert_eq!(journal_len(&database).await?, before);
    Ok(())
}

fn journal_errors(report: &graphql_orm::graphql::orm::SchemaTargetValidationReport) -> Vec<String> {
    report
        .rls_diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == SchemaDiagnosticSeverity::Error)
        .map(|diagnostic| diagnostic.message.clone())
        .collect()
}

/// A database whose recorder predates the guard, or lost it, fails
/// validation until the schema target is applied again.
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn validation_reports_a_missing_truncate_guard() -> Result<(), Box<dyn std::error::Error>> {
    let (owner, database) = journaled("postgres-journal-truncate-drift").await?;
    let target = graphql_orm_schema_target();
    // The owner validates here, so only guard diagnostics are compared.
    let guard = |messages: Vec<String>| {
        messages
            .into_iter()
            .filter(|message| message.contains("TRUNCATE") || message.contains("truncate"))
            .collect::<Vec<_>>()
    };
    let clean = guard(journal_errors(
        &database.schema().validate_target(&target).await?,
    ));
    assert!(clean.is_empty(), "{clean:?}");

    sqlx::query("DROP TRIGGER graphql_orm_change_journal_truncate ON trunc_children")
        .execute(&owner)
        .await?;
    let missing = guard(journal_errors(
        &database.schema().validate_target(&target).await?,
    ));
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert!(
        missing[0].contains("Missing change journal TRUNCATE guard")
            && missing[0].contains("trunc_children"),
        "{missing:?}"
    );
    sqlx::query("TRUNCATE trunc_children")
        .execute(&owner)
        .await
        .expect("without its guard the table truncates, which validation reported");

    sqlx::query("DROP FUNCTION graphql_orm.refuse_change_journal_truncate() CASCADE")
        .execute(&owner)
        .await?;
    let missing = guard(journal_errors(
        &database.schema().validate_target(&target).await?,
    ));
    assert!(
        missing
            .iter()
            .any(|message| message.contains("refuse_change_journal_truncate() is missing")),
        "{missing:?}"
    );

    // Installing the recorder again restores the function and every guard.
    database
        .ensure_change_journal_recorder(&graphql_orm_entity_metadata())
        .await?;
    let repaired = guard(journal_errors(
        &database.schema().validate_target(&target).await?,
    ));
    assert!(repaired.is_empty(), "{repaired:?}");
    let error = sqlx::query("TRUNCATE trunc_children")
        .execute(&owner)
        .await
        .expect_err("the guard is back");
    assert!(refused_truncate(&error), "{error}");
    Ok(())
}

/// The journal records single-key tables only. A foreign key that lets the
/// database delete or null rows of any other table when a journaled row is
/// deleted would change rows without a journal entry, so it is refused when
/// the recorder is planned or installed.
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn a_cascade_into_a_table_without_a_recorder_is_refused()
-> Result<(), Box<dyn std::error::Error>> {
    let (owner, database) = journaled("postgres-journal-cascade").await?;

    // In the schema target: the child loses its single-column key, so it has
    // no recorder, while its foreign key still cascades.
    let mut target = graphql_orm_schema_target();
    assert!(change_journal_cascade_gaps(&target.schema).is_empty());
    let child = target
        .schema
        .tables
        .iter_mut()
        .find(|table| table.table_name == "trunc_children")
        .expect("child table");
    child.primary_keys = vec!["id".into(), "parent_id".into()];
    for column in &mut child.columns {
        column.is_primary_key = true;
    }
    let gaps = change_journal_cascade_gaps(&target.schema);
    assert_eq!(
        gaps,
        [ChangeJournalCascadeGap {
            table_name: "trunc_children".into(),
            journaled_table: "trunc_parents".into(),
            on_delete: DeletePolicy::Cascade,
        }]
    );
    let refused = database
        .schema()
        .plan_schema_target("2", "composite child", &target)
        .await
        .expect_err("the plan is refused");
    assert_eq!(
        ChangeJournalCascadeGap::from_error(&refused),
        Some(&gaps[0])
    );
    let reported = journal_errors(&database.schema().validate_target(&target).await?);
    assert!(
        reported
            .iter()
            .any(|message| message.contains("Unjournaled cascade")),
        "{reported:?}"
    );
    // A foreign key that restricts the delete changes no row on its own.
    for table in &mut target.schema.tables {
        for foreign_key in &mut table.foreign_keys {
            foreign_key.on_delete = DeletePolicy::Restrict;
        }
    }
    assert!(change_journal_cascade_gaps(&target.schema).is_empty());

    // In the catalog: a table no schema target declares.
    let entities = [TruncParent::metadata(), TruncChild::metadata()];
    for (action, refused) in [
        ("ON DELETE CASCADE", true),
        ("ON DELETE SET NULL", true),
        ("ON UPDATE CASCADE", true),
        ("ON DELETE RESTRICT", false),
        ("", false),
    ] {
        sqlx::raw_sql(&format!(
            "DROP TABLE IF EXISTS trunc_host_links; \
             CREATE TABLE trunc_host_links (parent_id text REFERENCES trunc_parents(id) {action}, position integer NOT NULL, UNIQUE (parent_id, position))"
        ))
        .execute(&owner)
        .await?;
        let installed = database.ensure_change_journal_recorder(&entities).await;
        if refused {
            let error = installed.expect_err("the recorder install is refused");
            assert!(
                error
                    .to_string()
                    .contains("graphql-orm change journal cannot record table trunc_host_links"),
                "{action}: {error}"
            );
        } else {
            installed.unwrap_or_else(|error| panic!("{action}: {error}"));
        }
    }
    Ok(())
}
