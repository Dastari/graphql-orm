//! Desired former/new-parent contract, deliberately not implemented by base #254.
//! All tests are UNRUN source specifications. No ignored/waived positive: the
//! former-parent cases are expected to expose the missing committed-edge API.
//! Never make them pass by weakening current-row or tuple provenance checks.
#![cfg(feature = "sqlite")]

use graphql_orm::async_graphql::{Request, Response};
use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
use graphql_orm::prelude::*;
use graphql_orm::uuid::Uuid;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use std::task::Poll;
use tokio::time::{Duration, timeout};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "transition_parents", plural = "TransitionParents", repository_mutations = true)]
#[graphql_orm(operation_authorization(categories = ["subscription"], all_scopes = ["transition.parent"]))]
struct TransitionParent {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    security_tenant_id: Uuid,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    customer_id: Uuid,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: Uuid,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "transition_children", plural = "TransitionChildren")]
#[graphql_orm(operation_authorization(categories = ["subscription"], all_scopes = ["transition.child"]))]
struct TransitionChild {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: Uuid,
    security_tenant_id: Uuid,
    customer_id: Uuid,
    target_id: Option<Uuid>,
    label: String,
    #[graphql(skip)]
    #[relation(target = "TransitionParent", from = ["security_tenant_id", "customer_id", "target_id"], to = ["security_tenant_id", "customer_id", "id"], propagate_change = "up", emit_fk = false)]
    parent: Option<TransitionParent>,
}

schema_roots! { query_custom_ops: [], generated_mutations: "none", entities: [TransitionParent, TransitionChild] }

const CURRENT_AUTH: usize = 1;
const SOURCE_ROW: usize = 2;
const OLD_PARENT_ROW: usize = 4;
const NEW_PARENT_ROW: usize = 8;
const MAPPED_FIELD: usize = 16;
const CHILD_ASSURANCE: usize = 32;
const PARENT_ASSURANCE: usize = 64;

#[derive(Clone, Debug, Default)]
struct Authority(Arc<AtomicUsize>);
impl EntityPolicy for Authority {
    fn can_access_entity<'a>(
        &'a self, ctx: Option<&'a async_graphql::Context<'_>>, _: &'a Database,
        _: &'static str, _: Option<&'static str>, kind: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(ctx.is_none() || kind != EntityAccessKind::Read
                || self.0.load(Ordering::SeqCst) & CURRENT_AUTH == 0)
        })
    }
}
impl RowPolicy for Authority {
    fn can_read_row<'a>(
        &'a self, _: Option<&'a async_graphql::Context<'_>>, _: &'a Database,
        entity: &'static str, _: Option<&'static str>, _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let denied = self.0.load(Ordering::SeqCst);
            if entity == "TransitionChild" { return Ok(denied & SOURCE_ROW == 0); }
            let parent = row.downcast_ref::<TransitionParent>().expect("typed parent policy row");
            Ok(!(parent.id == uid(100) && denied & OLD_PARENT_ROW != 0
                || parent.id == uid(200) && denied & NEW_PARENT_ROW != 0))
        })
    }
    fn can_write_row<'a>(
        &'a self, _: Option<&'a async_graphql::Context<'_>>, _: &'a Database,
        _: &'static str, _: Option<&'static str>, _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    // No deleted-preimage permission: a relation move is not a deletion.
}
impl FieldPolicy for Authority {
    fn can_read_field<'a>(
        &'a self, _: &'a async_graphql::Context<'_>, _: &'a Database,
        entity: &'static str, field: &'static str, _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(entity != "TransitionChild" || field != "targetId"
                || self.0.load(Ordering::SeqCst) & MAPPED_FIELD == 0)
        })
    }
    fn can_write_field<'a>(
        &'a self, _: &'a async_graphql::Context<'_>, _: &'a Database,
        _: &'static str, _: &'static str, _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
