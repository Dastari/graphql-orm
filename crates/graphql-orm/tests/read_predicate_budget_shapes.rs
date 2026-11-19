// This fixture selects one backend per build.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))),
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
))]
#![allow(dead_code, unused_imports, clippy::let_unit_value)]
//! Every read predicate that builds runs. Shapes grown to the composition
//! budget (relations, negations, plain and wide `and`/`or` levels, closures,
//! generated `or` lists) execute on every statement and GraphQL path, and
//! balanced grouping keeps the meaning of the plain chain.

#[path = "support/graphql_case.rs"]
mod graphql_case;
#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_case::{argument_name as a, field_name as f, resolver_name as r};
use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
#[cfg(not(feature = "mssql"))]
use graphql_orm::sqlx::Row;
use serde_json::{Value, json};
use std::sync::Mutex;

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "mssql")]
type Backend = MssqlBackend;
type B = Backend;

#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex)]
#[graphql_entity(table = "zr_parents", plural = "ZrParents", default_sort = "id ASC")]
struct ZrParent {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[graphql(skip)]
    #[relation(
        target = "ZrKid",
        from = "id",
        to = "parent_id",
        multiple,
        filter,
        emit_fk = false,
        order_aggregate(name = "KidCount", aggregate = "count")
    )]
    kids: Vec<ZrKid>,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "zr_kids",
    plural = "ZrKids",
    default_sort = "id ASC",
    keyset = "id asc"
)]
struct ZrKid {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    parent_id: String,
    #[filterable(type = "number")]
    score: Option<i64>,
    #[filterable(type = "number")]
    num: Option<i64>,
}

impl BatchLoadEntity for ZrKid {
    fn batch_column() -> &'static str {
        "parent_id"
    }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, graphql_orm::sqlx::Error> {
        row.try_get("parent_id")
    }
}

schema_roots! { query_custom_ops: [], entities: [ZrParent, ZrKid], }

static KID_POLICY: Mutex<Option<ReadPredicate>> = Mutex::new(None);
/// Held by every test that sets `KID_POLICY`, so tests running in parallel
/// never see each other's policy.
static KID_POLICY_TESTS: Mutex<()> = Mutex::new(());

struct Policy;
impl RowPolicy<Backend> for Policy {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            Ok(match (entity, KID_POLICY.lock().unwrap().clone()) {
                ("ZrKid", Some(p)) => ReadVisibility::Complete(p),
                _ => ReadVisibility::Unrestricted,
            })
        })
    }
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
impl EntityPolicy<Backend> for Policy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

#[cfg(feature = "postgres")]
type Owned = owned_postgres::OwnedPostgres;
#[cfg(feature = "mssql")]
type Owned = owned_mssql::OwnedSqlServer;
#[cfg(feature = "sqlite")]
type Owned = ();

#[cfg(feature = "mssql")]
const TEXT: &str = "NVARCHAR(64)";
#[cfg(not(feature = "mssql"))]
const TEXT: &str = "TEXT";

fn seed() -> Vec<String> {
    let mut s = vec![
        format!("CREATE TABLE zr_parents (id {TEXT} NOT NULL PRIMARY KEY)"),
        format!(
            "CREATE TABLE zr_kids (id {TEXT} NOT NULL PRIMARY KEY, parent_id {TEXT} NOT NULL, score BIGINT NULL, num BIGINT NULL)"
        ),
        "INSERT INTO zr_parents (id) VALUES ('p1'), ('p2'), ('p3'), ('p4'), ('p5')".to_owned(),
    ];
    let rows = (1..=60)
        .map(|i: i64| {
            let score = if i % 7 == 0 {
                "NULL".to_owned()
            } else {
                (i % 10).to_string()
            };
            let num = if i % 11 == 0 {
                "NULL".to_owned()
            } else {
                ((i * 7) % 13).to_string()
            };
            format!("('k{i:02}', 'p{}', {score}, {num})", i % 5 + 1)
        })
        .collect::<Vec<_>>();
    s.push(format!(
        "INSERT INTO zr_kids (id, parent_id, score, num) VALUES {}",
        rows.join(", ")
    ));
    s
}

