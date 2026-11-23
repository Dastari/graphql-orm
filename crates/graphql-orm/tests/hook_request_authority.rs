//! A mutation hook's nested writes can be authorized from the originating
//! request, a worker presents a named system access, and a call with no
//! authority stays distinguishable and deniable (#291). The propagated
//! authority is opt-in and bounded to the hook invocation's own transaction.

#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(backend = "sqlite", table = "hook_orders", plural = "HookOrders")]
struct HookOrder {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    label: String,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(backend = "sqlite", table = "hook_lines", plural = "HookLines")]
struct HookLine {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    order_id: String,
    note: String,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(backend = "sqlite", table = "hook_audits", plural = "HookAudits")]
struct HookAudit {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    note: String,
}

schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [HookOrder, HookLine, HookAudit] }

fn lines_of(order: &str) -> HookLineWhereInput {
    HookLineWhereInput {
        order_id: Some(graphql_orm::graphql::filters::StringFilter {
            eq: Some(order.into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn new_line(id: &str) -> CreateHookLineInput {
    CreateHookLineInput {
        id: id.into(),
        order_id: "o9".into(),
        note: "n".into(),
    }
}

/// What the hook observed and kept while it ran.
#[derive(Default)]
struct Observed {
    /// `MutationContext::database()` presented the request during the hook.
    public_handle_presented: Vec<bool>,
    /// The transaction's own policy handle presented it during the hook.
    policy_handle_presented: Vec<bool>,
    /// The same, inside a `with_system_access` scope.
    scope_presented: Vec<(bool, Option<String>)>,
    /// Whether a pool-level check on the public handle was admitted during
    /// the hook, from the hook itself and from a task it spawned.
    public_checks_admitted: Vec<bool>,
    /// Whether the post-commit action's handle was admitted.
    deferred_admitted: Vec<bool>,
    /// Handles the hook kept past its invocation.
    kept_public: Option<Database>,
    kept_policy: Option<Database>,
}

#[derive(Clone, Copy, PartialEq)]
enum Cleanup {
    /// Delete the order's lines with no scope of the hook's own.
    Plain,
    /// The same, and observe every handle the hook can reach.
    Probe,
    /// Keep the policy handle, then never complete.
    Hang,
}

async fn admitted(db: &Database) -> bool {
    db.ensure_entity_access(
        None,
        "HookLine",
        None,
        EntityAccessKind::Write,
        EntityAccessSurface::Repository,
    )
    .await
    .is_ok()
}

/// Deletes an order's lines before the order is deleted, and writes an audit
/// row for each deleted line from the nested hook invocation.
struct CleanupHook {
    cleanup: Cleanup,
    observed: Arc<Mutex<Observed>>,
}
impl MutationHook for CleanupHook {
    fn on_mutation<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            let presents = |db: &Database| db.policy_authority().hook_request().is_some();
            if event.entity_name == "HookLine"
                && event.action == ChangeAction::Deleted
                && event.phase == MutationPhase::After
            {
                // A nested invocation: no request of its own.
                assert!(ctx.is_none());
                hook_ctx
                    .insert::<HookAudit>(CreateHookAuditInput {
                        id: format!("audit-{}", event.id),
                        note: "line deleted".into(),
                    })
                    .await
                    .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                return Ok(());
            }
            if event.entity_name != "HookOrder"
                || event.action != ChangeAction::Deleted
                || event.phase != MutationPhase::Before
            {
                return Ok(());
            }
            let order = event.id.clone();
            if self.cleanup == Cleanup::Hang {
                self.observed.lock().unwrap().kept_policy =
                    Some(hook_ctx.__policy_database().clone());
                graphql_orm::futures::future::pending::<()>().await;
            }
            if self.cleanup == Cleanup::Probe {
                let public = hook_ctx.database().clone();
                let policy = hook_ctx.__policy_database().clone();
                let from_hook = admitted(&public).await;
                let spawned = public.clone();
                let from_task = tokio::spawn(async move { admitted(&spawned).await })
                    .await
                    .unwrap();
                let in_scope = hook_ctx
                    .with_system_access(SystemAccess::new("probe-scope"), |tx| {
                        Box::pin(async move {
                            let authority = tx.__policy_database().policy_authority();
                            (
                                authority.hook_request().is_some(),
                                authority.system().map(|access| access.name.clone()),
                            )
                        })
                    })
                    .await;
                let deferred = self.observed.clone();
                hook_ctx.defer(move |db| async move {
                    let allowed = admitted(&db).await;
                    deferred.lock().unwrap().deferred_admitted.push(allowed);
                    Ok::<(), String>(())
                });
                let mut observed = self.observed.lock().unwrap();
                observed.public_handle_presented.push(presents(&public));
                observed.policy_handle_presented.push(presents(&policy));
                observed.scope_presented.push(in_scope);
                observed.public_checks_admitted.push(from_hook);
                observed.public_checks_admitted.push(from_task);
                observed.kept_public = Some(public);
                observed.kept_policy = Some(policy);
            }
            hook_ctx
                .delete_where::<HookLine>(lines_of(&order))
                .await
                .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            Ok(())
        })
    }
}

/// One scope policy per entity. A policy that wraps another forwards the
/// authority to the inner policy's `*_with_authority` method.
struct PerEntity {
    orders: ScopeEntityPolicy,
    lines: ScopeEntityPolicy,
}
impl PerEntity {
    fn of(&self, entity: &str) -> &ScopeEntityPolicy {
        if entity == "HookOrder" {
            &self.orders
        } else {
            &self.lines
        }
    }
}
impl EntityPolicy for PerEntity {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        entity: &'static str,
        key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        self.of(entity)
            .can_access_entity(ctx, db, entity, key, kind, surface)
    }
    fn can_access_entity_with_authority<'a>(
        &'a self,
        authority: PolicyAuthority<'a>,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        entity: &'static str,
        key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        self.of(entity)
            .can_access_entity_with_authority(authority, ctx, db, entity, key, kind, surface)
    }
}

