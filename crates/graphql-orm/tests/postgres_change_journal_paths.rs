//! The PostgreSQL change journal binds each declared table path to the table
//! the ORM's own writes reach, for every naming form: plain and mixed-case
//! unquoted names (which PostgreSQL folds), quoted and dotted quoted names.
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

/// An unquoted mixed-case name: the ORM's SQL leaves it unquoted, so
/// PostgreSQL folds it to `journalmixed`.
#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "JournalMixed",
    plural = "JournalMixeds",
    default_sort = "id ASC"
)]
struct JournalMixed {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub id: String,

    pub label: String,
}

/// An unquoted mixed-case qualified path, folded to `journalapp.rows`.
#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "JournalApp.Rows",
    plural = "JournalAppRows",
    default_sort = "id ASC"
)]
struct JournalAppRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub id: String,

    pub label: String,
}

/// A bare name that only exists outside the journal's schema.
#[allow(dead_code)]
#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "journal_far_rows",
    plural = "JournalFarRows",
    default_sort = "id ASC"
)]
struct JournalFarRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub id: String,

    pub note: String,
}

/// A table owned by a non-superuser schema owner.
#[allow(dead_code)]
#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "postgres",
    table = "journal_applied",
    plural = "JournalApplieds",
    default_sort = "id ASC"
)]
struct JournalApplied {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub id: String,

    pub note: String,
}