async fn database() -> (Database<Backend>, Owned) {
    #[cfg(feature = "sqlite")]
    let (db, owned) = (
        Database::<Backend>::connect_sqlite("sqlite::memory:")
            .await
            .unwrap(),
        (),
    );
    #[cfg(feature = "postgres")]
    let (db, owned) = {
        let owned = owned_postgres::OwnedPostgres::start("budget-shapes").unwrap();
        let db = Database::<Backend>::connect_postgres(&owned.url)
            .await
            .unwrap();
        (db, owned)
    };
    #[cfg(feature = "mssql")]
    let (db, owned) = {
        let owned = owned_mssql::OwnedSqlServer::start().await.unwrap();
        let db = Database::<Backend>::connect_ado(&owned.connection_string)
            .await
            .unwrap();
        (db, owned)
    };
    #[cfg(not(feature = "mssql"))]
    for statement in seed() {
        graphql_orm::sqlx::query(&statement)
            .execute(db.pool())
            .await
            .unwrap();
    }
    #[cfg(feature = "mssql")]
    owned_mssql::execute_batch(&owned.connection_string, &seed().join(";\n"))
        .await
        .unwrap();
    (db, owned)
}

fn on_thread(stack: usize, body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(stack)
        .spawn(body)
        .unwrap()
        .join()
        .unwrap();
}
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

// ---------- measuring (copy of the PR's counter, for reporting) ----------
fn units(sql: &str) -> (u32, u32, u32) {
    let bytes = sql.as_bytes();
    let mut open: Vec<(bool, u32)> = Vec::new();
    let (mut sub, mut un, mut dd, mut du, mut ds) = (0u32, 0u32, 0u32, 0u32, 0u32);
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            q @ (b'\'' | b'"' | b'[') => {
                let c = if q == b'[' { b']' } else { q };
                i += 1;
                while i < bytes.len() && bytes[i] != c {
                    i += 1;
                }
            }
            b'(' => {
                let rest = sql[i + 1..].trim_start();
                let starts = |w: &str| {
                    rest.get(..w.len())
                        .is_some_and(|h| h.eq_ignore_ascii_case(w))
                };
                let before = sql[..i].trim_end();
                let neg = before.len() >= 3
                    && before[before.len() - 3..].eq_ignore_ascii_case("NOT")
                    && !before
                        .as_bytes()
                        .get(before.len().wrapping_sub(4))
                        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
                let s = starts("SELECT") || starts("WITH");
                let w = if s {
                    11
                } else if neg {
                    2
                } else {
                    1
                };
                sub += u32::from(s);
                un += w;
                open.push((s, w));
                dd = dd.max(open.len() as u32);
                du = du.max(un);
                ds = ds.max(sub);
            }
            b')' => {
                if let Some((s, w)) = open.pop() {
                    sub -= u32::from(s);
                    un -= w;
                }
            }
            _ => {}
        }
        i += 1;
    }
    (dd, du, ds)
}
fn rendered(p: &ReadPredicate) -> String {
    EntityQuery::<ZrKid>::new()
        .with_read_visibility(&ReadVisibility::Complete(p.clone()))
        .unwrap()
        .build_delete_sql()
        .0
}

fn leaf() -> ReadPredicate {
    ReadPredicate::column::<B, ZrKid>("score", ReadPredicateOp::Gte, SqlValue::Int(0)).unwrap()
}
fn other_leaf() -> ReadPredicate {
    ReadPredicate::is_not_null::<B, ZrKid>("id").unwrap()
}
fn wrap(p: ReadPredicate) -> async_graphql::Result<ReadPredicate> {
    ReadPredicate::related::<B, ZrKid, ZrKid>(&[("id", "id")], p)
}
fn grow(
    mut p: ReadPredicate,
    step: &dyn Fn(ReadPredicate, usize) -> async_graphql::Result<ReadPredicate>,
    guard: &dyn Fn(&ReadPredicate) -> bool,
) -> (ReadPredicate, usize) {
    for n in 0..100_000 {
        match step(p.clone(), n) {
            Ok(next) if guard(&next) => p = next,
            _ => return (p, n),
        }
    }
    panic!("never refused")
}
fn alternate(p: ReadPredicate, n: usize) -> async_graphql::Result<ReadPredicate> {
    if n.is_multiple_of(2) {
        p.and(other_leaf())
    } else {
        p.or(other_leaf())
    }
}
/// One `and`/`or` level of `width` children with the deep child first.
fn wide_alternate(
    width: usize,
) -> impl Fn(ReadPredicate, usize) -> async_graphql::Result<ReadPredicate> {
    move |mut p, n| {
        for _ in 1..width {
            p = if n.is_multiple_of(2) {
                p.and(other_leaf())?
            } else {
                p.or(other_leaf())?
            };
        }
        Ok(p)
    }
}

