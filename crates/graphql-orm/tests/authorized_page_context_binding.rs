// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]

//! `EntityQuery::fetch_authorized_page` in a handwritten resolver (#169).
//!
//! The resolver builds an `AuthorizedPageBinding` from server state on every
//! request. A continuation is accepted only under the binding it was issued
//! under, and every page re-resolves entity, row and field authorization.

#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::async_graphql::{Context, Request, Response, SimpleObject};
use graphql_orm::prelude::*;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "bound_page_rows",
        plural = "BoundPageRows",
        auth = "none",
        keyset = "seq asc, id asc"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "bound_page_rows",
        plural = "BoundPageRows",
        auth = "none",
        keyset = "seq asc, id asc"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "bound_page_rows",
        plural = "BoundPageRows",
        auth = "none",
        keyset = "seq asc, id asc"
    )
)]
struct BoundPageRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "number")]
    #[sortable]
    seq: i64,
    #[filterable(type = "string")]
    owner: String,
    #[graphql_orm(read_policy = "body.read")]
    body: String,
}

/// Trusted server state for one request. Nothing here is a GraphQL argument.
#[derive(Clone, Debug)]
struct ServerStamps {
    snapshot_generation: String,
    source_registration: String,
    source_incarnation: String,
    workspace: String,
    original_principal: String,
    current_principal: String,
    grants: String,
    credential_revision: String,
    configuration_revision: String,
    placement_revision: String,
}
impl ServerStamps {
    fn base() -> Self {
        Self {
            snapshot_generation: "generation-41".into(),
            source_registration: "registration-7".into(),
            source_incarnation: "incarnation-2".into(),
            workspace: "workspace-north".into(),
            original_principal: "alice".into(),
            current_principal: "alice".into(),
            grants: "rows.read".into(),
            credential_revision: "credential-3".into(),
            configuration_revision: "configuration-9".into(),
            placement_revision: "placement-5".into(),
        }
    }
    fn binding(&self) -> Result<AuthorizedPageBinding, AuthorizedPageBindingError> {
        AuthorizedPageBinding::builder()
            .component("snapshot-generation", &self.snapshot_generation)
            .component("source-registration", &self.source_registration)
            .component("source-incarnation", &self.source_incarnation)
            .component("workspace", &self.workspace)
            .component("original-principal", &self.original_principal)
            .component("current-principal", &self.current_principal)
            .component("grants", &self.grants)
            .component("credential-revision", &self.credential_revision)
            .component("configuration-revision", &self.configuration_revision)
            .component("placement-revision", &self.placement_revision)
            .build()
    }
    /// Every value a public error or cursor must never repeat.
    fn values(&self) -> [&str; 9] {
        [
            &self.snapshot_generation,
            &self.source_registration,
            &self.source_incarnation,
            &self.workspace,
            &self.original_principal,
            &self.grants,
            &self.credential_revision,
            &self.configuration_revision,
            &self.placement_revision,
        ]
    }
}

/// The principal the policies authorize, resolved by the host per request.
#[derive(Clone)]
struct Principal(String);

/// A page of rows returned by the handwritten resolver.
#[derive(SimpleObject, GraphQLSemanticObject)]
#[graphql_orm(classification = "internal")]
struct BoundRowPage {
    /// The visible rows of this page.
    #[graphql_orm(maximum_items = 10)]
    nodes: Vec<BoundPageRow>,
    /// Why the scan stopped and how to resume it.
    page_info: ScanPageInfo,
}

#[derive(Default)]
struct PageQueries;

#[graphql_orm_custom_operations(kind = "query", authorization = false)]
#[graphql_orm::async_graphql::Object]
impl PageQueries {
    /// Pages through the rows of the current server context.
    async fn bound_rows(
        &self,
        ctx: &Context<'_>,
        after: Option<String>,
        limit: Option<i64>,
    ) -> async_graphql::Result<BoundRowPage> {
        let db = ctx.data::<Database>()?;
        let binding = ctx
            .data::<ServerStamps>()?
            .binding()
            .map_err(AuthorizedPageBindingError::into_graphql_error)?;
        let page = EntityQuery::<BoundPageRow, DefaultBackend>::new()
            .fetch_authorized_page(
                db,
                Some(ctx),
                None,
                AuthorizedPageRequest::new(binding)
                    .after(after)
                    .limit(limit),
            )
            .await?;
        Ok(BoundRowPage {
            nodes: page.nodes,
            page_info: page.page_info,
        })
    }
}

