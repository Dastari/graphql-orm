// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres"))]

#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use async_graphql::SimpleObject;
#[cfg(feature = "change-journal")]
use graphql_orm::graphql::orm::{BackupChangeAction, ChangeWindow};
use graphql_orm::graphql::orm::{
    Entity, GraphqlOrmBackupRuntime, Migration, MigrationRunner, RestoreContext,
    build_migration_plan,
};
use graphql_orm::prelude::*;
use graphql_orm::rust_decimal::Decimal;
use sqlx::Row;
use std::collections::BTreeMap;

#[derive(SimpleObject, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Default)]
struct BackupProfile {
    display_name: String,
    flags: Vec<String>,
}

#[cfg(feature = "change-journal")]
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn change_journal_records_generated_repository_writes()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone()).with_change_journal();
    apply_schema(&database, "2026051403_backup_change_journal").await?;
    database
        .ensure_change_journal_recorder(&graphql_orm_entity_metadata())
        .await?;

    let account = BackupRtAccount::insert(
        &database,
        CreateBackupRtAccountInput {
            email: "journal@example.com".to_string(),
            password_hash: "secret-hash".to_string(),
            reset_token: None,
            profile: BackupProfile {
                display_name: "Journal".to_string(),
                flags: vec![],
            },
            preferences: None,
            avatar: vec![9, 8, 7],
            nickname: None,
            balance: Decimal::new(12345, 2),
            credit_limit: None,
        },
    )
    .await?;

    let until = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let changes = database
        .export_changes(ChangeWindow {
            after_sequence: None,
            until,
        })
        .await?;

    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].entity_name, "BackupRtAccount");
    assert_eq!(changes[0].table_name, "backup_rt_accounts");
    assert_eq!(changes[0].primary_key, account.id.to_string());
    assert_eq!(changes[0].action, BackupChangeAction::Create);
    Ok(())
}

/// Restore imports never journal, even while the database-side recorder is
/// installed and the target database journals its own writes.
#[cfg(feature = "change-journal")]
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn restore_does_not_journal_while_writes_do() -> Result<(), Box<dyn std::error::Error>> {
    let source_pool = setup_pool().await?;
    let source = graphql_orm::db::Database::new(source_pool.clone());
    apply_schema(&source, "2026051404_backup_journal_source").await?;
    let account = BackupRtAccount::insert(
        &source,
        CreateBackupRtAccountInput {
            email: "restore@example.com".to_string(),
            password_hash: "secret-hash".to_string(),
            reset_token: None,
            profile: BackupProfile::default(),
            preferences: None,
            avatar: vec![1],
            nickname: None,
            balance: Decimal::new(1, 0),
            credit_limit: None,
        },
    )
    .await?;
    let snapshot_schema =
        source.schema_snapshot("2026051404_backup_journal", &graphql_orm_entity_metadata());
    let mut snapshot = source.begin_consistent_snapshot().await?;
    let mut rows_by_table = BTreeMap::new();
    for descriptor in source.list_backup_entities(&graphql_orm_entity_metadata()) {
        let rows = source.export_table_rows(&mut snapshot, &descriptor).await?;
        rows_by_table.insert(descriptor.table_name, rows);
    }
    drop(snapshot);

    let dest_pool = setup_pool().await?;
    let dest = graphql_orm::db::Database::new(dest_pool.clone()).with_change_journal();
    apply_schema(&dest, "2026051405_backup_journal_dest").await?;
    dest.ensure_change_journal_recorder(&graphql_orm_entity_metadata())
        .await?;
    // A role default that turns recording on for every session does not make
    // restore journal: restore switches it off for its own transactions.
    #[cfg(feature = "postgres")]
    let (dest_pool, dest) = {
        sqlx::query("ALTER ROLE CURRENT_USER SET graphql_orm.change_journal = 'on'")
            .execute(&dest_pool)
            .await?;
        let fresh = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect_with(dest_pool.connect_options().as_ref().clone())
            .await?;
        let enabled: String =
            sqlx::query_scalar("SELECT current_setting('graphql_orm.change_journal')")
                .fetch_one(&fresh)
                .await?;
        assert_eq!(enabled, "on");
        let database = graphql_orm::db::Database::new(fresh.clone()).with_change_journal();
        (fresh, database)
    };
    let _ = &dest_pool;
    let dest_schema =
        dest.schema_snapshot("2026051404_backup_journal", &graphql_orm_entity_metadata());
    dest.restore_backup_rows(
        &snapshot_schema,
        &dest_schema,
        &rows_by_table,
        &RestoreContext::empty_database(),
    )
    .await?;
    let window = ChangeWindow {
        after_sequence: None,
        until: i64::MAX,
    };
    assert!(dest.export_changes(window.clone()).await?.is_empty());

    let note = BackupRtNote::insert(
        &dest,
        CreateBackupRtNoteInput {
            account_id: account.id,
            body: "after restore".to_string(),
        },
    )
    .await?;
    let changes = dest.export_changes(window).await?;
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(changes[0].entity_name, "BackupRtNote");
    assert_eq!(changes[0].primary_key, note.id.to_string());
    assert_eq!(changes[0].action, BackupChangeAction::Create);
    Ok(())
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "backup_rt_accounts",
    plural = "BackupRtAccounts",
    default_sort = "email ASC"
)]
struct BackupRtAccount {
    #[primary_key]
    #[filterable(type = "uuid")]
    pub id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub email: String,