/// Shapes grown to the predicate budget. With `fragment_headroom`, every
/// step must also build as a relation fragment, as a generated relation
/// filter wraps it.
fn shapes(fragment_headroom: bool) -> Vec<(&'static str, ReadPredicate)> {
    let guard = move |p: &ReadPredicate| {
        !fragment_headroom
            || ReadPredicate::related::<B, ZrParent, ZrKid>(
                &[("id", "parent_id")],
                p.clone().and(leaf()).unwrap(),
            )
            .is_ok()
    };
    let not_step = |p: ReadPredicate, _| p.not();
    let wrap_step = |p: ReadPredicate, _| wrap(p);
    let mut out = Vec::new();
    // A: subqueries, then negations.
    let (p, n1) = grow(leaf(), &wrap_step, &guard);
    let (p, n2) = grow(p, &not_step, &guard);
    eprintln!("A: {n1} related + {n2} not");
    out.push(("A subqueries+nots", p));
    // B: negations, then subqueries, then negations.
    let (p, n1) = grow(leaf(), &not_step, &guard);
    let (p, n2) = grow(p, &wrap_step, &guard);
    let (p, n3) = grow(p, &not_step, &guard);
    eprintln!("B: {n1} not + {n2} related + {n3} not");
    out.push(("B nots+subqueries", p));
    // C: plain parentheses, then subqueries, then negations.
    let (p, n1) = grow(leaf(), &alternate, &guard);
    let (p, n2) = grow(p, &wrap_step, &guard);
    let (p, n3) = grow(p, &not_step, &guard);
    eprintln!("C: {n1} alternations + {n2} related + {n3} not");
    out.push(("C parens+subqueries+nots", p));
    // D: half subqueries, then negations interleaved with alternations.
    let (p, n1) = grow(
        leaf(),
        &|p, n| {
            if n < 40 {
                wrap(p)
            } else {
                Err(async_graphql::Error::new("stop"))
            }
        },
        &guard,
    );
    let (p, n2) = grow(
        p,
        &|p, n| {
            if n.is_multiple_of(3) {
                p.not()
            } else {
                alternate(p, n)
            }
        },
        &guard,
    );
    eprintln!("D: {n1} related + {n2} mixed");
    out.push(("D mixed", p));
    for (name, p) in &out {
        let (depth, units, subq) = units(&rendered(p));
        eprintln!("  {name}: depth {depth} units {units} subqueries {subq}");
    }
    out
}

async fn run_statement_paths(db: &Database<Backend>, name: &str, p: &ReadPredicate) -> Vec<String> {
    let mut failures = Vec::new();
    let query = EntityQuery::<ZrKid, Backend>::new()
        .with_read_visibility(&ReadVisibility::Complete(p.clone()))
        .unwrap();
    if let Err(e) = query.fetch_all(db).await {
        failures.push(format!("{name}: fetch_all: {e}"));
    }
    if let Err(e) = query.count(db).await {
        failures.push(format!("{name}: count: {e}"));
    }
    *KID_POLICY.lock().unwrap() = Some(p.clone());
    if let Err(e) = EntityQuery::<ZrKid, Backend>::new()
        .fetch_authorized_keyset(
            db,
            None,
            None,
            KeysetPageInput {
                after: None,
                limit: Some(2),
                include_total_count: true,
            },
        )
        .await
    {
        failures.push(format!("{name}: keyset: {e:?}"));
    }
    failures
}

fn nested_where(levels: usize) -> Value {
    let mut w = json!({ f("kids"): [{ f("score"): { f("gte"): 0 } }] });
    for _ in 0..levels {
        w = json!({ f("not"): w });
    }
    w
}