#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", auth: "none", entities: [BoundPageRow], described_query_types: [PageQueries], }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", auth: "none", entities: [BoundPageRow], described_query_types: [PageQueries], }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", auth: "none", entities: [BoundPageRow], described_query_types: [PageQueries], }

/// Policy state the tests change between pages.
#[derive(Clone, Default)]
struct Authority {
    entity_denied: Arc<AtomicBool>,
    body_denied: Arc<AtomicBool>,
    denied_rows: Arc<Mutex<HashSet<String>>>,
    row_checks: Arc<AtomicUsize>,
}
fn principal<'a>(ctx: Option<&'a Context<'_>>) -> &'a str {
    ctx.and_then(|ctx| ctx.data_opt::<Principal>())
        .map(|principal| principal.0.as_str())
        .unwrap_or("alice")
}
impl EntityPolicy for Authority {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(!self.entity_denied.load(Ordering::SeqCst)) })
    }
}
impl RowPolicy for Authority {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async { Ok(ReadVisibility::CallbackOnly) })
    }
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            self.row_checks.fetch_add(1, Ordering::SeqCst);
            let row = row.downcast_ref::<BoundPageRow>().unwrap();
            Ok(row.owner == principal(ctx) && !self.denied_rows.lock().unwrap().contains(&row.id))
        })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}
impl FieldPolicy for Authority {
    fn can_read_field<'a>(
        &'a self,
        _: &'a Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        key: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(
            async move { Ok(key != Some("body.read") || !self.body_denied.load(Ordering::SeqCst)) },
        )
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a Context<'_>,
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
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
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

const BATCH: u32 = 4;
const BUDGET: u32 = 8;
const KEY: [u8; 32] = [
    0x51, 0x0c, 0x9e, 0x27, 0xb3, 0x44, 0xfa, 0x18, 0x6d, 0x90, 0x02, 0xc7, 0x3b, 0xe5, 0x7a, 0x81,
    0x1f, 0xd6, 0x49, 0xa0, 0x35, 0x8c, 0xee, 0x13, 0x72, 0x0b, 0xc4, 0x5d, 0x96, 0x2a, 0xf8, 0x67,
];

fn keys(id: &str, key: [u8; 32]) -> AuthorizedPageKeys {
    AuthorizedPageKeys::new(id, key).unwrap()
}
fn page_config(keys: AuthorizedPageKeys) -> AuthorizedPageConfig {
    AuthorizedPageConfig::new(keys, BATCH, BUDGET)
        .unwrap()
        .with_max_page_size(10)
        .unwrap()
}

struct Fixture {
    /// Policies and observer installed; no page configuration yet.
    bare: Database,
    /// `bare` with the shared key `k1`.
    db: Database,
    authority: Authority,
    capture: Capture,
    owner: FixtureOwner,
}

/// 60 rows in ties of three by `seq`. The first 30 belong to another
/// principal, so several whole scan budgets hold no visible row; the rest
/// alternate.
async fn fixture(label: &str) -> Result<Fixture, Box<dyn std::error::Error>> {
    let _ = label;
    #[cfg(feature = "sqlite")]
    let (mut bare, owner) = (
        Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?,
        FixtureOwner,
    );
    #[cfg(feature = "postgres")]
    let (mut bare, owner) = {
        let owner = FixtureOwner::start(label)?;
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&owner.url)
            .await?;
        (Database::new(pool), owner)
    };
    #[cfg(feature = "mssql")]
    let (mut bare, owner) = {
        let owner = FixtureOwner::start().await?;
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await?;
        (
            Database::builder(pool)
                .schema_policy(SchemaPolicy::ExternalWritable)
                .build(),
            owner,
        )
    };
    let execute = |sql: String| {
        let db = bare.clone();
        async move { <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), &sql, &[]).await }
    };
    execute(
        "CREATE TABLE bound_page_rows (id VARCHAR(80) PRIMARY KEY, seq BIGINT NOT NULL, \
         owner VARCHAR(80) NOT NULL, body VARCHAR(100) NOT NULL)"
            .into(),
    )
    .await?;
    for index in 0..60 {
        let row_owner = if index < 30 || index % 2 == 1 {
            "bob"
        } else {
            "alice"
        };
        execute(format!(
            "INSERT INTO bound_page_rows VALUES ('{index:03}', {}, '{row_owner}', 'body-{index:03}')",
            index / 3
        ))
        .await?;
    }
    let authority = Authority::default();
    let capture = Capture::default();
    bare.set_entity_policy(authority.clone());
    bare.set_row_policy(authority.clone());
    bare.set_field_policy(authority.clone());
    let bare = bare.with_read_query_observer(capture.clone());
    let db = bare
        .clone()
        .with_authorized_page_config(page_config(keys("k1", KEY)));
    Ok(Fixture {
        bare,
        db,
        authority,
        capture,
        owner,
    })
}