    #[backup(redact)]
    pub password_hash: String,

    #[backup(exclude)]
    pub reset_token: Option<String>,

    #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
    pub profile: BackupProfile,

    #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
    pub preferences: Option<BackupProfile>,

    pub avatar: Vec<u8>,

    pub nickname: Option<String>,

    #[filterable(type = "decimal")]
    #[sortable]
    #[graphql_orm(decimal(precision = 12, scale = 2))]
    pub balance: Decimal,

    #[filterable(type = "decimal")]
    #[sortable]
    #[graphql_orm(decimal(precision = 12, scale = 2))]
    pub credit_limit: Option<Decimal>,

    #[sortable]
    pub created_at: i64,

    #[sortable]
    pub updated_at: i64,
}

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql_entity(
    table = "backup_rt_notes",
    plural = "BackupRtNotes",
    default_sort = "body ASC"
)]
#[graphql(complex)]
struct BackupRtNote {
    #[primary_key]
    #[filterable(type = "string")]
    pub id: String,

    #[filterable(type = "uuid")]
    pub account_id: graphql_orm::uuid::Uuid,

    #[filterable(type = "string")]
    #[sortable]
    pub body: String,

    #[graphql(skip)]
    #[relation(target = "BackupRtAccount", from = "account_id", to = "id")]
    pub account: Option<BackupRtAccount>,
}

impl graphql_orm::graphql::loaders::BatchLoadEntity for BackupRtAccount {
    fn batch_column() -> &'static str {
        "id"
    }

    #[cfg(feature = "sqlite")]
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        row.try_get("id")
    }

    #[cfg(feature = "postgres")]
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, sqlx::Error> {
        let id: graphql_orm::uuid::Uuid = row.try_get("id")?;
        Ok(id.to_string())
    }
}

schema_roots! {
    query_custom_ops: [],
    entities: [BackupRtAccount, BackupRtNote],
}

#[cfg(feature = "sqlite")]
type TestPool = sqlx::SqlitePool;
#[cfg(feature = "postgres")]
type TestPool = sqlx::PgPool;

#[cfg(feature = "sqlite")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    Ok(sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?)
}

#[cfg(feature = "postgres")]
async fn setup_pool() -> Result<TestPool, Box<dyn std::error::Error>> {
    let pool = owned_postgres::OwnedPostgres::pool("backup-roundtrip", 5).await?;
    sqlx::query("DROP TABLE IF EXISTS backup_rt_notes")
        .execute(&pool)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS backup_rt_accounts")
        .execute(&pool)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS __graphql_orm_migrations")
        .execute(&pool)
        .await?;
    Ok(pool)
}