async fn run_graphql_paths(
    schema: &async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>,
    name: &str,
    p: &ReadPredicate,
) -> Vec<String> {
    *KID_POLICY.lock().unwrap() = Some(p.clone());
    let mut failures = Vec::new();
    for (label, query) in [
        (
            "parents(where nested 6 + orderBy count) { kids }",
            format!(
                "query($w: ZrParentWhereInput!) {{ rows: {}({}: $w, {}: [{{ KidCount: DESC }}]) {{ edges {{ node {{ {} kids {{ edges {{ node {{ {} }} }} }} }} }} }} }}",
                r("zrParents"),
                a("where"),
                a("orderBy"),
                f("id"),
                f("id")
            ),
        ),
        (
            "parents(where nested 6)",
            format!(
                "query($w: ZrParentWhereInput!) {{ rows: {}({}: $w) {{ edges {{ node {{ {} }} }} }} }}",
                r("zrParents"),
                a("where"),
                f("id")
            ),
        ),
        (
            "parents(orderBy count) { kids }",
            format!(
                "query {{ rows: {}({}: [{{ KidCount: DESC }}]) {{ edges {{ node {{ {} kids {{ edges {{ node {{ {} }} }} }} }} }} }} }}",
                r("zrParents"),
                a("orderBy"),
                f("id"),
                f("id")
            ),
        ),
        (
            "kids list",
            format!(
                "query {{ rows: {} {{ edges {{ node {{ {} }} }} }} }}",
                r("zrKids"),
                f("id")
            ),
        ),
    ] {
        let response = schema
            .execute(async_graphql::Request::new(query).variables(
                async_graphql::Variables::from_json(json!({ "w": nested_where(6) })),
            ))
            .await;
        for e in &response.errors {
            let code = e
                .extensions
                .as_ref()
                .and_then(|x| x.get("code"))
                .map(ToString::to_string);
            if code.as_deref() == Some("\"AUTHORIZATION_MISCONFIGURED\"") {
                eprintln!("  {name} / {label}: refused (AUTHORIZATION_MISCONFIGURED)");
            } else {
                failures.push(format!("{name} / {label}: {e:?}"));
            }
        }
        if response.errors.is_empty() {
            eprintln!("  {name} / {label}: ok");
        }
    }
    failures
}

#[test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "owned database lane"
)]
fn near_limit_shapes_execute_on_every_path() {
    on_thread(2 << 20, || {
        let _serial = KID_POLICY_TESTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        block_on(async {
            let (mut db, owned) = database().await;
            db.set_row_policy(Policy);
            db.set_entity_policy(Policy);
            let mut failures = Vec::new();
            let at_limit = shapes(false);
            for (name, p) in &at_limit {
                failures.extend(run_statement_paths(&db, name, p).await);
            }
            let headroom = shapes(true);
            let schema = async_graphql::Schema::build(
                QueryRoot::default(),
                MutationRoot::default(),
                SubscriptionRoot::default(),
            )
            .data(db)
            .data(AuthSubject::builder("reader").build())
            .finish();
            for (name, p) in at_limit.iter().chain(headroom.iter()) {
                failures.extend(run_graphql_paths(&schema, name, p).await);
            }
            let _keep = owned;
            assert!(failures.is_empty(), "{failures:#?}");
        })
    });
}

#[test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "owned database lane"
)]
fn wide_alternations_execute_when_they_build() {
    on_thread(2 << 20, || {
        block_on(async {
            let (db, owned) = database().await;
            let mut failures = Vec::new();
            for width in [8usize, 16, 32] {
                let step = wide_alternate(width);
                let (p, levels) = grow(leaf(), &step, &|_| true);
                let (depth, units_, _) = units(&rendered(&p));
                eprintln!("width {width}: {levels} levels, depth {depth}, units {units_}");
                // Also the smallest level count that would cross SQLite's
                // 1,000-deep expression limit, if the predicate goes that deep.
                for take in [levels.min(20), levels.min(40), levels] {
                    let (q, _) = grow(leaf(), &step, &|_| true);
                    let _ = q;
                    let mut p = leaf();
                    for n in 0..take {
                        p = step(p, n).unwrap();
                    }
                    let query = EntityQuery::<ZrKid, Backend>::new()
                        .with_read_visibility(&ReadVisibility::Complete(p.clone()))
                        .unwrap();
                    match query.fetch_all(&db).await {
                        Ok(rows) => {
                            eprintln!("  width {width} levels {take}: ok ({} rows)", rows.len())
                        }
                        Err(e) => failures.push(format!("width {width} levels {take}: {e}")),
                    }
                }
            }
            let _keep = owned;
            assert!(failures.is_empty(), "{failures:#?}");
        })
    });
}

