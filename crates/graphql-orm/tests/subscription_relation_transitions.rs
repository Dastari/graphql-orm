//! Generated relation transitions with explicit fixture-only historical authority.
//! Every case remains UNRUN until a fresh captain scope; no policy opt-in here
//! grants production access. Ordinary stale-edge checks remain independent.
#![cfg(feature = "sqlite")]

use graphql_orm::async_graphql::{Request, Response, Variables};
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
    #[filterable(type = "uuid")]
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
#[graphql_orm(operation_authorization(categories = ["update"], all_scopes = ["transition.update"]))]
struct TransitionChild {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "uuid")]
    id: Uuid,
    security_tenant_id: Uuid,
    customer_id: Uuid,
    target_id: Option<Uuid>,
    label: String,
    #[graphql(skip)]
    #[relation(target = "TransitionParent", from = ["security_tenant_id", "customer_id", "target_id"], to = ["security_tenant_id", "customer_id", "id"], propagate_change = "up", emit_fk = false)]
    parent: Option<TransitionParent>,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "composite_transition_children", plural = "CompositeTransitionChildren", repository_mutations = true)]
#[graphql_orm(operation_authorization(categories = ["subscription"], all_scopes = ["transition.child"]))]
struct CompositeTransitionChild {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    security_tenant_id: Uuid,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: Uuid,
    customer_id: Uuid,
    target_id: Option<Uuid>,
    #[graphql(skip)]
    #[relation(target = "TransitionParent", from = ["security_tenant_id", "customer_id", "target_id"], to = ["security_tenant_id", "customer_id", "id"], propagate_change = "up", emit_fk = false)]
    parent: Option<TransitionParent>,
}

// Dedicated conditional fixture: the original unversioned entities and all
// existing case bodies keep their original write contracts.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "versioned_transition_children", plural = "VersionedTransitionChildren")]
#[graphql_orm(operation_authorization(categories = ["subscription"], all_scopes = ["transition.child"]))]
struct VersionedTransitionChild {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: Uuid,
    security_tenant_id: Uuid,
    customer_id: Uuid,
    target_id: Option<Uuid>,
    #[filterable(type = "string")]
    label: String,
    #[graphql_orm(version, default = "0")]
    version: i64,
    #[graphql(skip)]
    #[relation(target = "TransitionParent", from = ["security_tenant_id", "customer_id", "target_id"], to = ["security_tenant_id", "customer_id", "id"], propagate_change = "up", emit_fk = false)]
    parent: Option<TransitionParent>,
}

schema_roots! {
    query_custom_ops: [],
    generated_mutations: "allowlist",
    generated_mutation_allowlist: [TransitionChild],
    entities: [TransitionParent, TransitionChild, CompositeTransitionChild, VersionedTransitionChild]
}

const CURRENT_AUTH: usize = 1;
const SOURCE_ROW: usize = 2;
const OLD_PARENT_ROW: usize = 4;
const NEW_PARENT_ROW: usize = 8;
const MAPPED_FIELD: usize = 16;
const CHILD_ASSURANCE: usize = 32;
const PARENT_ASSURANCE: usize = 64;
const HISTORICAL_EDGE: usize = 128;
const AFTER_SERIALIZATION: usize = 256;