async fn apply_schema(
    database: &graphql_orm::db::Database,
    version: &'static str,
) -> Result<(), Box<dyn std::error::Error>> {
    let target_schema = graphql_orm::graphql::orm::SchemaModel::from_entities(&[
        <BackupRtAccount as Entity>::metadata(),
        <BackupRtNote as Entity>::metadata(),
    ]);
    let plan = build_migration_plan(
        if cfg!(feature = "postgres") {
            graphql_orm::graphql::orm::DatabaseBackend::Postgres
        } else {
            graphql_orm::graphql::orm::DatabaseBackend::Sqlite
        },
        &graphql_orm::graphql::orm::SchemaModel {
            extensions: Vec::new(),
            tables: Vec::new(),
        },
        &target_schema,
    );
    let statements: &'static [&'static str] = Box::leak(
        plan.statements
            .iter()
            .map(|statement| Box::leak(statement.clone().into_boxed_str()) as &'static str)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    database
        .apply_migrations(&[Migration {
            version,
            description: "backup_roundtrip_schema",
            statements,
        }])
        .await?;
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn full_logical_backup_restores_into_empty_database() -> Result<(), Box<dyn std::error::Error>>
{
    let source_pool = setup_pool().await?;
    let source = graphql_orm::db::Database::new(source_pool.clone());
    apply_schema(&source, "2026051401_backup_roundtrip_source").await?;

    let account = BackupRtAccount::insert(
        &source_pool,
        CreateBackupRtAccountInput {
            email: "ada@example.com".to_string(),
            password_hash: "secret-hash".to_string(),
            reset_token: Some("one-time".to_string()),
            profile: BackupProfile {
                display_name: "Ada".to_string(),
                flags: vec!["admin".to_string(), "research".to_string()],
            },
            preferences: None,
            avatar: vec![0, 1, 2, 3, 255],
            nickname: Some("countess".to_string()),
            balance: Decimal::new(987654, 2),
            credit_limit: Some(Decimal::new(2500000, 2)),
        },
    )
    .await?;
    let note = BackupRtNote::insert(
        &source_pool,
        CreateBackupRtNoteInput {
            account_id: account.id,
            body: "first note".to_string(),
        },
    )
    .await?;

    let source_snapshot = source.schema_snapshot(
        "2026051401_backup_roundtrip",
        &graphql_orm_entity_metadata(),
    );
    assert!(!source_snapshot.schema_hash.is_empty());

    let mut descriptors = source.list_backup_entities(&graphql_orm_entity_metadata());
    descriptors.sort_by_key(|descriptor| (descriptor.export_order, descriptor.table_name.clone()));
    let mut snapshot = source.begin_consistent_snapshot().await?;
    let mut exported = Vec::new();
    for descriptor in &descriptors {
        exported.push((
            descriptor.clone(),
            source.export_table_rows(&mut snapshot, descriptor).await?,
        ));
    }
    drop(snapshot);

    let dest_pool = setup_pool().await?;
    let dest = graphql_orm::db::Database::new(dest_pool.clone());
    apply_schema(&dest, "2026051402_backup_roundtrip_dest").await?;

    let rows_by_table = exported
        .into_iter()
        .map(|(descriptor, rows)| (descriptor.table_name, rows))
        .collect::<BTreeMap<_, _>>();
    let dest_snapshot = dest.schema_snapshot(
        "2026051401_backup_roundtrip",
        &graphql_orm_entity_metadata(),
    );
    let reports = dest
        .restore_backup_rows(
            &source_snapshot,
            &dest_snapshot,
            &rows_by_table,
            &RestoreContext::empty_database(),
        )
        .await?;
    for report in &reports {
        let expected = rows_by_table
            .get(&report.table_name)
            .map(Vec::len)
            .unwrap_or(0);
        assert_eq!(report.rows_imported, expected);
        assert_eq!(report.rows_validated, expected);
    }

    let restored_account = BackupRtAccount::get(&dest_pool, &account.id)
        .await?
        .expect("account should restore");
    assert_eq!(restored_account.email, account.email);
    assert_eq!(restored_account.password_hash, "[graphql-orm:redacted]");
    assert_eq!(restored_account.reset_token, None);
    assert_eq!(restored_account.profile, account.profile);
    assert_eq!(restored_account.preferences, None);
    assert_eq!(restored_account.avatar, account.avatar);
    assert_eq!(restored_account.nickname, account.nickname);
    assert_eq!(restored_account.balance, account.balance);
    assert_eq!(restored_account.credit_limit, account.credit_limit);
    assert_eq!(restored_account.created_at, account.created_at);
    assert_eq!(restored_account.updated_at, account.updated_at);

    let restored_note = BackupRtNote::get(&dest_pool, &note.id)
        .await?
        .expect("note should restore");
    assert_eq!(restored_note.id, note.id);
    assert_eq!(restored_note.account_id, account.id);
    assert_eq!(restored_note.body, note.body);

    Ok(())
}

/// Single-column keys are unchanged by composite-key support: the schema hash and the serialized
/// descriptors (the manifest's schema member) are the values graphql-orm 0.70.1 produced.
#[test]
fn single_key_schema_hash_and_descriptors_are_unchanged_from_0_70() {
    use sha2::Digest;
    let snapshot = graphql_orm::graphql::orm::schema_snapshot_from_entities(
        graphql_orm::graphql::orm::DatabaseBackend::Sqlite,
        "pinned",
        &graphql_orm_entity_metadata(),
    );
    let manifest = serde_json::to_string(&snapshot.entities).unwrap();
    let digest: String = sha2::Sha256::digest(manifest.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(snapshot.schema_hash, "19ca3667d8a87bfe");
    assert_eq!(
        digest,
        "68078e4ff204b83be3f3ad501dda9c8399eb9e13858af3427560d46b2e5bd07b"
    );
    assert!(!manifest.contains("primary_key_columns"));
}

#[cfg(feature = "change-journal")]
async fn journaled_database(
    version: &'static str,
) -> Result<(graphql_orm::db::Database, BackupRtAccount), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool).with_change_journal();
    apply_schema(&database, version).await?;
    database
        .ensure_change_journal_recorder(&graphql_orm_entity_metadata())
        .await?;
    let account = BackupRtAccount::insert(
        &database,
        CreateBackupRtAccountInput {
            email: "sequence@example.com".to_string(),
            password_hash: "secret-hash".to_string(),
            reset_token: None,
            profile: BackupProfile {
                display_name: "Sequence".to_string(),
                flags: vec![],
            },
            preferences: None,
            avatar: vec![1],
            nickname: None,
            balance: Decimal::new(100, 2),
            credit_limit: None,
        },
    )
    .await?;
    Ok((database, account))
}

#[cfg(feature = "change-journal")]
fn note(account: &BackupRtAccount, body: impl Into<String>) -> CreateBackupRtNoteInput {
    CreateBackupRtNoteInput {
        account_id: account.id,
        body: body.into(),
    }
}

#[cfg(feature = "change-journal")]
fn window(after_sequence: Option<i64>) -> ChangeWindow {
    ChangeWindow {
        after_sequence,
        until: i64::MAX,
    }
}

/// #591: journal ids are random, so they cannot mark a position. Consecutive
/// incremental exports, each continuing from the last entry's sequence, return
/// every entry exactly once and in the order it was written, even when many
/// entries share one `changed_at` second.
#[cfg(feature = "change-journal")]
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn incremental_exports_return_every_entry_exactly_once_in_order()
-> Result<(), Box<dyn std::error::Error>> {
    let (database, account) = journaled_database("2026111501_backup_journal_sequence").await?;
    let mut written = vec![account.id.to_string()];
    let mut exported = Vec::new();
    let mut position = None;
    // Three bursts, each exported incrementally. Every burst takes well
    // under a second, so most entries share a `changed_at` value.
    for burst in 0..3 {
        for index in 0..40 {
            let created =
                BackupRtNote::insert(&database, note(&account, format!("{burst}-{index}"))).await?;
            written.push(created.id);
        }
        let changes = database.export_changes(window(position)).await?;
        assert!(!changes.is_empty());
        position = changes.last().map(|change| change.sequence);
        exported.extend(changes);
        // Nothing new: the position is the end of the journal.
        assert!(database.export_changes(window(position)).await?.is_empty());
    }
    assert_eq!(
        exported
            .iter()
            .map(|change| change.primary_key.clone())
            .collect::<Vec<_>>(),
        written,
        "every entry once, in write order"
    );
    assert!(
        exported
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    let ids = exported
        .iter()
        .map(|change| change.id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), exported.len());
    let seconds = exported
        .iter()
        .map(|change| change.changed_at)
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        seconds.len() < exported.len(),
        "entries must share a second for this test to mean anything"
    );
    // One export of the whole journal returns the same run.
    assert_eq!(database.export_changes(window(None)).await?, exported);

    // `until` ends the run at the first later entry instead of skipping it,
    // so an export that returns nothing leaves nothing behind.
    let before_everything = ChangeWindow {
        after_sequence: None,
        until: exported[0].changed_at - 1,
    };
    assert!(database.export_changes(before_everything).await?.is_empty());
    let up_to_first = ChangeWindow {
        after_sequence: None,
        until: exported[0].changed_at,
    };
    let first_second = database.export_changes(up_to_first).await?;
    assert!(!first_second.is_empty());
    assert_eq!(first_second, exported[..first_second.len()]);
    Ok(())
}

