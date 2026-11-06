//! `MutationContext::defer` is the after-commit callback API. These tests pin each of its guarantees on
//! every write path: it runs once and only after a successful commit, never after a rollback, a failure
//! is reported without changing the committed result, and its order relative to `Changed` events and
//! `notify_with` is fixed.
#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "write_records",
        plural = "WriteRecords",
        upsert = "record_key",
        notify_with = "notify"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "write_records",
        plural = "WriteRecords",
        upsert = "record_key",
        notify_with = "notify"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "write_records",
        plural = "WriteRecords",
        schema_policy = "external_writable",
        upsert = "record_key",
        notify_with = "notify"
    )
)]
struct WriteRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[sortable]
    #[filterable(type = "string")]
    record_key: String,
    required: String,
    optional: Option<String>,
    other: String,
}
#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [WriteRecord] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [WriteRecord] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [WriteRecord] }
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

#[cfg(feature = "mssql")]
struct AllowEntityPolicy;
#[cfg(feature = "mssql")]
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
        let owner = FixtureOwner::start("after-commit-defer")?;
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
        let mut db = Database::builder(pool)
            .schema_policy(SchemaPolicy::ExternalWritable)
            .build();
        db.set_entity_policy(AllowEntityPolicy);
        Ok((db, owner))
    }
}
async fn execute(db: &Database, sql: &str) -> graphql_orm::Result<()> {
    <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[]).await?;
    Ok(())
}

/// Everything the hook, the deferred actions, `notify_with` and the error handler record, in the order
/// it happened.
#[derive(Default)]
struct State {
    log: Mutex<Vec<String>>,
    reported: Mutex<Vec<String>>,
    /// Make every deferred action return an error after it records itself.
    fail_deferred: AtomicBool,
    /// Also register an action from the `Before` hook.
    defer_in_before: AtomicBool,
    /// Refuse the Nth `After` hook call of the test (1-based), after registering its action. 0 is off.
    refuse_after_nth: AtomicUsize,
    after_calls: AtomicUsize,
    before_calls: AtomicUsize,
    changed: Mutex<Option<tokio::sync::broadcast::Receiver<WriteRecordChangedEvent>>>,
}
impl State {
    fn push(&self, line: String) {
        self.log.lock().unwrap().push(line);
    }
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.log.lock().unwrap())
    }
    /// Only the deferred-action lines, taking the whole log.
    fn take_deferred(&self) -> Vec<String> {
        self.take()
            .into_iter()
            .filter(|line| line.starts_with("deferred"))
            .collect()
    }
    fn reported(&self) -> Vec<String> {
        self.reported.lock().unwrap().clone()
    }
    /// Record a line for each `Changed` event that has already been emitted.
    fn drain_changed(&self) {
        if let Some(receiver) = self.changed.lock().unwrap().as_mut() {
            while receiver.try_recv().is_ok() {
                self.push("changed".into());
            }
        }
    }
}

/// An action that reads the row through the pool it is handed, which is a different connection from the
/// committed transaction's, and records whether the committed state is visible.
async fn record_deferred(
    state: &State,
    db: &Database,
    label: String,
    id: graphql_orm::uuid::Uuid,
    expect: Option<String>,
) -> Result<(), String> {
    state.drain_changed();
    let rows = DefaultBackend::fetch_rows(
        db.pool(),
        &format!("SELECT other FROM write_records WHERE id = '{id}'"),
        &[],
    )
    .await
    .map_err(|error| error.to_string())?;
    let committed = match &expect {
        None => rows.is_empty(),
        Some(other) => {
            rows.first()
                .map(|row| DefaultBackend::try_get_string(row, "other").unwrap())
                .as_ref()
                == Some(other)
        }
    };
    state.push(format!("deferred {label} committed={committed}"));
    if state.fail_deferred.load(Ordering::SeqCst) {
        Err("deferred failed".into())
    } else {
        Ok(())
    }
}

