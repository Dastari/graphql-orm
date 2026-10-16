#![cfg(all(feature = "sqlite", feature = "auth-agql"))]

use agql_auth::{
    AccessTokenGrantKind, AccessTokenMetadata, AuthError, AuthPrincipal, AuthResult, AuthUser,
    Clock, CurrentPrincipalResolver, FixedClock, PrincipalReference, ResolvedPrincipal,
    TokenStatus, TokenStatusChecker, TokenStatusRequest,
};
use graphql_orm::async_graphql::{Context, Object, Request};
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::orm::{EntityAccessKind, EntityAccessSurface};
use graphql_orm::prelude::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
};
use time::OffsetDateTime;

#[path = "current_authority/await_release.rs"]
mod await_release;

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    backend = "sqlite",
    table = "secure_projects",
    plural = "SecureProjects",
    read_policy = "records.read",
    write_policy = "records.write"
)]
struct SecureProject {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    name: String,
}
#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    graphql_orm::async_graphql::SimpleObject,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    Debug,
    PartialEq,
)]
#[graphql(complex)]
#[graphql_entity(
    backend = "sqlite",
    table = "secure_records",
    plural = "SecureRecords",
    read_policy = "records.read",
    write_policy = "records.write",
    federation_key
)]
struct SecureRecord {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    value: String,
    project_id: String,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(target = "SecureProject", from = "project_id", to = "id")]
    project: Option<SecureProject>,
}
impl BatchLoadEntity for SecureProject {
    fn batch_column() -> &'static str {
        "id"
    }
    fn batch_key_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<String, sqlx::Error> {
        use sqlx::Row;
        row.try_get("id")
    }
}
#[derive(Default)]
struct CustomQuery;
#[Object]
impl CustomQuery {
    async fn capture_owned(&self, ctx: &Context<'_>) -> graphql_orm::async_graphql::Result<bool> {
        await_release::capture_owned(ctx)
    }

    #[graphql(
        guard = "AgqlCurrentAuthorityGuard::new(\"SecureRecord\", Some(\"records.read\"), EntityAccessKind::Read, EntityAccessSurface::GraphqlQuery)"
    )]
    async fn accepted_subject(
        &self,
        ctx: &Context<'_>,
    ) -> graphql_orm::async_graphql::Result<String> {
        let accepted = AgqlAcceptedRequest::from_context(ctx)?;
        // Business authorization is separate from the immediate-status guard.
        if !accepted.subject().has_scope("records.read") {
            return Err("business policy denied".into());
        }
        Ok(accepted.principal().subject().to_owned())
    }
}
#[derive(Default)]
struct CustomMutation;
#[Object]
impl CustomMutation {
    #[graphql(
        guard = "AgqlCurrentAuthorityGuard::new(\"SecureRecord\", Some(\"records.write\"), EntityAccessKind::Write, EntityAccessSurface::GraphqlMutation)"
    )]
    async fn revoke_self(&self, ctx: &Context<'_>) -> graphql_orm::async_graphql::Result<bool> {
        let accepted = AgqlAcceptedRequest::from_context(ctx)?;
        if !accepted.subject().has_scope("records.write") {
            return Err("business policy denied".into());
        }
        ctx.data_unchecked::<Arc<Host>>()
            .revoked
            .store(true, Ordering::SeqCst);
        Ok(true)
    }
}
schema_roots! { auth: "optional", query_custom_ops: [], entities: [SecureRecord, SecureProject], extra_query_types: [CustomQuery], extra_mutation_types: [CustomMutation] }

