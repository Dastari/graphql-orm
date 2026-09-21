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
schema_roots! { query_custom_ops: [], entities: [VisibilityItem], }

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
    };
    let capture = Capture::default();
    let mut database = database;
    database.set_row_policy(policy.clone());
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
async fn legacy_offsets_stay_exact_and_provider_errors_fail_before_querying() {
    for mode in [Mode::Callback, Mode::Partial] {
        let (db, _, _) = setup(mode, 40, 19).await;
        let result = data(&db, "a", "{ visibilityItems(page: {limit: 3, offset: 4}, orderBy: [{rank: ASC}]) { edges { node { rank } } pageInfo { totalCount } } }".into()).await;
        assert_eq!(result["visibilityItems"]["pageInfo"]["totalCount"], 20);
        assert_eq!(result["visibilityItems"]["edges"][0]["node"]["rank"], 44);
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
