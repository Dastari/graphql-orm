#![cfg(feature = "sqlite")]
use graphql_orm::prelude::*;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicI64, AtomicUsize, Ordering},
};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "visibility_items",
    plural = "VisibilityItems",
    keyset = "priority asc nulls last, rank asc, id asc"
)]
#[graphql_orm(operation_authorization(categories = ["keyset_list"], all_scopes = ["records.page"]))]
struct VisibilityItem {
    #[primary_key]
    #[filterable(type = "uuid")]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "number")]
    #[sortable]
    rank: i64,
    #[filterable(type = "number")]
    priority: Option<i64>,
    #[filterable(type = "string")]
    owner: String,
    #[graphql_orm(read_policy = "payload.read")]
    payload: String,
}

#[derive(Clone)]
struct ServerGeneration(Arc<AtomicI64>);

#[derive(Default)]
struct CustomQuery;

#[async_graphql::Object]
impl CustomQuery {
    async fn protected_page(
        &self,
        ctx: &async_graphql::Context<'_>,
        after: Option<String>,
    ) -> async_graphql::Result<String> {
        let db = ctx.data::<Database>()?;
        let generation = ctx.data::<ServerGeneration>()?.0.load(Ordering::Relaxed);
        let context = AuthorizedScanContext::new(generation.to_be_bytes())?;
        let page = EntityQuery::<VisibilityItem, SqliteBackend>::new()
            .fetch_authorized_scan_with_context(
                db,
                Some(ctx),
                None,
                AuthorizedScanInput {
                    after,
                    limit: Some(3),
                    ..Default::default()
                },
                &context,
            )
            .await?;
        let current_generation = ctx.data::<ServerGeneration>()?.0.load(Ordering::Relaxed);
        if AuthorizedScanContext::new(current_generation.to_be_bytes())? != context {
            return Err(OrmPublicError::new(OrmErrorCode::CursorInvalid).into_graphql_error());
        }
        Ok(json!({
            "ids": page.nodes.iter().map(|node| node.id.to_string()).collect::<Vec<_>>(),
            "cursor": page.page_info.continuation,
        })
        .to_string())
    }
}

schema_roots! { query_custom_ops: [], entities: [VisibilityItem], extra_query_types: [CustomQuery], }

#[derive(Clone, Copy)]
enum Mode {
    Callback,
    Partial,
    Complete,
    Unrestricted,
    Error,
}
#[derive(Clone)]
struct Policy {
    mode: Mode,
    minimum: Arc<AtomicI64>,
    evaluations: Arc<AtomicUsize>,
    mutate_generation_after_row: Option<Arc<AtomicI64>>,
}
fn owner<'a>(ctx: Option<&'a async_graphql::Context<'_>>) -> &'a str {
    ctx.and_then(|ctx| ctx.data_opt::<String>())
        .map(String::as_str)
        .unwrap_or("a")
}
impl RowPolicy for Policy {
    fn read_visibility<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            let filter = VisibilityItemWhereInput {
                owner: Some(StringFilter {
                    eq: Some(owner(ctx).to_string()),
                    ..Default::default()
                }),
                rank: matches!(self.mode, Mode::Complete).then(|| IntFilter {
                    gte: Some(self.minimum.load(Ordering::Relaxed) as i32),
                    ..Default::default()
                }),
                ..Default::default()
            };
            Ok(match self.mode {
                Mode::Callback => ReadVisibility::CallbackOnly,
                Mode::Partial => ReadVisibility::Prefilter(ReadPredicate::from_filter::<
                    SqliteBackend,
                    _,
                >(&filter)?),
                Mode::Complete => ReadVisibility::Complete(ReadPredicate::from_filter::<
                    SqliteBackend,
                    _,
                >(&filter)?),
                Mode::Unrestricted => ReadVisibility::Unrestricted,
                Mode::Error => return Err(OrmPublicError::forbidden().into_graphql_error()),
            })
        })
    }
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            self.evaluations.fetch_add(1, Ordering::Relaxed);
            if let Some(generation) = &self.mutate_generation_after_row {
                generation.store(2, Ordering::Relaxed);
            }
            let row = row.downcast_ref::<VisibilityItem>().unwrap();
            // A complete decision deliberately bypasses this callback.
            Ok(!matches!(self.mode, Mode::Unrestricted | Mode::Complete)
                && row.owner == owner(ctx)
                && row.rank >= self.minimum.load(Ordering::Relaxed))
        })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<(String, usize)>>>);
impl ReadQueryObserver for Capture {
    fn on_read(&self, sql: &str, rows: usize) {
        self.0.lock().unwrap().push((sql.to_string(), rows));
    }
}
impl Capture {
    fn take(&self) -> Vec<(String, usize)> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}