#[derive(Clone, Debug, Default)]
struct Authority(Arc<AtomicUsize>, Arc<AtomicUsize>);
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
            if entity == "TransitionChild" || entity == "CompositeTransitionChild"
                || entity == "VersionedTransitionChild" { return Ok(denied & SOURCE_ROW == 0); }
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
    fn can_read_relation_transition<'a>(
        &'a self, _: &'a async_graphql::Context<'_>, _: &'a Database,
        transition: &'a graphql_orm::graphql::subscriptions::RelationTransitionRead<'a>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            // Deliberate fixture-only grant over this exact owned namespace.
            // A production provider must also establish historical SQL/RLS.
            if self.0.load(Ordering::SeqCst) & AFTER_SERIALIZATION != 0
                && self.1.fetch_add(1, Ordering::SeqCst) >= 2 { return Ok(false); }
            let permitted = self.0.load(Ordering::SeqCst) & HISTORICAL_EDGE == 0
                && transition.target_entity == "TransitionParent"
                && transition.relation == "parent"
                && transition.mapping == &[("security_tenant_id", "security_tenant_id"),
                    ("customer_id", "customer_id"), ("target_id", "id")]
                && transition.before_values.first() == Some(&SqlValue::Uuid(uid(1)))
                && transition.after_values.first() == Some(&SqlValue::Uuid(uid(1)))
                && transition.current_target.downcast_ref::<TransitionParent>().is_some();
            Ok(permitted && match transition.source_entity {
                "TransitionChild" => transition.before_source.downcast_ref::<TransitionChild>().is_some()
                    && transition.after_source.downcast_ref::<TransitionChild>().is_some()
                    && transition.current_source.downcast_ref::<TransitionChild>().is_some(),
                "CompositeTransitionChild" => transition.before_source.downcast_ref::<CompositeTransitionChild>().is_some()
                    && transition.after_source.downcast_ref::<CompositeTransitionChild>().is_some()
                    && transition.current_source.downcast_ref::<CompositeTransitionChild>().is_some(),
                "VersionedTransitionChild" => transition.before_source.downcast_ref::<VersionedTransitionChild>().is_some()
                    && transition.after_source.downcast_ref::<VersionedTransitionChild>().is_some()
                    && transition.current_source.downcast_ref::<VersionedTransitionChild>().is_some(),
                _ => false,
            })
        })
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
            Ok((entity != "TransitionChild" && entity != "CompositeTransitionChild"
                && entity != "VersionedTransitionChild") || field != "targetId"
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
        ("transitionChildChanged", "transition.child"),
        ("compositeTransitionChildChanged", "transition.child"),
        ("versionedTransitionChildChanged", "transition.child")] {
        registry.require(GraphqlOperationKind::Subscription, field, policy).unwrap();
    }
    schema_builder(db).data(AssuranceEnforcement::new(
        Arc::new(registry.build().unwrap()), Arc::new(authority))).finish()
}
async fn fixture() -> (Database, Authority) {
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
        .connect("sqlite::memory:").await.unwrap();
    let mut db = Database::new(pool);
    sqlx::query("PRAGMA foreign_keys = ON").execute(db.pool()).await.unwrap();
    sqlx::query("CREATE TABLE transition_labels (label TEXT PRIMARY KEY)").execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO transition_labels VALUES ('initial'), ('healthy')").execute(db.pool()).await.unwrap();
    // Owned disposable schema only; every fixture write/event below uses the ORM.
    sqlx::query("CREATE TABLE transition_parents (security_tenant_id TEXT NOT NULL, customer_id TEXT NOT NULL, id TEXT NOT NULL, PRIMARY KEY(security_tenant_id, customer_id, id))").execute(db.pool()).await.unwrap();
    sqlx::query("CREATE TABLE transition_children (id TEXT PRIMARY KEY, security_tenant_id TEXT NOT NULL, customer_id TEXT NOT NULL, target_id TEXT, label TEXT NOT NULL REFERENCES transition_labels(label) DEFERRABLE INITIALLY DEFERRED)").execute(db.pool()).await.unwrap();
    sqlx::query("CREATE TABLE composite_transition_children (security_tenant_id TEXT NOT NULL, id TEXT NOT NULL, customer_id TEXT NOT NULL, target_id TEXT, PRIMARY KEY(security_tenant_id,id))").execute(db.pool()).await.unwrap();
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
    expect_parent_source(stream, expected, uid(999).to_string()).await;
}
async fn expect_parent_source<S: Stream<Item = Response> + Unpin>(
    stream: &mut S, expected: &[(u128,u128,u128)], source_id: String,
) {
    let mut actual = Vec::new();
    for _ in expected {
        let response = timeout(Duration::from_secs(2), stream.next()).await
            .expect("required parent invalidation missing").expect("stream ended");
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        let event = &data["transitionParentChanged"];
        assert_eq!(event["sourceId"], source_id);
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

#[tokio::test]
async fn historical_permission_is_independent_and_default_deny() {
    let (db, authority) = fixture().await;
    let schema = schema(db.clone(), authority.clone());
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    authority.0.store(HISTORICAL_EDGE, Ordering::SeqCst);
    move_to(&db, 20, Some(200)).await;
    expect_parents(&mut stream, &[(1,20,200)]).await;
    authority.0.store(0, Ordering::SeqCst);
    move_to(&db, 10, Some(100)).await;
    expect_parents(&mut stream, &[(1,20,200), (1,10,100)]).await;
}

#[derive(Clone)]
struct CurrentOnly(Authority, bool);
impl RowPolicy for CurrentOnly {
    fn read_visibility<'a>(
        &'a self, _: Option<&'a async_graphql::Context<'_>>, _: &'a Database,
        entity: &'static str, _: Option<&'static str>, _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            if self.1 && entity == "TransitionChild" {
                let filter = TransitionChildWhereInput {
                    id: Some(UuidFilter { eq: Some(uid(999)), ..Default::default() }),
                    ..Default::default()
                };
                Ok(ReadVisibility::Complete(ReadPredicate::from_filter::<SqliteBackend, _>(&filter)?))
            } else if self.1 && entity == "TransitionParent" {
                let filter = TransitionParentWhereInput {
                    security_tenant_id: Some(UuidFilter { eq: Some(uid(1)), ..Default::default() }),
                    ..Default::default()
                };
                Ok(ReadVisibility::Complete(ReadPredicate::from_filter::<SqliteBackend, _>(&filter)?))
            } else { Ok(ReadVisibility::CallbackOnly) }
        })
    }
    fn can_read_row<'a>(
        &'a self, ctx: Option<&'a async_graphql::Context<'_>>, db: &'a Database,
        entity: &'static str, policy: Option<&'static str>, surface: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        self.0.can_read_row(ctx, db, entity, policy, surface, row)
    }
    fn can_write_row<'a>(
        &'a self, ctx: Option<&'a async_graphql::Context<'_>>, db: &'a Database,
        entity: &'static str, policy: Option<&'static str>, surface: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        self.0.can_write_row(ctx, db, entity, policy, surface, row)
    }
    fn can_read_deleted_preimage<'a>(
        &'a self, _: Option<&'a async_graphql::Context<'_>>, _: &'a Database,
        _: &'static str, _: Option<&'static str>, _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    // Even explicit deletion permission is not historical-edge permission.
    // No historical override: default DENY even with complete SQL visibility.
}

#[tokio::test]
async fn current_only_callback_and_sql_providers_cannot_grant_historical_edges() {
    for sql_only in [false, true] {
        let (mut db, authority) = fixture().await;
        db.set_row_policy(CurrentOnly(authority.clone(), sql_only));
        let schema = schema(db.clone(), authority);
        let mut stream = Box::pin(schema.execute_stream(request(true)));
        register(&mut stream).await;
        move_to(&db, 20, Some(200)).await;
        expect_parents(&mut stream, &[(1,20,200)]).await;
        touch(&db).await;
        expect_parents(&mut stream, &[(1,20,200)]).await;
    }
}