// ---------- randomized balanced vs unbalanced comparison ----------
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn random_tree(rng: &mut Lcg, depth: u32, leaves: &mut usize) -> (ReadPredicate, String) {
    if depth == 0 || rng.below(10) < 3 || *leaves > 3000 {
        *leaves += 1;
        let column = if rng.below(2) == 0 { "score" } else { "num" };
        match rng.below(8) {
            6 => (
                ReadPredicate::is_null::<B, ZrKid>(column).unwrap(),
                format!("{column} IS NULL"),
            ),
            7 => (
                ReadPredicate::is_not_null::<B, ZrKid>(column).unwrap(),
                format!("{column} IS NOT NULL"),
            ),
            op => {
                let value = rng.below(14) as i64;
                let (op, sql) = [
                    (ReadPredicateOp::Eq, "="),
                    (ReadPredicateOp::Ne, "<>"),
                    (ReadPredicateOp::Lt, "<"),
                    (ReadPredicateOp::Lte, "<="),
                    (ReadPredicateOp::Gt, ">"),
                    (ReadPredicateOp::Gte, ">="),
                ][op as usize];
                (
                    ReadPredicate::column::<B, ZrKid>(column, op, SqlValue::Int(value)).unwrap(),
                    format!("{column} {sql} {value}"),
                )
            }
        }
    } else {
        match rng.below(5) {
            0 => {
                let (p, s) = random_tree(rng, depth - 1, leaves);
                (p.not().unwrap(), format!("NOT ({s})"))
            }
            kind => {
                let and = kind.is_multiple_of(2);
                let n = if rng.below(3) == 0 {
                    2 + rng.below(4)
                } else {
                    20 + rng.below(90)
                };
                let (mut p, first) = random_tree(rng, depth - 1, leaves);
                let mut parts = vec![format!("({first})")];
                for _ in 1..n {
                    let (q, s) = random_tree(rng, depth - 1, leaves);
                    p = if and {
                        p.and(q).unwrap()
                    } else {
                        p.or(q).unwrap()
                    };
                    parts.push(format!("({s})"));
                }
                (p, parts.join(if and { " AND " } else { " OR " }))
            }
        }
    }
}

#[cfg(not(feature = "mssql"))]
#[test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "owned database lane"
)]
fn balanced_grouping_matches_the_unbalanced_form() {
    on_thread(2 << 20, || {
        block_on(async {
            let (db, owned) = database().await;
            let mut rng = Lcg(0x781d_2026);
            let mut trials = 0;
            let mut chunked = 0;
            for _ in 0..150 {
                let mut leaves = 0;
                let (p, reference) = random_tree(&mut rng, 3, &mut leaves);
                let mut got = EntityQuery::<ZrKid, Backend>::new()
                    .with_read_visibility(&ReadVisibility::Complete(p.clone()))
                    .unwrap()
                    .fetch_all(&db)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|k| k.id)
                    .collect::<Vec<_>>();
                got.sort();
                // Negated form too: NOT of the whole tree.
                let negated = p.clone().not().unwrap();
                let mut got_not = EntityQuery::<ZrKid, Backend>::new()
                    .with_read_visibility(&ReadVisibility::Complete(negated))
                    .unwrap()
                    .fetch_all(&db)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|k| k.id)
                    .collect::<Vec<_>>();
                got_not.sort();
                let fetch = |sql: String| {
                    let db = &db;
                    async move {
                        let mut ids = graphql_orm::sqlx::query(&sql)
                            .fetch_all(db.pool())
                            .await
                            .unwrap()
                            .iter()
                            .map(|row| row.try_get::<String, _>("id").unwrap())
                            .collect::<Vec<_>>();
                        ids.sort();
                        ids
                    }
                };
                let expected = fetch(format!("SELECT id FROM zr_kids WHERE {reference}")).await;
                let expected_not =
                    fetch(format!("SELECT id FROM zr_kids WHERE NOT ({reference})")).await;
                assert_eq!(got, expected, "reference: {reference}");
                assert_eq!(got_not, expected_not, "reference: NOT {reference}");
                if rendered(&p).matches("((").count() > 0 && leaves > 32 {
                    chunked += 1;
                }
                trials += 1;
            }
            eprintln!("{trials} trials, {chunked} with > 32 leaves");
            let _keep = owned;
        })
    });
}

#[test]
fn not_of_a_twenty_five_thousand_term_or_stays_flat() {
    on_thread(2 << 20, || {
        let mut wide = leaf();
        // About 30 bytes a term, the column bound to its table, so about
        // 750 KB, within the byte budget.
        for _ in 0..25_000 {
            wide = wide
                .or(ReadPredicate::is_null::<B, ZrKid>("id").unwrap())
                .unwrap();
        }
        let negated = wide.clone().not().unwrap();
        let sql = rendered(&negated);
        let (depth, units_, _) = units(&sql);
        eprintln!(
            "not(or 25k): depth {depth} units {units_} bytes {}",
            sql.len()
        );
        assert!(depth < 24);
        let _ = format!("{negated:?}");
        let copy = negated.clone();
        drop((copy, negated, wide));
        // and of nots of wide ors, and not of and of those
        let mut p = leaf();
        for _ in 0..100 {
            let mut o = leaf();
            for _ in 0..100 {
                o = o.or(other_leaf()).unwrap();
            }
            p = p.and(o.not().unwrap()).unwrap();
        }
        let p = p.not().unwrap();
        let (depth, _, _) = units(&rendered(&p));
        eprintln!("not(and of 100 not(or 100)): depth {depth}");
        let _ = format!("{p:?}");
    });
}