/// #591: a journal created by an earlier release has no sequence. Export
/// refuses it rather than ordering by random id, and preparing the journal
/// again numbers the existing entries in the order they were written.
#[cfg(feature = "change-journal")]
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn a_journal_without_a_sequence_is_refused_then_upgraded()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone()).with_change_journal();
    apply_schema(&database, "2026111502_backup_journal_upgrade").await?;
    // The journal as releases before 0.157.0 created it.
    let (id_type, integer) = if cfg!(feature = "postgres") {
        ("UUID", "BIGINT")
    } else {
        ("TEXT", "INTEGER")
    };
    sqlx::query(&format!(
        "CREATE TABLE \"__graphql_orm_change_log\" (
            id {id_type} PRIMARY KEY,
            entity_name TEXT NOT NULL,
            table_name TEXT NOT NULL,
            primary_key TEXT NOT NULL,
            action TEXT NOT NULL,
            changed_at {integer} NOT NULL,
            transaction_id TEXT NULL,
            row_hash TEXT NULL,
            actor_id TEXT NULL,
            correlation_id TEXT NULL
        )"
    ))
    .execute(&pool)
    .await?;
    // Random ids in descending order, written in ascending time order.
    let cast = if cfg!(feature = "postgres") {
        "::uuid"
    } else {
        ""
    };
    for (id, key, changed_at) in [
        ("ffffffff-0000-4000-8000-000000000001", "first", 100),
        ("88888888-0000-4000-8000-000000000002", "second", 100),
        ("11111111-0000-4000-8000-000000000003", "third", 200),
    ] {
        sqlx::query(&format!(
            "INSERT INTO \"__graphql_orm_change_log\" (id, entity_name, table_name, primary_key, action, changed_at) \
             VALUES ('{id}'{cast}, 'BackupRtNote', 'backup_rt_notes', '{key}', 'create', {changed_at})"
        ))
        .execute(&pool)
        .await?;
    }
    let refused = database
        .export_changes(window(None))
        .await
        .expect_err("a journal without a sequence cannot be exported in order");
    assert!(refused.to_string().contains("seq"), "{refused}");

    database
        .ensure_change_journal_recorder(&graphql_orm_entity_metadata())
        .await?;
    // Preparing an upgraded journal again changes nothing.
    database.ensure_change_journal_table().await?;
    let upgraded = database.export_changes(window(None)).await?;
    assert_eq!(
        upgraded
            .iter()
            .map(|change| (change.sequence, change.primary_key.as_str()))
            .collect::<Vec<_>>(),
        [(1, "first"), (2, "second"), (3, "third")]
    );

    // New entries continue after the numbered ones.
    let account = BackupRtAccount::insert(
        &database,
        CreateBackupRtAccountInput {
            email: "upgrade@example.com".to_string(),
            password_hash: "secret-hash".to_string(),
            reset_token: None,
            profile: BackupProfile {
                display_name: "Upgrade".to_string(),
                flags: vec![],
            },
            preferences: None,
            avatar: vec![1],
            nickname: None,
            balance: Decimal::new(100, 2),
            credit_limit: None,
        },
    )
    .await?;
    let next = database.export_changes(window(Some(3))).await?;
    assert_eq!(next.len(), 1, "{next:?}");
    assert_eq!(next[0].sequence, 4);
    assert_eq!(next[0].primary_key, account.id.to_string());
    Ok(())
}