fn alice_ids() -> Vec<String> {
    (30..60)
        .filter(|index| index % 2 == 0)
        .map(|index| format!("{index:03}"))
        .collect()
}

fn query(after: Option<&str>, limit: Option<i64>, fields: &str) -> String {
    format!(
        "{{ boundRows(after: {}, limit: {}) {{ nodes {{ {fields} }} pageInfo {{ status continuation totalCount }} }} }}",
        serde_json::to_string(&after).unwrap(),
        serde_json::to_string(&limit).unwrap(),
    )
}

async fn run(
    db: &Database,
    stamps: &ServerStamps,
    who: &str,
    after: Option<&str>,
    limit: Option<i64>,
    fields: &str,
) -> Response {
    schema_builder(db.clone())
        .finish()
        .execute(
            Request::new(query(after, limit, fields))
                .data(stamps.clone())
                .data(Principal(who.to_string())),
        )
        .await
}

/// One decoded page: ids, status and continuation.
#[derive(Debug, PartialEq)]
struct Page {
    ids: Vec<String>,
    status: String,
    continuation: Option<String>,
}

async fn page(
    db: &Database,
    stamps: &ServerStamps,
    who: &str,
    after: Option<&str>,
    limit: i64,
) -> Page {
    let response = run(db, stamps, who, after, Some(limit), "id seq owner").await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let root = &data["boundRows"];
    assert_eq!(root["pageInfo"]["totalCount"], Value::Null);
    Page {
        ids: root["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|node| node["id"].as_str().unwrap().to_string())
            .collect(),
        status: root["pageInfo"]["status"].as_str().unwrap().to_string(),
        continuation: root["pageInfo"]["continuation"]
            .as_str()
            .map(str::to_string),
    }
}

/// Walk to exhaustion, returning every page.
async fn traverse(replicas: &[&Database], stamps: &ServerStamps, limit: i64) -> Vec<Page> {
    let mut pages: Vec<Page> = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let db = replicas[pages.len() % replicas.len()];
        let next = page(db, stamps, "alice", after.as_deref(), limit).await;
        after = next.continuation.clone();
        let done = next.status == "EXHAUSTED";
        pages.push(next);
        if done {
            assert!(after.is_none());
            return pages;
        }
        assert!(after.is_some(), "an unfinished page must be resumable");
        assert!(pages.len() < 100, "traversal did not terminate");
    }
}

fn ids(pages: &[Page]) -> Vec<String> {
    pages.iter().flat_map(|page| page.ids.clone()).collect()
}

/// The single public error of a refused request: `(code, reason)`.
fn refusal(response: &Response, hidden: &[&str]) -> (String, Option<String>) {
    assert_eq!(response.errors.len(), 1, "{response:?}");
    assert_eq!(response.data, async_graphql::Value::Null);
    let text = serde_json::to_string(&response.errors).unwrap();
    for value in hidden {
        assert!(!text.contains(value), "error repeats a value: {text}");
    }
    let extensions = response.errors[0].extensions.as_ref().unwrap();
    let text = |name: &str| match extensions.get(name) {
        Some(async_graphql::Value::String(value)) => Some(value.clone()),
        _ => None,
    };
    (text("code").unwrap(), text("reason"))
}

fn invalid_cursor() -> (String, Option<String>) {
    ("CURSOR_INVALID".into(), None)
}