/// Apply `steps` in turn, stopping at the first refusal.
/// One composition step, applied with its step number.
type Step<'a> = &'a dyn Fn(ReadPredicate, usize) -> async_graphql::Result<ReadPredicate>;

fn build(steps: &[(Step<'_>, usize)]) -> Option<ReadPredicate> {
    let mut predicate = leaf();
    for (step, count) in steps {
        for n in 0..*count {
            predicate = step(predicate, n).ok()?;
        }
    }
    Some(predicate)
}

#[cfg(feature = "mssql")]
#[test]
#[ignore = "owned database lane"]
fn sql_server_caps_refuse_what_the_server_cannot_run() {
    on_thread(2 << 20, || {
        block_on(async {
            let (db, owned) = database().await;
            let wrap_step = |p: ReadPredicate, _| wrap(p);
            let not_step = |p: ReadPredicate, _| p.not();
            // Forty nested relations exhausted the optimizer (error 8623), as
            // did 39 with nine negations: both are refused when built.
            assert!(build(&[(&wrap_step, 40)]).is_none());
            assert!(build(&[(&wrap_step, 39), (&not_step, 9)]).is_none());
            let mut failures = Vec::new();
            let run = |name: String, p: ReadPredicate| {
                let db = &db;
                async move {
                    let query = EntityQuery::<ZrKid, Backend>::new()
                        .with_read_visibility(&ReadVisibility::Complete(p))
                        .unwrap();
                    let mut failures = Vec::new();
                    if let Err(e) = query.fetch_all(db).await {
                        failures.push(format!("{name}: fetch_all: {e}"));
                    }
                    if let Err(e) = query.count(db).await {
                        failures.push(format!("{name}: count: {e}"));
                    }
                    failures
                }
            };
            // The deepest relation chain that builds, then as many negations
            // and alternations as still build, run.
            let (chain, related) = grow(leaf(), &wrap_step, &|_| true);
            assert_eq!(related as u32, READ_PREDICATE_MSSQL_MAX_RELATION_NESTING);
            let (negated, _) = grow(chain.clone(), &not_step, &|_| true);
            let (alternated, _) = grow(chain.clone(), &alternate, &|_| true);
            // The default closure inside as many relations as build, then
            // negations.
            let closure =
                ReadPredicate::related_closure::<B, ZrKid>(&[("parent_id", "id")], leaf()).unwrap();
            let (wrapped, around) = grow(closure, &wrap_step, &|_| true);
            assert!(around >= 4, "{around}");
            let (wrapped_negated, _) = grow(wrapped.clone(), &not_step, &|_| true);
            for (name, p) in [
                ("deepest relation chain", chain),
                ("relation chain and negations", negated),
                ("relation chain and alternations", alternated),
                ("closure in relations", wrapped),
                ("closure in relations and negations", wrapped_negated),
            ] {
                let (depth, units_, subqueries) = units(&rendered(&p));
                eprintln!("{name}: depth {depth} units {units_} subqueries {subqueries}");
                failures.extend(run(name.to_owned(), p).await);
            }
            let _keep = owned;
            assert!(failures.is_empty(), "{failures:#?}");
        })
    });
}

#[test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "owned database lane"
)]
fn generated_or_lists_render_shallow_and_run() {
    on_thread(2 << 20, || {
        block_on(async {
            let (db, owned) = database().await;
            // SQL Server takes at most 2,100 bind parameters per statement,
            // which the composition budget does not cover.
            let lengths: &[usize] = if cfg!(feature = "mssql") {
                &[500, 1100, 2000]
            } else {
                &[500, 1100, 3000]
            };
            let mut failures = Vec::new();
            for &n in lengths {
                let filter = ZrKidWhereInput {
                    or: Some(
                        (0..n)
                            .map(|i| ZrKidWhereInput {
                                num: Some(IntFilter {
                                    eq: Some((i % 13) as i32),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .collect(),
                    ),
                    ..Default::default()
                };
                let p = ReadPredicate::from_filter::<B, _>(&filter).unwrap();
                let (depth, _, _) = units(&rendered(&p));
                // About log2(n) levels, not n.
                assert!(
                    depth <= 2 + (n as f64).log2().ceil() as u32 + 4,
                    "or {n}: depth {depth}"
                );
                let result = EntityQuery::<ZrKid, Backend>::new()
                    .with_read_visibility(&ReadVisibility::Complete(p))
                    .unwrap()
                    .fetch_all(&db)
                    .await;
                match result {
                    Ok(rows) => assert_eq!(rows.len(), 55, "or {n}"),
                    Err(e) => failures.push(format!("or {n}: {e}")),
                }
            }
            let _keep = owned;
            assert!(failures.is_empty(), "{failures:#?}");
        })
    });
}

/// A where input asking for `filters` kid relation filters joined by `or`.
async fn parents_with_kid_filters(
    schema: &async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>,
    filters: usize,
) -> async_graphql::Response {
    let alternatives = (0..filters)
        .map(|_| json!({ f("kids"): [{ f("score"): { f("gte"): 0 } }] }))
        .collect::<Vec<_>>();
    schema
        .execute(
            async_graphql::Request::new(format!(
                "query($w: ZrParentWhereInput!) {{ rows: {}({}: $w) {{ edges {{ node {{ {} }} }} }} }}",
                r("zrParents"),
                a("where"),
                f("id")
            ))
            .variables(async_graphql::Variables::from_json(
                json!({ "w": { f("or"): alternatives } }),
            )),
        )
        .await
}

fn code(response: &async_graphql::Response) -> Option<String> {
    response.errors.first().and_then(|error| {
        error
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code"))
            .map(ToString::to_string)
    })
}

#[test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "owned database lane"
)]
fn relation_subqueries_are_budgeted_per_statement() {
    on_thread(2 << 20, || {
        let _serial = KID_POLICY_TESTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        block_on(async {
            let (mut db, owned) = database().await;
            db.set_row_policy(Policy);
            db.set_entity_policy(Policy);
            let mssql = cfg!(feature = "mssql");
            let (nesting, total, statement) = if mssql {
                (
                    READ_PREDICATE_MSSQL_MAX_RELATION_NESTING,
                    READ_PREDICATE_MSSQL_MAX_RELATIONS,
                    READ_PREDICATE_MSSQL_MAX_STATEMENT_RELATIONS,
                )
            } else {
                (
                    READ_PREDICATE_MAX_RELATION_NESTING,
                    READ_PREDICATE_MAX_RELATIONS,
                    READ_PREDICATE_MAX_STATEMENT_RELATIONS,
                )
            };
            // A host chain deeper than the nesting limit is refused when it
            // is built (the reviewed shape was 126 deep).
            let (deepest, depth) = grow(leaf(), &|p, _| wrap(p), &|_| true);
            assert_eq!(depth as u32, nesting);
            assert!(wrap(deepest.clone()).is_err());
            // So is an `or` of chains past the total.
            let one = grow(
                leaf(),
                &|p, n| {
                    if n < 3 {
                        wrap(p)
                    } else {
                        Err(async_graphql::Error::new("stop"))
                    }
                },
                &|_| true,
            )
            .0;
            let (_, terms) = grow(one.clone(), &|p, _| p.or(one.clone()), &|_| true);
            assert_eq!((terms + 1) * 3, total as usize / 3 * 3, "{terms}");
            // Each kid relation filter embeds the kid policy (three nested
            // relations) plus its own: four relation subqueries. The statement
            // budget admits `statement / 4` of them, and refuses one more.
            *KID_POLICY.lock().unwrap() = Some(one);
            let schema = async_graphql::Schema::build(
                QueryRoot::default(),
                MutationRoot::default(),
                SubscriptionRoot::default(),
            )
            .data(db)
            .data(AuthSubject::builder("reader").build())
            .finish();
            let admitted = statement as usize / 4;
            let response = parents_with_kid_filters(&schema, admitted).await;
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            let response = parents_with_kid_filters(&schema, admitted + 1).await;
            assert_eq!(
                code(&response).as_deref(),
                Some("\"AUTHORIZATION_MISCONFIGURED\""),
                "{:?}",
                response.errors
            );
            *KID_POLICY.lock().unwrap() = None;
            let _keep = owned;
        })
    });
}

#[test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "owned database lane"
)]
fn not_related_chains_stop_at_the_case_and_relation_limits() {
    on_thread(2 << 20, || {
        let _serial = KID_POLICY_TESTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        block_on(async {
            let (db, owned) = database().await;
            let (deepest, levels) = grow(
                leaf(),
                &|p, _| ReadPredicate::not_related::<B, ZrKid, ZrKid>(&[("id", "id")], p),
                &|_| true,
            );
            // Two CASE levels each: at most five within the CASE limit of 10.
            // SQL Server's nine-relation total stops it at two.
            assert_eq!(levels, if cfg!(feature = "mssql") { 2 } else { 5 });
            let failures = run_statement_paths(&db, "deepest not_related chain", &deepest).await;
            let _keep = owned;
            assert!(failures.is_empty(), "{failures:#?}");
        })
    });
}