/// #591 on PostgreSQL: a position is assigned before its transaction commits.
/// An export must not return a later position while an earlier one is still
/// uncommitted, or the next incremental export would skip it.
#[cfg(all(feature = "change-journal", feature = "postgres"))]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn export_waits_for_journaled_transactions_that_hold_earlier_positions()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{ChangeExportTimedOut, TransactionMode};
    use std::sync::Arc;

    let (database, account) = journaled_database("2026111503_backup_journal_open").await?;
    let position = database
        .export_changes(window(None))
        .await?
        .last()
        .map(|change| change.sequence);

    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let held = {
        let (database, started, release) = (database.clone(), started.clone(), release.clone());
        let input = note(&account, "held");
        tokio::spawn(async move {
            database
                .transaction(TransactionMode::Default, |context| {
                    Box::pin(async move {
                        let created = context.insert::<BackupRtNote>(input).await?;
                        started.notify_one();
                        release.notified().await;
                        Ok(created.id)
                    })
                })
                .await
        })
    };
    started.notified().await;
    // A later position commits while the earlier one is still open.
    let later = BackupRtNote::insert(&database, note(&account, "later")).await?;

    // A bounded export gives up without returning the later entry.
    let timed_out = database
        .export_changes_with_wait(window(position), std::time::Duration::from_millis(200))
        .await
        .expect_err("the open transaction holds an earlier position");
    assert!(
        ChangeExportTimedOut::from_error(&timed_out).is_some(),
        "{timed_out:?}"
    );

    // An unbounded export waits for the open transaction, then returns both
    // entries in position order.
    let export = {
        let database = database.clone();
        tokio::spawn(async move { database.export_changes(window(position)).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert!(!export.is_finished(), "the export must wait");
    release.notify_one();
    let held_id = held.await??;
    let changes = export.await??;
    assert_eq!(
        changes
            .iter()
            .map(|change| change.primary_key.clone())
            .collect::<Vec<_>>(),
        [held_id, later.id]
    );
    let position = changes.last().map(|change| change.sequence);

    // A rolled-back transaction leaves a gap in the positions and nothing to
    // export; later entries still follow.
    let rolled_back = database
        .transaction(TransactionMode::Default, |context| {
            let input = note(&account, "rolled back");
            Box::pin(async move {
                context.insert::<BackupRtNote>(input).await?;
                Err::<(), _>(OrmPublicError::new(OrmErrorCode::Conflict))
            })
        })
        .await;
    assert!(rolled_back.is_err());
    assert!(database.export_changes(window(position)).await?.is_empty());
    let after = BackupRtNote::insert(&database, note(&account, "after")).await?;
    let changes = database.export_changes(window(position)).await?;
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(changes[0].primary_key, after.id);
    assert!(changes[0].sequence > position.unwrap() + 1);
    Ok(())
}

/// The SQLite journal as releases before the sequence column created it,
/// with three entries.
#[cfg(all(feature = "change-journal", feature = "sqlite"))]
async fn legacy_sqlite_journal(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE \"__graphql_orm_change_log\" (
            id TEXT PRIMARY KEY,
            entity_name TEXT NOT NULL,
            table_name TEXT NOT NULL,
            primary_key TEXT NOT NULL,
            action TEXT NOT NULL,
            changed_at INTEGER NOT NULL,
            transaction_id TEXT NULL,
            row_hash TEXT NULL,
            actor_id TEXT NULL,
            correlation_id TEXT NULL
        )",
    )
    .execute(pool)
    .await?;
    for (id, key, changed_at) in [
        ("ffffffff-0000-4000-8000-000000000001", "first", 100),
        ("88888888-0000-4000-8000-000000000002", "second", 100),
        ("11111111-0000-4000-8000-000000000003", "third", 200),
    ] {
        sqlx::query(&format!(
            "INSERT INTO \"__graphql_orm_change_log\" (id, entity_name, table_name, primary_key, action, changed_at) \
             VALUES ('{id}', 'BackupRtNote', 'backup_rt_notes', '{key}', 'create', {changed_at})"
        ))
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Assert that a refused, failed or cancelled upgrade left nothing behind:
/// the next use of the pooled connection is outside any transaction, the
/// journal is the legacy table with its three entries, and no rebuilt table
/// exists.
#[cfg(all(feature = "change-journal", feature = "sqlite"))]
async fn assert_legacy_sqlite_journal_untouched(
    pool: &sqlx::SqlitePool,
) -> Result<(), sqlx::Error> {
    // Fails with "cannot start a transaction within a transaction" when the
    // connection came back to the pool inside the upgrade's transaction.
    let mut connection = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await?;
    sqlx::query("ROLLBACK").execute(&mut *connection).await?;
    drop(connection);
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('__graphql_orm_change_log')")
            .fetch_all(pool)
            .await?;
    assert!(!columns.is_empty());
    assert!(!columns.iter().any(|column| column == "seq"), "{columns:?}");
    let keys: Vec<String> =
        sqlx::query_scalar("SELECT primary_key FROM \"__graphql_orm_change_log\" ORDER BY rowid")
            .fetch_all(pool)
            .await?;
    assert_eq!(keys, ["first", "second", "third"]);
    Ok(())
}

#[cfg(all(feature = "change-journal", feature = "sqlite"))]
async fn assert_upgraded_sqlite_journal(
    database: &graphql_orm::db::Database,
) -> Result<(), Box<dyn std::error::Error>> {
    let upgraded = database.export_changes(window(None)).await?;
    assert_eq!(
        upgraded
            .iter()
            .map(|change| (change.sequence, change.primary_key.as_str()))
            .collect::<Vec<_>>(),
        [(1, "first"), (2, "second"), (3, "third")]
    );
    let leftovers: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE name = '__graphql_orm_change_log_sequenced'",
    )
    .fetch_one(database.pool())
    .await?;
    assert_eq!(leftovers, 0);
    Ok(())
}

/// The upgrade needs a table name for the rebuilt journal. A table that
/// already has that name is not the upgrade's to drop: the upgrade refuses
/// and leaves that table, the journal and their rows alone.
#[cfg(all(feature = "change-journal", feature = "sqlite"))]
#[tokio::test]
async fn journal_upgrade_refuses_an_existing_table_with_the_rebuild_name()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::ChangeJournalUpgradeConflict;

    let pool = setup_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone()).with_change_journal();
    legacy_sqlite_journal(&pool).await?;
    sqlx::query("CREATE TABLE \"__graphql_orm_change_log_sequenced\" (id TEXT PRIMARY KEY, note TEXT NOT NULL)")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO \"__graphql_orm_change_log_sequenced\" VALUES ('kept', 'host data')")
        .execute(&pool)
        .await?;
    let definition =
        "SELECT sql FROM sqlite_master WHERE name = '__graphql_orm_change_log_sequenced'";
    let before: String = sqlx::query_scalar(definition).fetch_one(&pool).await?;

    for _ in 0..2 {
        let refused = database
            .ensure_change_journal_table()
            .await
            .expect_err("the rebuild name is taken");
        let conflict = ChangeJournalUpgradeConflict::from_error(&refused)
            .unwrap_or_else(|| panic!("{refused:?}"));
        assert_eq!(conflict.name, "__graphql_orm_change_log_sequenced");
        let after: String = sqlx::query_scalar(definition).fetch_one(&pool).await?;
        assert_eq!(after, before);
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT id, note FROM \"__graphql_orm_change_log_sequenced\"")
                .fetch_all(&pool)
                .await?;
        assert_eq!(rows, [("kept".into(), "host data".into())]);
        assert_legacy_sqlite_journal_untouched(&pool).await?;
    }

    // Once the name is free the upgrade runs, and the host's table keeps its
    // rows under its new name.
    sqlx::query("ALTER TABLE \"__graphql_orm_change_log_sequenced\" RENAME TO host_kept")
        .execute(&pool)
        .await?;
    database.ensure_change_journal_table().await?;
    assert_upgraded_sqlite_journal(&database).await?;
    let kept: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM host_kept")
        .fetch_one(&pool)
        .await?;
    assert_eq!(kept, 1);
    Ok(())
}

