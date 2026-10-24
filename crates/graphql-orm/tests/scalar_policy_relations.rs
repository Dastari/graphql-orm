#![cfg(feature = "sqlite")]

use graphql_orm::async_graphql::{Context, Guard, Request, Response, SimpleObject};
use graphql_orm::futures::{Stream, StreamExt, future::BoxFuture, future::poll_fn};
use graphql_orm::prelude::*;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::task::Poll;
use tokio::time::{Duration, timeout};

#[derive(Clone, Copy)]
struct Actor { admin: bool, host: bool, extra: bool }
struct HostGuard;
impl Guard for HostGuard {
    async fn check(&self, ctx: &Context<'_>) -> graphql_orm::async_graphql::Result<()> {
        if ctx.data::<Actor>()?.host { Ok(()) } else { Err("host denied".into()) }
    }
}
struct ExtraGuard;
impl Guard for ExtraGuard {
    async fn check(&self, ctx: &Context<'_>) -> graphql_orm::async_graphql::Result<()> {
        if ctx.data::<Actor>()?.extra { Ok(()) } else { Err("extra denied".into()) }
    }
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "policy_targets", plural = "PolicyTargets", auth = "none")]
struct PolicyTarget {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[sortable]
    title: String,
}
impl BatchLoadEntity for PolicyTarget {
    fn batch_column() -> &'static str { "id" }
    fn batch_key_from_row(row: &graphql_orm::sqlx::sqlite::SqliteRow) -> Result<String, graphql_orm::sqlx::Error> {
        use graphql_orm::sqlx::Row;
        row.try_get("id")
    }
}

#[graphql_policy_object]
#[derive(GraphQLEntity, GraphQLOperations, GraphQLRelations, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(complex, guard = "HostGuard")]
#[graphql_entity(backend = "sqlite", table = "guarded_records", plural = "GuardedRecords", auth = "none")]
struct GuardedRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[sortable]
    title: String,
    #[graphql_orm(read_policy = "admin.secret")]
    #[filterable(type = "string")]
    secret: Option<String>,
    #[graphql(guard = "ExtraGuard")]
    extra: String,
    target_id: String,
    #[graphql(skip)]
    #[relation(target = "PolicyTarget", from = "target_id", to = "id", emit_fk = false, propagate_change = "none")]
    target: Option<PolicyTarget>,
}

#[graphql_policy_object]
#[derive(GraphQLEntity, GraphQLOperations, GraphQLRelations, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(complex, rename_fields = "PascalCase")]
#[graphql_entity(backend = "sqlite", table = "pascal_records", plural = "PascalRecords", auth = "none")]
struct PascalRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[sortable]
    title: String,
    #[graphql_orm(read_policy = "admin.secret")]
    secret: Option<String>,
    target_id: String,
    #[graphql(skip)]
    #[relation(target = "PolicyTarget", from = "target_id", to = "id", emit_fk = false, propagate_change = "none")]
    target: Option<PolicyTarget>,
}

schema_roots! { backend: "sqlite", auth: "none", generated_mutations: "none", query_custom_ops: [], entities: [GuardedRecord, PascalRecord, PolicyTarget], }

#[derive(Clone)]
struct Policy(Arc<AtomicBool>);
impl EntityPolicy for Policy {
    fn can_access_entity<'a>(&'a self, _: Option<&'a Context<'_>>, _: &'a Database, _: &'static str, _: Option<&'static str>, _: EntityAccessKind, _: EntityAccessSurface) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
impl RowPolicy for Policy {
    fn can_read_row<'a>(&'a self, _: Option<&'a Context<'_>>, _: &'a Database, _: &'static str, _: Option<&'static str>, _: EntityAccessSurface, _: &'a (dyn std::any::Any + Send + Sync)) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> { Box::pin(async { Ok(true) }) }
    fn can_write_row<'a>(&'a self, _: Option<&'a Context<'_>>, _: &'a Database, _: &'static str, _: Option<&'static str>, _: EntityAccessSurface, _: &'a (dyn std::any::Any + Send + Sync)) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> { Box::pin(async { Ok(true) }) }
}
impl FieldPolicy for Policy {
    fn can_read_field<'a>(&'a self, ctx: &'a Context<'_>, _: &'a Database, _: &'static str, field: &'static str, policy: Option<&'static str>, row: Option<&'a (dyn std::any::Any + Send + Sync)>) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async move {
            if policy == Some("admin.secret") || field == "extra" {
                let _ = row;
                return Ok(ctx.data::<Actor>()?.admin && self.0.load(Ordering::SeqCst));
            }
            Ok(policy.is_none())
        })
    }
    fn can_write_field<'a>(&'a self, _: &'a Context<'_>, _: &'a Database, _: &'static str, _: &'static str, _: Option<&'static str>, _: Option<&'a (dyn std::any::Any + Send + Sync)>, _: Option<&'a (dyn std::any::Any + Send + Sync)>) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> { Box::pin(async { Ok(true) }) }
}