#[tokio::test]
async fn same_context_continuation_is_stable_bounded_and_never_falsely_exhausted()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = fixture("authorized-page-continuation").await?;
    let (db, stamps) = (&fixture.db, ServerStamps::base());
    fixture.capture.take();

    let mut pages: Vec<Page> = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let checks = fixture.authority.row_checks.load(Ordering::SeqCst);
        let next = page(db, &stamps, "alice", after.as_deref(), 4).await;
        // Scan work is bounded per request: statements, rows and callbacks.
        let reads = fixture.capture.take();
        assert!(!reads.is_empty());
        assert!(reads.iter().all(|(_, rows)| *rows <= BATCH as usize));
        let scanned: usize = reads.iter().map(|(_, rows)| rows).sum();
        assert!(scanned <= BUDGET as usize, "{scanned} rows in one request");
        assert_eq!(
            fixture.authority.row_checks.load(Ordering::SeqCst) - checks,
            scanned
        );
        match next.status.as_str() {
            "PAGE_FULL" => assert_eq!(next.ids.len(), 4),
            "BUDGET_EXHAUSTED" => assert_eq!(scanned, BUDGET as usize),
            "EXHAUSTED" => assert!(next.continuation.is_none()),
            other => panic!("unexpected status {other}"),
        }
        // The same continuation in the same context gives the same page.
        if let Some(cursor) = after.as_deref() {
            let again = page(db, &stamps, "alice", Some(cursor), 4).await;
            assert_eq!((&again.ids, &again.status), (&next.ids, &next.status));
            fixture.capture.take();
        }
        after = next.continuation.clone();
        let done = next.status == "EXHAUSTED";
        pages.push(next);
        if done {
            break;
        }
        assert!(after.is_some());
        assert!(pages.len() < 100);
    }
    assert_eq!(ids(&pages), alice_ids());
    // The first 30 candidates are invisible: whole budgets return no row and
    // still resume, rather than reporting exhaustion.
    let empty: Vec<&Page> = pages.iter().filter(|page| page.ids.is_empty()).collect();
    assert!(empty.len() >= 3, "{pages:?}");
    assert!(
        empty[..3]
            .iter()
            .all(|page| page.status == "BUDGET_EXHAUSTED" && page.continuation.is_some())
    );
    assert_eq!(pages.iter().filter(|p| p.status == "EXHAUSTED").count(), 1);
    assert!(pages.iter().any(|page| page.status == "PAGE_FULL"));

    // Continuations are opaque: no position, binding or key material.
    for cursor in pages.iter().filter_map(|page| page.continuation.as_deref()) {
        assert!(cursor.starts_with("gomp1.k1."));
        assert!(cursor.len() <= AUTHORIZED_PAGE_DEFAULT_MAX_CURSOR_BYTES);
        for value in stamps.values() {
            assert!(!cursor.contains(value));
        }
    }

    // The page size is bounded: an over-limit request is refused, not truncated.
    fixture.capture.take();
    let over = run(db, &stamps, "alice", None, Some(11), "id").await;
    assert_eq!(refusal(&over, &[]).0, "PAGE_LIMIT_EXCEEDED");
    let zero = run(db, &stamps, "alice", None, Some(0), "id").await;
    assert_eq!(refusal(&zero, &[]).0, "INVALID_INPUT");
    assert!(fixture.capture.take().is_empty());
    let default = run(db, &stamps, "bob", None, None, "id").await;
    assert!(default.errors.is_empty(), "{:?}", default.errors);

    fixture.owner.cleanup()?;
    Ok(())
}