/// A one-connection pool on a rollback-journal database file in a fresh
/// temporary directory. A file survives the pool replacing its connection,
/// which an in-memory database does not.
#[cfg(all(feature = "change-journal", feature = "sqlite"))]
async fn file_sqlite_pool() -> Result<
    (
        std::path::PathBuf,
        sqlx::sqlite::SqliteConnectOptions,
        sqlx::SqlitePool,
    ),
    Box<dyn std::error::Error>,
> {
    use std::str::FromStr;
    let directory = std::env::temp_dir().join(format!(
        "graphql-orm-journal-upgrade-{}",
        graphql_orm::uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir(&directory)?;
    let options = sqlx::sqlite::SqliteConnectOptions::from_str(&format!(
        "sqlite://{}",
        directory.join("journal.db").display()
    ))?
    .create_if_missing(true)
    .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete)
    .busy_timeout(std::time::Duration::from_millis(100));
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await?;
    Ok((directory, options, pool))
}

/// A commit that fails must not hand the pool a connection that is still
/// inside the upgrade's transaction. A reader holding a shared lock on a
/// rollback-journal database makes the upgrade's `COMMIT` fail.
#[cfg(all(feature = "change-journal", feature = "sqlite"))]
#[tokio::test]
async fn journal_upgrade_rolls_back_when_its_commit_fails() -> Result<(), Box<dyn std::error::Error>>
{
    use sqlx::ConnectOptions;

    let (directory, options, pool) = file_sqlite_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone()).with_change_journal();
    legacy_sqlite_journal(&pool).await?;

    // A second connection reads inside an open transaction: it holds a
    // shared lock, which lets the upgrade write but not commit.
    let mut reader = options.connect().await?;
    sqlx::query("BEGIN").execute(&mut reader).await?;
    let seen: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM \"__graphql_orm_change_log\"")
        .fetch_one(&mut reader)
        .await?;
    assert_eq!(seen, 3);

    let failed = database
        .ensure_change_journal_table()
        .await
        .expect_err("the reader's lock refuses the commit");
    assert!(
        failed.to_string().contains("locked") || failed.to_string().contains("busy"),
        "{failed:?}"
    );
    // The single pooled connection is reused here, while the lock is held.
    assert_legacy_sqlite_journal_untouched(&pool).await?;
    let leftovers: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE name = '__graphql_orm_change_log_sequenced'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        leftovers, 0,
        "uncommitted DDL is visible on the pooled connection"
    );
    let refused = database.export_changes(window(None)).await;
    assert!(refused.is_err());

    // With the lock gone the retry succeeds.
    sqlx::query("ROLLBACK").execute(&mut reader).await?;
    drop(reader);
    database.ensure_change_journal_table().await?;
    assert_upgraded_sqlite_journal(&database).await?;
    pool.close().await;
    std::fs::remove_dir_all(&directory)?;
    Ok(())
}