/// What each explicit-authority callback was asked under.
type Asked = Arc<Mutex<Vec<(&'static str, &'static str, String)>>>;

/// Decides on the explicit authority alone:
/// - a GraphQL request is admitted;
/// - the named system access `billing-worker` is admitted;
/// - a hook's originating request is admitted when its subject holds
///   `lines.write`;
/// - a call that presents nothing is refused.
struct Explicit(Asked);
impl Explicit {
    fn decide(
        &self,
        callback: &'static str,
        authority: PolicyAuthority<'_>,
        ctx: Option<&async_graphql::Context<'_>>,
        db: &Database,
        entity: &'static str,
    ) -> bool {
        if ctx.is_some() {
            return true;
        }
        // The authority's system access is the handle's.
        assert_eq!(
            authority.system().map(|access| &access.name),
            db.system_access().map(|access| &access.name)
        );
        let (label, allowed) = if let Some(access) = authority.system() {
            (
                format!("system={}", access.name),
                access.name == "billing-worker",
            )
        } else if let Some(request) = authority.hook_request() {
            let subject = request.subject();
            (
                format!(
                    "hook={}",
                    subject.map_or("anonymous", |subject| subject.id.as_str())
                ),
                subject.is_some_and(|subject| subject.has_scope("lines.write")),
            )
        } else {
            assert!(authority.is_none());
            ("none".to_owned(), false)
        };
        self.0.lock().unwrap().push((callback, entity, label));
        allowed
    }
    fn unreachable<T>() -> BoxFuture<'static, async_graphql::Result<T>> {
        Box::pin(async { panic!("the ORM calls the with_authority method") })
    }
}
impl EntityPolicy for Explicit {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Self::unreachable()
    }
    fn can_access_entity_with_authority<'a>(
        &'a self,
        authority: PolicyAuthority<'a>,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.decide("entity", authority, ctx, db, entity)) })
    }
}
impl RowPolicy for Explicit {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Self::unreachable()
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Self::unreachable()
    }
    fn read_visibility_with_authority<'a>(
        &'a self,
        authority: PolicyAuthority<'a>,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            self.decide("visibility", authority, ctx, db, entity);
            Ok(ReadVisibility::CallbackOnly)
        })
    }
    fn can_read_row_with_authority<'a>(
        &'a self,
        authority: PolicyAuthority<'a>,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.decide("row-read", authority, ctx, db, entity)) })
    }
    fn can_write_row_with_authority<'a>(
        &'a self,
        authority: PolicyAuthority<'a>,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.decide("row-write", authority, ctx, db, entity)) })
    }
}
impl FieldPolicy for Explicit {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
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
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_read_repository_field_with_authority<'a>(
        &'a self,
        _: PolicyAuthority<'a>,
        _: Option<AccessContext<'a>>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_repository_field_with_authority<'a>(
        &'a self,
        authority: PolicyAuthority<'a>,
        access: Option<AccessContext<'a>>,
        db: &'a Database,
        entity: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            // `access` keeps carrying the system access only.
            assert_eq!(
                access.and_then(AccessContext::system).map(|a| &a.name),
                authority.system().map(|a| &a.name)
            );
            Ok(self.decide("field-write", authority, None, db, entity))
        })
    }
}

