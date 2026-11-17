#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))),
))]

//! Generated `updateMany` and `deleteMany` filters need the same field read
//! authority as a generated list query's filter: a bulk write's affected count,
//! or a refusal on a row the caller can read, would otherwise reveal values of
//! a field the caller cannot read.
//!
//! Every field a filter names, through `and`, `or` and `not` at any depth, is
//! checked before any SQL. A denial is refused in the payload with the list
//! query's public message and code, and writes nothing. Related-row filters
//! are refused for bulk writes whatever the caller may read.

#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::async_graphql::{Request, Variables};
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use serde_json::{Value, json};
use std::any::Any;
use std::collections::HashSet;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "bulk_groups",
    plural = "BulkGroups",
    auth = "none",
    default_sort = "id ASC"
)]
pub struct BulkGroup {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    pub label: String,
    #[relation(
        target = "BulkMember",
        from = "id",
        to = "group_id",
        multiple,
        filter,
        emit_fk = false
    )]
    pub members: Vec<BulkMember>,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "bulk_members",
    plural = "BulkMembers",
    auth = "none",
    default_sort = "id ASC"
)]
pub struct BulkMember {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    #[filterable(type = "string")]
    pub group_id: String,
    #[filterable(type = "string")]
    pub name: String,
    #[graphql_orm(read_policy = "score.read")]
    #[filterable(type = "number")]
    pub score: i32,
}

schema_roots! {
    query_custom_ops: [],
    entities: [BulkGroup, BulkMember],
}

/// Field authority denying a configurable set of policy keys.
#[derive(Clone, Default)]
struct FieldAuthority(Arc<Mutex<HashSet<&'static str>>>);
impl FieldAuthority {
    fn set_denied(&self, denied: bool) {
        let mut keys = self.0.lock().unwrap();
        if denied {
            keys.insert("score.read");
        } else {
            keys.clear();
        }
    }
    fn allows(&self, key: Option<&'static str>) -> bool {
        key.is_none_or(|key| !self.0.lock().unwrap().contains(key))
    }
}
impl FieldPolicy for FieldAuthority {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        policy: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.allows(policy)) })
    }
    fn can_read_repository_field<'a>(
        &'a self,
        _: Option<AccessContext<'a>>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

/// Counts every generated read statement and every row-write decision: the
/// bulk write path selects its targets, then asks about each one.
#[derive(Clone, Default)]
struct Probe {
    reads: Arc<AtomicUsize>,
    row_decisions: Arc<AtomicUsize>,
}
impl Probe {
    fn reset(&self) {
        self.reads.store(0, Ordering::SeqCst);
        self.row_decisions.store(0, Ordering::SeqCst);
    }
    fn total(&self) -> usize {
        self.reads.load(Ordering::SeqCst) + self.row_decisions.load(Ordering::SeqCst)
    }
}
struct CountReads(Arc<AtomicUsize>);
impl ReadQueryObserver for CountReads {
    fn on_read(&self, _: &str, _: usize) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
impl RowPolicy for Probe {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        self.row_decisions.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(true) })
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

struct Fixture {
    db: Database,
    authority: FieldAuthority,
    probe: Probe,
    owner: FixtureOwner,
}

async fn execute(db: &Database, sql: &str) {
    <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[])
        .await
        .unwrap();
}

async fn fixture() -> Fixture {
    #[cfg(feature = "sqlite")]
    let (mut db, owner) = (
        Database::connect_sqlite("sqlite::memory:").await.unwrap(),
        FixtureOwner,
    );
    #[cfg(feature = "postgres")]
    let (mut db, owner) = {
        let owner = FixtureOwner::start("bulk-filter-authority").unwrap();
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&owner.url)
            .await
            .unwrap();
        (Database::new(pool), owner)
    };
    execute(
        &db,
        "CREATE TABLE bulk_groups (id TEXT PRIMARY KEY, label TEXT NOT NULL)",
    )
    .await;
    execute(
        &db,
        "CREATE TABLE bulk_members (id TEXT PRIMARY KEY, group_id TEXT NOT NULL, name TEXT NOT NULL, score INTEGER NOT NULL)",
    )
    .await;
    let authority = FieldAuthority::default();
    db.set_field_policy(authority.clone());
    let probe = Probe::default();
    db.set_row_policy(probe.clone());
    let db = db.with_read_query_observer(CountReads(probe.reads.clone()));
    Fixture {
        db,
        authority,
        probe,
        owner,
    }
}