schema_roots! {
    backend: "postgres",
    query_custom_ops: [],
    entities: [JournalQuoted, JournalDotted, JournalMixed, JournalAppRow],
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
    sqlx::raw_sql("CREATE SCHEMA journal_app; CREATE SCHEMA journalapp")
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
    assert_eq!(arguments.len(), 4, "{arguments:?}");
    // Unquoted names fold: the triggers sit on the folded tables the ORM writes.
    assert!(
        arguments.iter().any(|row| row.starts_with(
            "journalmixed:public\\000public\\000journalmixed\\000JournalMixed\\000JournalMixed\\000"
        )),
        "{arguments:?}"
    );
    assert!(
        arguments.iter().any(|row| row.starts_with(
            "rows:public\\000journalapp\\000rows\\000JournalAppRow\\000JournalApp.Rows\\000"
        )),
        "{arguments:?}"
    );
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
    JournalMixed::insert(
        &database,
        CreateJournalMixedInput {
            id: "mixed-1".to_string(),
            label: "first".to_string(),
        },
    )
    .await?;
    JournalAppRow::insert(
        &database,
        CreateJournalAppRowInput {
            id: "app-1".to_string(),
            label: "first".to_string(),
        },
    )
    .await?;

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
        entry(
            "JournalMixed",
            "JournalMixed",
            "mixed-1",
            BackupChangeAction::Create,
        ),
        entry(
            "JournalAppRow",
            "JournalApp.Rows",
            "app-1",
            BackupChangeAction::Create,
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

/// The ORM check for `metadata` in a fresh transaction on `database`.
async fn recorder_check(
    database: &graphql_orm::db::Database,
    metadata: &'static graphql_orm::graphql::orm::EntityMetadata,
) -> Result<Result<(), String>, Box<dyn std::error::Error>> {
    let mut context = graphql_orm::graphql::orm::MutationContext::begin(
        database,
        graphql_orm::graphql::orm::TransactionMode::Default,
    )
    .await?;
    let outcome = context
        .check_change_journal_recorder(metadata)
        .await
        .map_err(|error| error.to_string());
    context.rollback().await?;
    Ok(outcome)
}

/// A decoy table literally named `"JournalMixed"` does not capture the
/// binding: the registry binds the declared name to the folded table the ORM
/// writes, and a tampered binding fails closed and is reported.
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn unquoted_names_bind_to_the_table_the_orm_writes() -> Result<(), Box<dyn std::error::Error>>
{
    use graphql_orm::graphql::orm::Entity;
    let owner = owned_postgres::OwnedPostgres::pool("postgres-journal-folding", 5).await?;
    sqlx::raw_sql("CREATE SCHEMA journal_app; CREATE SCHEMA journalapp")
        .execute(&owner)
        .await?;
    let database = graphql_orm::db::Database::builder(owner.clone())
        .change_journal_enabled(true)
        .build();
    let target = graphql_orm_schema_target();
    let plan = database
        .schema()
        .plan_schema_target("1", "journal folding", &target)
        .await?;
    database
        .schema()
        .apply_schema_target(&plan, ApplyOptions::default())
        .await?;
    // An owner-owned decoy with the literal mixed-case name, then the
    // recorder installed again with the decoy present.
    sqlx::query("CREATE TABLE public.\"JournalMixed\" (id text PRIMARY KEY, label text NOT NULL)")
        .execute(&owner)
        .await?;
    database
        .ensure_change_journal_recorder(&graphql_orm_entity_metadata())
        .await?;
    let on_decoy: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_trigger WHERE tgrelid = 'public.\"JournalMixed\"'::regclass AND tgname = 'graphql_orm_change_journal'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(on_decoy, 0, "the decoy carries no recorder trigger");
    let bound: String = sqlx::query_scalar(
        "SELECT relid::text FROM graphql_orm.change_journal_tables WHERE declared_table = 'JournalMixed'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(bound, "journalmixed");

    JournalMixed::insert(
        &database,
        CreateJournalMixedInput {
            id: "folded-1".to_string(),
            label: "x".to_string(),
        },
    )
    .await?;
    let journaled: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM __graphql_orm_change_log WHERE table_name = 'JournalMixed' AND primary_key = 'folded-1'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(journaled, 1);
    let report = database.schema().validate_target(&target).await?;
    let drift = report
        .rls_diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == SchemaDiagnosticSeverity::Error)
        .filter(|diagnostic| {
            diagnostic
                .message
                .to_ascii_lowercase()
                .contains("change journal")
        })
        .filter(|diagnostic| !diagnostic.message.contains("acts as the journal owner"))
        .collect::<Vec<_>>();
    assert!(drift.is_empty(), "{drift:#?}");

    // Moving the binding to the decoy fails closed and is reported.
    sqlx::query(
        "UPDATE graphql_orm.change_journal_tables SET relid = 'public.\"JournalMixed\"'::regclass WHERE declared_table = 'JournalMixed'",
    )
    .execute(&owner)
    .await?;
    let refused = recorder_check(&database, <JournalMixed as Entity>::metadata()).await?;
    assert!(
        matches!(&refused, Err(message) if message.contains("registry binding")),
        "{refused:?}"
    );
    let mut tx = owner.begin().await?;
    sqlx::query("SET LOCAL graphql_orm.change_journal = 'on'")
        .execute(&mut *tx)
        .await?;
    let raw = sqlx::query("INSERT INTO journalmixed (id, label) VALUES ('folded-2', 'x')")
        .execute(&mut *tx)
        .await;
    assert!(
        matches!(&raw, Err(error) if error.to_string().contains("does not match the table")),
        "{raw:?}"
    );
    drop(tx);
    let report = database.schema().validate_target(&target).await?;
    assert!(
        report
            .rls_diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.severity == SchemaDiagnosticSeverity::Error
                    && diagnostic
                        .message
                        .contains("the ORM's writes resolve it to")
            ),
        "{:#?}",
        report.rls_diagnostics
    );
    database
        .ensure_change_journal_recorder(&graphql_orm_entity_metadata())
        .await?;
    assert_eq!(
        recorder_check(&database, <JournalMixed as Entity>::metadata()).await?,
        Ok(())
    );
    Ok(())
}

/// A bare entity table name that resolves outside the journal's schema fails
/// when the recorder is installed, instead of binding there.
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn bare_names_outside_the_journal_schema_fail_at_install()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::Entity;
    let owner = owned_postgres::OwnedPostgres::pool("postgres-journal-far", 5).await?;
    sqlx::raw_sql(
        "CREATE SCHEMA journal_far; CREATE TABLE journal_far.journal_far_rows (id text PRIMARY KEY, note text NOT NULL)",
    )
    .execute(&owner)
    .await?;
    let options = owner
        .connect_options()
        .as_ref()
        .clone()
        .options([("search_path", "public,journal_far")]);
    let far = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    let refused = graphql_orm::db::Database::new(far)
        .ensure_change_journal_recorder(&[<JournalFarRow as Entity>::metadata()])
        .await;
    assert!(
        matches!(&refused, Err(error) if error.to_string().contains("a bare table name must resolve in the journal schema")),
        "{refused:?}"
    );
    let installed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_trigger WHERE tgname = 'graphql_orm_change_journal'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(installed, 0, "the failed install rolled back");
    Ok(())
}