struct Hook(Arc<State>);
impl MutationHook for Hook {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            let state = self.0.clone();
            if event.phase == MutationPhase::Before {
                state.before_calls.fetch_add(1, Ordering::SeqCst);
                if state.defer_in_before.load(Ordering::SeqCst) {
                    let state = state.clone();
                    hook_ctx.defer(move |db| async move {
                        state.push("deferred before-hook".into());
                        let _ = db;
                        Ok::<(), String>(())
                    });
                }
                return Ok(());
            }
            let row = if event.action == ChangeAction::Deleted {
                event.before::<WriteRecord>()?
            } else {
                event.after::<WriteRecord>()?
            }
            .expect("row");
            let expect = (event.action != ChangeAction::Deleted).then(|| row.other.clone());
            let label = format!("{:?} {}", event.action, row.record_key);
            let id = row.id;
            let run_state = state.clone();
            hook_ctx.defer(move |db| async move {
                record_deferred(&run_state, &db, label, id, expect).await
            });
            let nth = state.after_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if state.refuse_after_nth.load(Ordering::SeqCst) == nth {
                return Err(async_graphql::Error::new("refused"));
            }
            Ok(())
        })
    }
}

struct Reporter(Arc<State>);
impl PostCommitErrorHandler for Reporter {
    fn on_post_commit_error<'a>(&'a self, _: &'a Database, error: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.0.reported.lock().unwrap().push(error.into());
        })
    }
}

async fn notify(
    ctx: &async_graphql::Context<'_>,
    _: &<DefaultBackend as OrmBackend>::Pool,
    entity: &WriteRecord,
    action: ChangeAction,
) -> async_graphql::Result<()> {
    ctx.data_unchecked::<Arc<State>>()
        .push(format!("notify {action:?} {}", entity.record_key));
    Ok(())
}