async fn reset_rows(db: &Database) {
    execute(db, "DELETE FROM bulk_members").await;
    execute(db, "DELETE FROM bulk_groups").await;
    execute(
        db,
        "INSERT INTO bulk_groups (id, label) VALUES ('g1', 'one'), ('g2', 'two')",
    )
    .await;
    execute(
        db,
        "INSERT INTO bulk_members (id, group_id, name, score) VALUES ('m1', 'g1', 'alpha', 40), ('m2', 'g1', 'beta', 90), ('m3', 'g2', 'gamma', 70)",
    )
    .await;
}

async fn rows(db: &Database, table: &str, column: &str) -> Vec<String> {
    let rows = <DefaultBackend as OrmBackend>::fetch_rows(
        db.pool(),
        &format!("SELECT {column} FROM {table} ORDER BY id"),
        &[],
    )
    .await
    .unwrap();
    rows.iter()
        .map(|row| DefaultBackend::try_get_string(row, column).unwrap())
        .collect()
}

async fn run(fixture: &Fixture, query: &str, filter: &Value) -> Value {
    let schema = schema_builder(fixture.db.clone()).finish();
    let response = schema
        .execute(
            Request::new(query)
                .variables(Variables::from_json(json!({ "w": filter })))
                .data(AuthSubject::new("fixture-principal")),
        )
        .await;
    serde_json::to_value(&response).unwrap()
}

/// Filters naming the read-protected `score` field directly and through every
/// boolean combinator, with the number of rows each matches.
fn protected_filters() -> Vec<(Value, usize)> {
    vec![
        (json!({ "score": { "gt": 50 } }), 2),
        (
            json!({ "name": { "eq": "alpha" }, "score": { "gte": 0 } }),
            1,
        ),
        (
            json!({ "and": [{ "name": { "eq": "beta" } }, { "score": { "gt": 50 } }] }),
            1,
        ),
        (
            json!({ "or": [{ "name": { "eq": "none" } }, { "score": { "gt": 80 } }] }),
            1,
        ),
        (json!({ "not": { "score": { "lte": 50 } } }), 2),
        (
            json!({ "and": [{ "or": [{ "not": { "score": { "gt": 50 } } }, { "id": { "eq": "none" } }] }] }),
            1,
        ),
        (
            json!({ "not": { "and": [{ "id": { "eq": "m1" } }, { "or": [{ "score": { "lt": 0 } }] }] } }),
            3,
        ),
    ]
}

const LIST: &str =
    "query($w: BulkMemberWhereInput) { bulkMembers(where: $w) { edges { node { id } } } }";
