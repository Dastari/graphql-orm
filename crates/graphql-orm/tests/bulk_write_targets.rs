//! Target rows of generated writes: the bulk-write row bound, the SQL
//! write-target prefilter, and bare-pool inserts taking the authorized path.

// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]

#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(backend = "sqlite", table = "target_records", plural = "TargetRecords")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "target_records",
        plural = "TargetRecords"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "target_records",
        plural = "TargetRecords",
        schema_policy = "external_writable"
    )
)]
struct TargetRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[sortable]
    #[filterable(type = "string")]
    record_key: String,
    #[filterable(type = "string")]
    owner: String,
    other: String,
}

/// The same table behind an entity that declares a write policy key.
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "target_records",
        plural = "GuardedRecords",
        write_policy = "records.write"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "target_records",
        plural = "GuardedRecords",
        write_policy = "records.write"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "target_records",
        plural = "GuardedRecords",
        schema_policy = "external_writable",
        write_policy = "records.write"
    )
)]
struct GuardedRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[sortable]
    #[filterable(type = "string")]
    record_key: String,
    owner: String,
    other: String,
}

#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [TargetRecord, GuardedRecord] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [TargetRecord, GuardedRecord] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [TargetRecord, GuardedRecord] }

#[cfg(feature = "sqlite")]
struct FixtureOwner;
#[cfg(feature = "sqlite")]
impl FixtureOwner {
    fn cleanup(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }
}
#[cfg(feature = "postgres")]
type FixtureOwner = owned_postgres::OwnedPostgres;
#[cfg(feature = "mssql")]
type FixtureOwner = owned_mssql::OwnedSqlServer;

struct AllowEntityPolicy;
impl EntityPolicy for AllowEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

/// What the row policy's `read_visibility` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Visibility {
    CallbackOnly,
    /// `owner = 'mine'`, with residual callbacks.
    Prefilter,
    /// `owner = 'mine'`, sufficient for reads.
    Complete,
    /// The policy fails.
    Error,
}

/// The caller owns the rows whose `owner` is `mine`, and can neither read nor
/// write any other row (unless `writes_theirs`, when it may write but not
/// read them). Every row callback records the row it was asked about.
struct OwnerRows {
    prefilter: bool,
    visibility: Visibility,
    writes_theirs: bool,
    asked: Arc<Mutex<Vec<String>>>,
    /// SQL Server: on each write callback, whether another session could
    /// update the hidden row `key4` at once (see [`hidden_row_is_free`]).
    #[cfg(feature = "mssql")]
    probe: Option<(String, Arc<Mutex<Vec<bool>>>)>,
}
impl OwnerRows {
    fn new(prefilter: bool, asked: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            prefilter,
            visibility: Visibility::CallbackOnly,
            writes_theirs: false,
            asked,
            #[cfg(feature = "mssql")]
            probe: None,
        }
    }
    fn with_visibility(mut self, visibility: Visibility) -> Self {
        self.visibility = visibility;
        self
    }
    fn decide(&self, row: &(dyn std::any::Any + Send + Sync), write: bool) -> bool {
        let row = row
            .downcast_ref::<TargetRecord>()
            .expect("only TargetRecord rows are written here");
        self.asked.lock().unwrap().push(row.record_key.clone());
        row.owner == "mine" || (write && self.writes_theirs)
    }
}
fn mine() -> ReadPredicate {
    ReadPredicate::column::<DefaultBackend, TargetRecord>(
        "owner",
        ReadPredicateOp::Eq,
        SqlValue::String("mine".into()),
    )
    .unwrap()
}
impl RowPolicy for OwnerRows {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            Ok(match self.visibility {
                Visibility::CallbackOnly => ReadVisibility::CallbackOnly,
                Visibility::Prefilter => ReadVisibility::Prefilter(mine()),
                Visibility::Complete => ReadVisibility::Complete(mine()),
                Visibility::Error => {
                    return Err(async_graphql::Error::new("visibility unavailable"));
                }
            })
        })
    }
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.decide(row, false)) })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            // The bulk write's transaction is open, and its targets locked.
            #[cfg(feature = "mssql")]
            if let Some((connection, free)) = &self.probe {
                let result = hidden_row_is_free(connection).await;
                free.lock().unwrap().push(result);
            }
            Ok(self.decide(row, true))
        })
    }
    fn write_target_prefilter<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<Option<ReadPredicate>>> {
        Box::pin(async move {
            assert!(ctx.is_some(), "only GraphQL writes ask for a prefilter");
            assert_eq!(surface, EntityAccessSurface::GraphqlMutation);
            if !self.prefilter {
                return Ok(None);
            }
            if self.writes_theirs {
                // The rows the caller can read or write: all of them.
                return Ok(Some(ReadPredicate::column::<DefaultBackend, TargetRecord>(
                    "owner",
                    ReadPredicateOp::Ne,
                    SqlValue::String(String::new()),
                )?));
            }
            Ok(Some(mine()))
        })
    }
}