#[tokio::test]
async fn every_page_applies_current_entity_row_and_field_authorization()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = fixture("authorized-page-authorization").await?;
    let (db, stamps) = (&fixture.db, ServerStamps::base());
    let expected = alice_ids();

    // Reach the first page that holds visible rows. `expected` then starts
    // at the first row after the cursor.
    let mut first = page(db, &stamps, "alice", None, 2).await;
    while first.ids.is_empty() {
        first = page(db, &stamps, "alice", first.continuation.as_deref(), 2).await;
    }
    assert_eq!(first.ids, expected[..first.ids.len()]);
    let expected = &expected[first.ids.len()..];
    let cursor = first.continuation.clone().unwrap();

    // Row authorization: a row denied after the cursor was issued is skipped.
    fixture
        .authority
        .denied_rows
        .lock()
        .unwrap()
        .insert(expected[0].clone());
    let next = page(db, &stamps, "alice", Some(&cursor), 2).await;
    assert_eq!(next.ids, expected[1..3]);
    fixture.authority.denied_rows.lock().unwrap().clear();
    assert_eq!(
        page(db, &stamps, "alice", Some(&cursor), 2).await.ids,
        expected[..2]
    );

    // The cursor is a position, never an authorization: under an unchanged
    // binding the receiving request's own row decision still applies.
    let other = page(db, &stamps, "bob", Some(&cursor), 2).await;
    assert!(!other.ids.is_empty());
    assert!(other.ids.iter().all(|id| !alice_ids().contains(id)));

    // Field authorization: a field denied after the cursor was issued is not
    // released on the next page.
    let allowed = run(db, &stamps, "alice", Some(&cursor), Some(2), "id body").await;
    assert!(allowed.errors.is_empty(), "{:?}", allowed.errors);
    assert!(
        allowed
            .data
            .into_json()?
            .to_string()
            .contains(&format!("body-{}", expected[0]))
    );
    fixture.authority.body_denied.store(true, Ordering::SeqCst);
    let denied = run(db, &stamps, "alice", Some(&cursor), Some(2), "id body").await;
    assert!(!denied.errors.is_empty());
    assert!(!serde_json::to_string(&denied)?.contains("body-"));
    assert_eq!(
        page(db, &stamps, "alice", Some(&cursor), 2).await.ids,
        expected[..2]
    );
    fixture.authority.body_denied.store(false, Ordering::SeqCst);

    // Entity authorization: denied before the cursor is opened or a row read.
    fixture
        .authority
        .entity_denied
        .store(true, Ordering::SeqCst);
    fixture.capture.take();
    let checks = fixture.authority.row_checks.load(Ordering::SeqCst);
    let denied = run(db, &stamps, "alice", Some(&cursor), Some(2), "id").await;
    assert_eq!(refusal(&denied, &[&cursor]).0, "FORBIDDEN");
    assert!(fixture.capture.take().is_empty());
    assert_eq!(fixture.authority.row_checks.load(Ordering::SeqCst), checks);
    fixture
        .authority
        .entity_denied
        .store(false, Ordering::SeqCst);
    assert_eq!(
        page(db, &stamps, "alice", Some(&cursor), 2).await.ids,
        expected[..2]
    );

    fixture.owner.cleanup()?;
    Ok(())
}

#[tokio::test]
async fn a_changed_context_binding_is_rejected_before_any_row_is_read()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = fixture("authorized-page-binding").await?;
    let (db, base) = (&fixture.db, ServerStamps::base());
    let first = page(db, &base, "alice", None, 3).await;
    let cursor = first.continuation.clone().unwrap();
    let expected = page(db, &base, "alice", Some(&cursor), 3).await;

    type Change = (&'static str, fn(&mut ServerStamps));
    let changes: [Change; 10] = [
        ("snapshot generation", |s| {
            s.snapshot_generation = "generation-42".into()
        }),
        ("source registration", |s| {
            s.source_registration = "registration-8".into()
        }),
        ("source incarnation", |s| {
            s.source_incarnation = "incarnation-3".into()
        }),
        ("workspace", |s| s.workspace = "workspace-south".into()),
        ("original principal", |s| {
            s.original_principal = "carol".into()
        }),
        ("current principal", |s| {
            s.current_principal = "carol".into()
        }),
        ("grants", |s| s.grants = "rows.read,rows.export".into()),
        ("credential revision", |s| {
            s.credential_revision = "credential-4".into()
        }),
        ("configuration revision", |s| {
            s.configuration_revision = "configuration-10".into()
        }),
        ("placement revision", |s| {
            s.placement_revision = "placement-6".into()
        }),
    ];
    for (name, change) in changes {
        let mut changed = base.clone();
        change(&mut changed);
        let hidden: Vec<&str> = base
            .values()
            .into_iter()
            .chain(changed.values())
            .chain([cursor.as_str()])
            .collect();
        // Deterministic: the same refusal every time, with no row read.
        for _ in 0..3 {
            fixture.capture.take();
            let checks = fixture.authority.row_checks.load(Ordering::SeqCst);
            let response = run(db, &changed, "alice", Some(&cursor), Some(3), "id").await;
            assert_eq!(refusal(&response, &hidden), invalid_cursor(), "{name}");
            assert!(fixture.capture.take().is_empty(), "{name} read rows");
            assert_eq!(
                fixture.authority.row_checks.load(Ordering::SeqCst),
                checks,
                "{name} ran a row check"
            );
        }
        // The changed context starts its own sequence, whose continuation the
        // original context refuses in turn.
        let restarted = page(db, &changed, "alice", None, 3).await;
        assert_eq!(restarted, {
            let mut same = page(db, &base, "alice", None, 3).await;
            same.continuation = restarted.continuation.clone();
            same
        });
        let foreign = restarted.continuation.unwrap();
        let response = run(db, &base, "alice", Some(&foreign), Some(3), "id").await;
        assert_eq!(refusal(&response, &[]), invalid_cursor(), "{name}");
        // The original context is unaffected.
        let again = page(db, &base, "alice", Some(&cursor), 3).await;
        assert_eq!(
            (&again.ids, &again.status),
            (&expected.ids, &expected.status)
        );
    }

    fixture.owner.cleanup()?;
    Ok(())
}