/// Poll `future` at most `polls` times, then drop it. Returns its output when
/// it finished first.
#[cfg(all(feature = "change-journal", feature = "sqlite"))]
async fn drop_after<F: std::future::Future>(polls: usize, future: F) -> Option<F::Output> {
    let mut future = std::pin::pin!(future);
    let mut remaining = polls;
    std::future::poll_fn(move |context| {
        if remaining == 0 {
            return std::task::Poll::Ready(None);
        }
        remaining -= 1;
        match future.as_mut().poll(context) {
            std::task::Poll::Ready(output) => std::task::Poll::Ready(Some(output)),
            std::task::Poll::Pending => {
                if remaining == 0 {
                    std::task::Poll::Ready(None)
                } else {
                    std::task::Poll::Pending
                }
            }
        }
    })
    .await
}

/// Cancelling the upgrade at any await point rolls it back: the pooled
/// connection comes back outside a transaction and without partial DDL.
#[cfg(all(feature = "change-journal", feature = "sqlite"))]
#[tokio::test]
async fn journal_upgrade_rolls_back_when_it_is_cancelled() -> Result<(), Box<dyn std::error::Error>>
{
    let (directory, _, pool) = file_sqlite_pool().await?;
    let database = graphql_orm::db::Database::new(pool.clone()).with_change_journal();
    legacy_sqlite_journal(&pool).await?;
    let mut cancelled = 0;
    let mut finished = false;
    for polls in 1..200 {
        match drop_after(polls, database.ensure_change_journal_table()).await {
            Some(result) => {
                result?;
                finished = true;
                break;
            }
            None => {
                cancelled += 1;
                // A cancelled upgrade is all or nothing. The commit may
                // already have been running when the call was dropped; then
                // the journal is completely upgraded. Otherwise it is
                // completely untouched. Either way the pooled connection is
                // outside any transaction and no rebuilt table is left.
                let leftovers: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name = '__graphql_orm_change_log_sequenced'",
                )
                .fetch_one(&pool)
                .await?;
                assert_eq!(leftovers, 0, "cancelled after {polls} polls");
                let sequenced: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM pragma_table_info('__graphql_orm_change_log') WHERE name = 'seq'",
                )
                .fetch_one(&pool)
                .await?;
                if sequenced == 1 {
                    let mut connection = pool.acquire().await?;
                    sqlx::query("BEGIN IMMEDIATE")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("ROLLBACK").execute(&mut *connection).await?;
                    finished = true;
                    break;
                }
                assert_legacy_sqlite_journal_untouched(&pool).await?;
            }
        }
    }
    assert!(finished, "the upgrade never completed");
    assert!(
        cancelled >= 5,
        "only {cancelled} cancellation points were exercised"
    );
    assert_upgraded_sqlite_journal(&database).await?;
    pool.close().await;
    std::fs::remove_dir_all(&directory)?;
    Ok(())
}