async fn setup(mode: Mode, min: i64, budget: u32) -> (Database, Policy, Capture) {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE visibility_items (id TEXT PRIMARY KEY, rank INTEGER NOT NULL, priority INTEGER, owner TEXT NOT NULL, payload TEXT NOT NULL)")
        .execute(database.pool()).await.unwrap();
    for i in 0..120 {
        sqlx::query("INSERT INTO visibility_items VALUES (?, ?, ?, ?, ?)")
            .bind(graphql_orm::uuid::Uuid::from_u128(i + 1).to_string())
            .bind((i / 2) as i64)
            .bind(if i < 100 { Some(1_i64) } else { None })
            .bind(if i % 2 == 0 { "a" } else { "b" })
            .bind("x".repeat(1024))
            .execute(database.pool())
            .await
            .unwrap();
    }
    let policy = Policy {
        mode,
        minimum: Arc::new(AtomicI64::new(min)),
        evaluations: Arc::new(AtomicUsize::new(0)),
        mutate_generation_after_row: None,
    };
    let capture = Capture::default();
    let mut database = database;
    database.set_row_policy(policy.clone());
    database.set_field_policy(ReadPayload);
    let database = database
        .with_authorized_scan_config(AuthorizedScanConfig::new([7; 32], 7, budget))
        .with_read_query_observer(capture.clone());
    (database, policy, capture)
}
async fn execute(db: &Database, user: Option<&str>, query: String) -> async_graphql::Response {
    let schema = schema_builder(db.clone()).finish();
    let request = async_graphql::Request::new(query);
    let request = if let Some(user) = user {
        request.data(user.to_string()).data(
            AuthSubject::builder(user)
                .scopes(vec!["records.page".into()])
                .build(),
        )
    } else {
        request
    };
    schema.execute(request).await
}
async fn execute_custom_page(
    schema: &AppSchema,
    generation: Arc<AtomicI64>,
    after: Option<&str>,
) -> async_graphql::Response {
    let query = format!(
        "{{ protectedPage(after: {}) }}",
        serde_json::to_string(&after).unwrap(),
    );
    let request = async_graphql::Request::new(query)
        .data("a".to_string())
        .data(AuthSubject::builder("a").scopes(vec!["records.page".into()]).build())
        .data(ServerGeneration(generation));
    schema.execute(request).await
}
async fn data(db: &Database, user: &str, query: String) -> Value {
    let response = execute(db, Some(user), query).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()
}
fn scan_query(after: Option<&str>, limit: usize) -> String {
    format!(
        "{{ visibilityItemsScan(page: {{limit: {limit}, after: {}}}) {{ nodes {{ id rank owner priority }} pageInfo {{ status continuation totalCount }} }} }}",
        serde_json::to_string(&after).unwrap()
    )
}
fn assert_bounded(queries: &[(String, usize)], max: usize) {
    assert!(!queries.is_empty());
    for (sql, rows) in queries {
        if !sql.contains("COUNT(") {
            assert!(sql.contains("LIMIT"), "unbounded SQL: {sql}");
            assert!(*rows <= max, "{rows} rows: {sql}");
        }
    }
}

#[tokio::test]
async fn custom_resolver_reads_current_server_context_on_each_request() {
    let (db, _, capture) = setup(Mode::Callback, 0, 19).await;
    let schema = schema_builder(db).finish();
    let generation = Arc::new(AtomicI64::new(1));
    let first = execute_custom_page(&schema, generation.clone(), None).await;
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    let first: Value = serde_json::from_str(
        first.data.into_json().unwrap()["protectedPage"].as_str().unwrap(),
    )
    .unwrap();
    let cursor = first["cursor"].as_str().unwrap();
    assert_eq!(first["ids"].as_array().unwrap().len(), 3);

    let second = execute_custom_page(&schema, generation.clone(), Some(cursor)).await;
    assert!(second.errors.is_empty(), "{:?}", second.errors);
    let second: Value = serde_json::from_str(
        second.data.into_json().unwrap()["protectedPage"].as_str().unwrap(),
    )
    .unwrap();
    assert_ne!(first["ids"], second["ids"]);
    assert_bounded(&capture.take(), 7);

    generation.store(2, Ordering::Relaxed);
    let changed = execute_custom_page(&schema, generation, Some(cursor)).await;
    assert!(!changed.errors.is_empty());
    assert!(capture.take().is_empty(), "mismatch must fail before a read");
}