impl AssuranceRequirementEvaluator for Authority {
    fn enforce(&self, _: &async_graphql::Context<'_>, _: AssuranceActorClass, policy: &str)
        -> async_graphql::Result<()> {
        let flag = match policy {
            "transition.child" => CHILD_ASSURANCE,
            "transition.parent" => PARENT_ASSURANCE,
            _ => return Err("unexpected operation coordinate".into()),
        };
        if self.0.load(Ordering::SeqCst) & flag == 0 { Ok(()) }
        else { Err("current assurance revoked".into()) }
    }
}
fn uid(value: u128) -> Uuid { Uuid::from_u128(value) }
fn request(child_scope: bool) -> Request {
    let scopes = if child_scope { vec!["transition.parent".into(), "transition.child".into()] }
        else { vec!["transition.parent".into()] };
    Request::new("subscription { transitionParentChanged { sourceId path transitionParent { securityTenantId customerId id } } }")
        .data(AuthSubject::with_details("reader", vec![], scopes, None))
}
fn schema(db: Database, authority: Authority) -> AppSchema {
    let mut registry = OperationAssuranceRegistry::builder(
        graphql_orm_operation_catalog(), AssuranceSchemaConfig::legacy());
    for (field, policy) in [("transitionParentChanged", "transition.parent"),
        ("transitionChildChanged", "transition.child")] {
        registry.require(GraphqlOperationKind::Subscription, field, policy).unwrap();
    }
    schema_builder(db).data(AssuranceEnforcement::new(
        Arc::new(registry.build().unwrap()), Arc::new(authority))).finish()
}
async fn fixture() -> (Database, Authority) {
    let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    // Owned disposable schema only; every fixture write/event below uses the ORM.
    sqlx::query("CREATE TABLE transition_parents (security_tenant_id TEXT NOT NULL, customer_id TEXT NOT NULL, id TEXT NOT NULL, PRIMARY KEY(security_tenant_id, customer_id, id))").execute(db.pool()).await.unwrap();
    sqlx::query("CREATE TABLE transition_children (id TEXT PRIMARY KEY, security_tenant_id TEXT NOT NULL, customer_id TEXT NOT NULL, target_id TEXT, label TEXT NOT NULL)").execute(db.pool()).await.unwrap();
    let authority = Authority::default();
    db.set_entity_policy(authority.clone());
    db.set_row_policy(authority.clone());
    db.set_field_policy(authority.clone());
    for (security, customer, target) in [(1,10,100), (1,20,200), (1,20,100), (2,10,100)] {
        TransitionParent::insert(&db, CreateTransitionParentInput {
            security_tenant_id: uid(security), customer_id: uid(customer), id: uid(target),
        }).await.unwrap();
    }
    TransitionChild::insert(&db, CreateTransitionChildInput {
        id: uid(999), security_tenant_id: uid(1), customer_id: uid(10),
        target_id: Some(uid(100)), label: "initial".into(),
    }).await.unwrap();
    (db, authority)
}
async fn register<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        other => panic!("unexpected registration result {other:?}"),
    }).await;
}
async fn expect_parents<S: Stream<Item = Response> + Unpin>(stream: &mut S, expected: &[(u128,u128,u128)]) {
    let mut actual = Vec::new();
    for _ in expected {
        let response = timeout(Duration::from_secs(2), stream.next()).await
            .expect("required parent invalidation missing").expect("stream ended");
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        let event = &data["transitionParentChanged"];
        assert_eq!(event["sourceId"], uid(999).to_string());
        assert_eq!(event["path"], serde_json::json!(["parent"]));
        let row = &event["transitionParent"];
        actual.push((row["securityTenantId"].as_str().unwrap().to_owned(),
            row["customerId"].as_str().unwrap().to_owned(), row["id"].as_str().unwrap().to_owned()));
    }
    let mut wanted = expected.iter().map(|(s,c,t)|
        (uid(*s).to_string(),uid(*c).to_string(),uid(*t).to_string())).collect::<Vec<_>>();
    actual.sort(); wanted.sort(); assert_eq!(actual, wanted);
    assert!(timeout(Duration::from_millis(100), stream.next()).await.is_err(),
        "duplicate or colliding tenant/customer parent received an event");
}
async fn move_to(db: &Database, customer: u128, target: Option<u128>) {
    TransitionChild::update_by_id(db, &uid(999), UpdateTransitionChildInput {
        customer_id: Some(uid(customer)), target_id: Some(target.map(uid)),
        ..Default::default()
    }).await.unwrap().expect("current source must exist");
}
async fn touch(db: &Database) {
    TransitionChild::update_by_id(db, &uid(999), UpdateTransitionChildInput {
        label: Some("healthy".into()), ..Default::default()
    }).await.unwrap().unwrap();
}