#[tokio::test]
async fn tampered_truncated_oversized_expired_and_rotated_cursors_are_refused()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = fixture("authorized-page-cursors").await?;
    let (db, stamps) = (&fixture.db, ServerStamps::base());
    let cursor = page(db, &stamps, "alice", None, 3)
        .await
        .continuation
        .unwrap();
    let expected = page(db, &stamps, "alice", Some(&cursor), 3).await;

    let mut tampered = cursor.clone().into_bytes();
    let last = tampered.len() - 1;
    tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
    let middle = cursor.len() - 20;
    let mut flipped = cursor.clone().into_bytes();
    flipped[middle] = if flipped[middle] == b'A' { b'B' } else { b'A' };
    let refused = [
        String::from_utf8(tampered)?,
        String::from_utf8(flipped)?,
        cursor[..cursor.len() - 1].to_string(),
        cursor[..cursor.len() / 2].to_string(),
        "gomp1.k1.".to_string(),
        String::new(),
        cursor.replacen("gomp1.", "gomp2.", 1),
        cursor.replacen("gomp1.", "goms1.", 1),
        format!("{cursor}{}", "A".repeat(4)),
        format!("gomp1.k1.{}", "A".repeat(64 * 1024)),
        "A".repeat(AUTHORIZED_PAGE_DEFAULT_MAX_CURSOR_BYTES + 1),
    ];
    fixture.capture.take();
    let checks = fixture.authority.row_checks.load(Ordering::SeqCst);
    for candidate in &refused {
        let response = run(db, &stamps, "alice", Some(candidate), Some(3), "id").await;
        assert_eq!(refusal(&response, &stamps.values()), invalid_cursor());
        assert_eq!(response.errors[0].message, "invalid cursor");
    }
    assert!(fixture.capture.take().is_empty());
    assert_eq!(fixture.authority.row_checks.load(Ordering::SeqCst), checks);

    // A smaller configured bound refuses a cursor that is otherwise valid.
    let tight = fixture.bare.clone().with_authorized_page_config(
        page_config(keys("k1", KEY)).with_max_cursor_bytes(cursor.len() - 1)?,
    );
    let response = run(&tight, &stamps, "alice", Some(&cursor), Some(3), "id").await;
    assert_eq!(refusal(&response, &[]), invalid_cursor());

    // Expiry: the continuation outlives neither its lifetime nor a restart.
    let brief = fixture.bare.clone().with_authorized_page_config(
        page_config(keys("k1", KEY)).with_cursor_ttl(Duration::from_millis(50))?,
    );
    let expiring = page(&brief, &stamps, "alice", None, 3)
        .await
        .continuation
        .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    fixture.capture.take();
    for replica in [&brief, db] {
        let response = run(replica, &stamps, "alice", Some(&expiring), Some(3), "id").await;
        assert_eq!(
            refusal(&response, &[&expiring]),
            (
                "CURSOR_INVALID".to_string(),
                Some(AUTHORIZED_PAGE_CURSOR_EXPIRED_REASON.to_string())
            )
        );
    }
    assert!(fixture.capture.take().is_empty());
    // A changed context learns nothing about expiry.
    let mut changed = stamps.clone();
    changed.workspace = "workspace-south".into();
    let response = run(db, &changed, "alice", Some(&expiring), Some(3), "id").await;
    assert_eq!(refusal(&response, &[]), invalid_cursor());

    // Rotation: `k2` becomes active while `k1` is still accepted, then `k1`
    // is removed.
    let next_key = [0xA7; 32];
    let rotating = fixture
        .bare
        .clone()
        .with_authorized_page_config(page_config(keys("k2", next_key).accepting("k1", KEY)?));
    let rotated = fixture
        .bare
        .clone()
        .with_authorized_page_config(page_config(keys("k2", next_key)));
    let resumed = page(&rotating, &stamps, "alice", Some(&cursor), 3).await;
    assert_eq!(
        (&resumed.ids, &resumed.status),
        (&expected.ids, &expected.status)
    );
    let reissued = resumed.continuation.unwrap();
    assert!(reissued.starts_with("gomp1.k2."));
    assert_eq!(
        page(&rotated, &stamps, "alice", Some(&reissued), 3)
            .await
            .ids,
        page(&rotating, &stamps, "alice", Some(&reissued), 3)
            .await
            .ids
    );
    fixture.capture.take();
    for (replica, token) in [(&rotated, &cursor), (db, &reissued)] {
        let response = run(replica, &stamps, "alice", Some(token), Some(3), "id").await;
        assert_eq!(
            refusal(&response, &[token.as_str()]),
            (
                "CURSOR_INVALID".to_string(),
                Some(AUTHORIZED_PAGE_CURSOR_KEY_UNAVAILABLE_REASON.to_string())
            )
        );
    }
    assert!(fixture.capture.take().is_empty());

    fixture.owner.cleanup()?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_replicas_share_the_key_and_a_wrong_key_is_refused()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = fixture("authorized-page-replicas").await?;
    let stamps = ServerStamps::base();
    // Two replicas configured independently from the same service secret.
    let replica_a = fixture.db.clone();
    let replica_b = fixture
        .bare
        .clone()
        .with_authorized_page_config(page_config(keys("k1", KEY)));

    let mut walks = Vec::new();
    for walker in 0..8_i64 {
        let (a, b, stamps) = (replica_a.clone(), replica_b.clone(), stamps.clone());
        walks.push(tokio::spawn(async move {
            // Alternate replicas page by page, starting on either.
            let order = if walker % 2 == 0 { [&a, &b] } else { [&b, &a] };
            ids(&traverse(&order, &stamps, 1 + walker % 4).await)
        }));
    }
    for walk in walks {
        assert_eq!(walk.await?, alice_ids());
    }

    // A replica holding different bytes under the same key identifier can
    // neither open the shared cursors nor issue ones the others accept.
    let mut wrong = KEY;
    wrong[31] ^= 1;
    let stranger = fixture
        .bare
        .clone()
        .with_authorized_page_config(page_config(keys("k1", wrong)));
    let shared = page(&replica_a, &stamps, "alice", None, 3)
        .await
        .continuation
        .unwrap();
    let foreign = page(&stranger, &stamps, "alice", None, 3)
        .await
        .continuation
        .unwrap();
    fixture.capture.take();
    for (replica, token) in [
        (&stranger, &shared),
        (&replica_a, &foreign),
        (&replica_b, &foreign),
    ] {
        let response = run(replica, &stamps, "alice", Some(token), Some(3), "id").await;
        assert_eq!(refusal(&response, &[token.as_str()]), invalid_cursor());
    }
    assert!(fixture.capture.take().is_empty());
    assert!(
        page(&replica_b, &stamps, "alice", Some(&shared), 3)
            .await
            .continuation
            .is_some()
    );

    fixture.owner.cleanup()?;
    Ok(())
}