#[tokio::test]
async fn custom_resolver_discards_page_when_context_changes_during_scan() {
    let (mut db, mut policy, capture) = setup(Mode::Callback, 0, 19).await;
    let generation = Arc::new(AtomicI64::new(1));
    policy.mutate_generation_after_row = Some(generation.clone());
    db.set_row_policy(policy);
    let schema = schema_builder(db).finish();

    let result = execute_custom_page(&schema, generation, None).await;
    assert!(!result.errors.is_empty());
    assert!(result.data.into_json().unwrap()["protectedPage"].is_null());
    assert_bounded(&capture.take(), 7);
}

#[tokio::test]
async fn unrestricted_and_complete_execute_database_limits_and_authorized_counts() {
    for (mode, expected) in [(Mode::Unrestricted, 100), (Mode::Complete, 50)] {
        let (db, policy, capture) = setup(mode, 10, 30).await;
        let result = data(&db, "a", "{ visibilityItems(where: {rank: {gte: 10}}, orderBy: [{rank: DESC}], page: {limit: 5, offset: 2}) { edges { node { rank owner } } pageInfo { totalCount hasNextPage } } }".into()).await;
        assert_eq!(
            result["visibilityItems"]["pageInfo"]["totalCount"],
            expected
        );
        let ranks = result["visibilityItems"]["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["node"]["rank"].as_i64().unwrap())
            .collect::<Vec<_>>();
        assert!(ranks.windows(2).all(|w| w[0] >= w[1]));
        assert_bounded(&capture.take(), 5);
        assert_eq!(policy.evaluations.load(Ordering::Relaxed), 0);
        let result = data(&db, "a", "{ visibilityItemsKeyset(page: {limit: 5, includeTotalCount: true}) { edges { node { owner rank } } pageInfo { totalCount hasNextPage } } }".into()).await;
        assert_eq!(
            result["visibilityItemsKeyset"]["pageInfo"]["totalCount"],
            if matches!(mode, Mode::Unrestricted) {
                120
            } else {
                50
            }
        );
        assert_bounded(&capture.take(), 6);
        data(&db, "a", "{ visibilityItemsKeyset(page: {limit: 5}) { edges { node { id } } pageInfo { totalCount } } }".into()).await;
        assert!(
            capture
                .take()
                .iter()
                .all(|(sql, _)| !sql.contains("COUNT("))
        );
    }
}

#[tokio::test]
async fn scan_crosses_denied_batches_and_traverses_equal_and_nullable_keys() {
    for mode in [Mode::Callback, Mode::Partial] {
        let (db, _, capture) = setup(mode, 40, 19).await;
        let first = data(&db, "a", scan_query(None, 6)).await;
        assert!(
            first["visibilityItemsScan"]["nodes"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            first["visibilityItemsScan"]["pageInfo"]["status"],
            "BUDGET_EXHAUSTED"
        );
        let mut cursor = first["visibilityItemsScan"]["pageInfo"]["continuation"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(cursor.starts_with("goms1."));
        let mut ids = std::collections::HashSet::new();
        let mut ranks = Vec::new();
        for _ in 0..30 {
            let result = data(&db, "a", scan_query(Some(&cursor), 6)).await;
            let page = &result["visibilityItemsScan"];
            for row in page["nodes"].as_array().unwrap() {
                assert_eq!(row["owner"], "a");
                assert!(
                    ids.insert(row["id"].as_str().unwrap().to_owned()),
                    "duplicate"
                );
                ranks.push(row["rank"].as_i64().unwrap());
            }
            if page["pageInfo"]["status"] == "EXHAUSTED" {
                assert!(page["pageInfo"]["continuation"].is_null());
                break;
            }
            cursor = page["pageInfo"]["continuation"]
                .as_str()
                .unwrap()
                .to_owned();
        }
        assert_eq!(ranks, (40..60).collect::<Vec<_>>());
        let queries = capture.take();
        assert_bounded(&queries, 7);
        assert!(queries.iter().all(|(sql, _)| !sql.contains("COUNT(")));
    }
}

#[tokio::test]
async fn cursors_reauthorize_changes_and_cross_user_reuse_and_reject_tampering() {
    for mode in [Mode::Callback, Mode::Complete] {
        let (db, policy, _) = setup(mode, 0, 60).await;
        let first = data(&db, "a", scan_query(None, 3)).await;
        let cursor = first["visibilityItemsScan"]["pageInfo"]["continuation"]
            .as_str()
            .unwrap();
        policy.minimum.store(30, Ordering::Relaxed);
        let next = data(&db, "b", scan_query(Some(cursor), 3)).await;
        for row in next["visibilityItemsScan"]["nodes"].as_array().unwrap() {
            assert_eq!(row["owner"], "b");
            assert!(row["rank"].as_i64().unwrap() >= 30);
        }
        assert!(
            !execute(&db, Some("a"), scan_query(Some(&format!("{cursor}00")), 3))
                .await
                .errors
                .is_empty()
        );
        assert!(
            !execute(&db, None, scan_query(Some(cursor), 3))
                .await
                .errors
                .is_empty()
        );
    }
}

#[tokio::test]
async fn custom_scan_context_binds_each_server_stamp_and_keeps_current_policy() {
    let (db, policy, capture) = setup(Mode::Callback, 0, 19).await;
    let original = AuthorizedScanContext::new(b"snapshot=1|registration=7|incarnation=a|workspace=w|original=p|current=p|grants=3|credential=4|configuration=5|placement=6").unwrap();
    let first = EntityQuery::<VisibilityItem, SqliteBackend>::new()
        .fetch_authorized_scan_with_context(&db, None, None, AuthorizedScanInput { limit: Some(3), ..Default::default() }, &original)
        .await.unwrap();
    assert_eq!(first.nodes.len(), 3);
    let cursor = first.page_info.continuation.unwrap();
    assert!(cursor.starts_with("goms2."));
    capture.take();

    for changed in [
        b"snapshot=2|registration=7|incarnation=a|workspace=w|original=p|current=p|grants=3|credential=4|configuration=5|placement=6".as_slice(),
        b"snapshot=1|registration=8|incarnation=a|workspace=w|original=p|current=p|grants=3|credential=4|configuration=5|placement=6",
        b"snapshot=1|registration=7|incarnation=b|workspace=w|original=p|current=p|grants=3|credential=4|configuration=5|placement=6",
        b"snapshot=1|registration=7|incarnation=a|workspace=x|original=p|current=p|grants=3|credential=4|configuration=5|placement=6",
        b"snapshot=1|registration=7|incarnation=a|workspace=w|original=q|current=p|grants=3|credential=4|configuration=5|placement=6",
        b"snapshot=1|registration=7|incarnation=a|workspace=w|original=p|current=q|grants=3|credential=4|configuration=5|placement=6",
        b"snapshot=1|registration=7|incarnation=a|workspace=w|original=p|current=p|grants=4|credential=4|configuration=5|placement=6",
        b"snapshot=1|registration=7|incarnation=a|workspace=w|original=p|current=p|grants=3|credential=5|configuration=5|placement=6",
        b"snapshot=1|registration=7|incarnation=a|workspace=w|original=p|current=p|grants=3|credential=4|configuration=6|placement=6",
        b"snapshot=1|registration=7|incarnation=a|workspace=w|original=p|current=p|grants=3|credential=4|configuration=5|placement=7",
    ] {
        let changed = AuthorizedScanContext::new(changed).unwrap();
        let result = EntityQuery::<VisibilityItem, SqliteBackend>::new()
            .fetch_authorized_scan_with_context(&db, None, None, AuthorizedScanInput { after: Some(cursor.clone()), limit: Some(3), ..Default::default() }, &changed)
            .await;
        assert!(result.is_err());
        assert!(capture.take().is_empty(), "mismatch must fail before a read");
    }

    policy.minimum.store(30, Ordering::Relaxed);
    let next = EntityQuery::<VisibilityItem, SqliteBackend>::new()
        .fetch_authorized_scan_with_context(&db, None, None, AuthorizedScanInput { after: Some(cursor.clone()), limit: Some(3), ..Default::default() }, &original)
        .await.unwrap();
    assert!(next.nodes.iter().all(|node| node.rank >= 30));
    assert_bounded(&capture.take(), 7);

    for invalid_cursor in [format!("{cursor}00"), cursor[..cursor.len() - 2].to_string(), "goms9.00".into(), format!("goms2.{}", "a".repeat(32770))] {
        assert!(EntityQuery::<VisibilityItem, SqliteBackend>::new()
            .fetch_authorized_scan_with_context(&db, None, None, AuthorizedScanInput { after: Some(invalid_cursor), limit: Some(3), ..Default::default() }, &original)
            .await.is_err());
    }
    assert!(AuthorizedScanContext::new(b"").is_err());
    assert!(AuthorizedScanContext::new(vec![0; 2049]).is_err());
}

#[tokio::test]
async fn custom_scan_context_rejects_original_credential_and_grant_reversal() {
    let (db, _, capture) = setup(Mode::Callback, 0, 19).await;
    let original_state = json!({
        "snapshot": 1,
        "original_credential_id": "credential-a",
        "original_credential_expiry": 1000,
        "current_grants": ["records.page"],
        "grant_epoch": 3,
    });
    let context = AuthorizedScanContext::new(serde_json::to_vec(&original_state).unwrap()).unwrap();
    let first = EntityQuery::<VisibilityItem, SqliteBackend>::new()
        .fetch_authorized_scan_with_context(&db, None, None, AuthorizedScanInput { limit: Some(3), ..Default::default() }, &context)
        .await.unwrap();
    let cursor = first.page_info.continuation.unwrap();
    capture.take();

    let mut changed_id = original_state.clone();
    changed_id["original_credential_id"] = json!("credential-b");
    let mut changed_expiry = original_state.clone();
    changed_expiry["original_credential_expiry"] = json!(1001);
    let mut withdrawn = original_state.clone();
    withdrawn["current_grants"] = json!([]);
    withdrawn["grant_epoch"] = json!(4);
    let mut restored = original_state;
    restored["grant_epoch"] = json!(5);
    for state in [changed_id, changed_expiry, withdrawn, restored] {
        let changed = AuthorizedScanContext::new(serde_json::to_vec(&state).unwrap()).unwrap();
        assert!(EntityQuery::<VisibilityItem, SqliteBackend>::new()
            .fetch_authorized_scan_with_context(&db, None, None, AuthorizedScanInput { after: Some(cursor.clone()), limit: Some(3), ..Default::default() }, &changed)
            .await.is_err());
        assert!(capture.take().is_empty(), "changed context must fail before a read");
    }
}

#[tokio::test]
async fn custom_scan_context_rotates_keys_and_preserves_empty_residual_pages() {
    let (db, _, _) = setup(Mode::Callback, 40, 19).await;
    let context = AuthorizedScanContext::new(b"dataset=one|owner=one").unwrap();
    let first = EntityQuery::<VisibilityItem, SqliteBackend>::new()
        .fetch_authorized_scan_with_context(&db, None, None, AuthorizedScanInput { limit: Some(6), ..Default::default() }, &context)
        .await.unwrap();
    assert!(first.nodes.is_empty());
    assert_eq!(first.page_info.status, ScanStatus::BudgetExhausted);
    let cursor = first.page_info.continuation.unwrap();
    let replica = db.clone().with_authorized_scan_config(
        AuthorizedScanConfig::new([7; 32], 7, 19),
    );
    let same_key = EntityQuery::<VisibilityItem, SqliteBackend>::new()
        .fetch_authorized_scan_with_context(&replica, None, None, AuthorizedScanInput { after: Some(cursor.clone()), limit: Some(6), ..Default::default() }, &context)
        .await.unwrap();
    assert!(same_key.nodes.is_empty());
    let aliased_key = db.clone().with_authorized_scan_config(
        AuthorizedScanConfig::new([7; 32], 7, 19)
            .with_protected_key_id(2)
            .with_retired_protected_key(0, [7; 32], std::time::SystemTime::now() + std::time::Duration::from_secs(60)).unwrap()
    );
    let forged_id = cursor.replacen("goms2.00000000", "goms2.00000002", 1);
    assert_ne!(forged_id, cursor);
    assert!(EntityQuery::<VisibilityItem, SqliteBackend>::new()
        .fetch_authorized_scan_with_context(&aliased_key, None, None, AuthorizedScanInput { after: Some(forged_id), limit: Some(6), ..Default::default() }, &context)
        .await.is_err());
    let rotated = db.clone().with_authorized_scan_config(
        AuthorizedScanConfig::new([8; 32], 7, 19)
            .with_protected_key_id(2)
            .with_retired_protected_key(0, [7; 32], std::time::SystemTime::now() + std::time::Duration::from_secs(60)).unwrap()
    );
    let continued = EntityQuery::<VisibilityItem, SqliteBackend>::new()
        .fetch_authorized_scan_with_context(&rotated, None, None, AuthorizedScanInput { after: Some(cursor.clone()), limit: Some(6), ..Default::default() }, &context)
        .await.unwrap();
    assert!(matches!(continued.page_info.status, ScanStatus::BudgetExhausted | ScanStatus::PageFull | ScanStatus::Exhausted));
    let wrong_key = db.with_authorized_scan_config(AuthorizedScanConfig::new([9; 32], 7, 19));
    assert!(EntityQuery::<VisibilityItem, SqliteBackend>::new()
        .fetch_authorized_scan_with_context(&wrong_key, None, None, AuthorizedScanInput { after: Some(cursor), limit: Some(6), ..Default::default() }, &context)
        .await.is_err());
}

#[tokio::test]
async fn custom_scan_context_rejects_expired_and_retired_cursors() {
    let (db, _, _) = setup(Mode::Complete, 0, 19).await;
    let context = AuthorizedScanContext::new(b"dataset=one|owner=one").unwrap();
    let short = db.with_authorized_scan_config(
        AuthorizedScanConfig::new([7; 32], 7, 19)
            .with_protected_ttl(std::time::Duration::from_secs(1)).unwrap()
    );
    let first = EntityQuery::<VisibilityItem, SqliteBackend>::new()
        .fetch_authorized_scan_with_context(&short, None, None, AuthorizedScanInput { limit: Some(3), ..Default::default() }, &context)
        .await.unwrap();
    let cursor = first.page_info.continuation.unwrap();
    let retired = short.clone().with_authorized_scan_config(
        AuthorizedScanConfig::new([8; 32], 7, 19)
            .with_protected_key_id(2)
            .with_retired_protected_key(0, [7; 32], std::time::UNIX_EPOCH).unwrap()
    );
    assert!(EntityQuery::<VisibilityItem, SqliteBackend>::new()
        .fetch_authorized_scan_with_context(&retired, None, None, AuthorizedScanInput { after: Some(cursor.clone()), limit: Some(3), ..Default::default() }, &context)
        .await.is_err());
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert!(EntityQuery::<VisibilityItem, SqliteBackend>::new()
        .fetch_authorized_scan_with_context(&short, None, None, AuthorizedScanInput { after: Some(cursor), limit: Some(3), ..Default::default() }, &context)
        .await.is_err());
}

#[tokio::test]
async fn bounded_offsets_stay_exact_and_provider_errors_fail_before_querying() {
    for mode in [Mode::Callback, Mode::Partial] {
        let (db, _, capture) = setup(mode, 40, 128).await;
        let result = data(&db, "a", "{ visibilityItems(page: {limit: 3, offset: 4}, orderBy: [{rank: ASC}]) { edges { node { rank } } pageInfo { totalCount } } }".into()).await;
        assert_eq!(result["visibilityItems"]["pageInfo"]["totalCount"], 20);
        assert_eq!(result["visibilityItems"]["edges"][0]["node"]["rank"], 44);
        let rows = capture.take();
        assert!(rows.iter().all(|(_, count)| *count <= 7));

        assert!(!execute(&db, Some("a"), "{ visibilityItemsScan(page: {limit: 3, includeTotalCount: true}) { nodes { id } } }".into()).await.errors.is_empty());
    }
    let (db, _, capture) = setup(Mode::Error, 0, 19).await;
    for query in [
        scan_query(None, 3),
        "{ visibilityItems(page: {limit: 3}) { edges {node {id}} } }".into(),
        "{ visibilityItemsKeyset(page: {limit: 3}) { edges {node {id}} } }".into(),
    ] {
        assert!(!execute(&db, Some("a"), query).await.errors.is_empty());
        assert!(capture.take().is_empty());
    }
}

#[tokio::test]
async fn predicate_composition_and_repository_paths() {
    let (db, _, _) = setup(Mode::Complete, 10, 30).await;
    let filter = VisibilityItemWhereInput {
        id: Some(UuidFilter {
            in_list: Some(vec![
                graphql_orm::uuid::Uuid::from_u128(23),
                graphql_orm::uuid::Uuid::from_u128(25),
            ]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let p = ReadPredicate::from_filter::<SqliteBackend, _>(&filter).unwrap();
    let composed = p.clone().and(p.clone().or(p).unwrap()).unwrap();
    let rows = EntityQuery::<VisibilityItem>::new()
        .with_read_visibility(&ReadVisibility::Complete(composed))
        .unwrap()
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(
        ReadPredicate::from_filter::<SqliteBackend, _>(&VisibilityItemWhereInput::default())
            .is_err()
    );
    let page = VisibilityItem::keyset_page(
        &db,
        filter,
        KeysetPageInput {
            limit: Some(1),
            include_total_count: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(page.page_info.total_count, Some(2));
    assert!(page.page_info.has_next_page);
    let tail = VisibilityItem::keyset_connection_page(
        &db,
        VisibilityItemWhereInput::default(),
        KeysetConnectionInput {
            last: Some(3),
            include_total_count: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(tail.page_info.total_count, Some(50));
    assert_eq!(tail.edges.last().unwrap().node.rank, 59);
}

/// Reproduce: cargo test -p graphql-orm --release --test authorized_pagination benchmark -- --ignored --nocapture
#[tokio::test]
#[ignore = "large release-mode pagination benchmark"]
async fn benchmark() {
    for sparse in [false, true] {
        for mode in [Mode::Callback, Mode::Complete, Mode::Unrestricted] {
            let prefix = format!(
                "{}-{}-",
                if sparse { "sparse" } else { "dense" },
                match mode {
                    Mode::Callback => "callback",
                    Mode::Complete => "complete",
                    _ => "unrestricted",
                }
            );
            if std::env::var("PAGINATION_BENCH_CASE")
                .is_ok_and(|selected| !selected.starts_with(&prefix))
            {
                continue;
            }
            let (db, policy, capture) = setup(mode, if sparse { 279000 } else { 0 }, 4096).await;
            let db = db.with_authorized_scan_config(AuthorizedScanConfig::new([7; 32], 256, 4096));
            sqlx::query("DELETE FROM visibility_items")
                .execute(db.pool())
                .await
                .unwrap();
            sqlx::query("WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<279286) INSERT INTO visibility_items SELECT printf('%08x-0000-0000-0000-%012x', x, x), x, 1, 'a', printf('%01024d', x) FROM n").execute(db.pool()).await.unwrap();
            sqlx::query("CREATE INDEX visibility_order ON visibility_items(CASE WHEN priority IS NULL THEN 1 ELSE 0 END, priority, CASE WHEN rank IS NULL THEN 1 ELSE 0 END, rank, CASE WHEN id IS NULL THEN 1 ELSE 0 END, id)")
                .execute(db.pool())
                .await
                .unwrap();
            for scan in [false, true] {
                let case = format!(
                    "{}-{}-{}",
                    if sparse { "sparse" } else { "dense" },
                    match mode {
                        Mode::Callback => "callback",
                        Mode::Complete => "complete",
                        _ => "unrestricted",
                    },
                    if scan { "scan" } else { "offset" }
                );
                if std::env::var("PAGINATION_BENCH_CASE").is_ok_and(|selected| selected != case) {
                    continue;
                }
                capture.take();
                policy.evaluations.store(0, Ordering::Relaxed);
                let started = std::time::Instant::now();
                let result = data(&db, "a", if scan {scan_query(None, 50)} else {"{ visibilityItems(page: {limit: 50}, orderBy: [{rank: ASC}]) { edges {node {id}} pageInfo {totalCount} } }".into()}).await;
                let elapsed = started.elapsed();
                let queries = capture.take();
                let fetched: usize = queries
                    .iter()
                    .filter(|(q, _)| !q.contains("COUNT("))
                    .map(|(_, n)| n)
                    .sum();
                println!(
                    "{}",
                    json!({"sparse":sparse,"mode":match mode {Mode::Callback=>"callback",Mode::Complete=>"complete",_=>"unrestricted"},"scan":scan,"status":result["visibilityItemsScan"]["pageInfo"]["status"],"max_batch_rows":queries.iter().filter(|(q,_)| !q.contains("COUNT(")).map(|(_,n)| *n).max().unwrap_or(0),"fetched_decoded":fetched,"policy_evaluations":policy.evaluations.load(Ordering::Relaxed),"queries":queries.len(),"ms":elapsed.as_secs_f64()*1000.0,"nodes":if scan {result["visibilityItemsScan"]["nodes"].as_array().unwrap().len()} else {result["visibilityItems"]["edges"].as_array().unwrap().len()}})
                );
            }
            db.pool().close().await;
        }
    }
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "visibility_grants", plural = "VisibilityGrants")]
struct VisibilityGrant {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    owner: String,
    #[filterable(type = "string")]
    subject: String,
}

#[tokio::test]
async fn typed_relation_predicates_bind_values_and_reject_entity_mismatch() {
    let (db, _, _) = setup(Mode::Unrestricted, 0, 19).await;
    sqlx::query("CREATE TABLE visibility_grants (id TEXT PRIMARY KEY, owner TEXT, subject TEXT)")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO visibility_grants VALUES ('00000000-0000-0000-0000-000000000001', 'b', 'reader')").execute(db.pool()).await.unwrap();
    let grants = ReadPredicate::from_filter::<SqliteBackend, _>(&VisibilityGrantWhereInput {
        subject: Some(StringFilter {
            eq: Some("reader".into()),
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap();
    assert!(
        EntityQuery::<VisibilityItem>::new()
            .with_read_visibility(&ReadVisibility::Complete(grants.clone()))
            .is_err()
    );
    assert!(
        ReadPredicate::related::<SqliteBackend, VisibilityItem, VisibilityGrant>(
            &[("owner; DROP TABLE visibility_items", "owner")],
            grants.clone()
        )
        .is_err()
    );
    let p = ReadPredicate::related::<SqliteBackend, VisibilityItem, VisibilityGrant>(
        &[("owner", "owner")],
        grants,
    )
    .unwrap();
    let query = EntityQuery::<VisibilityItem>::new()
        .with_read_visibility(&ReadVisibility::Complete(p))
        .unwrap();
    assert_eq!(query.count(&db).await.unwrap(), 60);
    let rows = query
        .paginate(&PageInput {
            limit: Some(3),
            offset: Some(0),
        })
        .fetch_all(&db)
        .await
        .unwrap();
    assert!(rows.iter().all(|r| r.owner == "b"));
}

#[tokio::test]
async fn exhausted_budget_boundary_and_scan_opt_in_are_explicit() {
    let (db, policy, capture) = setup(Mode::Callback, 100, 120).await;
    let first = data(&db, "a", scan_query(None, 3)).await;
    assert_eq!(
        first["visibilityItemsScan"]["pageInfo"]["status"],
        "BUDGET_EXHAUSTED"
    );
    assert_eq!(policy.evaluations.load(Ordering::Relaxed), 120);
    assert_eq!(capture.take().iter().map(|(_, n)| n).sum::<usize>(), 120);
    let cursor = first["visibilityItemsScan"]["pageInfo"]["continuation"]
        .as_str()
        .unwrap();
    let last = data(&db, "a", scan_query(Some(cursor), 3)).await;
    assert_eq!(
        last["visibilityItemsScan"]["pageInfo"]["status"],
        "EXHAUSTED"
    );
    assert!(
        last["visibilityItemsScan"]["nodes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let disabled = Database::new(db.pool().clone());
    assert!(
        !execute(&disabled, Some("a"), scan_query(None, 3))
            .await
            .errors
            .is_empty()
    );
}

struct DenyEntity;
impl EntityPolicy for DenyEntity {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}
struct ReadPayload;
impl FieldPolicy for ReadPayload {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
    fn can_read_repository_field<'a>(
        &'a self,
        _: Option<AccessContext<'a>>,
        _: &'a Database,
        entity: &'static str,
        field: &'static str,
        key: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(entity == "VisibilityItem"
                && (key.is_none() || (field == "payload" && key == Some("payload.read"))))
        })
    }
}
struct DenyFields;
impl FieldPolicy for DenyFields {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}
#[tokio::test]
async fn unrestricted_decisions_do_not_bypass_scopes_entity_or_field_policies() {
    let (mut db, _, capture) = setup(Mode::Unrestricted, 0, 19).await;
    let schema = schema_builder(db.clone()).finish();
    for scopes in [vec![], vec!["records.*".into()]] {
        let response = schema
            .execute(
                async_graphql::Request::new(scan_query(None, 3))
                    .data(AuthSubject::builder("a").scopes(scopes).build()),
            )
            .await;
        assert!(!response.errors.is_empty());
        assert!(capture.take().is_empty());
    }
    db.set_entity_policy(DenyEntity);
    assert!(
        !execute(&db, Some("a"), scan_query(None, 3))
            .await
            .errors
            .is_empty()
    );
    assert!(capture.take().is_empty());
    let (mut db, _, _) = setup(Mode::Unrestricted, 0, 19).await;
    db.set_field_policy(DenyFields);
    assert!(
        !execute(
            &db,
            Some("a"),
            "{ visibilityItemsScan(page: {limit: 3}) { nodes { payload } } }".into()
        )
        .await
        .errors
        .is_empty()
    );
    assert!(
        VisibilityItem::authorized_scan(
            &db,
            VisibilityItemWhereInput::default(),
            AuthorizedScanInput::default()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn empty_keyset_page_probes_include_authorized_cursor_boundary() {
    let (db, policy, _) = setup(Mode::Complete, 59, 19).await;
    let first = VisibilityItem::keyset_page(
        &db,
        VisibilityItemWhereInput::default(),
        KeysetPageInput {
            limit: Some(1),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let after = first.page_info.end_cursor;
    assert!(!first.page_info.has_next_page);
    let last = VisibilityItem::keyset_page(
        &db,
        VisibilityItemWhereInput::default(),
        KeysetPageInput {
            after: after.clone(),
            limit: Some(1),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(last.edges.is_empty());
    assert!(last.page_info.has_previous_page);
    policy.minimum.store(60, Ordering::Relaxed);
    let revoked = VisibilityItem::keyset_page(
        &db,
        VisibilityItemWhereInput::default(),
        KeysetPageInput {
            after,
            limit: Some(1),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!revoked.page_info.has_previous_page);
}

#[tokio::test]
async fn callback_offset_budget_exhaustion_never_releases_partial_totals_or_nodes() {
    let (db, policy, capture) = setup(Mode::Callback, 0, 19).await;
    let response = execute(
        &db,
        Some("a"),
        "{ visibilityItems(page: {limit: 1}) { edges {node {id}} pageInfo {totalCount} } }".into(),
    )
    .await;
    assert_eq!(response.errors.len(), 1);
    assert_eq!(
        response.errors[0].extensions.as_ref().unwrap().get("code"),
        Some(&async_graphql::Value::from("AUTHORIZATION_MISCONFIGURED"))
    );
    assert_eq!(response.data, async_graphql::Value::Null);
    let reads = capture.take();
    assert_eq!(reads.iter().map(|(_, count)| *count).sum::<usize>(), 19);
    assert!(reads.iter().all(|(_, count)| *count <= 7));
    assert_eq!(policy.evaluations.load(Ordering::Relaxed), 19);
}