async fn fixture() -> Result<(Database, FixtureOwner), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    {
        Ok((
            Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?,
            FixtureOwner,
        ))
    }
    #[cfg(feature = "postgres")]
    {
        let owner = FixtureOwner::start("bulk-write-targets")?;
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&owner.url)
            .await?;
        Ok((Database::new(pool), owner))
    }
    #[cfg(feature = "mssql")]
    {
        let owner = FixtureOwner::start().await?;
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await?;
        let db = Database::builder(pool)
            .schema_policy(SchemaPolicy::ExternalWritable)
            .build();
        Ok((db, owner))
    }
}

async fn execute(db: &Database, sql: &str) -> graphql_orm::Result<()> {
    <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[]).await?;
    Ok(())
}

/// Four rows: `key1` and `key2` are the caller's, `key3` and `key4` are not.
async fn setup() -> (Database, FixtureOwner) {
    let (mut db, owner) = fixture().await.unwrap();
    #[cfg(feature = "sqlite")]
    let ddl = "CREATE TABLE target_records (id TEXT PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, owner TEXT NOT NULL, other TEXT NOT NULL)";
    #[cfg(feature = "postgres")]
    let ddl = "CREATE TABLE target_records (id UUID PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, owner TEXT NOT NULL, other TEXT NOT NULL)";
    #[cfg(feature = "mssql")]
    let ddl = "CREATE TABLE target_records (id UNIQUEIDENTIFIER PRIMARY KEY, record_key NVARCHAR(100) UNIQUE NOT NULL, owner NVARCHAR(100) NOT NULL, other NVARCHAR(100) NOT NULL)";
    execute(&db, ddl).await.unwrap();
    for (id, owner_name) in [(1, "mine"), (2, "mine"), (3, "theirs"), (4, "theirs")] {
        execute(&db, &format!("INSERT INTO target_records VALUES ('00000000-0000-0000-0000-00000000000{id}', 'key{id}', '{owner_name}', 'before')")).await.unwrap();
    }
    db.set_entity_policy(AllowEntityPolicy);
    (db, owner)
}

async fn stored(db: &Database) -> Vec<(String, String)> {
    DefaultBackend::fetch_rows(
        db.pool(),
        "SELECT record_key, other FROM target_records ORDER BY record_key",
        &[],
    )
    .await
    .unwrap()
    .iter()
    .map(|row| {
        (
            DefaultBackend::try_get_string(row, "record_key").unwrap(),
            DefaultBackend::try_get_string(row, "other").unwrap(),
        )
    })
    .collect()
}

fn all_before(rows: &[(String, String)]) -> bool {
    rows.iter().all(|(_, other)| other == "before")
}

async fn mutate(db: &Database, mutation: &str) -> serde_json::Value {
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("test"))
        .finish();
    let response = schema.execute(format!("mutation {{ {mutation} }}")).await;
    assert!(response.errors.is_empty(), "{response:?}");
    let json = response.data.into_json().unwrap();
    json.as_object().unwrap().values().next().unwrap().clone()
}

const UPDATE_ALL: &str = r#"updateTargetRecords(where: {recordKey: {startsWith: "key"}}, input: {other: "after"}) { success affectedCount error errorCode }"#;
const DELETE_ALL: &str = r#"deleteTargetRecords(where: {recordKey: {startsWith: "key"}}) { success deletedCount error errorCode }"#;

fn bounded(db: &Database, bound: Option<i64>) -> Database {
    db.clone()
        .with_pagination_config(PaginationConfig::default().with_max_bulk_write_rows(bound))
}