#[derive(Clone, Copy)]
enum Policy {
    /// `ScopeEntityPolicy::new`, as in the issue.
    Scope,
    /// A scope policy per entity.
    PerEntity,
    /// The explicit-authority entity, row and field policy.
    Explicit,
}

struct Fixture {
    db: Database,
    observed: Arc<Mutex<Observed>>,
    asked: Asked,
}

async fn setup(policy: Policy, propagate: bool, cleanup: Cleanup) -> Fixture {
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    for ddl in [
        "CREATE TABLE hook_orders (id TEXT PRIMARY KEY, label TEXT NOT NULL)",
        "CREATE TABLE hook_lines (id TEXT PRIMARY KEY, order_id TEXT NOT NULL, note TEXT NOT NULL)",
        "CREATE TABLE hook_audits (id TEXT PRIMARY KEY, note TEXT NOT NULL)",
        "INSERT INTO hook_orders VALUES ('o1', 'first')",
        "INSERT INTO hook_lines VALUES ('l1', 'o1', 'a'), ('l2', 'o1', 'b'), ('l3', 'o2', 'c')",
    ] {
        graphql_orm::sqlx::query(ddl)
            .execute(db.pool())
            .await
            .unwrap();
    }
    let asked: Asked = Default::default();
    match policy {
        Policy::Scope => {
            db.set_entity_policy(ScopeEntityPolicy::new(&["shop.read"], &["shop.write"]))
        }
        Policy::PerEntity => db.set_entity_policy(PerEntity {
            orders: ScopeEntityPolicy::new(&[], &["orders.write"]),
            lines: ScopeEntityPolicy::new(&[], &["lines.write"]),
        }),
        Policy::Explicit => {
            db.set_entity_policy(Explicit(asked.clone()));
            db.set_row_policy(Explicit(asked.clone()));
            db.set_field_policy(Explicit(asked.clone()));
        }
    }
    let observed: Arc<Mutex<Observed>> = Default::default();
    db.set_mutation_hook(CleanupHook {
        cleanup,
        observed: observed.clone(),
    });
    // Off by default; the host opts in.
    assert!(!db.propagates_request_authority_to_hooks());
    let db = db.with_request_authority_propagated_to_hooks(propagate);
    Fixture {
        db,
        observed,
        asked,
    }
}

