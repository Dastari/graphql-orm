//! A mutation hook's nested write under the agql-auth current-authority
//! bridge (#291): the originating request's accepted identity is revalidated
//! for every nested decision, and the credential's bindings must admit the
//! hook operation explicitly.

#![cfg(all(feature = "sqlite", feature = "auth-agql"))]

use agql_auth::{
    AccessTokenGrantKind, AccessTokenMetadata, AuthPrincipal, AuthResult, AuthUser, Clock,
    CurrentPrincipalResolver, FixedClock, PrincipalReference, ResolvedPrincipal, TokenStatus,
    TokenStatusChecker, TokenStatusRequest,
};
use graphql_orm::async_graphql::{Context, Request};
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use time::OffsetDateTime;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(backend = "sqlite", table = "bridge_orders", plural = "BridgeOrders")]
struct BridgeOrder {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    label: String,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(backend = "sqlite", table = "bridge_lines", plural = "BridgeLines")]
struct BridgeLine {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    order_id: String,
}

schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [BridgeOrder, BridgeLine] }

struct Host {
    principal: AuthPrincipal,
    clock: FixedClock,
    /// Whether the bindings admit a hook's nested operation.
    hook_binding_allowed: AtomicBool,
    /// Revoke the token once the hook's nested operation reaches the bindings.
    revoke_at_hook_binding: AtomicBool,
    revoked: AtomicBool,
    status_checks: AtomicUsize,
    request_bindings: AtomicUsize,
    hook_bindings: AtomicUsize,
}
#[async_trait::async_trait]
impl TokenStatusChecker for Host {
    async fn check(&self, _: TokenStatusRequest<'_>) -> AuthResult<TokenStatus> {
        self.status_checks.fetch_add(1, Ordering::SeqCst);
        Ok(if self.revoked.load(Ordering::SeqCst) {
            TokenStatus::Revoked
        } else {
            TokenStatus::Active
        })
    }
}
#[async_trait::async_trait]
impl CurrentPrincipalResolver for Host {
    async fn resolve(&self, reference: &PrincipalReference) -> AuthResult<ResolvedPrincipal> {
        ResolvedPrincipal::new(reference.clone(), self.principal.clone(), self.clock.now())
    }
}
impl AgqlBindingAuthorizer for Host {
    fn authorize<'a>(
        &'a self,
        _: &'a Context<'_>,
        _: &'a AuthPrincipal,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, AuthResult<bool>> {
        self.request_bindings.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(true) })
    }
    fn authorize_hook_operation<'a>(
        &'a self,
        principal: &'a AuthPrincipal,
        entity: &'static str,
        _: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, AuthResult<bool>> {
        Box::pin(async move {
            self.hook_bindings.fetch_add(1, Ordering::SeqCst);
            assert_eq!(principal, &self.principal);
            assert_eq!(surface, EntityAccessSurface::Repository);
            if self.revoke_at_hook_binding.load(Ordering::SeqCst) {
                self.revoked.store(true, Ordering::SeqCst);
            }
            Ok(self.hook_binding_allowed.load(Ordering::SeqCst)
                && entity == "BridgeLine"
                && kind == EntityAccessKind::Write)
        })
    }
}

/// The default bindings: [`AgqlBindingAuthorizer::authorize_hook_operation`]
/// is not implemented.
struct RequestOnlyBindings;
impl AgqlBindingAuthorizer for RequestOnlyBindings {
    fn authorize<'a>(
        &'a self,
        _: &'a Context<'_>,
        _: &'a AuthPrincipal,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, AuthResult<bool>> {
        Box::pin(async { Ok(true) })
    }
}

