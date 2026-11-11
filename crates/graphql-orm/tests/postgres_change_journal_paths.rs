//! The PostgreSQL change journal recorder resolves declared table paths with
//! the ORM's identifier rules, including quoted and dotted quoted names.
//!
//! Container-backed tests are ignored by default and run only against the
//! test-owned disposable PostgreSQL container.
#![cfg(all(feature = "postgres", feature = "change-journal"))]

#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::graphql::orm::{
    ApplyOptions, BackupChangeAction, ChangeWindow, SchemaDiagnosticSeverity,
};
use graphql_orm::prelude::*;

/// A case-sensitive quoted table name.
#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "\"JournalUsers\"",
    plural = "JournalQuoteds",
    default_sort = "id ASC"
)]
struct JournalQuoted {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub id: String,

    pub label: String,
}

/// A schema-qualified table whose quoted name contains a dot.
#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "journal_app.\"a.b\"",
    plural = "JournalDotteds",
    default_sort = "id ASC"
)]
struct JournalDotted {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub id: String,

    pub label: String,
}

schema_roots! {
    backend: "postgres",
    query_custom_ops: [],
    entities: [JournalQuoted, JournalDotted],
}

const QUOTED: &str = "\"JournalUsers\"";
const DOTTED: &str = "journal_app.\"a.b\"";

async fn entries(
    database: &graphql_orm::db::Database,
) -> Result<Vec<Entry>, Box<dyn std::error::Error>> {
    let mut entries = database
        .export_changes(ChangeWindow {
            after_snapshot_id: None,
            until: i64::MAX,
        })
        .await?
        .into_iter()
        .map(|change| {
            (
                change.entity_name,
                change.table_name,
                change.primary_key,
                change.action,
            )
        })
        .collect::<Vec<_>>();
    entries.sort_by(entry_order);
    Ok(entries)
}

type Entry = (String, String, String, BackupChangeAction);

fn entry_order(left: &Entry, right: &Entry) -> std::cmp::Ordering {
    (&left.1, &left.2, format!("{:?}", left.3)).cmp(&(&right.1, &right.2, format!("{:?}", right.3)))
}