#[tokio::test]
async fn committed_move_invalidates_both_parents_with_complete_business_keys() {
    let (db, authority) = fixture().await;
    let schema = schema(db.clone(), authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
    tx.update_by_id::<TransitionChild>(&uid(999), UpdateTransitionChildInput {
        customer_id: Some(uid(20)), target_id: Some(Some(uid(200))), ..Default::default()
    }).await.unwrap().unwrap();
    expect_parents(&mut stream, &[]).await; // Queued preimage and postimage are not releases.
    tx.commit_and_emit().await.unwrap();
    expect_parents(&mut stream, &[(1,10,100), (1,20,200)]).await;
}

#[tokio::test]
async fn rollback_releases_neither_parent_then_real_commit_is_positive() {
    let (db, authority) = fixture().await;
    let schema = schema(db.clone(), authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
    tx.update_by_id::<TransitionChild>(&uid(999), UpdateTransitionChildInput {
        customer_id: Some(uid(20)), target_id: Some(Some(uid(200))), ..Default::default()
    }).await.unwrap().unwrap();
    tx.rollback().await.unwrap();
    expect_parents(&mut stream, &[]).await;
    move_to(&db, 20, Some(200)).await;
    expect_parents(&mut stream, &[(1,10,100), (1,20,200)]).await;
}

#[tokio::test]
async fn nullable_or_missing_new_parent_still_invalidates_former_parent() {
    for target in [None, Some(404)] {
        let (db, authority) = fixture().await;
        let schema = schema(db.clone(), authority);
        let mut stream = Box::pin(schema.execute_stream(request(true)));
        register(&mut stream).await;
        move_to(&db, 20, target).await;
        expect_parents(&mut stream, &[(1,10,100)]).await;
        move_to(&db, 20, Some(200)).await;
        expect_parents(&mut stream, &[(1,20,200)]).await;
    }
}

#[tokio::test]
async fn current_source_auth_field_and_assurance_revocation_suppresses_both() {
    for denied in [CURRENT_AUTH, SOURCE_ROW, MAPPED_FIELD, CHILD_ASSURANCE, PARENT_ASSURANCE] {
        let (db, authority) = fixture().await;
        let schema = schema(db.clone(), authority.clone());
        let mut stream = Box::pin(schema.execute_stream(request(true)));
        register(&mut stream).await;
        move_to(&db, 20, Some(200)).await;
        authority.0.store(denied, Ordering::SeqCst); // Revoke after COMMIT, before release.
        expect_parents(&mut stream, &[]).await;
        authority.0.store(0, Ordering::SeqCst);
        touch(&db).await;
        expect_parents(&mut stream, &[(1,20,200)]).await;
    }
}

#[tokio::test]
async fn each_parent_row_is_authorized_independently_at_release() {
    for (denied, expected) in [(OLD_PARENT_ROW, (1,20,200)), (NEW_PARENT_ROW, (1,10,100))] {
        let (db, authority) = fixture().await;
        let schema = schema(db.clone(), authority.clone());
        let mut stream = Box::pin(schema.execute_stream(request(true)));
        register(&mut stream).await;
        move_to(&db, 20, Some(200)).await;
        authority.0.store(denied, Ordering::SeqCst);
        expect_parents(&mut stream, &[expected]).await;
        authority.0.store(0, Ordering::SeqCst);
        touch(&db).await;
        expect_parents(&mut stream, &[(1,20,200)]).await;
    }
}

#[tokio::test]
async fn source_operation_scope_is_required_even_when_parent_scope_is_present() {
    let (db, authority) = fixture().await;
    let schema = schema(db.clone(), authority);
    let mut denied = Box::pin(schema.execute_stream(request(false)));
    let mut allowed = Box::pin(schema.execute_stream(request(true)));
    register(&mut denied).await; register(&mut allowed).await;
    move_to(&db, 20, Some(200)).await;
    expect_parents(&mut denied, &[]).await;
    expect_parents(&mut allowed, &[(1,10,100), (1,20,200)]).await;
}

#[tokio::test]
async fn unchanged_edge_deduplicates_old_and_new_parent_identity() {
    let (db, authority) = fixture().await;
    let schema = schema(db.clone(), authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    touch(&db).await;
    expect_parents(&mut stream, &[(1,10,100)]).await;
}