async fn fixture() -> (Database, Policy) {
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await.unwrap();
    for sql in [
        "CREATE TABLE policy_targets (id TEXT PRIMARY KEY, title TEXT NOT NULL)",
        "CREATE TABLE guarded_records (id TEXT PRIMARY KEY, title TEXT NOT NULL, secret TEXT, extra TEXT NOT NULL, target_id TEXT NOT NULL)",
        "CREATE TABLE pascal_records (id TEXT PRIMARY KEY, title TEXT NOT NULL, secret TEXT, target_id TEXT NOT NULL)",
        "INSERT INTO policy_targets VALUES ('t', 'related public title')",
        "INSERT INTO guarded_records VALUES ('r', 'public title', 'private scalar', 'extra value', 't')",
        "INSERT INTO pascal_records VALUES ('p', 'Pascal public title', 'private Pascal scalar', 't')",
    ] { graphql_orm::sqlx::query(sql).execute(db.pool()).await.unwrap(); }
    let policy = Policy(Arc::new(AtomicBool::new(true)));
    db.set_authorization_mode(AuthorizationMode::ExplicitPolicyForAllExposedOperations);
    db.set_entity_policy(policy.clone());
    db.set_row_policy(policy.clone());
    db.set_field_policy(policy.clone());
    (db, policy)
}
fn request(query: &str, admin: bool) -> Request {
    Request::new(query).data(Actor { admin, host: true, extra: true }).data(AuthSubject::new("actor"))
}
async fn registered<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) { Poll::Pending => Poll::Ready(()), other => panic!("unexpected event {other:?}") }).await;
}

#[tokio::test]
async fn protected_scalar_denial_keeps_public_fields_and_generated_relations() {
    let (db, _) = fixture().await;
    let schema = schema_builder(db).finish();
    let public = schema.execute(request("{ guardedRecord(id: \"r\") { title target { title } } }", false)).await;
    assert!(public.errors.is_empty(), "{:?}", public.errors);
    let value = public.data.into_json().unwrap();
    assert_eq!(value["guardedRecord"]["target"]["title"], "related public title");
    let denied = schema.execute(request("{ guardedRecord(id: \"r\") { title leak: secret } }", false)).await;
    assert!(!denied.errors.is_empty());
    assert!(!format!("{:?}", denied.data).contains("private scalar"));
    let allowed = schema.execute(request("{ guardedRecord(id: \"r\") { secret } }", true)).await;
    assert!(allowed.errors.is_empty(), "{:?}", allowed.errors);
    assert_eq!(allowed.data.into_json().unwrap()["guardedRecord"]["secret"], "private scalar");
}

#[tokio::test]
async fn keyless_field_policy_is_enforced_alongside_an_existing_field_guard() {
    let (db, _) = fixture().await;
    let schema = schema_builder(db).finish();
    let query = "{ guardedRecord(id: \"r\") { extra } }";
    let denied = schema.execute(request(query, false)).await;
    assert!(!denied.errors.is_empty());
    assert!(!format!("{:?}", denied.data).contains("extra value"));
    let allowed = schema.execute(request(query, true)).await;
    assert!(allowed.errors.is_empty(), "{:?}", allowed.errors);
    assert_eq!(allowed.data.into_json().unwrap()["guardedRecord"]["extra"], "extra value");
}