type TestSchema = async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>;
struct Fixture {
    db: Database,
    owner: FixtureOwner,
    state: Arc<State>,
    schema: TestSchema,
}
async fn setup() -> Fixture {
    let (mut db, owner) = fixture().await.unwrap();
    #[cfg(feature = "sqlite")]
    let ddl = "CREATE TABLE write_records (id TEXT PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, required TEXT NOT NULL, optional TEXT, other TEXT NOT NULL)";
    #[cfg(feature = "postgres")]
    let ddl = "CREATE TABLE write_records (id UUID PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, required TEXT NOT NULL, optional TEXT, other TEXT NOT NULL)";
    #[cfg(feature = "mssql")]
    let ddl = "CREATE TABLE write_records (id UNIQUEIDENTIFIER PRIMARY KEY, record_key NVARCHAR(100) UNIQUE NOT NULL, required NVARCHAR(100) NOT NULL, optional NVARCHAR(100), other NVARCHAR(100) NOT NULL)";
    execute(&db, ddl).await.unwrap();
    for id in 1..=3 {
        execute(&db, &format!("INSERT INTO write_records VALUES ('00000000-0000-0000-0000-00000000000{id}', 'key{id}', 'before', NULL, 'before')")).await.unwrap();
    }
    let state = Arc::new(State::default());
    db.set_mutation_hook(Hook(state.clone()));
    db.set_post_commit_error_handler(Reporter(state.clone()));
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("test"))
        .data(state.clone())
        .finish();
    Fixture {
        db,
        owner,
        state,
        schema,
    }
}
async fn values(db: &Database) -> Vec<String> {
    let mut values: Vec<_> =
        DefaultBackend::fetch_rows(db.pool(), "SELECT other FROM write_records", &[])
            .await
            .unwrap()
            .iter()
            .map(|r| DefaultBackend::try_get_string(r, "other").unwrap())
            .collect();
    values.sort();
    values
}
async fn gql(schema: &TestSchema, mutation: &str) -> serde_json::Value {
    let response = schema.execute(format!("mutation {{ {mutation} }}")).await;
    assert!(response.errors.is_empty(), "{response:?}");
    let json = response.data.into_json().unwrap();
    json.as_object().unwrap().values().next().unwrap().clone()
}
const ID1: &str = "00000000-0000-0000-0000-000000000001";
fn by_prefix() -> WriteRecordWhereInput {
    WriteRecordWhereInput {
        record_key: Some(graphql_orm::graphql::filters::StringFilter {
            starts_with: Some("key".into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn update_to(other: &str) -> UpdateWriteRecordInput {
    UpdateWriteRecordInput {
        other: Some(other.into()),
        ..Default::default()
    }
}
fn create(key: &str, other: &str) -> CreateWriteRecordInput {
    CreateWriteRecordInput {
        record_key: key.into(),
        required: "r".into(),
        optional: None,
        other: other.into(),
    }
}
fn sorted(mut lines: Vec<String>) -> Vec<String> {
    lines.sort();
    lines
}

#[tokio::test]
async fn graphql_single_row_writes_defer_once_after_commit() {
    let mut f = setup().await;
    let cases = [
        (
            "createWriteRecord(input: {recordKey: \"new\", required: \"r\", other: \"made\"}) { success }".to_string(),
            "deferred Created new committed=true",
        ),
        (
            format!("updateWriteRecord(id: \"{ID1}\", input: {{other: \"after\"}}) {{ success }}"),
            "deferred Updated key1 committed=true",
        ),
        (
            "upsertWriteRecord(input: {recordKey: \"upserted\", required: \"r\", other: \"up\"}) { success }".into(),
            "deferred Created upserted committed=true",
        ),
        (
            "upsertWriteRecord(input: {recordKey: \"key2\", required: \"r\", other: \"again\"}) { success }".into(),
            "deferred Updated key2 committed=true",
        ),
        (
            format!("deleteWriteRecord(id: \"{ID1}\") {{ success }}"),
            "deferred Deleted key1 committed=true",
        ),
    ];
    for (mutation, expected) in cases {
        let result = gql(&f.schema, &mutation).await;
        assert_eq!(result["success"], true, "{mutation}");
        // Exactly one action ran, and it had already run when the response came back.
        assert_eq!(f.state.take_deferred(), [expected], "{mutation}");
        assert!(f.state.reported().is_empty());
    }
    f.owner.cleanup().unwrap();
}

#[tokio::test]
async fn graphql_bulk_writes_defer_once_per_row_after_commit() {
    let mut f = setup().await;
    let update = gql(
        &f.schema,
        r#"updateWriteRecords(where: {recordKey: {startsWith: "key"}}, input: {other: "bulk"}) { success affectedCount }"#,
    )
    .await;
    assert_eq!(update["affectedCount"], 3);
    assert_eq!(
        sorted(f.state.take_deferred()),
        [
            "deferred Updated key1 committed=true",
            "deferred Updated key2 committed=true",
            "deferred Updated key3 committed=true"
        ]
    );
    let delete = gql(
        &f.schema,
        r#"deleteWriteRecords(where: {recordKey: {startsWith: "key"}}) { success deletedCount }"#,
    )
    .await;
    assert_eq!(delete["deletedCount"], 3);
    assert_eq!(
        sorted(f.state.take_deferred()),
        [
            "deferred Deleted key1 committed=true",
            "deferred Deleted key2 committed=true",
            "deferred Deleted key3 committed=true"
        ]
    );
    assert!(f.state.reported().is_empty());
    f.owner.cleanup().unwrap();
}

#[tokio::test]
async fn repository_writes_defer_once_per_row_after_commit() {
    let mut f = setup().await;
    let db = &f.db;
    WriteRecord::insert_many(db, vec![create("a", "one"), create("b", "two")])
        .await
        .unwrap();
    assert_eq!(
        sorted(f.state.take_deferred()),
        [
            "deferred Created a committed=true",
            "deferred Created b committed=true"
        ]
    );
    assert_eq!(
        WriteRecord::update_where(db, by_prefix(), update_to("bulk"))
            .await
            .unwrap(),
        3
    );
    assert_eq!(f.state.take_deferred().len(), 3);
    WriteRecord::update_where_bounded(
        db,
        by_prefix(),
        update_to("bounded"),
        MutationLimit::new(3).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(f.state.take_deferred().len(), 3);
    assert_eq!(WriteRecord::delete_where(db, by_prefix()).await.unwrap(), 3);
    assert_eq!(f.state.take_deferred().len(), 3);
    WriteRecord::delete_where_bounded(
        db,
        WriteRecordWhereInput {
            record_key: Some(graphql_orm::graphql::filters::StringFilter {
                eq: Some("a".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        MutationLimit::new(5).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        f.state.take_deferred(),
        ["deferred Deleted a committed=true"]
    );
    WriteRecord::delete_all(db).await.unwrap();
    assert_eq!(
        f.state.take_deferred(),
        ["deferred Deleted b committed=true"]
    );
    assert!(values(db).await.is_empty());
    f.owner.cleanup().unwrap();
}

#[tokio::test]
async fn explicit_transactions_defer_hook_and_direct_actions_after_commit_in_order() {
    let mut f = setup().await;
    let state = f.state.clone();
    let direct = state.clone();
    let value =
        f.db.transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.update_by_id::<WriteRecord>(
                    &graphql_orm::uuid::Uuid::parse_str(ID1).unwrap(),
                    update_to("in-tx"),
                )
                .await
                .map_err(OrmPublicError::from)?;
                tx.defer(move |db| async move {
                    record_deferred(
                        &direct,
                        &db,
                        "direct".into(),
                        graphql_orm::uuid::Uuid::parse_str(ID1).unwrap(),
                        Some("in-tx".into()),
                    )
                    .await
                });
                // Nothing runs while the transaction is open.
                assert!(
                    !direct_log_has_deferred(&state),
                    "an action ran before commit"
                );
                Ok::<_, OrmPublicError>("returned")
            })
        })
        .await
        .map_err(|error| format!("{error:?}"))
        .unwrap();
    assert_eq!(value, "returned");
    assert_eq!(
        f.state.take_deferred(),
        [
            "deferred Updated key1 committed=true",
            "deferred direct committed=true"
        ]
    );
    f.owner.cleanup().unwrap();
}
fn direct_log_has_deferred(state: &State) -> bool {
    state
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|line| line.starts_with("deferred"))
}

/// Run `mutation` against a hook that refuses the Nth `After` call, and check that no action ran and
/// nothing was stored.
async fn refused(f: &Fixture, mutation: &str, nth: usize, registered: usize) {
    f.state.refuse_after_nth.store(nth, Ordering::SeqCst);
    f.state.after_calls.store(0, Ordering::SeqCst);
    let response = f.schema.execute(format!("mutation {{ {mutation} }}")).await;
    let json = response.data.into_json().unwrap();
    let payload = json
        .as_object()
        .and_then(|data| data.values().next().cloned())
        .unwrap_or(serde_json::Value::Null);
    assert!(
        !response.errors.is_empty() || payload["success"] == false,
        "the hook refusal must fail the write: {mutation}"
    );
    // The hook registered its actions before refusing, so the write really had actions queued.
    assert_eq!(
        f.state.after_calls.load(Ordering::SeqCst),
        registered,
        "{mutation}"
    );
    assert_eq!(f.state.take_deferred(), Vec::<String>::new(), "{mutation}");
    assert_eq!(values(&f.db).await, vec!["before"; 3], "{mutation}");
    assert!(f.state.reported().is_empty());
    f.state.refuse_after_nth.store(0, Ordering::SeqCst);
}

#[tokio::test]
async fn a_hook_refusal_runs_no_deferred_action() {
    let mut f = setup().await;
    for mutation in [
        r#"createWriteRecord(input: {recordKey: "new", required: "r", other: "after"}) { success }"#.to_string(),
        format!("updateWriteRecord(id: \"{ID1}\", input: {{other: \"after\"}}) {{ success }}"),
        r#"upsertWriteRecord(input: {recordKey: "key1", required: "r", other: "after"}) { success }"#.to_string(),
        format!("deleteWriteRecord(id: \"{ID1}\") {{ success }}"),
    ] {
        refused(&f, &mutation, 1, 1).await;
    }
    f.owner.cleanup().unwrap();
}

#[tokio::test]
async fn a_later_rows_refusal_discards_the_actions_of_earlier_rows() {
    let mut f = setup().await;
    // The third row's hook refuses after rows one and two registered actions.
    refused(
        &f,
        r#"updateWriteRecords(where: {recordKey: {startsWith: "key"}}, input: {other: "after"}) { success }"#,
        3,
        3,
    )
    .await;
    refused(
        &f,
        r#"deleteWriteRecords(where: {recordKey: {startsWith: "key"}}) { success }"#,
        3,
        3,
    )
    .await;

    f.state.refuse_after_nth.store(3, Ordering::SeqCst);
    f.state.after_calls.store(0, Ordering::SeqCst);
    assert!(
        WriteRecord::update_where(&f.db, by_prefix(), update_to("after"))
            .await
            .is_err()
    );
    assert_eq!(f.state.after_calls.load(Ordering::SeqCst), 3);
    assert!(f.state.take_deferred().is_empty());
    f.state.after_calls.store(0, Ordering::SeqCst);
    assert!(
        WriteRecord::insert_many(
            &f.db,
            vec![create("a", "x"), create("b", "x"), create("c", "x")]
        )
        .await
        .is_err()
    );
    assert!(f.state.take_deferred().is_empty());
    assert_eq!(values(&f.db).await, vec!["before"; 3]);
    f.owner.cleanup().unwrap();
}

#[tokio::test]
async fn a_database_failure_runs_no_deferred_action() {
    let mut f = setup().await;
    f.state.defer_in_before.store(true, Ordering::SeqCst);
    // Single row: the unique constraint refuses the write after the Before hook queued an action.
    let response = f
        .schema
        .execute(format!(
            "mutation {{ updateWriteRecord(id: \"{ID1}\", input: {{recordKey: \"key2\"}}) {{ success }} }}"
        ))
        .await;
    let json = response.data.into_json().unwrap();
    assert!(
        !response.errors.is_empty() || json["updateWriteRecord"]["success"] == false,
        "{json:?}"
    );
    assert_eq!(f.state.before_calls.load(Ordering::SeqCst), 1);
    assert!(f.state.take().is_empty());

    // Bulk: the first row writes, queues its actions, and a later row hits the same constraint.
    f.state.before_calls.store(0, Ordering::SeqCst);
    let bulk = gql(
        &f.schema,
        r#"updateWriteRecords(where: {recordKey: {startsWith: "key"}}, input: {recordKey: "dup"}) { success }"#,
    )
    .await;
    assert_eq!(bulk["success"], false);
    assert!(f.state.before_calls.load(Ordering::SeqCst) >= 2);
    assert!(f.state.take().is_empty());
    assert!(
        WriteRecord::update_where(
            &f.db,
            by_prefix(),
            UpdateWriteRecordInput {
                record_key: Some("dup".into()),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    assert!(f.state.take().is_empty());
    assert_eq!(values(&f.db).await, vec!["before"; 3]);
    assert!(f.state.reported().is_empty());
    f.owner.cleanup().unwrap();
}

#[tokio::test]
async fn a_rolled_back_explicit_transaction_runs_no_deferred_action() {
    let mut f = setup().await;
    let direct = f.state.clone();
    let result =
        f.db.transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.update_by_id::<WriteRecord>(
                    &graphql_orm::uuid::Uuid::parse_str(ID1).unwrap(),
                    update_to("in-tx"),
                )
                .await
                .map_err(OrmPublicError::from)?;
                tx.defer(move |_db| async move {
                    direct.push("deferred direct".into());
                    Ok::<(), String>(())
                });
                Err::<(), _>(OrmPublicError::new(OrmErrorCode::Conflict))
            })
        })
        .await;
    assert!(result.is_err());
    assert!(f.state.take().is_empty());
    assert_eq!(values(&f.db).await, vec!["before"; 3]);

    // A database failure inside the callback rolls back too, including the earlier hook action.
    let result =
        f.db.transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.update_by_id::<WriteRecord>(
                    &graphql_orm::uuid::Uuid::parse_str(ID1).unwrap(),
                    update_to("in-tx"),
                )
                .await
                .map_err(OrmPublicError::from)?;
                tx.update_by_id::<WriteRecord>(
                    &graphql_orm::uuid::Uuid::parse_str(ID1).unwrap(),
                    UpdateWriteRecordInput {
                        record_key: Some("key2".into()),
                        ..Default::default()
                    },
                )
                .await
                .map_err(OrmPublicError::from)?;
                Ok::<(), OrmPublicError>(())
            })
        })
        .await;
    assert!(result.is_err());
    assert!(f.state.take().is_empty());
    assert_eq!(values(&f.db).await, vec!["before"; 3]);
    f.owner.cleanup().unwrap();
}