#[tokio::test]
async fn pages_fail_closed_unconfigured_and_stay_separate_from_existing_scans()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = fixture("authorized-page-compatibility").await?;
    let stamps = ServerStamps::base();
    let request =
        |after: Option<String>| AuthorizedPageRequest::new(stamps.binding().unwrap()).after(after);
    let scan = |after: Option<String>| AuthorizedScanInput {
        after,
        limit: Some(3),
        include_total_count: false,
    };
    let code = |error: async_graphql::Error| match error.extensions.unwrap().get("code") {
        Some(async_graphql::Value::String(code)) => code.clone(),
        other => panic!("no code: {other:?}"),
    };
    let query = EntityQuery::<BoundPageRow, DefaultBackend>::new;

    // Neither API is enabled by default, and neither enables the other.
    let scan_only = fixture
        .bare
        .clone()
        .with_authorized_scan_config(AuthorizedScanConfig::new(KEY, BATCH, BUDGET));
    fixture.capture.take();
    for db in [&fixture.bare, &scan_only] {
        let error = query()
            .fetch_authorized_page(db, None, None, request(None))
            .await
            .unwrap_err();
        assert_eq!(code(error), "AUTHORIZATION_MISCONFIGURED");
    }
    let error = query()
        .fetch_authorized_scan(&fixture.db, None, None, scan(None))
        .await
        .unwrap_err();
    assert_eq!(code(error), "AUTHORIZATION_MISCONFIGURED");
    assert!(fixture.capture.take().is_empty());

    // With both installed over the same secret, each codec refuses the
    // other's continuation and the existing scan behaves as before.
    let both = fixture
        .db
        .clone()
        .with_authorized_scan_config(AuthorizedScanConfig::new(KEY, BATCH, BUDGET));
    let scanned = BoundPageRow::authorized_scan(&both, Default::default(), scan(None))
        .await
        .unwrap();
    assert_eq!(scanned.page_info.status, ScanStatus::BudgetExhausted);
    let scan_cursor = scanned.page_info.continuation.unwrap();
    assert!(scan_cursor.starts_with("goms1."));
    let paged = query()
        .fetch_authorized_page(&both, None, None, request(None).limit(Some(3)))
        .await
        .unwrap();
    assert_eq!(paged.page_info.status, ScanStatus::BudgetExhausted);
    assert!(paged.nodes.is_empty() && paged.page_info.total_count.is_none());
    let page_cursor = paged.page_info.continuation.unwrap();
    let error = query()
        .fetch_authorized_page(&both, None, None, request(Some(scan_cursor.clone())))
        .await
        .unwrap_err();
    assert_eq!(code(error), "CURSOR_INVALID");
    let error = query()
        .fetch_authorized_scan(&both, None, None, scan(Some(page_cursor.clone())))
        .await
        .unwrap_err();
    assert_eq!(code(error), "CURSOR_INVALID");
    assert!(
        query()
            .fetch_authorized_scan(&both, None, None, scan(Some(scan_cursor)))
            .await
            .is_ok()
    );

    // A continuation is also bound to the caller-visible filter.
    let owned_by = |owner: &str| BoundPageRowWhereInput {
        owner: Some(StringFilter {
            eq: Some(owner.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let filtered = query()
        .filter_with_entity_matching(&owned_by("alice"))
        .fetch_authorized_page(&both, None, None, request(None).limit(Some(3)))
        .await
        .unwrap();
    assert_eq!(filtered.nodes.len(), 3);
    let filtered_cursor = filtered.page_info.continuation.unwrap();
    for other in [
        query(),
        query().filter_with_entity_matching(&owned_by("bob")),
    ] {
        let error = other
            .fetch_authorized_page(&both, None, None, request(Some(filtered_cursor.clone())))
            .await
            .unwrap_err();
        assert_eq!(code(error), "CURSOR_INVALID");
    }
    let resumed = query()
        .filter_with_entity_matching(&owned_by("alice"))
        .fetch_authorized_page(
            &both,
            None,
            None,
            request(Some(filtered_cursor)).limit(Some(3)),
        )
        .await
        .unwrap();
    assert_eq!(
        resumed
            .nodes
            .iter()
            .map(|row| row.id.clone())
            .collect::<Vec<_>>(),
        alice_ids()[3..6]
    );
    let error = query()
        .fetch_authorized_page(&both, None, None, request(Some(page_cursor)))
        .await;
    assert!(error.is_ok());

    fixture.owner.cleanup()?;
    Ok(())
}