/// The plan only drops stray recorder triggers on tables whose owner's
/// privileges the applying role has; validation reports the rest.
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn stray_triggers_on_other_owners_tables_are_reported_not_dropped()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{Entity, SchemaTarget};
    let owner = owned_postgres::OwnedPostgres::pool("postgres-journal-owners", 5).await?;
    let token = graphql_orm::uuid::Uuid::new_v4().simple().to_string();
    let database_name: String = sqlx::query_scalar("SELECT current_database()::text")
        .fetch_one(&owner)
        .await?;
    let connect = |role: &str| {
        owner
            .connect_options()
            .as_ref()
            .clone()
            .username(role)
            .password(&token)
    };
    sqlx::raw_sql(&format!(
        "CREATE ROLE journal_applier LOGIN NOSUPERUSER PASSWORD '{token}'; \
         CREATE ROLE journal_other LOGIN NOSUPERUSER PASSWORD '{token}'; \
         GRANT CREATE ON DATABASE \"{database_name}\" TO journal_applier; \
         GRANT USAGE, CREATE ON SCHEMA public TO journal_applier, journal_other"
    ))
    .execute(&owner)
    .await?;
    let applier = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(connect("journal_applier"))
        .await?;
    let other = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(connect("journal_other"))
        .await?;
    sqlx::query("CREATE TABLE journal_applied (id text PRIMARY KEY, note text NOT NULL)")
        .execute(&applier)
        .await?;
    let applier_database = graphql_orm::db::Database::new(applier.clone());
    let entities = [<JournalApplied as Entity>::metadata()];
    applier_database
        .ensure_change_journal_recorder(&entities)
        .await?;

    // Another role owns a table and attaches the recorder under another name.
    sqlx::raw_sql(
        "GRANT USAGE ON SCHEMA graphql_orm TO journal_other; \
         GRANT EXECUTE ON FUNCTION graphql_orm.record_change_journal_entry() TO journal_other",
    )
    .execute(&applier)
    .await?;
    sqlx::raw_sql(
        "CREATE TABLE other_rows (id text PRIMARY KEY); \
         CREATE TRIGGER stray_copy AFTER INSERT ON other_rows FOR EACH ROW \
         EXECUTE FUNCTION graphql_orm.record_change_journal_entry('public', 'public', 'other_rows', 'JournalApplied', 'journal_applied', 'id', 'text')",
    )
    .execute(&other)
    .await?;

    // Re-installing as the applier succeeds and leaves that trigger alone.
    applier_database
        .ensure_change_journal_recorder(&entities)
        .await?;
    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_trigger WHERE tgrelid = 'other_rows'::regclass AND tgname = 'stray_copy'",
    )
    .fetch_one(&owner)
    .await?;
    assert_eq!(remaining, 1);
    let target = SchemaTarget::from_entities(&entities, &[None]);
    let report = applier_database.schema().validate_target(&target).await?;
    assert!(
        report
            .rls_diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.severity == SchemaDiagnosticSeverity::Error
                    && diagnostic.message.contains("stray_copy")
            ),
        "{:#?}",
        report.rls_diagnostics
    );
    Ok(())
}