struct Host {
    current: Mutex<AuthPrincipal>,
    live_users: Mutex<Option<Arc<LoginStore>>>,
    clock: FixedClock,
    revoked: AtomicBool,
    unavailable: AtomicBool,
    resolver_unavailable: AtomicBool,
    binding_error: AtomicBool,
    disabled: AtomicBool,
    binding_allowed: AtomicBool,
    revoke_after_root: AtomicBool,
    checks: AtomicUsize,
    checker_advance: AtomicI64,
    resolver_advance: AtomicI64,
    binding_advance: AtomicI64,
    business_advance: AtomicI64,
    binding_pause: Mutex<Option<Arc<await_release::Pause>>>,
    business_pause: Mutex<Option<Arc<await_release::Pause>>>,
    business_calls: AtomicUsize,
    current_owner: Mutex<Option<String>>,
}
#[async_trait::async_trait]
impl TokenStatusChecker for Host {
    async fn check(&self, _: TokenStatusRequest<'_>) -> AuthResult<TokenStatus> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        self.clock
            .advance_seconds(self.checker_advance.swap(0, Ordering::SeqCst));
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(AuthError::AuthServiceUnavailable);
        }
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
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(AuthError::AuthServiceUnavailable);
        }
        if self.resolver_unavailable.load(Ordering::SeqCst) {
            return Err(AuthError::Store("private resolver details".into()));
        }
        if self.disabled.load(Ordering::SeqCst) {
            return Err(AuthError::UserDisabled);
        }
        let mut principal = self.current.lock().unwrap().clone();
        let live_users = self.live_users.lock().unwrap().clone();
        if let Some(store) = live_users {
            let user = agql_auth::UserStore::find_user_by_id(store.as_ref(), &reference.subject)
                .await?
                .ok_or(AuthError::InvalidCredentials)?;
            if user.disabled {
                return Err(AuthError::UserDisabled);
            }
            let AuthPrincipal::User(current) = &mut principal else {
                return Err(AuthError::Forbidden);
            };
            current.roles = user.roles;
            current.scopes = user.scopes;
        }
        self.clock
            .advance_seconds(self.resolver_advance.swap(0, Ordering::SeqCst));
        ResolvedPrincipal::new(reference.clone(), principal, self.clock.now())
    }
}
impl AgqlBindingAuthorizer for Host {
    fn authorize<'a>(
        &'a self,
        ctx: &'a Context<'_>,
        principal: &'a AuthPrincipal,
        entity: &'static str,
        _: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, AuthResult<bool>> {
        Box::pin(async move {
            await_release::pause(&self.binding_pause).await;
            self.clock
                .advance_seconds(self.binding_advance.swap(0, Ordering::SeqCst));
            if self.binding_error.load(Ordering::SeqCst) {
                return Err(AuthError::Store("private binding details".into()));
            }
            let AuthPrincipal::User(user) = principal else {
                let AuthPrincipal::ApiToken(token) = principal else {
                    unreachable!()
                };
                return Ok(self.binding_allowed.load(Ordering::SeqCst)
                    && ctx.data_opt::<TransportAccepted>().is_some()
                    && token.audience.as_deref() == Some("test-api")
                    && token.principal_kind.as_str() == "service"
                    && token.resource_type.is_none()
                    && token.resource_id.is_none());
            };
            if user.token_claims.purpose.as_deref() != Some("access_token")
                || !self.binding_allowed.load(Ordering::SeqCst)
            {
                return Ok(false);
            }
            // This host does not support sender/resource/operation-bound tokens on
            // its generic entity routes. Unsupported restrictions must deny.
            if user.token_claims.cnf.is_some()
                || user.token_claims.operation.is_some()
                || user.token_claims.resource_id.is_some()
                || user.token_claims.resource_type.is_some()
            {
                return Ok(false);
            }
            if entity == "SecureRecord"
                && matches!(surface, EntityAccessSurface::GraphqlQuery)
                && self.revoke_after_root.swap(false, Ordering::SeqCst)
            {
                self.revoked.store(true, Ordering::SeqCst);
            }
            Ok(ctx.data_opt::<TransportAccepted>().is_some()
                && matches!(entity, "SecureRecord" | "SecureProject")
                && matches!(kind, EntityAccessKind::Read | EntityAccessKind::Write)
                && matches!(
                    surface,
                    EntityAccessSurface::GraphqlQuery
                        | EntityAccessSurface::GraphqlSubscription
                        | EntityAccessSurface::GraphqlMutation
                        | EntityAccessSurface::GraphqlRelation
                        | EntityAccessSurface::Repository
                ))
        })
    }
}
struct TransportAccepted;
struct Business(Arc<Host>);
impl EntityPolicy for Business {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a Context<'_>>,
        db: &'a Database,
        entity: &'static str,
        policy: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async move {
            self.0.business_calls.fetch_add(1, Ordering::SeqCst);
            await_release::pause(&self.0.business_pause).await;
            let result = ScopeEntityPolicy::new(&["records.read"], &["records.write"])
                .can_access_entity(ctx, db, entity, policy, kind, surface)
                .await;
            self.0
                .clock
                .advance_seconds(self.0.business_advance.swap(0, Ordering::SeqCst));
            result
        })
    }
}
fn principal() -> AuthPrincipal {
    AuthPrincipal::User(AuthUser {
        user_id: "user-1".into(),
        session_id: uuid::Uuid::from_u128(1),
        roles: vec!["reader".into()],
        scopes: vec!["records.read".into(), "records.write".into()],
        session: Default::default(),
        token_claims: AccessTokenMetadata {
            authorization_roles: vec!["catalog.reader".into()],
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
async fn fixture() -> (AppSchema, Arc<Host>, AgqlAcceptedRequest) {
    fixture_for(principal(), None, false).await
}
async fn fixture_for(
    principal: AuthPrincipal,
    schema_subject: Option<AuthSubject>,
    schema_identity: bool,
) -> (AppSchema, Arc<Host>, AgqlAcceptedRequest) {
    fixture_config(principal, schema_subject, schema_identity, None).await
}
async fn fixture_config(
    principal: AuthPrincipal,
    schema_subject: Option<AuthSubject>,
    schema_identity: bool,
    deleted_grant: Option<bool>,
) -> (AppSchema, Arc<Host>, AgqlAcceptedRequest) {
    let host = Arc::new(Host {
        current: Mutex::new(principal.clone()),
        live_users: Mutex::new(None),
        clock: FixedClock::new(OffsetDateTime::from_unix_timestamp(2_000_000_000).unwrap()),
        revoked: AtomicBool::new(false),
        unavailable: AtomicBool::new(false),
        resolver_unavailable: AtomicBool::new(false),
        binding_error: AtomicBool::new(false),
        disabled: AtomicBool::new(false),
        binding_allowed: AtomicBool::new(true),
        revoke_after_root: AtomicBool::new(false),
        checks: AtomicUsize::new(0),
        checker_advance: AtomicI64::new(0),
        resolver_advance: AtomicI64::new(0),
        binding_advance: AtomicI64::new(0),
        business_advance: AtomicI64::new(0),
        binding_pause: Mutex::new(None),
        business_pause: Mutex::new(None),
        business_calls: AtomicUsize::new(0),
        current_owner: Mutex::new(Some("user-1".into())),
    });
    let enforcement = Arc::new(AgqlCurrentAuthorityEnforcement::new(
        host.clone(),
        host.clone(),
        Arc::new(host.clock.clone()),
        host.clone(),
    ));
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE secure_records (id TEXT PRIMARY KEY, value TEXT NOT NULL, project_id TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO secure_records VALUES ('record-1', 'safe', 'project-1')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE secure_projects (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO secure_projects VALUES ('project-1', 'project')")
        .execute(&pool)
        .await
        .unwrap();
    let mut db = Database::with_entity_policy(
        pool,
        AgqlCurrentAuthorityPolicy::new(enforcement.clone(), Business(host.clone())),
    )
    .with_authorization_mode(AuthorizationMode::ExplicitPolicyForAllExposedOperations);
    if let Some(allowed) = deleted_grant {
        db.set_row_policy(DeletedRows { allowed });
    }
    let mut builder = schema_builder(db)
        .data(enforcement)
        .data(host.clone())
        .data(await_release::Captured::default());
    if let Some(subject) = schema_subject {
        builder = builder.data(subject);
    }
    if schema_identity {
        let bundle = AgqlAcceptedRequest::new(principal.clone());
        let AuthPrincipal::User(user) = principal.clone() else {
            unreachable!()
        };
        builder = builder
            .data(bundle.clone())
            .data(principal.clone())
            .data(user)
            .data(bundle.subject().clone())
            .data(bundle.db_context().clone());
    }
    let schema = builder.finish();
    (schema, host, AgqlAcceptedRequest::new(principal))
}
fn request(bundle: &AgqlAcceptedRequest, query: &str) -> Request {
    bundle
        .clone()
        .inject(Request::new(query).data(TransportAccepted))
        .unwrap()
}
const READ: &str = "{ secureRecords { edges { node { id value } } } }";

#[tokio::test]
async fn accepted_request_works_and_each_alias_revalidates() {
    let (schema, host, bundle) = fixture().await;
    let response = schema.execute(request(&bundle, "{ a: secureRecords { edges { node { id } } } b: secureRecords { edges { node { id } } } }")).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(host.checks.load(Ordering::SeqCst), 6);
    assert_eq!(bundle.principal(), &principal());
}
#[tokio::test]
async fn missing_and_conflicting_request_copies_deny() {
    let (schema, _, bundle) = fixture().await;
    let mut cases = vec![
        Request::new(READ),
        Request::new(READ).data(bundle.clone()),
        request(&bundle, READ).data(AuthSubject::new("other")),
        request(&bundle, READ).data(AgqlAcceptedRequest::new({
            let mut p = principal();
            if let AuthPrincipal::User(u) = &mut p {
                u.user_id = "other".into();
            }
            p
        })),
    ];
    for candidate in cases.drain(..) {
        assert!(!schema.execute(candidate).await.errors.is_empty());
    }
    assert!(
        schema
            .execute(request(&bundle, READ))
            .await
            .errors
            .is_empty()
    );
}
#[tokio::test]
async fn revoked_unavailable_and_current_grant_narrowing_deny() {
    for mode in 0..6 {
        let (schema, host, bundle) = fixture().await;
        match mode {
            0 => host.revoked.store(true, Ordering::SeqCst),
            1 => host.unavailable.store(true, Ordering::SeqCst),
            _ => {
                let mut current = host.current.lock().unwrap();
                let AuthPrincipal::User(user) = &mut *current else {
                    unreachable!()
                };
                match mode {
                    2 => user.scopes.clear(),
                    3 => user.roles.clear(),
                    4 => user.token_claims.authorization_roles.clear(),
                    _ => user.user_id = "deleted-or-replaced".into(),
                }
            }
        }
        assert!(
            !schema
                .execute(request(&bundle, READ))
                .await
                .errors
                .is_empty(),
            "mode {mode}"
        );
    }
}
#[tokio::test]
async fn expiry_during_each_async_stage_denies() {
    for stage in 0..4 {
        let (schema, host, bundle) = fixture().await;
        [
            &host.checker_advance,
            &host.resolver_advance,
            &host.binding_advance,
            &host.business_advance,
        ][stage]
            .store(101, Ordering::SeqCst);
        assert!(
            !schema
                .execute(request(&bundle, READ))
                .await
                .errors
                .is_empty(),
            "stage {stage}"
        );
    }
}
#[tokio::test]
async fn sequential_mutation_observes_same_request_revocation() {
    let (schema, host, bundle) = fixture().await;
    let response = schema
        .execute(request(
            &bundle,
            "mutation { first: revokeSelf second: revokeSelf }",
        ))
        .await;
    assert!(host.revoked.load(Ordering::SeqCst));
    assert!(!response.errors.is_empty(), "{:?}", response.data);
    assert_eq!(host.checks.load(Ordering::SeqCst), 3);
}
#[tokio::test]
async fn binding_evidence_and_host_authorization_are_mandatory() {
    let (schema, host, bundle) = fixture().await;
    assert!(
        !schema
            .execute(bundle.clone().inject(Request::new(READ)).unwrap())
            .await
            .errors
            .is_empty()
    );
    host.binding_allowed.store(false, Ordering::SeqCst);
    assert!(
        !schema
            .execute(request(&bundle, READ))
            .await
            .errors
            .is_empty()
    );
    host.binding_allowed.store(true, Ordering::SeqCst);
    assert!(
        schema
            .execute(request(&bundle, "{ acceptedSubject }"))
            .await
            .errors
            .is_empty()
    );
}

fn api_principal() -> agql_auth::ApiTokenPrincipal {
    agql_auth::ApiTokenPrincipal {
        token_id: uuid::Uuid::from_u128(2),
        subject: "service-1".into(),
        principal_kind: "service".into(),
        scopes: vec!["records.read".into()],
        audience: Some("test-api".into()),
        resource_type: None,
        resource_id: None,
        expires_at: OffsetDateTime::from_unix_timestamp(2_000_000_100).unwrap(),
    }
}
#[tokio::test]
async fn every_required_copy_and_opposite_actor_are_checked() {
    let (schema, _, bundle) = fixture().await;
    for missing in 0..5 {
        let mut data = graphql_orm::async_graphql::Data::default();
        if missing != 0 {
            data.insert(bundle.principal().clone());
        }
        if missing != 1 {
            let AuthPrincipal::User(user) = bundle.principal() else {
                unreachable!()
            };
            data.insert(user.clone());
        }
        if missing != 2 {
            data.insert(bundle.subject().clone());
        }
        if missing != 3 {
            data.insert(bundle.db_context().clone());
        }
        if missing != 4 {
            data.insert(bundle.clone());
        }
        data.insert(TransportAccepted);
        let mut candidate = Request::new(READ);
        candidate.data = data;
        assert!(
            !schema.execute(candidate).await.errors.is_empty(),
            "missing copy {missing}"
        );
    }
    assert!(
        !schema
            .execute(request(&bundle, READ).data(api_principal()))
            .await
            .errors
            .is_empty()
    );
    let mut changed = principal();
    if let AuthPrincipal::User(user) = &mut changed {
        user.scopes.clear();
    }
    assert!(
        !schema
            .execute(request(&bundle, READ).data(changed))
            .await
            .errors
            .is_empty()
    );
    assert!(
        !schema
            .execute(request(&bundle, READ).data(DbAuthContext::default()))
            .await
            .errors
            .is_empty()
    );
}
#[tokio::test]
async fn shadowed_schema_identity_and_schema_only_bundle_deny() {
    let (schema, _, bundle) =
        fixture_for(principal(), Some(AuthSubject::new("other")), false).await;
    assert!(
        !schema
            .execute(request(&bundle, READ))
            .await
            .errors
            .is_empty()
    );
    let (schema, _, _) = fixture_for(principal(), None, true).await;
    assert!(
        !schema
            .execute(Request::new(READ).data(TransportAccepted))
            .await
            .errors
            .is_empty()
    );
}
#[tokio::test]
async fn unsupported_binding_restrictions_fail_closed() {
    for binding in 0..5 {
        let mut accepted = principal();
        let AuthPrincipal::User(user) = &mut accepted else {
            unreachable!()
        };
        match binding {
            0 => user.token_claims.purpose = Some("password_reset".into()),
            1 => user.token_claims.resource_id = Some("record-1".into()),
            2 => user.token_claims.resource_type = Some("SecureRecord".into()),
            3 => user.token_claims.cnf = Some(serde_json::from_value(serde_json::json!({"jkt":"sender-key"})).unwrap()),
            _ => user.token_claims.operation = Some(serde_json::from_value(serde_json::json!({"operation_name":"Allowed", "document_sha256":"a".repeat(64)})).unwrap()),
        }
        let (schema, _, bundle) = fixture_for(accepted, None, false).await;
        assert!(
            !schema
                .execute(request(&bundle, READ))
                .await
                .errors
                .is_empty(),
            "binding {binding}"
        );
    }
}
#[tokio::test]
async fn federation_rechecks_current_authority() {
    // Federation checks declared row policies even when collections retain
    // entity-only visibility. Keep deleted-preimage release denied here.
    let (schema, host, bundle) = fixture_config(principal(), None, false, Some(false)).await;
    let query = "{ _entities(representations: [{__typename: \"SecureRecord\", id: \"record-1\"}]) { ... on SecureRecord { id value } } }";
    let accepted = schema.execute(request(&bundle, query)).await;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    let data = accepted.data.into_json().unwrap();
    assert_eq!(data["_entities"][0]["id"], "record-1");
    assert_eq!(data["_entities"][0]["value"], "safe");
    assert_eq!(host.checks.load(Ordering::SeqCst), 3);
    host.revoked.store(true, Ordering::SeqCst);
    assert!(
        !schema
            .execute(request(&bundle, query))
            .await
            .errors
            .is_empty()
    );
}
#[tokio::test]
async fn ctx_none_repository_access_cannot_bypass_wrapper() {
    let (_, host, _) = fixture().await;
    let enforcement = Arc::new(AgqlCurrentAuthorityEnforcement::new(
        host.clone(),
        host.clone(),
        Arc::new(host.clock.clone()),
        host.clone(),
    ));
    let db = Database::with_entity_policy(
        sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap(),
        AgqlCurrentAuthorityPolicy::new(enforcement, Business(host)),
    );
    assert!(
        db.ensure_entity_access(
            None,
            "SecureRecord",
            Some("records.read"),
            EntityAccessKind::Read,
            EntityAccessSurface::Repository
        )
        .await
        .is_err()
    );
}
#[tokio::test]
async fn concurrent_request_copies_remain_immutable() {
    let (schema, host, bundle) = fixture().await;
    let (a, b) = tokio::join!(
        schema.execute(request(&bundle, "{ acceptedSubject }")),
        schema.execute(request(&bundle, READ))
    );
    assert!(a.errors.is_empty(), "{:?}", a.errors);
    assert!(b.errors.is_empty(), "{:?}", b.errors);
    assert_eq!(host.checks.load(Ordering::SeqCst), 5);
    assert_eq!(bundle.principal(), &principal());
}

#[test]
fn injection_rejects_existing_conflicts_without_mutating_data() {
    let bundle = AgqlAcceptedRequest::new(principal());
    assert!(
        bundle
            .clone()
            .inject(Request::new(READ).data(AuthSubject::new("other")))
            .is_err()
    );
    assert!(
        bundle
            .clone()
            .inject(Request::new(READ).data(api_principal()))
            .is_err()
    );
    let mut data = graphql_orm::async_graphql::Data::default();
    data.insert(AuthSubject::new("other"));
    assert!(bundle.insert_into_data(&mut data).is_err());
    assert!(!data.contains_key(&std::any::TypeId::of::<AuthPrincipal>()));
}

#[tokio::test]
async fn relationship_checks_current_authority_after_root_admission() {
    let (schema, host, bundle) = fixture().await;
    let query = "{ secureRecords { edges { node { id project { id name } } } } }";
    let allowed = schema.execute(request(&bundle, query)).await;
    assert!(allowed.errors.is_empty(), "{:?}", allowed.errors);
    assert!(host.checks.load(Ordering::SeqCst) >= 2);
    host.revoke_after_root.store(true, Ordering::SeqCst);
    let denied = schema.execute(request(&bundle, query)).await;
    assert!(!denied.errors.is_empty(), "{:?}", denied.data);
}

#[tokio::test]
async fn resolver_binding_errors_and_disabled_user_fail_closed_without_details() {
    for failure in 0..3 {
        let (schema, host, bundle) = fixture().await;
        [
            &host.resolver_unavailable,
            &host.binding_error,
            &host.disabled,
        ][failure]
            .store(true, Ordering::SeqCst);
        let response = schema.execute(request(&bundle, READ)).await;
        assert!(!response.errors.is_empty());
        assert!(!format!("{:?}", response.errors).contains("private"));
    }
}
#[tokio::test]
async fn delegated_parent_expiry_is_checked_after_async_resolution() {
    let mut delegation = principal();
    let AuthPrincipal::User(user) = &mut delegation else {
        unreachable!()
    };
    user.token_claims.grant_kind = Some(AccessTokenGrantKind::SessionBoundDelegation);
    let (schema, host, bundle) = fixture_for(delegation, None, false).await;
    let mut parent = principal();
    let AuthPrincipal::User(user) = &mut parent else {
        unreachable!()
    };
    user.token_claims.jti = Some("parent-token".into());
    user.token_claims.expires_at =
        Some(OffsetDateTime::from_unix_timestamp(2_000_000_020).unwrap());
    *host.current.lock().unwrap() = parent;
    assert!(
        schema
            .execute(request(&bundle, READ))
            .await
            .errors
            .is_empty()
    );
    host.resolver_advance.store(21, Ordering::SeqCst);
    assert!(
        !schema
            .execute(request(&bundle, READ))
            .await
            .errors
            .is_empty()
    );
}
#[tokio::test]
async fn api_principal_is_preserved_and_audience_binding_is_enforced() {
    let api = api_principal();
    let (schema, host, bundle) =
        fixture_for(AuthPrincipal::ApiToken(api.clone()), None, false).await;
    let response = schema.execute(request(&bundle, READ)).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let AuthPrincipal::User(user) = principal() else {
        unreachable!()
    };
    assert!(
        !schema
            .execute(request(&bundle, READ).data(user))
            .await
            .errors
            .is_empty()
    );
    host.revoked.store(true, Ordering::SeqCst);
    assert!(
        !schema
            .execute(request(&bundle, READ))
            .await
            .errors
            .is_empty()
    );
    let mut wrong_audience = api;
    wrong_audience.audience = Some("other-api".into());
    let (schema, _, bundle) =
        fixture_for(AuthPrincipal::ApiToken(wrong_audience), None, false).await;
    assert!(
        !schema
            .execute(request(&bundle, READ))
            .await
            .errors
            .is_empty()
    );
}

#[tokio::test]
async fn newly_added_current_grants_never_widen_accepted_request() {
    let (schema, host, bundle) = fixture().await;
    {
        let mut current = host.current.lock().unwrap();
        let AuthPrincipal::User(user) = &mut *current else {
            unreachable!()
        };
        user.scopes.push("admin.write".into());
        user.roles.push("admin".into());
        user.token_claims
            .authorization_roles
            .push("catalog.admin".into());
    }
    assert!(
        schema
            .execute(request(&bundle, READ))
            .await
            .errors
            .is_empty()
    );
    assert_eq!(bundle.principal(), &principal());
    assert!(!bundle.subject().has_scope("admin.write"));
}
#[tokio::test]
async fn custom_guard_does_not_replace_business_authorization_or_configuration() {
    let mut accepted = principal();
    let AuthPrincipal::User(user) = &mut accepted else {
        unreachable!()
    };
    user.scopes.clear();
    let (schema, _, bundle) = fixture_for(accepted, None, false).await;
    let denied = schema
        .execute(request(&bundle, "{ acceptedSubject }"))
        .await;
    assert!(!denied.errors.is_empty());
    let schema = graphql_orm::async_graphql::Schema::build(
        CustomQuery,
        graphql_orm::async_graphql::EmptyMutation,
        graphql_orm::async_graphql::EmptySubscription,
    )
    .finish();
    let response = schema
        .execute(request(&bundle, "{ acceptedSubject }"))
        .await;
    assert_eq!(response.errors.len(), 1);
    assert_eq!(response.errors[0].message, "authorization is misconfigured");
}
#[tokio::test]
async fn malformed_or_expired_accepted_references_deny() {
    for malformed in 0..3 {
        let mut accepted = principal();
        let AuthPrincipal::User(user) = &mut accepted else {
            unreachable!()
        };
        match malformed {
            0 => user.token_claims.jti = None,
            1 => user.token_claims.expires_at = None,
            _ => {
                user.token_claims.expires_at =
                    Some(OffsetDateTime::from_unix_timestamp(2_000_000_000).unwrap())
            }
        }
        let (schema, _, bundle) = fixture_for(accepted, None, false).await;
        assert!(
            !schema
                .execute(request(&bundle, READ))
                .await
                .errors
                .is_empty(),
            "malformed {malformed}"
        );
    }
}

// These exercise the S007 per-event hook with the real current-authority
// adapter. Generate witnessed events through mutations rather than raw senders.
#[tokio::test]
async fn subscription_event_revalidates_status_and_current_grants() {
    use graphql_orm::futures::{StreamExt, future::poll_fn};
    use std::task::Poll;
    use tokio::time::{Duration, timeout};
    for revoke in [true, false] {
        let (schema, host, bundle) = fixture().await;
        let mut stream = Box::pin(schema.execute_stream(request(
            &bundle,
            "subscription { secureRecordChanged { id secureRecord { value } } }",
        )));
        poll_fn(|cx| match stream.as_mut().poll_next_unpin(cx) {
            Poll::Pending => Poll::Ready(()),
            other => panic!("subscription did not wait for an event: {other:?}"),
        })
        .await;
        let admitted_checks = host.checks.load(Ordering::SeqCst);
        assert!(admitted_checks > 0);
        let control = schema.execute(request(&bundle,
            "mutation { createSecureRecord(input: {value: \"allowed\", projectId: \"project-1\"}) { success } }")).await;
        assert!(control.errors.is_empty(), "{:?}", control.errors);
        assert_eq!(
            control.data.into_json().unwrap()["createSecureRecord"]["success"],
            true
        );
        let released = timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert!(released.errors.is_empty(), "{:?}", released.errors);
        assert_eq!(
            released.data.into_json().unwrap()["secureRecordChanged"]["secureRecord"]["value"],
            "allowed"
        );
        let published = schema.execute(request(&bundle,
            "mutation { createSecureRecord(input: {value: \"queued\", projectId: \"project-1\"}) { success } }")).await;
        assert!(published.errors.is_empty(), "{:?}", published.errors);
        assert_eq!(
            published.data.into_json().unwrap()["createSecureRecord"]["success"],
            true
        );
        let before_release = host.checks.load(Ordering::SeqCst);
        if revoke {
            host.revoked.store(true, Ordering::SeqCst);
        } else {
            let mut current = host.current.lock().unwrap();
            let AuthPrincipal::User(user) = &mut *current else {
                unreachable!()
            };
            user.scopes.clear();
        }
        assert!(
            timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err(),
            "event admitted before authority changed escaped the per-event check"
        );
        assert!(
            host.checks.load(Ordering::SeqCst) > before_release,
            "subscription reused its admission status decision"
        );
    }
}

#[tokio::test]
async fn subscription_event_rechecks_expiry_after_async_binding() {
    use graphql_orm::futures::{StreamExt, future::poll_fn};
    use std::task::Poll;
    use tokio::time::{Duration, timeout};
    let (schema, host, bundle) = fixture().await;
    let mut stream = Box::pin(schema.execute_stream(request(
        &bundle,
        "subscription { secureRecordChanged { id secureRecord { value } } }",
    )));
    poll_fn(|cx| match stream.as_mut().poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        other => panic!("subscription did not wait for an event: {other:?}"),
    })
    .await;
    let published = schema.execute(request(&bundle,
        "mutation { createSecureRecord(input: {value: \"queued\", projectId: \"project-1\"}) { success } }")).await;
    assert!(published.errors.is_empty(), "{:?}", published.errors);
    assert_eq!(
        published.data.into_json().unwrap()["createSecureRecord"]["success"],
        true
    );
    let before_release = host.checks.load(Ordering::SeqCst);
    host.binding_advance.store(101, Ordering::SeqCst);
    assert!(
        timeout(Duration::from_millis(100), stream.next())
            .await
            .is_err(),
        "credential expired during event binding checks but an event escaped"
    );
    assert!(host.checks.load(Ordering::SeqCst) > before_release);
}

// Explicit deleted-preimage ownership is separate from ordinary row visibility.
struct DeletedRows {
    allowed: bool,
}
impl RowPolicy for DeletedRows {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, graphql_orm::async_graphql::Result<ReadVisibility>> {
        Box::pin(async { Ok(ReadVisibility::Unrestricted) })
    }
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(ctx
                .and_then(|ctx| ctx.data_opt::<AuthSubject>())
                .is_some_and(|user| user.id == "user-1")
                && row
                    .downcast_ref::<SecureRecord>()
                    .is_some_and(|record| record.project_id == "project-1"))
        })
    }
    fn can_read_deleted_preimage<'a>(
        &'a self,
        ctx: Option<&'a Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        Box::pin(async move {
            // The fixture's trusted ownership map grants user-1 this one record.
            // Unrestricted/ordinary reads alone deliberately do not grant it.
            Ok(self.allowed
                && entity == "SecureRecord"
                && surface == EntityAccessSurface::GraphqlSubscription
                && ctx
                    .and_then(|ctx| ctx.data_opt::<AuthSubject>())
                    .is_some_and(|user| user.id == "user-1")
                && row.downcast_ref::<SecureRecord>().is_some_and(|record| {
                    record.id == "record-1" && record.project_id == "project-1"
                }))
        })
    }
    fn can_write_row<'a>(
        &'a self,
        ctx: Option<&'a Context<'_>>,
        db: &'a Database,
        entity: &'static str,
        key: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, graphql_orm::async_graphql::Result<bool>> {
        self.can_read_row(ctx, db, entity, key, surface, row)
    }
}