#[tokio::test]
async fn a_failed_deferred_action_is_reported_and_the_committed_result_stands() {
    let mut f = setup().await;
    f.state.fail_deferred.store(true, Ordering::SeqCst);

    let single = gql(
        &f.schema,
        &format!("updateWriteRecord(id: \"{ID1}\", input: {{other: \"one\"}}) {{ success error }}"),
    )
    .await;
    assert_eq!(single["success"], true);
    assert!(single["error"].is_null());
    assert_eq!(f.state.reported(), ["deferred failed"]);
    assert!(values(&f.db).await.contains(&"one".to_string()));

    // A failure doesn't stop the remaining actions of the same commit.
    f.state.reported.lock().unwrap().clear();
    f.state.take();
    let bulk = gql(
        &f.schema,
        r#"updateWriteRecords(where: {recordKey: {startsWith: "key"}}, input: {other: "bulk"}) { success affectedCount }"#,
    )
    .await;
    assert_eq!(bulk["success"], true);
    assert_eq!(bulk["affectedCount"], 3);
    assert_eq!(f.state.take_deferred().len(), 3);
    assert_eq!(f.state.reported(), vec!["deferred failed"; 3]);
    assert_eq!(values(&f.db).await, vec!["bulk"; 3]);

    f.state.reported.lock().unwrap().clear();
    assert_eq!(
        WriteRecord::update_where(&f.db, by_prefix(), update_to("repo"))
            .await
            .unwrap(),
        3
    );
    assert_eq!(f.state.reported(), vec!["deferred failed"; 3]);
    assert_eq!(values(&f.db).await, vec!["repo"; 3]);

    f.state.reported.lock().unwrap().clear();
    let value =
        f.db.transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.update_by_id::<WriteRecord>(
                    &graphql_orm::uuid::Uuid::parse_str(ID1).unwrap(),
                    update_to("tx"),
                )
                .await
                .map_err(OrmPublicError::from)?;
                Ok::<_, OrmPublicError>("committed")
            })
        })
        .await
        .map_err(|error| format!("{error:?}"))
        .unwrap();
    assert_eq!(value, "committed");
    assert_eq!(f.state.reported(), ["deferred failed"]);
    assert!(values(&f.db).await.contains(&"tx".to_string()));
    f.owner.cleanup().unwrap();
}