fn parents_ordered_by_kid_counts(terms: usize) -> String {
    format!(
        "query {{ rows: {}({}: [{}]) {{ edges {{ node {{ {} }} }} }} }}",
        r("zrParents"),
        a("orderBy"),
        vec!["{ KidCount: DESC }"; terms].join(", "),
        f("id")
    )
}

#[test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "owned database lane"
)]
fn closures_and_count_orderings_are_charged_per_statement() {
    on_thread(2 << 20, || {
        let _serial = KID_POLICY_TESTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        block_on(async {
            let (mut db, owned) = database().await;
            db.set_row_policy(Policy);
            db.set_entity_policy(Policy);
            let schema = async_graphql::Schema::build(
                QueryRoot::default(),
                MutationRoot::default(),
                SubscriptionRoot::default(),
            )
            .data(db)
            .data(AuthSubject::builder("reader").build())
            .finish();
            let misconfigured = Some("\"AUTHORIZATION_MISCONFIGURED\"");
            // A closure counts as several relations: with a closure kid
            // policy, each kid filter costs 1 + 3 + 16 relations elsewhere
            // (the closure's own three subqueries and its weight) and 1 + 1
            // on SQL Server: three fit the budget of 64, or two of 5.
            let filters = if cfg!(feature = "mssql") { 2 } else { 3 };
            *KID_POLICY.lock().unwrap() = Some(
                ReadPredicate::related_closure::<B, ZrKid>(&[("parent_id", "id")], leaf()).unwrap(),
            );
            let response = parents_with_kid_filters(&schema, filters).await;
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            let response = parents_with_kid_filters(&schema, filters + 1).await;
            assert_eq!(
                code(&response).as_deref(),
                misconfigured,
                "{:?}",
                response.errors
            );
            // Each count ordering costs its policy plus its own subquery.
            let policy = if cfg!(feature = "mssql") { 1 } else { 4 };
            *KID_POLICY.lock().unwrap() = Some(
                grow(
                    leaf(),
                    &|p, n| {
                        if n < policy {
                            wrap(p)
                        } else {
                            Err(async_graphql::Error::new("stop"))
                        }
                    },
                    &|_| true,
                )
                .0,
            );
            let statement = if cfg!(feature = "mssql") {
                READ_PREDICATE_MSSQL_MAX_STATEMENT_RELATIONS
            } else {
                READ_PREDICATE_MAX_STATEMENT_RELATIONS
            } as usize;
            let admitted = statement / (policy + 1);
            assert!(admitted < MAX_ORDER_BY_TERMS, "{admitted}");
            let response = schema
                .execute(async_graphql::Request::new(parents_ordered_by_kid_counts(
                    admitted,
                )))
                .await;
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            let response = schema
                .execute(async_graphql::Request::new(parents_ordered_by_kid_counts(
                    admitted + 1,
                )))
                .await;
            assert_eq!(
                code(&response).as_deref(),
                misconfigured,
                "{:?}",
                response.errors
            );
            // An unrestricted target is charged too, and the list itself is
            // capped at 16 terms.
            *KID_POLICY.lock().unwrap() = None;
            let response = schema
                .execute(async_graphql::Request::new(parents_ordered_by_kid_counts(
                    MAX_ORDER_BY_TERMS + 1,
                )))
                .await;
            assert_eq!(
                code(&response).as_deref(),
                Some("\"INVALID_INPUT\""),
                "{:?}",
                response.errors
            );
            if cfg!(feature = "mssql") {
                let response = schema
                    .execute(async_graphql::Request::new(parents_ordered_by_kid_counts(
                        statement + 1,
                    )))
                    .await;
                assert_eq!(
                    code(&response).as_deref(),
                    misconfigured,
                    "{:?}",
                    response.errors
                );
            }
            let _keep = owned;
        })
    });
}