async fn count(db: &Database, table: &str) -> i64 {
    let count: (i64,) = graphql_orm::sqlx::query_as(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(db.pool())
        .await
        .unwrap();
    count.0
}

fn subject(scopes: &[&str]) -> AuthSubject {
    AuthSubject::from_parts(
        "user-1",
        Vec::new(),
        scopes.iter().map(|scope| (*scope).to_owned()).collect(),
        None,
    )
}

async fn delete_order(db: &Database, subject: AuthSubject) -> async_graphql::Response {
    schema_builder(db.clone())
        .data(subject)
        .finish()
        .execute(r#"mutation { deleteHookOrder(id: "o1") { success error } }"#)
        .await
}

fn succeeded(response: async_graphql::Response) -> bool {
    response.errors.is_empty()
        && response.data.into_json().unwrap()["deleteHookOrder"]["success"] == true
}

/// Nothing the mutation or its hooks wrote was kept.
async fn assert_untouched(db: &Database) {
    assert_eq!(count(db, "hook_orders").await, 1);
    assert_eq!(count(db, "hook_lines").await, 3);
    assert_eq!(count(db, "hook_audits").await, 0);
}

/// The order, its two lines and an audit row per line.
async fn assert_cleaned(db: &Database) {
    assert_eq!(count(db, "hook_orders").await, 0);
    assert_eq!(count(db, "hook_lines").await, 1);
    assert_eq!(count(db, "hook_audits").await, 2);
}

#[tokio::test]
async fn scope_policy_admits_hook_cleanup_from_the_callers_scope_when_opted_in() {
    let fixture = setup(Policy::Scope, true, Cleanup::Plain).await;
    let response = delete_order(&fixture.db, subject(&["shop.write"])).await;
    assert!(succeeded(response));
    // The nested hook's audit writes ran under the same request.
    assert_cleaned(&fixture.db).await;
}

#[tokio::test]
async fn scope_policy_refuses_hook_cleanup_without_the_opt_in() {
    let fixture = setup(Policy::Scope, false, Cleanup::Plain).await;
    let response = delete_order(&fixture.db, subject(&["shop.write"])).await;
    // Unchanged behaviour: the hook's write presents no subject.
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    let extensions = response.errors[0].extensions.as_ref().unwrap();
    assert_eq!(
        extensions.get("code"),
        Some(&async_graphql::Value::from("UNAUTHENTICATED"))
    );
    assert_untouched(&fixture.db).await;
}

#[tokio::test]
async fn scope_policy_refuses_hook_cleanup_the_caller_has_no_scope_for() {
    for propagate in [false, true] {
        // The caller may delete orders but holds no scope for lines.
        let fixture = setup(Policy::PerEntity, propagate, Cleanup::Plain).await;
        let response = delete_order(&fixture.db, subject(&["orders.write"])).await;
        assert!(!succeeded(response), "propagate={propagate}");
        assert_untouched(&fixture.db).await;
    }
    // With both scopes, and the opt-in, the wrapped policies admit it.
    let fixture = setup(Policy::PerEntity, true, Cleanup::Plain).await;
    let response = delete_order(&fixture.db, subject(&["orders.write", "lines.write"])).await;
    assert!(succeeded(response));
    assert_cleaned(&fixture.db).await;
    // Both scopes without the opt-in: refused as before.
    let fixture = setup(Policy::PerEntity, false, Cleanup::Plain).await;
    let response = delete_order(&fixture.db, subject(&["orders.write", "lines.write"])).await;
    assert!(!succeeded(response));
    assert_untouched(&fixture.db).await;
}

#[tokio::test]
async fn entity_row_and_field_policies_receive_the_hooks_originating_request() {
    let fixture = setup(Policy::Explicit, true, Cleanup::Plain).await;
    let response = delete_order(&fixture.db, subject(&["lines.write"])).await;
    assert!(succeeded(response));
    assert_cleaned(&fixture.db).await;
    let asked = fixture.asked.lock().unwrap();
    for (callback, entity) in [
        ("entity", "HookLine"),
        ("row-write", "HookLine"),
        // The nested hook invocation keeps the outer request.
        ("entity", "HookAudit"),
        ("row-write", "HookAudit"),
        ("field-write", "HookAudit"),
    ] {
        assert!(
            asked.iter().any(|(asked_callback, asked_entity, label)| {
                *asked_callback == callback && *asked_entity == entity && label == "hook=user-1"
            }),
            "{callback} {entity}: {asked:?}"
        );
    }
    // Every nested check presented the request: none presented nothing.
    assert!(asked.iter().all(|(_, _, label)| label == "hook=user-1"));
}

#[tokio::test]
async fn the_originating_request_is_held_to_the_policys_own_requirement() {
    // The opt-in is on, but this caller's request lacks the scope the policy
    // asks of a hook's originating request.
    let fixture = setup(Policy::Explicit, true, Cleanup::Plain).await;
    let response = delete_order(&fixture.db, subject(&["other"])).await;
    assert!(!succeeded(response));
    assert_untouched(&fixture.db).await;
    assert!(
        fixture
            .asked
            .lock()
            .unwrap()
            .iter()
            .any(|(_, entity, label)| *entity == "HookLine" && label == "hook=user-1")
    );
}

#[tokio::test]
async fn without_the_opt_in_a_hook_write_presents_no_authority() {
    let fixture = setup(Policy::Explicit, false, Cleanup::Probe).await;
    let response = delete_order(&fixture.db, subject(&["lines.write"])).await;
    assert!(!succeeded(response));
    assert_untouched(&fixture.db).await;
    let observed = fixture.observed.lock().unwrap();
    assert_eq!(observed.policy_handle_presented, [false]);
    assert_eq!(observed.public_handle_presented, [false]);
    // The policy told the call apart as one that presented nothing.
    assert!(
        fixture
            .asked
            .lock()
            .unwrap()
            .iter()
            .any(|(callback, entity, label)| *callback == "entity"
                && *entity == "HookLine"
                && label == "none")
    );
}

#[tokio::test]
async fn a_worker_presents_a_named_system_access_to_entity_and_row_policies() {
    let fixture = setup(Policy::Explicit, true, Cleanup::Plain).await;
    let db = &fixture.db;

    // No authority: refused, and distinguishable.
    let error = HookLine::insert(db, new_line("w0")).await.unwrap_err();
    assert_eq!(
        OrmPublicError::from_sqlx(&error).code,
        OrmErrorCode::Forbidden
    );
    assert_eq!(
        fixture.asked.lock().unwrap().as_slice(),
        [("entity", "HookLine", "none".to_owned())]
    );
    assert!(db.policy_authority().is_none());

    // Another name: refused.
    let other = db
        .clone()
        .with_system_access(SystemAccess::new("report-worker"));
    assert!(HookLine::insert(&other, new_line("w1")).await.is_err());

    // The name the policy admits.
    let worker = db
        .clone()
        .with_system_access(SystemAccess::new("billing-worker"));
    assert!(!worker.policy_authority().is_none());
    fixture.asked.lock().unwrap().clear();
    HookLine::insert(&worker, new_line("w2")).await.unwrap();
    worker
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.insert::<HookLine>(new_line("w3"))
                    .await
                    .map_err(OrmPublicError::from)?;
                Ok(())
            })
        })
        .await
        .unwrap();
    {
        let asked = fixture.asked.lock().unwrap();
        for callback in ["entity", "row-write", "field-write"] {
            assert!(
                asked.iter().any(|(asked_callback, _, label)| {
                    *asked_callback == callback && label == "system=billing-worker"
                }),
                "{callback}: {asked:?}"
            );
        }
        assert!(
            asked
                .iter()
                .all(|(_, _, label)| label == "system=billing-worker")
        );
    }
    assert_eq!(count(db, "hook_lines").await, 5);

    // A transaction with no authority stays refused.
    let outcome = db
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.insert::<HookLine>(new_line("w4"))
                    .await
                    .map_err(OrmPublicError::from)?;
                Ok(())
            })
        })
        .await;
    match outcome {
        Err(TransactionError::Rejected(error)) => assert_eq!(error.code, OrmErrorCode::Forbidden),
        other => panic!("expected a refusal: {other:?}"),
    }
    assert_eq!(count(db, "hook_lines").await, 5);
}