/// Deletes an order's lines before the order is deleted.
struct CleanupHook;
impl MutationHook for CleanupHook {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        hook_ctx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, graphql_orm::async_graphql::Result<()>> {
        Box::pin(async move {
            if event.entity_name == "BridgeOrder"
                && event.action == ChangeAction::Deleted
                && event.phase == MutationPhase::Before
            {
                hook_ctx
                    .delete_where::<BridgeLine>(BridgeLineWhereInput {
                        order_id: Some(graphql_orm::graphql::filters::StringFilter {
                            eq: Some(event.id.clone()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .await
                    .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            }
            Ok(())
        })
    }
}

fn principal() -> AuthPrincipal {
    AuthPrincipal::User(AuthUser {
        user_id: "user-1".into(),
        session_id: uuid::Uuid::from_u128(1),
        roles: Vec::new(),
        scopes: vec!["shop.write".into()],
        session: Default::default(),
        token_claims: AccessTokenMetadata {
            jti: Some("token-1".into()),
            purpose: Some("access_token".into()),
            session_family_id: Some("family-1".into()),
            session_version: Some("v1".into()),
            grant_kind: Some(AccessTokenGrantKind::UserSession),
            expires_at: Some(OffsetDateTime::from_unix_timestamp(2_000_000_100).unwrap()),
            ..Default::default()
        },
    })
}

async fn fixture(propagate: bool, host_bindings: bool) -> (Database, Arc<Host>) {
    let host = Arc::new(Host {
        principal: principal(),
        clock: FixedClock::new(OffsetDateTime::from_unix_timestamp(2_000_000_000).unwrap()),
        hook_binding_allowed: AtomicBool::new(true),
        revoke_at_hook_binding: AtomicBool::new(false),
        revoked: AtomicBool::new(false),
        status_checks: AtomicUsize::new(0),
        request_bindings: AtomicUsize::new(0),
        hook_bindings: AtomicUsize::new(0),
    });
    let bindings: Arc<dyn AgqlBindingAuthorizer> = if host_bindings {
        host.clone()
    } else {
        Arc::new(RequestOnlyBindings)
    };
    let enforcement = Arc::new(AgqlCurrentAuthorityEnforcement::new(
        host.clone(),
        host.clone(),
        Arc::new(host.clock.clone()),
        bindings,
    ));
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    for ddl in [
        "CREATE TABLE bridge_orders (id TEXT PRIMARY KEY, label TEXT NOT NULL)",
        "CREATE TABLE bridge_lines (id TEXT PRIMARY KEY, order_id TEXT NOT NULL)",
        "INSERT INTO bridge_orders VALUES ('o1', 'first')",
        "INSERT INTO bridge_lines VALUES ('l1', 'o1'), ('l2', 'o1'), ('l3', 'o2')",
    ] {
        sqlx::query(ddl).execute(&pool).await.unwrap();
    }
    let mut db = Database::with_entity_policy(
        pool,
        AgqlCurrentAuthorityPolicy::new(
            enforcement,
            ScopeEntityPolicy::new(&["shop.read"], &["shop.write"]),
        ),
    )
    .with_request_authority_propagated_to_hooks(propagate);
    db.set_mutation_hook(CleanupHook);
    (db, host)
}

async fn delete_order(db: &Database) -> bool {
    let request = AgqlAcceptedRequest::new(principal())
        .inject(Request::new(
            r#"mutation { deleteBridgeOrder(id: "o1") { success error } }"#,
        ))
        .unwrap();
    let response = schema_builder(db.clone()).finish().execute(request).await;
    response.errors.is_empty()
        && response.data.into_json().unwrap()["deleteBridgeOrder"]["success"] == true
}

async fn lines(db: &Database) -> i64 {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM bridge_lines")
        .fetch_one(db.pool())
        .await
        .unwrap();
    count.0
}

#[tokio::test]
async fn a_hook_write_is_revalidated_and_bound_on_the_originating_identity() {
    let (db, host) = fixture(true, true).await;
    assert!(delete_order(&db).await);
    assert_eq!(lines(&db).await, 1);
    // The nested decision asked the hook bindings, not the request bindings,
    // and read token status again around them and the business policy.
    assert_eq!(host.hook_bindings.load(Ordering::SeqCst), 1);
    let request_checks = {
        let (control, control_host) = fixture(true, true).await;
        sqlx::query("DELETE FROM bridge_lines")
            .execute(control.pool())
            .await
            .unwrap();
        control_host
            .hook_binding_allowed
            .store(false, Ordering::SeqCst);
        // No line matches, but the nested entity decision is still asked and
        // refused before any row is looked up.
        assert!(!delete_order(&control).await);
        control_host.status_checks.load(Ordering::SeqCst)
    };
    assert!(
        host.status_checks.load(Ordering::SeqCst) > request_checks,
        "an admitted nested decision revalidates after its bindings"
    );
}

#[tokio::test]
async fn a_hook_write_stays_refused_without_the_opt_in() {
    let (db, host) = fixture(false, true).await;
    assert!(!delete_order(&db).await);
    assert_eq!(lines(&db).await, 3);
    assert_eq!(host.hook_bindings.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_hook_write_is_refused_unless_the_bindings_admit_hook_operations() {
    // The default `authorize_hook_operation` denies.
    let (db, _) = fixture(true, false).await;
    assert!(!delete_order(&db).await);
    assert_eq!(lines(&db).await, 3);

    // A host implementation that refuses this operation.
    let (db, host) = fixture(true, true).await;
    host.hook_binding_allowed.store(false, Ordering::SeqCst);
    assert!(!delete_order(&db).await);
    assert_eq!(lines(&db).await, 3);
    assert_eq!(host.hook_bindings.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_token_revoked_during_the_hook_refuses_the_nested_write() {
    let (db, host) = fixture(true, true).await;
    host.revoke_at_hook_binding.store(true, Ordering::SeqCst);
    assert!(!delete_order(&db).await);
    assert_eq!(lines(&db).await, 3, "the mutation rolled back");
    assert!(host.revoked.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_named_system_access_is_not_an_accepted_principal() {
    let (db, _) = fixture(true, true).await;
    let worker = db.clone().with_system_access(SystemAccess::new("worker"));
    assert!(
        BridgeLine::delete_by_id(&worker, &"l1".to_owned())
            .await
            .is_err()
    );
    assert!(
        BridgeLine::delete_by_id(&db, &"l1".to_owned())
            .await
            .is_err()
    );
    assert_eq!(lines(&db).await, 3);
}