/// A snapshot restores in one transaction: when a later table fails in the
/// database, the tables imported before it are rolled back too, so the target
/// is still empty and the restore can simply be retried.
#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn a_failed_table_rolls_back_the_whole_snapshot_restore()
-> Result<(), Box<dyn std::error::Error>> {
    let source_pool = setup_pool().await?;
    let source = graphql_orm::db::Database::new(source_pool.clone());
    apply_schema(&source, "2026111501_backup_atomic_source").await?;
    let account = BackupRtAccount::insert(
        &source_pool,
        CreateBackupRtAccountInput {
            email: "atomic@example.com".to_string(),
            password_hash: "secret-hash".to_string(),
            reset_token: None,
            profile: BackupProfile::default(),
            preferences: None,
            avatar: vec![1],
            nickname: None,
            balance: Decimal::new(1, 0),
            credit_limit: None,
        },
    )
    .await?;
    BackupRtNote::insert(
        &source_pool,
        CreateBackupRtNoteInput {
            account_id: account.id,
            body: "atomic".to_string(),
        },
    )
    .await?;
    let schema_snapshot =
        source.schema_snapshot("2026111501_backup_atomic", &graphql_orm_entity_metadata());
    let mut snapshot = source.begin_consistent_snapshot().await?;
    let mut rows_by_table = BTreeMap::new();
    for descriptor in source.list_backup_entities(&graphql_orm_entity_metadata()) {
        let rows = source.export_table_rows(&mut snapshot, &descriptor).await?;
        rows_by_table.insert(descriptor.table_name, rows);
    }
    drop(snapshot);

    let dest_pool = setup_pool().await?;
    let dest = graphql_orm::db::Database::new(dest_pool.clone());
    apply_schema(&dest, "2026111502_backup_atomic_dest").await?;
    // The notes table restores after the accounts table. Its rows pass every
    // check made before the first write and are then refused by the database.
    #[cfg(feature = "sqlite")]
    let refuse: &[&str] = &[
        "CREATE TRIGGER backup_rt_notes_refuse BEFORE INSERT ON backup_rt_notes BEGIN SELECT RAISE(ABORT, 'notes are refused'); END",
    ];
    #[cfg(feature = "postgres")]
    let refuse: &[&str] = &[
        "CREATE OR REPLACE FUNCTION backup_rt_notes_refuse() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'notes are refused'; END $$",
        "CREATE TRIGGER backup_rt_notes_refuse BEFORE INSERT ON backup_rt_notes FOR EACH ROW EXECUTE FUNCTION backup_rt_notes_refuse()",
    ];
    for statement in refuse {
        sqlx::query(statement).execute(&dest_pool).await?;
    }

    let error = dest
        .restore_backup_rows(
            &schema_snapshot,
            &schema_snapshot,
            &rows_by_table,
            &RestoreContext::empty_database(),
        )
        .await
        .expect_err("the notes table is refused");
    assert!(error.to_string().contains("notes are refused"), "{error}");
    for table in ["backup_rt_accounts", "backup_rt_notes"] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&dest_pool)
            .await?;
        assert_eq!(
            count, 0,
            "{table} must be rolled back with the failed table"
        );
    }

    // With the fault removed the same restore succeeds, with no cleanup.
    #[cfg(feature = "sqlite")]
    let allow = "DROP TRIGGER backup_rt_notes_refuse";
    #[cfg(feature = "postgres")]
    let allow = "DROP TRIGGER backup_rt_notes_refuse ON backup_rt_notes";
    sqlx::query(allow).execute(&dest_pool).await?;
    dest.restore_backup_rows(
        &schema_snapshot,
        &schema_snapshot,
        &rows_by_table,
        &RestoreContext::empty_database(),
    )
    .await?;
    assert!(
        BackupRtAccount::get(&dest_pool, &account.id)
            .await?
            .is_some()
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned PostgreSQL container"
)]
async fn externally_managed_database_refuses_backup_and_restore()
-> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::{
        BackupRestoreExternallyManaged, ChangeWindow, restore_backup_rows,
    };

    #[cfg(feature = "postgres")]
    let pool = owned_postgres::OwnedPostgres::pool("external-backup", 1).await?;
    #[cfg(feature = "postgres")]
    sqlx::query("SET statement_timeout = '5s'")
        .execute(&pool)
        .await?;
    #[cfg(feature = "sqlite")]
    let pool = setup_pool().await?;

    let owned = Database::new(pool.clone());
    let db = Database::builder(pool.clone())
        .externally_managed_backup_restore(true)
        .build();
    apply_schema(&owned, "20261122_external_backup").await?;

    let account = BackupRtAccount::insert(
        &db,
        CreateBackupRtAccountInput {
            email: "external@example.com".into(),
            password_hash: "preserved".into(),
            reset_token: None,
            profile: BackupProfile::default(),
            preferences: None,
            avatar: vec![],
            nickname: None,
            balance: Decimal::ZERO,
            credit_limit: None,
        },
    )
    .await?;
    let schema = owned.schema_snapshot("20261122_external_backup", &graphql_orm_entity_metadata());
    let entity = schema
        .entities
        .iter()
        .find(|entity| entity.table_name == "backup_rt_accounts")
        .unwrap();
    let window = ChangeWindow {
        after_sequence: None,
        until: 0,
    };
    let mut snapshot = owned.begin_consistent_snapshot().await?;
    let rows = BTreeMap::new();
    let apply = RestoreContext::empty_database();
    let dry_run = RestoreContext::dry_run();

    for error in [
        db.begin_consistent_snapshot()
            .await
            .err()
            .expect("snapshot"),
        db.export_table_rows(&mut snapshot, entity)
            .await
            .unwrap_err(),
        db.export_changes(window.clone()).await.unwrap_err(),
        db.export_changes_with_wait(window, std::time::Duration::ZERO)
            .await
            .unwrap_err(),
        db.import_table_rows(entity, &[], &apply).await.unwrap_err(),
        db.restore_backup_rows(&schema, &schema, &rows, &apply)
            .await
            .unwrap_err(),
        restore_backup_rows(&db, &schema, &schema, &rows, &dry_run)
            .await
            .unwrap_err(),
        db.restore_in_progress().await.unwrap_err(),
        db.ensure_no_restore_in_progress().await.unwrap_err(),
    ] {
        let refusal = BackupRestoreExternallyManaged::from_error(&error).expect("coded refusal");
        assert_eq!(refusal.code(), "BACKUP_RESTORE_EXTERNALLY_MANAGED");
    }
    assert_eq!(
        owned.export_table_rows(&mut snapshot, entity).await?.len(),
        1
    );
    drop(snapshot);
    let preserved: String = sqlx::query_scalar("SELECT email FROM backup_rt_accounts")
        .fetch_one(&pool)
        .await?;
    assert_eq!(preserved, account.email);
    #[cfg(feature = "sqlite")]
    let markers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_schema WHERE name = '__graphql_orm_restore_marker'",
    )
    .fetch_one(&pool)
    .await?;
    #[cfg(feature = "postgres")]
    let markers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_catalog.pg_class WHERE relname = '__graphql_orm_restore_marker'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(markers, 0);
    Ok(())
}