/// Run `sql` as the owner in one transaction with recording on.
async fn record(owner: &sqlx::PgPool, sql: &str) -> Result<(), sqlx::Error> {
    let mut tx = owner.begin().await?;
    sqlx::query("SET LOCAL graphql_orm.change_journal = 'on'")
        .execute(&mut *tx)
        .await?;
    sqlx::query(sql).execute(&mut *tx).await?;
    tx.commit().await
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn quoted_and_dotted_table_names_journal() -> Result<(), Box<dyn std::error::Error>> {
    let owner = owned_postgres::OwnedPostgres::pool("postgres-journal-paths", 5).await?;
    sqlx::query("CREATE SCHEMA journal_app")
        .execute(&owner)
        .await?;
    let database = graphql_orm::db::Database::builder(owner.clone())
        .change_journal_enabled(true)
        .build();
    let target = graphql_orm_schema_target();
    let plan = database
        .schema()
        .plan_schema_target("1", "journal paths", &target)
        .await?;
    database
        .schema()
        .apply_schema_target(&plan, ApplyOptions::default())
        .await?;

    // Each trigger records the resolved schema and name of its table.
    let arguments: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text || ':' || encode(t.tgargs, 'escape') FROM pg_trigger t
         JOIN pg_class c ON c.oid = t.tgrelid
         WHERE t.tgname = 'graphql_orm_change_journal' ORDER BY 1",
    )
    .fetch_all(&owner)
    .await?;
    assert_eq!(arguments.len(), 2, "{arguments:?}");
    assert!(
        arguments
            .iter()
            .any(|row| row.starts_with("JournalUsers:public\\000public\\000JournalUsers\\000")),
        "{arguments:?}"
    );
    assert!(
        arguments
            .iter()
            .any(|row| row.starts_with("a.b:public\\000journal_app\\000a.b\\000")),
        "{arguments:?}"
    );

    JournalQuoted::insert(
        &database,
        CreateJournalQuotedInput {
            id: "quoted-1".to_string(),
            label: "first".to_string(),
        },
    )
    .await?;
    JournalQuoted::update_by_id(
        &database,
        &"quoted-1".to_string(),
        UpdateJournalQuotedInput {
            label: Some("second".to_string()),
        },
    )
    .await?;
    JournalDotted::insert(
        &database,
        CreateJournalDottedInput {
            id: "dotted-1".to_string(),
            label: "first".to_string(),
        },
    )
    .await?;
    assert!(JournalDotted::delete_by_id(&database, &"dotted-1".to_string()).await?);

    let entry = |entity: &str, table: &str, key: &str, action| {
        (
            entity.to_string(),
            table.to_string(),
            key.to_string(),
            action,
        )
    };
    let mut expected = vec![
        entry(
            "JournalQuoted",
            QUOTED,
            "quoted-1",
            BackupChangeAction::Create,
        ),
        entry(
            "JournalQuoted",
            QUOTED,
            "quoted-1",
            BackupChangeAction::Update,
        ),
        entry(
            "JournalDotted",
            DOTTED,
            "dotted-1",
            BackupChangeAction::Create,
        ),
        entry(
            "JournalDotted",
            DOTTED,
            "dotted-1",
            BackupChangeAction::Delete,
        ),
    ];
    expected.sort_by(entry_order);
    assert_eq!(entries(&database).await?, expected);

    // Validation keys both tables by their resolved schema and name.
    let report = database.schema().validate_target(&target).await?;
    let journal = report
        .rls_diagnostics
        .iter()
        .filter(|diagnostic| {
            let message = diagnostic.message.to_ascii_lowercase();
            (message.contains("change journal") || message.contains("record_change_journal_entry"))
                && !message.contains("acts as the journal owner")
        })
        .collect::<Vec<_>>();
    assert!(journal.is_empty(), "{journal:#?}");

    // A bare declared name resolves only in the journal's schema: the same
    // table name in another schema does not record.
    sqlx::raw_sql(
        "CREATE SCHEMA journal_other; \
         CREATE TABLE journal_other.\"JournalUsers\" (id text PRIMARY KEY, label text NOT NULL); \
         CREATE TRIGGER graphql_orm_change_journal AFTER INSERT OR UPDATE OR DELETE ON journal_other.\"JournalUsers\" FOR EACH ROW \
         EXECUTE FUNCTION graphql_orm.record_change_journal_entry('public', 'journal_other', 'JournalUsers', 'JournalQuoted', '\"JournalUsers\"', 'id', 'text')",
    )
    .execute(&owner)
    .await?;
    let other = record(
        &owner,
        "INSERT INTO journal_other.\"JournalUsers\" (id, label) VALUES ('other-1', 'x')",
    )
    .await;
    assert!(
        matches!(&other, Err(error) if error.to_string().contains("does not match the table")),
        "{other:?}"
    );
    // Validation warns about it and keys it apart from a same-named table.
    let report = database.schema().validate_target(&target).await?;
    let warned = report
        .rls_diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == SchemaDiagnosticSeverity::Warning)
        .filter_map(|diagnostic| diagnostic.table.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(warned, vec!["\"journal_other\".\"JournalUsers\""]);

    // A dotted quoted name is not the same as an unquoted dotted path.
    sqlx::raw_sql(
        "CREATE TABLE journal_app.a (id text PRIMARY KEY); \
         CREATE TRIGGER graphql_orm_change_journal AFTER INSERT ON journal_app.a FOR EACH ROW \
         EXECUTE FUNCTION graphql_orm.record_change_journal_entry('public', 'journal_app', 'a', 'JournalDotted', 'journal_app.a.b', 'id', 'text')",
    )
    .execute(&owner)
    .await?;
    let split = record(&owner, "INSERT INTO journal_app.a (id) VALUES ('split-1')").await;
    assert!(
        matches!(&split, Err(error) if error.to_string().contains("does not match the table")),
        "{split:?}"
    );
    Ok(())
}