#[tokio::test]
async fn current_field_revocation_blocks_later_query_and_queued_changed_snapshot() {
    let (db, policy) = fixture().await;
    let schema: AppSchema = schema_builder(db.clone()).finish();
    let mut stream = Box::pin(schema.execute_stream(request("subscription { guardedRecordChanged(key: \"r\") { id guardedRecord { title secret } } }", true)));
    registered(&mut stream).await;
    GuardedRecord::update_by_id(&db, &"r".to_owned(), UpdateGuardedRecordInput { title: Some("allowed update".into()), ..Default::default() }).await.unwrap();
    let allowed = timeout(Duration::from_secs(2), stream.next()).await.unwrap().unwrap();
    assert!(allowed.errors.is_empty(), "{:?}", allowed.errors);
    assert!(allowed.data.into_json().unwrap().to_string().contains("private scalar"));
    GuardedRecord::update_by_id(&db, &"r".to_owned(), UpdateGuardedRecordInput { title: Some("queued denied update".into()), ..Default::default() }).await.unwrap();
    policy.0.store(false, Ordering::SeqCst);
    assert!(timeout(Duration::from_millis(150), stream.next()).await.is_err());
    let denied = schema.execute(request("{ guardedRecord(id: \"r\") { secret } }", true)).await;
    assert!(!denied.errors.is_empty());
    assert!(!format!("{:?}", denied.data).contains("private scalar"));
}

#[tokio::test]
async fn preserves_host_guards_and_pascal_field_identities() {
    let (db, _) = fixture().await;
    let schema = schema_builder(db).finish();
    for (query, actor) in [
        ("{ guardedRecord(id: \"r\") { title } }", Actor { admin: true, host: false, extra: true }),
        ("{ guardedRecord(id: \"r\") { extra } }", Actor { admin: true, host: true, extra: false }),
    ] {
        let denied = schema.execute(Request::new(query).data(actor)).await;
        assert!(!denied.errors.is_empty());
    }
    let query = "{ pascalRecord(id: \"p\") { Title Secret Target { title } } }";
    let allowed = schema.execute(request(query, true)).await;
    assert!(allowed.errors.is_empty(), "{:?}", allowed.errors);
    assert_eq!(allowed.data.into_json().unwrap()["pascalRecord"]["Secret"], "private Pascal scalar");
    let denied = schema.execute(request(query, false)).await;
    assert!(!denied.errors.is_empty());
    assert!(!format!("{:?}", denied.data).contains("private Pascal scalar"));
}

#[tokio::test]
async fn missing_or_unrelated_field_policy_fails_closed_for_declared_scalar_policy() {
    let (original, policy) = fixture().await;
    let mut db = Database::<SqliteBackend>::new(original.pool().clone());
    db.set_authorization_mode(AuthorizationMode::ExplicitPolicyForAllExposedOperations);
    db.set_entity_policy(policy.clone());
    db.set_row_policy(policy);
    for unrelated in [false, true] {
        let mut candidate = db.clone();
        if unrelated { candidate.set_field_policy(UnrelatedPolicy); }
        let schema = schema_builder(candidate).finish();
        let denied = schema.execute(request("{ guardedRecord(id: \"r\") { secret } }", true)).await;
        assert!(!denied.errors.is_empty());
        assert!(!format!("{:?}", denied.data).contains("private scalar"));
    }
}

struct UnrelatedPolicy;
impl FieldPolicy for UnrelatedPolicy {
    fn can_read_field<'a>(&'a self, _: &'a Context<'_>, _: &'a Database, _: &'static str, _: &'static str, policy: Option<&'static str>, _: Option<&'a (dyn std::any::Any + Send + Sync)>) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> { Box::pin(async move { Ok(policy.is_none() || policy == Some("unrelated.read")) }) }
    fn can_write_field<'a>(&'a self, _: &'a Context<'_>, _: &'a Database, _: &'static str, _: &'static str, _: Option<&'static str>, _: Option<&'a (dyn std::any::Any + Send + Sync)>, _: Option<&'a (dyn std::any::Any + Send + Sync)>) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> { Box::pin(async { Ok(false) }) }
}