const UPDATE: &str = "mutation($w: BulkMemberWhereInput) { result: updateBulkMembers(where: $w, input: {name: \"renamed\"}) { success error errorCode affectedCount } }";
const DELETE: &str = "mutation($w: BulkMemberWhereInput) { result: deleteBulkMembers(where: $w) { success error errorCode deletedCount } }";

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned disposable database"
)]
async fn bulk_write_filters_need_the_read_authority_of_list_filters() {
    let mut fixture = fixture().await;
    let original = ["alpha", "beta", "gamma"];
    for (filter, matched) in protected_filters() {
        // The list query's refusal is the reference for the bulk writes.
        reset_rows(&fixture.db).await;
        fixture.authority.set_denied(true);
        fixture.probe.reset();
        let list = run(&fixture, LIST, &filter).await;
        let message = list["errors"][0]["message"].clone();
        let code = list["errors"][0]["extensions"]["code"].clone();
        assert!(message.is_string() && code.is_string(), "{filter}: {list}");
        assert_eq!(fixture.probe.total(), 0, "{filter}: list read SQL");

        for (mutation, count_field) in [(UPDATE, "affectedCount"), (DELETE, "deletedCount")] {
            reset_rows(&fixture.db).await;
            fixture.authority.set_denied(true);
            fixture.probe.reset();
            let denied = run(&fixture, mutation, &filter).await;
            assert!(denied.get("errors").is_none(), "{filter}: {denied}");
            let payload = &denied["data"]["result"];
            assert_eq!(payload["success"], false, "{filter}: {denied}");
            assert_eq!(payload["error"], message, "{filter}: {denied}");
            assert_eq!(payload["errorCode"], code, "{filter}: {denied}");
            assert_eq!(payload[count_field], 0, "{filter}: {denied}");
            assert_eq!(
                fixture.probe.total(),
                0,
                "{filter}: a denied filter reached SQL or row decisions"
            );
            assert_eq!(
                rows(&fixture.db, "bulk_members", "name").await,
                original,
                "{filter}"
            );

            // With read authority the same filter writes the rows it matches.
            fixture.authority.set_denied(false);
            fixture.probe.reset();
            let allowed = run(&fixture, mutation, &filter).await;
            let payload = &allowed["data"]["result"];
            assert_eq!(payload["success"], true, "{filter}: {allowed}");
            assert_eq!(payload[count_field], matched, "{filter}: {allowed}");
            assert!(fixture.probe.total() > 0, "{filter}: probe saw no work");
            let names = rows(&fixture.db, "bulk_members", "name").await;
            if mutation == UPDATE {
                let renamed = names.iter().filter(|name| *name == "renamed").count();
                assert_eq!((renamed, names.len()), (matched, 3), "{filter}");
            } else {
                assert_eq!(names.len(), 3 - matched, "{filter}");
            }
        }
    }
    fixture.owner.cleanup().unwrap();
}

#[tokio::test]
#[cfg_attr(
    feature = "postgres",
    ignore = "requires test-owned disposable database"
)]
async fn related_row_bulk_write_filters_are_refused_alike_whatever_the_caller_may_read() {
    let mut fixture = fixture().await;
    let update = "mutation($w: BulkGroupWhereInput) { result: updateBulkGroups(where: $w, input: {label: \"changed\"}) { success error errorCode affectedCount } }";
    let delete = "mutation($w: BulkGroupWhereInput) { result: deleteBulkGroups(where: $w) { success error errorCode deletedCount } }";
    let filters = [
        json!({ "members": [{ "score": { "gt": 50 } }] }),
        json!({ "or": [{ "id": { "eq": "none" } }, { "members": [{ "score": { "gt": 80 } }] }] }),
        json!({ "not": { "and": [{ "members": [{ "name": { "eq": "alpha" } }] }] } }),
    ];
    for filter in filters {
        for mutation in [update, delete] {
            let mut payloads = Vec::new();
            for denied in [true, false] {
                reset_rows(&fixture.db).await;
                fixture.authority.set_denied(denied);
                fixture.probe.reset();
                let response = run(&fixture, mutation, &filter).await;
                assert!(response.get("errors").is_none(), "{filter}: {response}");
                let payload = response["data"]["result"].clone();
                assert_eq!(payload["success"], false, "{filter}: {response}");
                assert_eq!(
                    payload["errorCode"], "INVALID_INPUT",
                    "{filter}: {response}"
                );
                assert_eq!(fixture.probe.total(), 0, "{filter}");
                assert_eq!(
                    rows(&fixture.db, "bulk_groups", "label").await,
                    ["one", "two"],
                    "{filter}"
                );
                payloads.push(payload);
            }
            assert_eq!(
                payloads[0], payloads[1],
                "{filter}: refusal depends on read authority"
            );
        }
    }
    fixture.owner.cleanup().unwrap();
}