#[tokio::test]
async fn deleted_preimage_opt_in_still_requires_immediate_current_authority() {
    use graphql_orm::futures::{StreamExt, future::poll_fn};
    use std::task::Poll;
    use tokio::time::{Duration, timeout};
    for mode in 0..5 {
        let (schema, host, bundle) =
            fixture_config(principal(), None, false, (mode != 4).then_some(mode != 3)).await;
        let mut stream = Box::pin(schema.execute_stream(request(
            &bundle,
            "subscription { secureRecordChanged { id secureRecord { value } } }",
        )));
        poll_fn(|cx| match stream.as_mut().poll_next_unpin(cx) {
            Poll::Pending => Poll::Ready(()),
            other => panic!("subscription did not wait for an event: {other:?}"),
        })
        .await;
        let deleted = schema
            .execute(request(
                &bundle,
                "mutation { deleteSecureRecord(id: \"record-1\") { success } }",
            ))
            .await;
        if mode == 4 {
            // The strict database also requires a row-write provider for this
            // marked mutation. Provider absence must deny before deleting or
            // emitting; the ordinary-read-only mode above exercises a real
            // delete whose event is suppressed by the missing preimage grant.
            assert_eq!(deleted.errors.len(), 1);
            assert_eq!(deleted.errors[0].message, "forbidden");
            let remaining: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM secure_records WHERE id = 'record-1'")
                    .fetch_one(schema.data::<Database>().unwrap().pool())
                    .await
                    .unwrap();
            assert_eq!(remaining, 1, "missing provider must deny before deleting");
            assert!(
                timeout(Duration::from_millis(100), stream.next())
                    .await
                    .is_err()
            );
            continue;
        }
        assert!(
            deleted.errors.is_empty(),
            "mode {mode}: {:?}",
            deleted.errors
        );
        assert_eq!(
            deleted.data.into_json().unwrap()["deleteSecureRecord"]["success"],
            true
        );
        let before_release = host.checks.load(Ordering::SeqCst);
        match mode {
            1 => host.revoked.store(true, Ordering::SeqCst),
            2 => {
                let mut current = host.current.lock().unwrap();
                let AuthPrincipal::User(user) = &mut *current else {
                    unreachable!()
                };
                user.scopes.clear();
            }
            _ => {}
        }
        if mode == 0 {
            let event = timeout(Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap();
            assert!(event.errors.is_empty(), "{:?}", event.errors);
            let data = event.data.into_json().unwrap();
            assert_eq!(data["secureRecordChanged"]["id"], "record-1");
            assert_eq!(data["secureRecordChanged"]["secureRecord"]["value"], "safe");
        } else {
            assert!(
                timeout(Duration::from_millis(100), stream.next())
                    .await
                    .is_err(),
                "deleted preimage escaped authority/default-deny check in mode {mode}"
            );
        }
        assert!(host.checks.load(Ordering::SeqCst) > before_release);
    }
}

#[derive(Default)]
struct LoginStore {
    user: Mutex<Option<agql_auth::StoredUser>>,
    refresh: Mutex<Vec<agql_auth::StoredRefreshToken>>,
}
#[async_trait::async_trait]
impl agql_auth::UserStore for LoginStore {
    async fn find_user_by_principal(
        &self,
        principal: &str,
    ) -> AuthResult<Option<agql_auth::StoredUser>> {
        Ok(self
            .user
            .lock()
            .unwrap()
            .as_ref()
            .filter(|user| user.principal == principal)
            .cloned())
    }
    async fn find_user_by_id(&self, id: &str) -> AuthResult<Option<agql_auth::StoredUser>> {
        Ok(self
            .user
            .lock()
            .unwrap()
            .as_ref()
            .filter(|user| user.id == id)
            .cloned())
    }
}
#[async_trait::async_trait]
impl agql_auth::RefreshTokenStore for LoginStore {
    async fn insert_refresh_token(&self, record: agql_auth::StoredRefreshToken) -> AuthResult<()> {
        self.refresh.lock().unwrap().push(record);
        Ok(())
    }
    async fn find_refresh_token_by_hash(
        &self,
        _: &str,
    ) -> AuthResult<Option<agql_auth::StoredRefreshToken>> {
        Err(AuthError::AuthServiceUnavailable)
    }
    async fn revoke_refresh_token(
        &self,
        _: uuid::Uuid,
        _: OffsetDateTime,
        _: Option<uuid::Uuid>,
        _: agql_auth::RefreshTokenRevocationReason,
    ) -> AuthResult<()> {
        Err(AuthError::AuthServiceUnavailable)
    }
    async fn revoke_refresh_token_family(
        &self,
        _: uuid::Uuid,
        _: OffsetDateTime,
        _: agql_auth::RefreshTokenRevocationReason,
    ) -> AuthResult<()> {
        Err(AuthError::AuthServiceUnavailable)
    }
    async fn touch_refresh_token(
        &self,
        _: uuid::Uuid,
        _: OffsetDateTime,
        _: Option<String>,
        _: Option<String>,
    ) -> AuthResult<()> {
        Err(AuthError::AuthServiceUnavailable)
    }
    async fn rotate_refresh_token(
        &self,
        _: uuid::Uuid,
        _: agql_auth::StoredRefreshToken,
        _: OffsetDateTime,
        _: Option<String>,
        _: Option<String>,
    ) -> AuthResult<bool> {
        Err(AuthError::AuthServiceUnavailable)
    }
}

#[tokio::test]
async fn password_login_signed_jwt_and_orm_share_current_user_authority() {
    let store = Arc::new(LoginStore::default());
    let auth = agql_auth::AuthService::new(
        agql_auth::AuthConfig::new("synthetic-local-signing-secret-at-least-32-bytes"),
        store.clone(),
        store.clone(),
    )
    .unwrap();
    *store.user.lock().unwrap() = Some(agql_auth::StoredUser {
        id: "user-1".into(),
        principal: "synthetic@example.invalid".into(),
        password_hash: auth.hash_password("synthetic-local-password").unwrap(),
        roles: vec!["reader".into()],
        scopes: vec!["records.read".into(), "records.write".into()],
        disabled: false,
    });
    let payload = auth
        .login(
            "synthetic@example.invalid",
            "synthetic-local-password",
            agql_auth::ClientMetadata::default(),
        )
        .await
        .unwrap();
    let accepted = auth
        .authenticate_access_token(&payload.access_token)
        .unwrap();
    let (schema, host, bundle) = fixture_for(AuthPrincipal::User(accepted), None, false).await;
    host.clock.set(OffsetDateTime::now_utc());
    *host.live_users.lock().unwrap() = Some(store.clone());
    assert!(
        schema
            .execute(request(&bundle, READ))
            .await
            .errors
            .is_empty()
    );
    store.user.lock().unwrap().as_mut().unwrap().disabled = true;
    // Signature validation still succeeds; the online operation must not.
    assert!(
        auth.authenticate_access_token(&payload.access_token)
            .is_ok()
    );
    assert!(
        !schema
            .execute(request(&bundle, READ))
            .await
            .errors
            .is_empty()
    );
    {
        let mut user = store.user.lock().unwrap();
        let user = user.as_mut().unwrap();
        user.disabled = false;
        user.scopes.clear();
    }
    assert!(
        !schema
            .execute(request(&bundle, READ))
            .await
            .errors
            .is_empty()
    );
    assert!(
        bundle.subject().has_scope("records.read"),
        "shared context must remain immutable"
    );
    *store.user.lock().unwrap() = None;
    assert!(
        !schema
            .execute(request(&bundle, READ))
            .await
            .errors
            .is_empty()
    );
    assert_eq!(store.refresh.lock().unwrap().len(), 1);
}