#[tokio::test]
async fn changed_events_precede_deferred_actions_which_precede_notify_with() {
    let mut f = setup().await;
    *f.state.changed.lock().unwrap() = Some(
        f.db.ensure_event_sender::<WriteRecordChangedEvent>()
            .subscribe(),
    );

    gql(
        &f.schema,
        &format!("updateWriteRecord(id: \"{ID1}\", input: {{other: \"one\"}}) {{ success }}"),
    )
    .await;
    assert_eq!(
        f.state.take(),
        [
            "changed",
            "deferred Updated key1 committed=true",
            "notify Updated key1"
        ]
    );

    gql(
        &f.schema,
        r#"updateWriteRecords(where: {recordKey: {startsWith: "key"}}, input: {other: "bulk"}) { success }"#,
    )
    .await;
    let log = f.state.take();
    let kinds: Vec<_> = log
        .iter()
        .map(|line| line.split(' ').next().unwrap())
        .collect();
    // Every `Changed` event, then every deferred action, then every `notify_with` call.
    assert_eq!(
        kinds,
        [
            "changed", "changed", "changed", "deferred", "deferred", "deferred", "notify",
            "notify", "notify"
        ],
        "{log:?}"
    );

    gql(
        &f.schema,
        r#"deleteWriteRecords(where: {recordKey: {startsWith: "key"}}) { success }"#,
    )
    .await;
    let kinds: Vec<_> = f
        .state
        .take()
        .iter()
        .map(|line| line.split(' ').next().unwrap().to_string())
        .collect();
    assert_eq!(
        kinds,
        [
            "changed", "changed", "changed", "deferred", "deferred", "deferred", "notify",
            "notify", "notify"
        ]
    );

    // Outside GraphQL there is no `notify_with`, so the order is `Changed` then deferred.
    WriteRecord::insert(&f.db, create("fresh", "x"))
        .await
        .unwrap();
    assert_eq!(
        f.state.take(),
        ["changed", "deferred Created fresh committed=true"]
    );
    f.owner.cleanup().unwrap();
}