#[tokio::test]
async fn the_originating_request_is_presented_only_on_the_hooks_own_transaction() {
    let fixture = setup(Policy::Explicit, true, Cleanup::Probe).await;
    let db = &fixture.db;
    let response = delete_order(db, subject(&["lines.write"])).await;
    assert!(succeeded(response));
    assert_cleaned(db).await;

    let (kept_public, kept_policy) = {
        let observed = fixture.observed.lock().unwrap();
        // During the hook, only the transaction's own policy handle presented
        // the request; the handle the hook can pass on did not.
        assert_eq!(observed.policy_handle_presented, [true]);
        assert_eq!(observed.public_handle_presented, [false]);
        // A pool-level call on that handle, from the hook or a task it
        // spawned, presented nothing.
        assert_eq!(observed.public_checks_admitted, [false, false]);
        // A named scope replaced the request while it was open.
        assert_eq!(
            observed.scope_presented,
            [(false, Some("probe-scope".to_owned()))]
        );
        // The post-commit action's handle presented nothing.
        assert_eq!(observed.deferred_admitted, [false]);
        (
            observed.kept_public.clone().unwrap(),
            observed.kept_policy.clone().unwrap(),
        )
    };

    // After the hook, a handle it kept presents nothing: not even a clone of
    // the transaction's policy handle.
    for kept in [&kept_public, &kept_policy] {
        assert!(kept.policy_authority().is_none());
        assert!(!admitted(kept).await);
        assert!(HookLine::insert(kept, new_line("k1")).await.is_err());
    }

    // A later repository call, transaction or request starts with none.
    assert!(HookLine::insert(db, new_line("k2")).await.is_err());
    let outcome = db
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                assert!(tx.__policy_database().policy_authority().is_none());
                tx.delete_where::<HookLine>(lines_of("o2"))
                    .await
                    .map_err(OrmPublicError::from)?;
                Ok(())
            })
        })
        .await;
    assert!(matches!(outcome, Err(TransactionError::Rejected(_))));
    assert_eq!(count(db, "hook_lines").await, 1);
}