#[tokio::test]
async fn failed_commit_discards_both_transition_legs() {
    let (db, authority) = fixture().await;
    let schema = schema(db.clone(), authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
    tx.update_by_id::<TransitionChild>(&uid(999), UpdateTransitionChildInput {
        customer_id: Some(uid(20)), target_id: Some(Some(uid(200))),
        label: Some("missing-label".into()), ..Default::default()
    }).await.unwrap().unwrap();
    assert!(tx.commit_and_emit().await.is_err());
    expect_parents(&mut stream, &[]).await;
    move_to(&db, 20, Some(200)).await;
    expect_parents(&mut stream, &[(1,10,100), (1,20,200)]).await;
}

#[tokio::test]
async fn composite_source_update_carries_complete_key_transition() {
    let (db, authority) = fixture().await;
    CompositeTransitionChild::insert(&db, CreateCompositeTransitionChildInput {
        security_tenant_id: uid(1), id: uid(888), customer_id: uid(10), target_id: Some(uid(100)),
    }).await.unwrap();
    let schema = schema(db.clone(), authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    CompositeTransitionChild::update_by_key(&db, &CompositeTransitionChildKey {
        security_tenant_id: uid(1), id: uid(888),
    }, UpdateCompositeTransitionChildInput {
        customer_id: Some(uid(20)), target_id: Some(Some(uid(200))), ..Default::default()
    }).await.unwrap().unwrap();
    let source_id = graphql_orm::graphql::orm::composite_key_id(&[
        SqlValue::Uuid(uid(1)), SqlValue::Uuid(uid(888)),
    ]);
    expect_parent_source(&mut stream, &[(1,10,100), (1,20,200)], source_id).await;
}

#[tokio::test]
async fn historical_authority_is_rechecked_after_payload_serialization() {
    let (db, authority) = fixture().await;
    let schema = schema(db.clone(), authority.clone());
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    move_to(&db, 20, Some(200)).await;
    authority.0.store(AFTER_SERIALIZATION, Ordering::SeqCst);
    expect_parents(&mut stream, &[(1,20,200)]).await;
    assert!(authority.1.load(Ordering::SeqCst) >= 3,
        "historical leg must reach the proof after successful payload preparation");
    authority.0.store(0, Ordering::SeqCst);
    touch(&db).await;
    expect_parents(&mut stream, &[(1,20,200)]).await;
}

#[tokio::test]
async fn missing_former_target_does_not_veto_readable_current_target() {
    let (db, authority) = fixture().await;
    let schema = schema(db.clone(), authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    move_to(&db, 20, Some(200)).await;
    assert!(TransitionParent::delete_by_key(&db, &TransitionParentKey {
        security_tenant_id: uid(1), customer_id: uid(10), id: uid(100),
    }).await.unwrap());
    expect_parents(&mut stream, &[(1,20,200)]).await;
    touch(&db).await;
    expect_parents(&mut stream, &[(1,20,200)]).await;
}

#[tokio::test]
async fn complete_sql_visibility_does_not_replace_current_association_callbacks() {
    for denied in [SOURCE_ROW, NEW_PARENT_ROW] {
        let (mut db, authority) = fixture().await;
        db.set_row_policy(CurrentOnly(authority.clone(), true));
        let schema = schema(db.clone(), authority.clone());
        let mut stream = Box::pin(schema.execute_stream(request(true)));
        register(&mut stream).await;
        move_to(&db, 20, Some(200)).await;
        authority.0.store(denied, Ordering::SeqCst);
        expect_parents(&mut stream, &[]).await;
        authority.0.store(0, Ordering::SeqCst);
        touch(&db).await;
        expect_parents(&mut stream, &[(1,20,200)]).await;
    }
}

// Exercise the actual schema_roots generated mutation, not repository calls or
// hand-composed roots. Only the test child exposes mutations in this fixture.
fn graphql_update_request(input: serde_json::Value, write_scope: bool) -> Request {
    let document = format!(
        "mutation($input: UpdateTransitionChildInput!) {{ updateTransitionChild(id: \"{}\", input: $input) {{ success error }} }}",
        uid(999),
    );
    let scopes = if write_scope { vec!["transition.update".into()] } else { vec![] };
    Request::new(document)
        .variables(Variables::from_json(serde_json::json!({"input": input})))
        .data(AuthSubject::with_details("writer", vec![], scopes, None))
}
fn graphql_move_input(label: &str) -> serde_json::Value {
    serde_json::json!({
        "customerId": uid(20).to_string(),
        "targetId": uid(200).to_string(),
        "label": label,
    })
}
fn assert_graphql_update_success(response: Response) {
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["updateTransitionChild"]["success"], true, "{data}");
    assert!(data["updateTransitionChild"]["error"].is_null(), "{data}");
}

#[tokio::test]
async fn graphql_update_commits_former_and_new_parent_invalidations() {
    let (db, authority) = fixture().await;
    let schema = schema(db, authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    assert_graphql_update_success(schema.execute(
        graphql_update_request(graphql_move_input("healthy"), true),
    ).await);
    expect_parents(&mut stream, &[(1,10,100), (1,20,200)]).await;
    assert_graphql_update_success(schema.execute(graphql_update_request(
        serde_json::json!({"label": "initial"}), true,
    )).await);
    expect_parents(&mut stream, &[(1,20,200)]).await;
}

#[tokio::test]
async fn graphql_update_failed_commit_discards_transition_and_retry_observes_original_edge() {
    let (db, authority) = fixture().await;
    let schema = schema(db, authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    let failed = schema.execute(graphql_update_request(
        graphql_move_input("missing-label"), true,
    )).await;
    // The deferred FK fails at actual COMMIT, after the resolver queued both legs.
    assert!(!failed.errors.is_empty(), "a failed COMMIT cannot return success");
    expect_parents(&mut stream, &[]).await;
    assert_graphql_update_success(schema.execute(
        graphql_update_request(graphql_move_input("healthy"), true),
    ).await);
    // Both keys are required: a partially committed first attempt would have
    // lost the original parent and cannot satisfy this restored positive.
    expect_parents(&mut stream, &[(1,10,100), (1,20,200)]).await;
}

#[tokio::test]
async fn graphql_update_preserves_default_historical_denial_and_current_subscriber_authority() {
    for denied in [HISTORICAL_EDGE, CURRENT_AUTH] {
        let (mut db, authority) = fixture().await;
        if denied == HISTORICAL_EDGE {
            // No historical hook override, despite complete current SQL
            // visibility and explicit deleted-preimage permission.
            db.set_row_policy(CurrentOnly(authority.clone(), true));
        }
        let schema = schema(db, authority.clone());
        let mut stream = Box::pin(schema.execute_stream(request(true)));
        register(&mut stream).await;
        assert_graphql_update_success(schema.execute(
            graphql_update_request(graphql_move_input("healthy"), true),
        ).await);
        // Revoke only after the successful mutation committed, before delivery.
        authority.0.store(denied, Ordering::SeqCst);
        if denied == HISTORICAL_EDGE {
            expect_parents(&mut stream, &[(1,20,200)]).await;
        } else {
            expect_parents(&mut stream, &[]).await;
        }
        authority.0.store(0, Ordering::SeqCst);
        assert_graphql_update_success(schema.execute(graphql_update_request(
            serde_json::json!({"label": "initial"}), true,
        )).await);
        expect_parents(&mut stream, &[(1,20,200)]).await;
    }
}

#[tokio::test]
async fn graphql_update_missing_operation_scope_leaves_original_edge_unchanged() {
    let (db, authority) = fixture().await;
    let schema = schema(db, authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    let denied = schema.execute(graphql_update_request(
        graphql_move_input("healthy"), false,
    )).await;
    assert!(!denied.errors.is_empty(), "update requires its own write scope");
    expect_parents(&mut stream, &[]).await;
    assert_graphql_update_success(schema.execute(
        graphql_update_request(graphql_move_input("healthy"), true),
    ).await);
    expect_parents(&mut stream, &[(1,10,100), (1,20,200)]).await;
}

async fn move_in_transaction(tx: &mut MutationContext<'_>, customer: u128, target: u128) {
    tx.update_by_id::<TransitionChild>(&uid(999), UpdateTransitionChildInput {
        customer_id: Some(uid(customer)), target_id: Some(Some(uid(target))), ..Default::default()
    }).await.unwrap().expect("current source must exist");
}

#[tokio::test]
async fn transaction_first_before_final_after_excludes_transient_parent() {
    let (db, authority) = fixture().await;
    let schema = schema(db.clone(), authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
    move_in_transaction(&mut tx, 20, 200).await; // A -> B, B never committed.
    move_in_transaction(&mut tx, 20, 100).await; // C shares A's id, B's customer.
    expect_parents(&mut stream, &[]).await;
    tx.commit_and_emit().await.unwrap();
    expect_parents(&mut stream, &[(1,10,100), (1,20,100)]).await;
}

#[tokio::test]
async fn transaction_return_to_original_parent_excludes_transient_parent() {
    let (db, authority) = fixture().await;
    let schema = schema(db.clone(), authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
    move_in_transaction(&mut tx, 20, 200).await;
    move_in_transaction(&mut tx, 10, 100).await;
    tx.commit_and_emit().await.unwrap();
    expect_parents(&mut stream, &[(1,10,100)]).await;
}

#[tokio::test]
async fn transaction_repeated_final_target_is_deduplicated() {
    let (db, authority) = fixture().await;
    let schema = schema(db.clone(), authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
    for (customer, target) in [(20,200), (20,100), (20,100), (20,100)] {
        move_in_transaction(&mut tx, customer, target).await;
    }
    tx.commit_and_emit().await.unwrap();
    expect_parents(&mut stream, &[(1,10,100), (1,20,100)]).await;
}

#[tokio::test]
async fn transaction_rollback_and_failed_commit_discard_all_aggregated_edges() {
    for fail_commit in [false, true] {
        let (db, authority) = fixture().await;
        let schema = schema(db.clone(), authority);
        let mut stream = Box::pin(schema.execute_stream(request(true)));
        register(&mut stream).await;
        let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
        move_in_transaction(&mut tx, 20, 200).await;
        move_in_transaction(&mut tx, 20, 100).await;
        if fail_commit {
            tx.update_by_id::<TransitionChild>(&uid(999), UpdateTransitionChildInput {
                label: Some("missing-label".into()), ..Default::default()
            }).await.unwrap().unwrap();
            assert!(tx.commit_and_emit().await.is_err());
        } else {
            tx.rollback().await.unwrap();
        }
        expect_parents(&mut stream, &[]).await;
        // A real subsequent generated write must still observe committed A.
        move_to(&db, 20, Some(200)).await;
        expect_parents(&mut stream, &[(1,10,100), (1,20,200)]).await;
    }
}

#[tokio::test]
async fn aggregated_transition_retains_current_and_historical_authorization() {
    for denial in [HISTORICAL_EDGE, SOURCE_ROW, MAPPED_FIELD, CURRENT_AUTH,
        CHILD_ASSURANCE, PARENT_ASSURANCE] {
        let (db, authority) = fixture().await;
        let schema = schema(db.clone(), authority.clone());
        let mut stream = Box::pin(schema.execute_stream(request(true)));
        register(&mut stream).await;
        let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
        move_in_transaction(&mut tx, 20, 200).await;
        move_in_transaction(&mut tx, 20, 100).await;
        tx.commit_and_emit().await.unwrap();
        authority.0.store(denial, Ordering::SeqCst);
        if denial == HISTORICAL_EDGE {
            expect_parents(&mut stream, &[(1,20,100)]).await;
        } else {
            expect_parents(&mut stream, &[]).await;
        }
        authority.0.store(0, Ordering::SeqCst);
        touch(&db).await;
        expect_parents(&mut stream, &[(1,20,100)]).await;
    }
}

#[tokio::test]
async fn composite_context_moves_preserve_first_and_final_complete_tuples() {
    let (db, authority) = fixture().await;
    CompositeTransitionChild::insert(&db, CreateCompositeTransitionChildInput {
        security_tenant_id: uid(1), id: uid(888), customer_id: uid(10), target_id: Some(uid(100)),
    }).await.unwrap();
    let schema = schema(db.clone(), authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
    for target in [200, 100] {
        tx.update_by_key::<CompositeTransitionChild>(&CompositeTransitionChildKey {
            security_tenant_id: uid(1), id: uid(888),
        }, UpdateCompositeTransitionChildInput {
            customer_id: Some(uid(20)), target_id: Some(Some(uid(target))), ..Default::default()
        }).await.unwrap().unwrap();
    }
    tx.commit_and_emit().await.unwrap();
    let source_id = graphql_orm::graphql::orm::composite_key_id(&[
        SqlValue::Uuid(uid(1)), SqlValue::Uuid(uid(888)),
    ]);
    expect_parent_source(&mut stream, &[(1,10,100), (1,20,100)], source_id).await;
}

#[tokio::test]
async fn inserted_then_moved_source_has_no_committed_historical_association() {
    let (db, authority) = fixture().await;
    let schema = schema(db.clone(), authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
    tx.insert::<TransitionChild>(CreateTransitionChildInput {
        id: uid(998), security_tenant_id: uid(1), customer_id: uid(10),
        target_id: Some(uid(100)), label: "initial".into(),
    }).await.unwrap();
    tx.update_by_id::<TransitionChild>(&uid(998), UpdateTransitionChildInput {
        customer_id: Some(uid(20)), target_id: Some(Some(uid(200))), ..Default::default()
    }).await.unwrap().unwrap();
    tx.commit_and_emit().await.unwrap();
    expect_parent_source(&mut stream, &[(1,20,200)], uid(998).to_string()).await;
}

#[tokio::test]
async fn aggregated_sources_with_shared_first_key_component_remain_independent() {
    let (db, authority) = fixture().await;
    for id in [888, 889] {
        CompositeTransitionChild::insert(&db, CreateCompositeTransitionChildInput {
            security_tenant_id: uid(1), id: uid(id), customer_id: uid(10), target_id: Some(uid(100)),
        }).await.unwrap();
    }
    let schema = schema(db.clone(), authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
    for (id, customer, target) in [(888,20,200), (889,20,200), (888,20,100), (889,10,100)] {
        tx.update_by_key::<CompositeTransitionChild>(&CompositeTransitionChildKey {
            security_tenant_id: uid(1), id: uid(id),
        }, UpdateCompositeTransitionChildInput {
            customer_id: Some(uid(customer)), target_id: Some(Some(uid(target))), ..Default::default()
        }).await.unwrap().unwrap();
    }
    tx.commit_and_emit().await.unwrap();
    let mut actual = Vec::new();
    for _ in 0..3 {
        let response = timeout(Duration::from_secs(2), stream.next()).await.unwrap().unwrap();
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        let event = &data["transitionParentChanged"];
        assert_eq!(event["path"], serde_json::json!(["parent"]));
        let row = &event["transitionParent"];
        actual.push((event["sourceId"].as_str().unwrap().to_owned(),
            row["securityTenantId"].as_str().unwrap().to_owned(),
            row["customerId"].as_str().unwrap().to_owned(), row["id"].as_str().unwrap().to_owned()));
    }
    let mut expected = [(888,10,100), (888,20,100), (889,10,100)].into_iter().map(|(id,c,t)| {
        (graphql_orm::graphql::orm::composite_key_id(&[SqlValue::Uuid(uid(1)), SqlValue::Uuid(uid(id))]),
            uid(1).to_string(), uid(c).to_string(), uid(t).to_string())
    }).collect::<Vec<_>>();
    actual.sort(); expected.sort(); assert_eq!(actual, expected);
    assert!(timeout(Duration::from_millis(100), stream.next()).await.is_err(),
        "transient or duplicate association was released");
}

#[tokio::test]
async fn aggregated_transitions_do_not_upgrade_default_historical_denial() {
    for sql_visibility in [false, true] {
        let (mut db, authority) = fixture().await;
        db.set_row_policy(CurrentOnly(authority.clone(), sql_visibility));
        let schema = schema(db.clone(), authority);
        let mut stream = Box::pin(schema.execute_stream(request(true)));
        register(&mut stream).await;
        let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
        move_in_transaction(&mut tx, 20, 200).await;
        move_in_transaction(&mut tx, 20, 100).await;
        tx.commit_and_emit().await.unwrap();
        expect_parents(&mut stream, &[(1,20,100)]).await;
    }
}

async fn conditional_fixture() -> (Database, Authority) {
    let (db, authority) = fixture().await;
    // Owned schema setup only. All conditional data mutations below are generated.
    sqlx::query("CREATE TABLE versioned_transition_children (id TEXT PRIMARY KEY, security_tenant_id TEXT NOT NULL, customer_id TEXT NOT NULL, target_id TEXT, label TEXT NOT NULL REFERENCES transition_labels(label) DEFERRABLE INITIALLY DEFERRED, version INTEGER NOT NULL DEFAULT 0)")
        .execute(db.pool()).await.unwrap();
    VersionedTransitionChild::insert(&db, CreateVersionedTransitionChildInput {
        id: uid(999), security_tenant_id: uid(1), customer_id: uid(10),
        target_id: Some(uid(100)), label: "initial".into(),
    }).await.unwrap();
    (db, authority)
}
fn cas_expected(label: &str) -> VersionedTransitionChildWhereInput {
    VersionedTransitionChildWhereInput {
        label: Some(StringFilter { eq: Some(label.into()), ..Default::default() }),
        ..Default::default()
    }
}
fn cas_move(customer: u128, target: u128) -> UpdateVersionedTransitionChildInput {
    UpdateVersionedTransitionChildInput {
        customer_id: Some(uid(customer)), target_id: Some(Some(uid(target))), ..Default::default()
    }
}
async fn cas_in_transaction(tx: &mut MutationContext<'_>, version: i64, customer: u128, target: u128) {
    let outcome = tx.compare_and_swap::<VersionedTransitionChild>(
        &uid(999), version, cas_expected("initial"), cas_move(customer, target),
    ).await.unwrap();
    let ConditionalUpdateOutcome::Updated(row) = outcome else { panic!("matching CAS must update"); };
    assert_eq!(row.version, version + 1);
    assert_eq!((row.customer_id, row.target_id), (uid(customer), Some(uid(target))));
}
fn predicate_key(security: u128) -> CompositeTransitionChildKey {
    CompositeTransitionChildKey { security_tenant_id: uid(security), id: uid(888) }
}
fn predicate_expected(security: u128) -> CompositeTransitionChildWhereInput {
    CompositeTransitionChildWhereInput {
        security_tenant_id: Some(UuidFilter { eq: Some(uid(security)), ..Default::default() }),
        ..Default::default()
    }
}
fn predicate_move(customer: u128, target: u128) -> UpdateCompositeTransitionChildInput {
    UpdateCompositeTransitionChildInput {
        customer_id: Some(uid(customer)), target_id: Some(Some(uid(target))), ..Default::default()
    }
}
fn predicate_source_id() -> String {
    graphql_orm::graphql::orm::composite_key_id(&[SqlValue::Uuid(uid(1)), SqlValue::Uuid(uid(888))])
}
async fn insert_predicate_source(db: &Database) {
    CompositeTransitionChild::insert(db, CreateCompositeTransitionChildInput {
        security_tenant_id: uid(1), id: uid(888), customer_id: uid(10), target_id: Some(uid(100)),
    }).await.unwrap();
}
async fn predicate_in_transaction(tx: &mut MutationContext<'_>, customer: u128, target: u128) {
    let outcome = tx.update_if::<CompositeTransitionChild>(
        &predicate_key(1), predicate_expected(1), predicate_move(customer, target),
    ).await.unwrap();
    let PredicateUpdateOutcome::Updated(row) = outcome else { panic!("matching predicate must update"); };
    assert_eq!((row.security_tenant_id, row.customer_id, row.target_id),
        (uid(1), uid(customer), Some(uid(target))));
}

#[tokio::test]
async fn conditional_cas_repository_commits_former_and_new_parents() {
    let (db, authority) = conditional_fixture().await;
    let schema = schema(db.clone(), authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    let outcome = VersionedTransitionChild::compare_and_swap(
        &db, &uid(999), 0, cas_expected("initial"), cas_move(20, 200),
    ).await.unwrap();
    let ConditionalUpdateOutcome::Updated(row) = outcome else { panic!("matching CAS must update"); };
    assert_eq!(row.version, 1);
    expect_parents(&mut stream, &[(1,10,100), (1,20,200)]).await;
}

#[tokio::test]
async fn conditional_cas_failed_version_predicate_and_missing_row_do_not_queue() {
    let (db, authority) = conditional_fixture().await;
    let schema = schema(db.clone(), authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    for (id, version, label) in [(999,9,"initial"), (999,0,"healthy"), (404,0,"initial")] {
        let outcome = VersionedTransitionChild::compare_and_swap(
            &db, &uid(id), version, cas_expected(label), cas_move(20, 100),
        ).await.unwrap();
        if id == 404 { assert!(matches!(outcome, ConditionalUpdateOutcome::NotFound)); }
        else { assert!(matches!(outcome, ConditionalUpdateOutcome::Conflict)); }
        expect_parents(&mut stream, &[]).await;
    }
    // Version0 and formerA must still be current after all failed attempts.
    let outcome = VersionedTransitionChild::compare_and_swap(
        &db, &uid(999), 0, cas_expected("initial"), cas_move(20, 200),
    ).await.unwrap();
    let ConditionalUpdateOutcome::Updated(row) = outcome else { panic!("original version must remain current"); };
    assert_eq!(row.version, 1);
    expect_parents(&mut stream, &[(1,10,100), (1,20,200)]).await;
}

#[tokio::test]
async fn conditional_predicate_uses_complete_key_and_keeps_failed_attempts_silent() {
    let (db, authority) = fixture().await;
    insert_predicate_source(&db).await;
    CompositeTransitionChild::insert(&db, CreateCompositeTransitionChildInput {
        security_tenant_id: uid(2), id: uid(888), customer_id: uid(10), target_id: Some(uid(100)),
    }).await.unwrap();
    let schema = schema(db.clone(), authority);
    let mut stream = Box::pin(schema.execute_stream(request(true)));
    register(&mut stream).await;
    assert!(matches!(CompositeTransitionChild::update_if(
        &db, &predicate_key(1), predicate_expected(2), predicate_move(20, 100),
    ).await.unwrap(), PredicateUpdateOutcome::PredicateConflict));
    let absent = CompositeTransitionChildKey { security_tenant_id: uid(1), id: uid(404) };
    assert!(matches!(CompositeTransitionChild::update_if(
        &db, &absent, predicate_expected(1), predicate_move(20, 100),
    ).await.unwrap(), PredicateUpdateOutcome::NotFound));
    expect_parents(&mut stream, &[]).await;
    assert!(matches!(CompositeTransitionChild::update_if(
        &db, &predicate_key(1), predicate_expected(1), predicate_move(20, 200),
    ).await.unwrap(), PredicateUpdateOutcome::Updated(_)));
    expect_parent_source(&mut stream, &[(1,10,100), (1,20,200)], predicate_source_id()).await;
    let untouched = CompositeTransitionChild::find_by_key(&db, &predicate_key(2)).await.unwrap().unwrap();
    assert_eq!((untouched.customer_id, untouched.target_id), (uid(10), Some(uid(100))));
    let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
    predicate_in_transaction(&mut tx, 20, 100).await;
    tx.commit_and_emit().await.unwrap();
    expect_parent_source(&mut stream, &[(1,20,200), (1,20,100)], predicate_source_id()).await;
}

#[tokio::test]
async fn conditional_cas_repeated_moves_and_failed_intermediate_attempt_keep_first_and_final() {
    for return_to_a in [false, true] {
        let (db, authority) = conditional_fixture().await;
        let schema = schema(db.clone(), authority);
        let mut stream = Box::pin(schema.execute_stream(request(true)));
        register(&mut stream).await;
        let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
        cas_in_transaction(&mut tx, 0, 20, 200).await;
        assert!(matches!(tx.compare_and_swap::<VersionedTransitionChild>(
            &uid(999), 0, cas_expected("initial"), cas_move(20, 100),
        ).await.unwrap(), ConditionalUpdateOutcome::Conflict));
        cas_in_transaction(&mut tx, 1, if return_to_a { 10 } else { 20 }, 100).await;
        expect_parents(&mut stream, &[]).await;
        tx.commit_and_emit().await.unwrap();
        if return_to_a { expect_parents(&mut stream, &[(1,10,100)]).await; }
        else { expect_parents(&mut stream, &[(1,10,100), (1,20,100)]).await; }
    }
}

#[tokio::test]
async fn conditional_and_regular_updates_share_first_and_final_aggregation_in_both_orders() {
    for cas_first in [false, true] {
        for return_to_a in [false, true] {
            let (db, authority) = conditional_fixture().await;
            let schema = schema(db.clone(), authority);
            let mut stream = Box::pin(schema.execute_stream(request(true)));
            register(&mut stream).await;
            let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
            let final_customer = if return_to_a { 10 } else { 20 };
            if cas_first {
                cas_in_transaction(&mut tx, 0, 20, 200).await;
                tx.update_by_id::<VersionedTransitionChild>(&uid(999), cas_move(final_customer, 100))
                    .await.unwrap().unwrap();
            } else {
                tx.update_by_id::<VersionedTransitionChild>(&uid(999), cas_move(20, 200))
                    .await.unwrap().unwrap();
                cas_in_transaction(&mut tx, 0, final_customer, 100).await;
            }
            tx.commit_and_emit().await.unwrap();
            if return_to_a { expect_parents(&mut stream, &[(1,10,100)]).await; }
            else { expect_parents(&mut stream, &[(1,10,100), (1,20,100)]).await; }
        }
    }
}

#[tokio::test]
async fn conditional_predicate_repeated_moves_ignore_failed_intermediate_predicate() {
    for return_to_a in [false, true] {
        let (db, authority) = fixture().await;
        insert_predicate_source(&db).await;
        let schema = schema(db.clone(), authority);
        let mut stream = Box::pin(schema.execute_stream(request(true)));
        register(&mut stream).await;
        let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
        predicate_in_transaction(&mut tx, 20, 200).await;
        assert!(matches!(tx.update_if::<CompositeTransitionChild>(
            &predicate_key(1), predicate_expected(2), predicate_move(20, 100),
        ).await.unwrap(), PredicateUpdateOutcome::PredicateConflict));
        predicate_in_transaction(&mut tx, if return_to_a { 10 } else { 20 }, 100).await;
        tx.commit_and_emit().await.unwrap();
        let expected = if return_to_a { vec![(1,10,100)] } else { vec![(1,10,100), (1,20,100)] };
        expect_parent_source(&mut stream, &expected, predicate_source_id()).await;
    }
}

#[tokio::test]
async fn conditional_rollback_and_failed_commit_discard_both_legs_and_preserve_original_edge() {
    for use_cas in [false, true] {
        for failed_commit in [false, true] {
            let (db, authority) = conditional_fixture().await;
            if !use_cas { insert_predicate_source(&db).await; }
            let schema = schema(db.clone(), authority);
            let mut stream = Box::pin(schema.execute_stream(request(true)));
            register(&mut stream).await;
            let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
            if use_cas { cas_in_transaction(&mut tx, 0, 20, 200).await; }
            else { predicate_in_transaction(&mut tx, 20, 200).await; }
            if failed_commit {
                // A real deferred FK violation causes backend COMMIT to fail.
                tx.update_by_id::<TransitionChild>(&uid(999), UpdateTransitionChildInput {
                    label: Some("missing-label".into()), ..Default::default()
                }).await.unwrap().unwrap();
                assert!(tx.commit_and_emit().await.is_err());
            } else { tx.rollback().await.unwrap(); }
            expect_parents(&mut stream, &[]).await;
            if use_cas {
                assert!(matches!(VersionedTransitionChild::compare_and_swap(
                    &db, &uid(999), 0, cas_expected("initial"), cas_move(20, 200),
                ).await.unwrap(), ConditionalUpdateOutcome::Updated(_)));
                expect_parents(&mut stream, &[(1,10,100), (1,20,200)]).await;
            } else {
                assert!(matches!(CompositeTransitionChild::update_if(
                    &db, &predicate_key(1), predicate_expected(1), predicate_move(20, 200),
                ).await.unwrap(), PredicateUpdateOutcome::Updated(_)));
                expect_parent_source(&mut stream, &[(1,10,100), (1,20,200)], predicate_source_id()).await;
            }
        }
    }
}

#[tokio::test]
async fn conditional_paths_leave_historical_authority_default_deny() {
    for use_cas in [false, true] {
        let (mut db, authority) = conditional_fixture().await;
        if !use_cas { insert_predicate_source(&db).await; }
        db.set_row_policy(CurrentOnly(authority.clone(), false));
        let schema = schema(db.clone(), authority);
        let mut stream = Box::pin(schema.execute_stream(request(true)));
        register(&mut stream).await;
        let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
        if use_cas { cas_in_transaction(&mut tx, 0, 20, 200).await; }
        else { predicate_in_transaction(&mut tx, 20, 200).await; }
        tx.commit_and_emit().await.unwrap();
        expect_parent_source(&mut stream, &[(1,20,200)],
            if use_cas { uid(999).to_string() } else { predicate_source_id() }).await;
    }
}

#[tokio::test]
async fn conditional_current_source_target_field_and_assurance_revocation_remain_independent() {
    for use_cas in [false, true] {
        for denial in [CURRENT_AUTH, SOURCE_ROW, MAPPED_FIELD, CHILD_ASSURANCE,
            PARENT_ASSURANCE, OLD_PARENT_ROW, NEW_PARENT_ROW, HISTORICAL_EDGE] {
            let (db, authority) = conditional_fixture().await;
            if !use_cas { insert_predicate_source(&db).await; }
            let schema = schema(db.clone(), authority.clone());
            let mut stream = Box::pin(schema.execute_stream(request(true)));
            register(&mut stream).await;
            let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
            if use_cas { cas_in_transaction(&mut tx, 0, 20, 200).await; }
            else { predicate_in_transaction(&mut tx, 20, 200).await; }
            tx.commit_and_emit().await.unwrap();
            authority.0.store(denial, Ordering::SeqCst);
            let expected = match denial {
                OLD_PARENT_ROW | HISTORICAL_EDGE => vec![(1,20,200)],
                NEW_PARENT_ROW => vec![(1,10,100)],
                _ => Vec::new(),
            };
            let source = if use_cas { uid(999).to_string() } else { predicate_source_id() };
            expect_parent_source(&mut stream, &expected, source.clone()).await;
            authority.0.store(0, Ordering::SeqCst);
            let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
            if use_cas { cas_in_transaction(&mut tx, 1, 20, 200).await; }
            else { predicate_in_transaction(&mut tx, 20, 200).await; }
            tx.commit_and_emit().await.unwrap();
            expect_parent_source(&mut stream, &[(1,20,200)], source).await;
        }
    }
}

#[tokio::test]
async fn conditional_each_source_requires_its_own_changed_operation_scope() {
    for use_cas in [false, true] {
        let (db, authority) = conditional_fixture().await;
        if !use_cas { insert_predicate_source(&db).await; }
        let schema = schema(db.clone(), authority);
        let mut denied = Box::pin(schema.execute_stream(request(false)));
        let mut allowed = Box::pin(schema.execute_stream(request(true)));
        register(&mut denied).await;
        register(&mut allowed).await;
        let mut tx = MutationContext::begin(&db, TransactionMode::Default).await.unwrap();
        if use_cas { cas_in_transaction(&mut tx, 0, 20, 200).await; }
        else { predicate_in_transaction(&mut tx, 20, 200).await; }
        tx.commit_and_emit().await.unwrap();
        expect_parent_source(&mut denied, &[],
            if use_cas { uid(999).to_string() } else { predicate_source_id() }).await;
        expect_parent_source(&mut allowed, &[(1,10,100), (1,20,200)],
            if use_cas { uid(999).to_string() } else { predicate_source_id() }).await;
    }
}