/// #446: a GraphQL bulk write over the bound is refused, never truncated.
#[tokio::test]
async fn bulk_writes_over_the_bound_are_refused_with_a_stable_code() {
    let (db, mut owner) = setup().await;
    assert_eq!(
        PaginationConfig::default().max_bulk_write_rows,
        Some(PaginationConfig::DEFAULT_MAX_BULK_WRITE_ROWS)
    );
    assert_eq!(PaginationConfig::unbounded().max_bulk_write_rows, None);

    // Four rows match and the bound is three: nothing is written.
    let tight = bounded(&db, Some(3));
    for mutation in [UPDATE_ALL, DELETE_ALL] {
        let result = mutate(&tight, mutation).await;
        assert_eq!(result["success"], false, "{result}");
        assert_eq!(result["errorCode"], "PAGE_LIMIT_EXCEEDED", "{result}");
        let message = result["error"].as_str().unwrap();
        assert!(
            !message.chars().any(|character| character.is_ascii_digit()),
            "the refusal names no row count: {message}"
        );
        let rows = stored(&db).await;
        assert_eq!(rows.len(), 4, "{mutation}");
        assert!(all_before(&rows), "{mutation}");
    }

    // At the bound the result is the one an unbounded write gives.
    let exact = bounded(&db, Some(4));
    let result = mutate(&exact, UPDATE_ALL).await;
    assert_eq!(result["success"], true, "{result}");
    assert_eq!(result["affectedCount"], 4);
    assert!(stored(&db).await.iter().all(|(_, other)| other == "after"));

    // Trusted repository code is not bounded by the GraphQL limit.
    let updated = TargetRecord::update_where(
        &tight,
        TargetRecordWhereInput {
            record_key: Some(graphql_orm::graphql::filters::StringFilter {
                starts_with: Some("key".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        UpdateTargetRecordInput {
            other: Some("repository".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(updated, 4);

    // `None` removes the bound.
    let unbounded = bounded(&db, None);
    let result = mutate(&unbounded, DELETE_ALL).await;
    assert_eq!(result["success"], true, "{result}");
    assert_eq!(result["deletedCount"], 4);
    assert!(stored(&db).await.is_empty());
    owner.cleanup().unwrap();
}

/// #602: with a write-target prefilter, hidden rows are never selected, so no
/// row callback sees them; results equal the callback-only ones.
#[tokio::test]
async fn write_target_prefilter_keeps_hidden_rows_from_row_callbacks() {
    for prefilter in [false, true] {
        let (mut db, mut owner) = setup().await;
        let asked = Arc::new(Mutex::new(Vec::new()));
        db.set_row_policy(OwnerRows::new(prefilter, asked.clone()));
        let hidden_asked = |asked: &Arc<Mutex<Vec<String>>>| {
            let mut asked = asked.lock().unwrap();
            let hidden = asked.iter().any(|key| key == "key3" || key == "key4");
            asked.clear();
            hidden
        };

        let result = mutate(&db, UPDATE_ALL).await;
        assert_eq!(result["success"], true, "{result}");
        assert_eq!(result["affectedCount"], 2, "prefilter={prefilter}");
        assert_eq!(hidden_asked(&asked), !prefilter, "updateMany");
        assert_eq!(
            stored(&db).await,
            [
                ("key1", "after"),
                ("key2", "after"),
                ("key3", "before"),
                ("key4", "before")
            ]
            .map(|(key, other)| (key.to_owned(), other.to_owned()))
        );

        // A hidden single-row target reads as missing either way.
        for mutation in [
            r#"updateTargetRecord(id: "00000000-0000-0000-0000-000000000003", input: {other: "x"}) { success error }"#,
            r#"deleteTargetRecord(id: "00000000-0000-0000-0000-000000000003") { success error }"#,
        ] {
            let result = mutate(&db, mutation).await;
            assert_eq!(result["success"], false, "{result}");
            assert_eq!(result["error"], "Entity not found");
            assert_eq!(hidden_asked(&asked), !prefilter, "{mutation}");
        }
        owner.cleanup().unwrap();
    }
}

fn take_asked(asked: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    let mut asked = asked.lock().unwrap();
    let mut keys: Vec<_> = asked.drain(..).collect();
    keys.sort();
    keys.dedup();
    keys
}

fn keys(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| (*key).to_owned()).collect()
}

/// The setting under test: `None` is the default handle, `Some(on)` sets
/// `Database::with_narrow_bulk_writes_to_read_visibility(on)` explicitly.
const MODES: [Option<bool>; 3] = [None, Some(false), Some(true)];

fn in_mode(db: Database, mode: Option<bool>) -> Database {
    match mode {
        None => db,
        Some(on) => db.with_narrow_bulk_writes_to_read_visibility(on),
    }
}

/// #752: with `narrow_bulk_writes_to_read_visibility` on, a SQL predicate from
/// the row policy's read visibility narrows the locking read of `updateMany`
/// and `deleteMany`, so a hidden row is never passed to a row callback. By
/// default (and with the setting off) every matching row is still asked about,
/// as before. Payloads and stored rows are the same in every mode.
#[tokio::test]
async fn read_visibility_keeps_hidden_rows_from_bulk_write_callbacks_when_opted_in() {
    let mut outcomes = Vec::new();
    for mode in MODES {
        for visibility in [
            Visibility::CallbackOnly,
            Visibility::Prefilter,
            Visibility::Complete,
        ] {
            let narrowed = mode == Some(true) && visibility != Visibility::CallbackOnly;
            let mut outcome = Vec::new();
            for mutation in [UPDATE_ALL, DELETE_ALL] {
                let (mut db, mut owner) = setup().await;
                // Off unless the host opts in, on the handle and the builder.
                assert!(!db.narrow_bulk_writes_to_read_visibility_enabled());
                assert!(
                    Database::builder(db.pool().clone())
                        .narrow_bulk_writes_to_read_visibility(true)
                        .build()
                        .narrow_bulk_writes_to_read_visibility_enabled()
                );
                let asked = Arc::new(Mutex::new(Vec::new()));
                db.set_row_policy(OwnerRows::new(false, asked.clone()).with_visibility(visibility));
                let payload = mutate(&in_mode(db.clone(), mode), mutation).await;
                assert_eq!(
                    payload["success"], true,
                    "{mode:?} {visibility:?} {payload}"
                );
                let expected = if narrowed {
                    keys(&["key1", "key2"])
                } else {
                    keys(&["key1", "key2", "key3", "key4"])
                };
                assert_eq!(
                    take_asked(&asked),
                    expected,
                    "{mode:?} {visibility:?} {mutation}"
                );
                outcome.push((payload, stored(&db).await));
                owner.cleanup().unwrap();
            }
            outcomes.push(((mode, visibility), outcome));
        }
    }
    for ((mode, visibility), outcome) in &outcomes[1..] {
        assert_eq!(&outcomes[0].1, outcome, "{mode:?} {visibility:?}");
    }
    let (update, rows) = &outcomes[0].1[0];
    assert_eq!(update["affectedCount"], 2);
    assert_eq!(
        rows,
        &[
            ("key1", "after"),
            ("key2", "after"),
            ("key3", "before"),
            ("key4", "before")
        ]
        .map(|(key, other)| (key.to_owned(), other.to_owned()))
    );
    let (delete, rows) = &outcomes[0].1[1];
    assert_eq!(delete["deletedCount"], 2);
    assert_eq!(rows.len(), 2);
}

/// A row the caller may write but not read stays a bulk-write target by
/// default, whatever the read visibility. With the setting on, a bulk write
/// narrowed by read visibility leaves it out, and a write-target prefilter
/// that selects it takes precedence and keeps it a target.
#[tokio::test]
async fn write_only_rows_stay_bulk_targets_unless_narrowing_is_opted_in() {
    for mode in MODES {
        for (visibility, prefilter) in [
            (Visibility::CallbackOnly, false),
            (Visibility::Prefilter, false),
            (Visibility::Complete, false),
            (Visibility::Prefilter, true),
            (Visibility::Complete, true),
        ] {
            let expected =
                if mode == Some(true) && visibility != Visibility::CallbackOnly && !prefilter {
                    2
                } else {
                    4
                };
            let (mut db, mut owner) = setup().await;
            let mut policy =
                OwnerRows::new(prefilter, Default::default()).with_visibility(visibility);
            policy.writes_theirs = true;
            db.set_row_policy(policy);
            let payload = mutate(&in_mode(db.clone(), mode), UPDATE_ALL).await;
            assert_eq!(payload["success"], true, "{payload}");
            assert_eq!(
                payload["affectedCount"], expected,
                "{mode:?} {visibility:?} prefilter={prefilter}"
            );
            owner.cleanup().unwrap();
        }
    }
}

/// With the setting on, a `read_visibility` error on the mutation surface
/// refuses a bulk write before any row changes. By default the bulk write
/// does not ask, and proceeds as before.
#[tokio::test]
async fn a_read_visibility_error_refuses_only_an_opted_in_bulk_write() {
    for mode in MODES {
        let (mut db, mut owner) = setup().await;
        db.set_row_policy(
            OwnerRows::new(false, Default::default()).with_visibility(Visibility::Error),
        );
        let payload = mutate(&in_mode(db.clone(), mode), UPDATE_ALL).await;
        if mode == Some(true) {
            assert_eq!(payload["success"], false, "{payload}");
            assert!(all_before(&stored(&db).await), "nothing was written");
        } else {
            assert_eq!(payload["success"], true, "{mode:?} {payload}");
            assert_eq!(payload["affectedCount"], 2, "{mode:?}");
        }
        owner.cleanup().unwrap();
    }
}

/// #752 on PostgreSQL: with the setting on, another transaction's lock on a
/// hidden row does not block a bulk write narrowed by read visibility. By
/// default, and without a predicate, the bulk write waits for that lock (here,
/// until `lock_timeout`).
#[cfg(feature = "postgres")]
#[tokio::test]
async fn a_lock_on_a_hidden_row_does_not_block_an_opted_in_narrowed_bulk_write() {
    use graphql_orm::sqlx::Connection as _;
    let (db, mut owner) = setup().await;
    // New sessions give up on a lock after a bounded wait, so the control
    // cases below fail instead of hanging.
    execute(
        &db,
        "DO $$ BEGIN EXECUTE format('ALTER DATABASE %I SET lock_timeout = %L', current_database(), '2s'); END $$",
    )
    .await
    .unwrap();
    let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&owner.url)
        .await
        .unwrap();
    let mut locker = graphql_orm::sqlx::PgConnection::connect(&owner.url)
        .await
        .unwrap();
    graphql_orm::sqlx::query("BEGIN")
        .execute(&mut locker)
        .await
        .unwrap();
    graphql_orm::sqlx::query("SELECT id FROM target_records WHERE record_key = 'key3' FOR UPDATE")
        .execute(&mut locker)
        .await
        .unwrap();
    let with_policy = |visibility, narrow: bool| {
        let mut db = Database::new(pool.clone()).with_narrow_bulk_writes_to_read_visibility(narrow);
        db.set_entity_policy(AllowEntityPolicy);
        db.set_row_policy(OwnerRows::new(false, Default::default()).with_visibility(visibility));
        db
    };
    let run = |db: Database, mutation: &'static str| async move {
        let schema = schema_builder(db).data(AuthSubject::new("test")).finish();
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            schema.execute(format!("mutation {{ {mutation} }}")),
        )
        .await
        .expect("lock_timeout bounds every wait")
    };

    // Controls: without a predicate, and with one but the setting off, the
    // bulk write selects, and so waits for, the locked hidden row; it gives up
    // and changes nothing.
    for (visibility, narrow) in [
        (Visibility::CallbackOnly, true),
        (Visibility::Prefilter, false),
    ] {
        let response = run(with_policy(visibility, narrow), UPDATE_ALL).await;
        let refused = !response.errors.is_empty()
            || response.data.clone().into_json().unwrap()["updateTargetRecords"]["success"]
                == false;
        assert!(
            refused,
            "{visibility:?} narrow={narrow}: the write waits on the lock: {response:?}"
        );
        assert!(all_before(&stored(&db).await));
    }

    for visibility in [Visibility::Prefilter, Visibility::Complete] {
        let response = run(with_policy(visibility, true), UPDATE_ALL).await;
        assert!(response.errors.is_empty(), "{response:?}");
        let payload = &response.data.into_json().unwrap()["updateTargetRecords"];
        assert_eq!(payload["success"], true, "{visibility:?} {payload}");
        assert_eq!(payload["affectedCount"], 2, "{visibility:?}");
    }
    let response = run(with_policy(Visibility::Prefilter, true), DELETE_ALL).await;
    assert!(response.errors.is_empty(), "{response:?}");
    let payload = &response.data.into_json().unwrap()["deleteTargetRecords"];
    assert_eq!(payload["success"], true, "{payload}");
    assert_eq!(payload["deletedCount"], 2);

    graphql_orm::sqlx::query("ROLLBACK")
        .execute(&mut locker)
        .await
        .unwrap();
    locker.close().await.unwrap();
    pool.close().await;
    assert_eq!(
        stored(&db).await,
        [("key3", "before"), ("key4", "before")]
            .map(|(key, other)| (key.to_owned(), other.to_owned()))
    );
    owner.cleanup().unwrap();
}

/// A new session on `connection` updates the hidden row `key4`, giving up on
/// a lock after one second: `true` when nothing held it.
#[cfg(feature = "mssql")]
async fn hidden_row_is_free(connection: &str) -> bool {
    owned_mssql::execute_batch(
        connection,
        "SET LOCK_TIMEOUT 1000; UPDATE target_records SET other = other WHERE record_key = 'key4'",
    )
    .await
    .is_ok()
}

/// A SQL Server session held open across statements.
#[cfg(feature = "mssql")]
async fn sql_server_session(
    connection: &str,
) -> graphql_orm::tiberius::Client<graphql_orm::tokio_util::compat::Compat<tokio::net::TcpStream>> {
    use graphql_orm::tokio_util::compat::TokioAsyncWriteCompatExt;
    let config = graphql_orm::tiberius::Config::from_ado_string(connection).unwrap();
    let tcp = tokio::net::TcpStream::connect(config.get_addr())
        .await
        .unwrap();
    tcp.set_nodelay(true).unwrap();
    graphql_orm::tiberius::Client::connect(config, tcp.compat_write())
        .await
        .unwrap()
}

#[cfg(feature = "mssql")]
async fn run_batch(
    session: &mut graphql_orm::tiberius::Client<
        graphql_orm::tokio_util::compat::Compat<tokio::net::TcpStream>,
    >,
    sql: &str,
) {
    session
        .simple_query(sql)
        .await
        .unwrap()
        .into_results()
        .await
        .unwrap();
}

/// Whether a write that gave up on a wait refused. Its session, abandoned by
/// the pool, may still be waiting on the server: end it.
#[cfg(feature = "mssql")]
async fn waited(
    connection: &str,
    response: async_graphql::Response,
    payload: serde_json::Value,
) -> bool {
    let refused = !response.errors.is_empty() || payload["success"] == false;
    owned_mssql::execute_batch(
        connection,
        "DECLARE @kill NVARCHAR(MAX) = N''; \
         SELECT @kill += N'KILL ' + CAST(session_id AS NVARCHAR(10)) + N'; ' \
         FROM sys.dm_exec_requests WHERE blocking_session_id <> 0 AND session_id <> @@SPID; \
         EXEC (@kill);",
    )
    .await
    .unwrap();
    refused
}

/// #752 on SQL Server: with the setting on, a bulk write narrowed by read
/// visibility neither waits on another transaction's lock on a hidden row nor
/// holds a lock on one, whatever access path the optimizer picks. A statement
/// of the write gives up after five seconds, so a wait fails fast. (A
/// `SET LOCK_TIMEOUT` sent through the pool runs inside `sp_executesql` and
/// does not outlast it; the probe's own session uses one.)
///
/// The controls (no predicate, or the setting off) read every matching row
/// under `UPDLOCK, HOLDLOCK`: they wait on the locked hidden row, and hold
/// the other one until they commit.
///
/// The one remaining wait is documented: with `READ_COMMITTED_SNAPSHOT` off,
/// the unlocked candidate read is a locking read committed read, which waits
/// for a hidden row another transaction has changed and not yet committed.
/// With `READ_COMMITTED_SNAPSHOT` on it reads the committed version instead.
#[cfg(feature = "mssql")]
#[tokio::test]
async fn a_lock_on_a_hidden_row_does_not_block_an_opted_in_narrowed_sql_server_bulk_write() {
    use graphql_orm::db::mssql::{MssqlPool, MssqlPoolOptions};
    let (db, mut owner) = setup().await;
    drop(db);
    let connection = owner.connection_string.clone();
    let probes = Arc::new(Mutex::new(Vec::new()));
    let open = || async {
        let pool = MssqlPool::connect_ado_external_writable_with(
            &connection,
            MssqlPoolOptions::new()
                .max_connections(1)
                .query_timeout(Some(std::time::Duration::from_secs(5))),
        )
        .await
        .unwrap();
        Database::builder(pool)
            .schema_policy(SchemaPolicy::ExternalWritable)
            .build()
    };
    let with_policy = |db: &Database, visibility, narrow: bool| {
        let mut db = db
            .clone()
            .with_narrow_bulk_writes_to_read_visibility(narrow);
        db.set_entity_policy(AllowEntityPolicy);
        let mut policy = OwnerRows::new(false, Default::default()).with_visibility(visibility);
        policy.probe = Some((connection.clone(), probes.clone()));
        db.set_row_policy(policy);
        db
    };
    let run = |db: Database, mutation: &'static str| async move {
        let schema = schema_builder(db).data(AuthSubject::new("test")).finish();
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            schema.execute(format!("mutation {{ {mutation} }}")),
        )
        .await
        .expect("the query timeout bounds every wait");
        let json = response.data.clone().into_json().unwrap();
        let payload = json
            .as_object()
            .and_then(|fields| fields.values().next().cloned())
            .unwrap_or_default();
        (response, payload)
    };
    let take_probes = || std::mem::take(&mut *probes.lock().unwrap());

    let db = open().await;

    // No other transaction: the narrowed write leaves the hidden rows free
    // while it runs. Without narrowing the write holds them.
    for visibility in [Visibility::Prefilter, Visibility::Complete] {
        let (response, payload) = run(with_policy(&db, visibility, true), UPDATE_ALL).await;
        assert!(response.errors.is_empty(), "{response:?}");
        assert_eq!(payload["success"], true, "{visibility:?} {payload}");
        assert_eq!(payload["affectedCount"], 2, "{visibility:?}");
        let free = take_probes();
        assert!(
            !free.is_empty() && free.iter().all(|free| *free),
            "{visibility:?}: the narrowed write holds no lock on a hidden row: {free:?}"
        );
    }
    let (_, payload) = run(with_policy(&db, Visibility::Prefilter, false), UPDATE_ALL).await;
    assert_eq!(payload["success"], true, "{payload}");
    assert!(
        take_probes().contains(&false),
        "control: the unnarrowed write holds the hidden rows it read"
    );

    // Another transaction holds update locks on the hidden row `key3`, in the
    // table and in its `record_key` index, as a concurrent write's locking
    // read does.
    let mut locker = sql_server_session(&connection).await;
    run_batch(
        &mut locker,
        "BEGIN TRANSACTION; SELECT * FROM target_records WITH (UPDLOCK, HOLDLOCK) WHERE record_key = 'key3'",
    )
    .await;
    for (visibility, narrow) in [
        (Visibility::CallbackOnly, true),
        (Visibility::Prefilter, false),
    ] {
        let (response, payload) = run(with_policy(&db, visibility, narrow), UPDATE_ALL).await;
        assert!(
            waited(&connection, response, payload).await,
            "{visibility:?} narrow={narrow}: the write waits on the lock"
        );
    }
    take_probes();
    for visibility in [Visibility::Prefilter, Visibility::Complete] {
        let (response, payload) = run(with_policy(&db, visibility, true), UPDATE_ALL).await;
        assert!(response.errors.is_empty(), "{response:?}");
        assert_eq!(payload["success"], true, "{visibility:?} {payload}");
        assert_eq!(payload["affectedCount"], 2, "{visibility:?}");
        assert!(take_probes().iter().all(|free| *free), "{visibility:?}");
    }
    run_batch(&mut locker, "ROLLBACK TRANSACTION").await;

    // Another transaction changed the hidden row `key3` and has not committed.
    // With `READ_COMMITTED_SNAPSHOT` off, the candidate read waits for it.
    run_batch(
        &mut locker,
        "BEGIN TRANSACTION; UPDATE target_records SET other = 'pending' WHERE record_key = 'key3'",
    )
    .await;
    let (response, payload) = run(with_policy(&db, Visibility::Prefilter, true), UPDATE_ALL).await;
    assert!(
        waited(&connection, response, payload).await,
        "documented: a locking read committed read waits for an uncommitted change"
    );
    run_batch(&mut locker, "ROLLBACK TRANSACTION").await;
    drop(locker);
    drop(db);
    take_probes();

    // With `READ_COMMITTED_SNAPSHOT` on, the candidate read does not wait.
    owned_mssql::execute_batch(
        &connection,
        "ALTER DATABASE CURRENT SET READ_COMMITTED_SNAPSHOT ON WITH ROLLBACK IMMEDIATE",
    )
    .await
    .unwrap();
    let db = open().await;
    let mut locker = sql_server_session(&connection).await;
    run_batch(
        &mut locker,
        "BEGIN TRANSACTION; UPDATE target_records SET other = 'pending' WHERE record_key = 'key3'",
    )
    .await;
    let (response, payload) = run(with_policy(&db, Visibility::Prefilter, false), UPDATE_ALL).await;
    assert!(
        waited(&connection, response, payload).await,
        "control: the unnarrowed write waits on the change"
    );
    take_probes();
    for (visibility, mutation) in [
        (Visibility::Prefilter, UPDATE_ALL),
        (Visibility::Complete, UPDATE_ALL),
        (Visibility::Prefilter, DELETE_ALL),
    ] {
        let (response, payload) = run(with_policy(&db, visibility, true), mutation).await;
        assert!(response.errors.is_empty(), "{response:?}");
        assert_eq!(
            payload["success"], true,
            "{visibility:?} {mutation} {payload}"
        );
        assert!(
            take_probes().iter().all(|free| *free),
            "{visibility:?} {mutation}"
        );
    }
    run_batch(&mut locker, "ROLLBACK TRANSACTION").await;
    drop(locker);
    assert_eq!(
        stored(&db).await,
        [("key3", "before"), ("key4", "before")]
            .map(|(key, other)| (key.to_owned(), other.to_owned()))
    );
    drop(db);
    owner.cleanup().unwrap();
}

/// The bulk bound counts the rows the caller may write. For a fixed set of
/// such rows the payload is the same whether or not other, hidden rows match
/// the filter, with a callback-only row policy and with a SQL prefilter.
#[tokio::test]
async fn hidden_matching_rows_never_change_a_bulk_write_outcome() {
    for prefilter in [false, true] {
        for (bound, mutation) in [
            (1, UPDATE_ALL),
            (2, UPDATE_ALL),
            (1, DELETE_ALL),
            (2, DELETE_ALL),
        ] {
            let mut payloads = Vec::new();
            for hidden_rows_match in [true, false] {
                let (mut db, mut owner) = setup().await;
                if !hidden_rows_match {
                    execute(&db, "DELETE FROM target_records WHERE owner = 'theirs'")
                        .await
                        .unwrap();
                }
                db.set_row_policy(OwnerRows::new(prefilter, Default::default()));
                let payload = mutate(&bounded(&db, Some(bound)), mutation).await;
                let mine: Vec<_> = stored(&db)
                    .await
                    .into_iter()
                    .filter(|(key, _)| key == "key1" || key == "key2")
                    .collect();
                payloads.push((payload, mine));
                owner.cleanup().unwrap();
            }
            assert_eq!(
                payloads[0], payloads[1],
                "prefilter={prefilter} bound={bound} {mutation}"
            );
            // Two rows are the caller's: a bound of one refuses, two admits.
            let (payload, _) = &payloads[0];
            if bound == 1 {
                assert_eq!(payload["success"], false, "{payload}");
                assert_eq!(payload["errorCode"], "PAGE_LIMIT_EXCEEDED");
            } else {
                assert_eq!(payload["success"], true, "{payload}");
            }
        }
    }
}

/// A row policy decided in Rust classifies candidates in pages under the scan
/// budget authorized reads use. A scan that would need more candidates is the
/// same host-configured refusal a read gets; a SQL prefilter avoids the scan.
#[tokio::test]
async fn a_bulk_write_candidate_scan_is_capped_by_the_read_scan_budget() {
    // A prefilter, or (opted in) the read visibility predicate, narrows in SQL.
    for (prefilter, narrow) in [(false, false), (true, false), (false, true)] {
        let (mut db, mut owner) = setup().await;
        db.set_row_policy(
            OwnerRows::new(prefilter, Default::default()).with_visibility(Visibility::Prefilter),
        );
        let db = db.with_narrow_bulk_writes_to_read_visibility(narrow);
        // A budget too small for even the visible rows refuses either way.
        let tiny = db
            .clone()
            .with_pagination_config(PaginationConfig::default().with_max_scanned_rows(Some(1)));
        let payload = mutate(&tiny, UPDATE_ALL).await;
        assert_eq!(payload["errorCode"], "PAGE_LIMIT_EXCEEDED", "{payload}");
        assert!(all_before(&stored(&db).await), "nothing was written");
        // Four candidates match; the budget allows three.
        let tight = db
            .clone()
            .with_pagination_config(PaginationConfig::default().with_max_scanned_rows(Some(3)));
        let payload = mutate(&tight, UPDATE_ALL).await;
        if prefilter || narrow {
            // SQL removes the hidden rows: two candidates, within the budget.
            assert_eq!(payload["success"], true, "{payload}");
            assert_eq!(payload["affectedCount"], 2);
        } else {
            assert_eq!(payload["success"], false, "{payload}");
            assert_eq!(payload["errorCode"], "PAGE_LIMIT_EXCEEDED");
            assert!(
                payload["error"].as_str().unwrap().contains("scan budget"),
                "{payload}"
            );
            assert!(all_before(&stored(&db).await), "nothing was written");
        }
        // A budget that covers the candidates admits the write either way.
        let roomy = db
            .clone()
            .with_pagination_config(PaginationConfig::default().with_max_scanned_rows(Some(4)));
        let payload = mutate(&roomy, DELETE_ALL).await;
        assert_eq!(payload["success"], true, "{payload}");
        assert_eq!(payload["deletedCount"], 2);
        owner.cleanup().unwrap();
    }
}

/// A narrowed bulk write reads its candidates in pages. On SQL Server each
/// page is read by key and the next starts after its last key: with hidden
/// rows between the visible ones and three pages of visible rows, every
/// visible row is written exactly once, as without narrowing.
#[tokio::test]
async fn a_narrowed_bulk_write_reads_every_page_of_candidates_once() {
    // 2,400 more matching rows; the even ones are the caller's.
    #[cfg(feature = "sqlite")]
    let rows = "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2400) \
        INSERT INTO target_records SELECT printf('10000000-0000-0000-0000-%012d', i), 'keyp' || i, \
        CASE WHEN i % 2 = 0 THEN 'mine' ELSE 'theirs' END, 'before' FROM n";
    #[cfg(feature = "postgres")]
    let rows = "INSERT INTO target_records SELECT ('10000000-0000-0000-0000-' || lpad(i::text, 12, '0'))::uuid, \
        'keyp' || i, CASE WHEN i % 2 = 0 THEN 'mine' ELSE 'theirs' END, 'before' FROM generate_series(1, 2400) AS i";
    #[cfg(feature = "mssql")]
    let rows = "WITH n AS (SELECT TOP (2400) ROW_NUMBER() OVER (ORDER BY (SELECT 1)) AS i \
        FROM sys.all_objects AS a CROSS JOIN sys.all_objects AS b) \
        INSERT INTO target_records SELECT CAST(CONCAT('10000000-0000-0000-0000-', RIGHT(CONCAT('000000000000', i), 12)) AS UNIQUEIDENTIFIER), \
        CONCAT('keyp', i), CASE WHEN i % 2 = 0 THEN 'mine' ELSE 'theirs' END, 'before' FROM n";
    let unbounded = PaginationConfig::default()
        .with_max_scanned_rows(None)
        .with_max_bulk_write_rows(None);
    for narrow in [false, true] {
        let (mut db, mut owner) = setup().await;
        execute(&db, rows).await.unwrap();
        let asked = Arc::new(Mutex::new(Vec::new()));
        db.set_row_policy(
            OwnerRows::new(false, asked.clone()).with_visibility(Visibility::Prefilter),
        );
        let db = db
            .with_narrow_bulk_writes_to_read_visibility(narrow)
            .with_pagination_config(unbounded);
        let payload = mutate(&db, UPDATE_ALL).await;
        assert_eq!(payload["success"], true, "narrow={narrow} {payload}");
        assert_eq!(payload["affectedCount"], 1202, "narrow={narrow}");
        let asked = take_asked(&asked);
        assert_eq!(
            asked.len(),
            if narrow { 1202 } else { 2404 },
            "narrow={narrow}"
        );
        let rows = stored(&db).await;
        let written = rows.iter().filter(|(_, other)| other == "after").count();
        assert_eq!(written, 1202, "narrow={narrow}");
        let payload = mutate(&db, DELETE_ALL).await;
        assert_eq!(payload["deletedCount"], 1202, "narrow={narrow} {payload}");
        assert!(all_before(&stored(&db).await));
        assert_eq!(stored(&db).await.len(), 1202);
        owner.cleanup().unwrap();
    }
}

fn new_record(key: &str) -> CreateTargetRecordInput {
    CreateTargetRecordInput {
        record_key: key.into(),
        owner: "mine".into(),
        other: "inserted".into(),
    }
}

/// #601: `insert` with a bare pool takes the same authorized path as a
/// `Database` with nothing installed.
#[tokio::test]
async fn bare_pool_insert_takes_the_authorized_path() {
    let (db, mut owner) = setup().await;

    // An entity that declares a write policy key has no provider on a bare
    // pool: the write is refused instead of running unchecked.
    let error = GuardedRecord::insert(
        db.pool(),
        CreateGuardedRecordInput {
            record_key: "guarded".into(),
            owner: "mine".into(),
            other: "inserted".into(),
        },
    )
    .await
    .expect_err("a bare pool carries no policy provider");
    assert_eq!(
        OrmPublicError::from_sqlx(&error).code,
        OrmErrorCode::AuthorizationMisconfigured
    );
    assert_eq!(stored(&db).await.len(), 4);

    // An entity without a policy key keeps working. On SQL Server the pool
    // itself was opened for entity writes, which the default handle honours.
    let inserted = TargetRecord::insert(db.pool(), new_record("bare"))
        .await
        .unwrap();
    assert_eq!(inserted.record_key, "bare");
    assert_eq!(stored(&db).await.len(), 5);

    // The same call on the `Database` is unchanged.
    let inserted = TargetRecord::insert(&db, new_record("handle"))
        .await
        .unwrap();
    assert_eq!(inserted.other, "inserted");
    owner.cleanup().unwrap();
}