#[tokio::test]
async fn a_repository_write_gives_its_hook_no_request_authority() {
    // The opt-in is on, but this order is deleted through the repository by
    // a worker: there is no originating request to present.
    let fixture = setup(Policy::Explicit, true, Cleanup::Probe).await;
    let worker = fixture
        .db
        .clone()
        .with_system_access(SystemAccess::new("billing-worker"));
    HookOrder::delete_by_id(&worker, &"o1".to_owned())
        .await
        .unwrap();
    let observed = fixture.observed.lock().unwrap();
    assert_eq!(observed.policy_handle_presented, [false]);
    assert!(
        fixture
            .asked
            .lock()
            .unwrap()
            .iter()
            .all(|(_, _, label)| !label.starts_with("hook="))
    );
}

#[tokio::test]
async fn an_abandoned_hook_invocation_revokes_the_request() {
    let fixture = setup(Policy::Explicit, true, Cleanup::Hang).await;
    let db = fixture.db.clone();
    let request = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        delete_order(&db, subject(&["lines.write"])),
    )
    .await;
    assert!(request.is_err(), "the hook never completes");
    let kept = fixture
        .observed
        .lock()
        .unwrap()
        .kept_policy
        .clone()
        .unwrap();
    assert!(kept.policy_authority().is_none());
    assert!(!admitted(&kept).await);
}
