#![cfg(all(feature = "receiver-admission", feature = "sqlite"))]
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agql_auth::{
    AccessTokenGrantKind, AccessTokenMetadata, ActorIdentity, AuthConfig, AuthPrincipal,
    AuthResult, AuthService, AuthUser, ExactOperationBinding, PrincipalReference,
    RefreshTokenRevocationReason, RefreshTokenStore, ResolvedPrincipal,
    SessionBoundDelegationBinding, SessionContext, StoredRefreshToken, StoredUser, UserStore,
    VerifiedActiveUserSession, VerifiedActiveUserSessionResolver,
};
use async_trait::async_trait;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

struct NoUsers;

#[async_trait]
impl UserStore for NoUsers {
    async fn find_user_by_principal(&self, _principal: &str) -> AuthResult<Option<StoredUser>> {
        Ok(None)
    }

    async fn find_user_by_id(&self, _user_id: &str) -> AuthResult<Option<StoredUser>> {
        Ok(None)
    }
}

#[derive(Default)]
struct NoRefreshTokens {
    mutations: AtomicUsize,
}

#[async_trait]
impl RefreshTokenStore for NoRefreshTokens {
    async fn insert_refresh_token(&self, _token: StoredRefreshToken) -> AuthResult<()> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn find_refresh_token_by_hash(
        &self,
        _token_hash: &str,
    ) -> AuthResult<Option<StoredRefreshToken>> {
        Ok(None)
    }

    async fn revoke_refresh_token(
        &self,
        _token_id: Uuid,
        _revoked_at: OffsetDateTime,
        _replaced_by_token_id: Option<Uuid>,
        _reason: RefreshTokenRevocationReason,
    ) -> AuthResult<()> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn revoke_refresh_token_family(
        &self,
        _session_family_id: Uuid,
        _revoked_at: OffsetDateTime,
        _reason: RefreshTokenRevocationReason,
    ) -> AuthResult<()> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn touch_refresh_token(
        &self,
        _token_id: Uuid,
        _used_at: OffsetDateTime,
        _ip_address: Option<String>,
        _user_agent: Option<String>,
    ) -> AuthResult<()> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn rotate_refresh_token(
        &self,
        _current_token_id: Uuid,
        _replacement: StoredRefreshToken,
        _rotated_at: OffsetDateTime,
        _ip_address: Option<String>,
        _user_agent: Option<String>,
    ) -> AuthResult<bool> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(false)
    }
}

#[derive(Clone)]
struct ActiveSession {
    user: AuthUser,
    verified_at: OffsetDateTime,
}

#[async_trait]
impl VerifiedActiveUserSessionResolver for ActiveSession {
    async fn resolve_active_user_session(
        &self,
        reference: &PrincipalReference,
    ) -> AuthResult<VerifiedActiveUserSession> {
        VerifiedActiveUserSession::from_authoritative_record(
            reference.clone(),
            self.user.clone(),
            "session-version-1",
            self.verified_at + Duration::hours(1),
            Some(self.verified_at + Duration::minutes(30)),
            self.verified_at,
        )
    }
}

use graphql_orm::graphql::auth_agql::{
    AgqlAcceptedRequest, AgqlReceiverContract, AgqlReceiverPolicy, AgqlReceiverRegistry,
    AgqlReceiverTransport,
};
use graphql_orm_operation_catalog::{
    RECEIVER_PURPOSE_CLAIM, ReceiverPurposeMetadata, ReceiverPurposeV1, exact_document_hash,
};
const DOCUMENT: &str =
    "query ReadA($id: String!, $tenant: String!){ item(id: $id, tenant: $tenant){ id } }";
const READ_B: &str = "query ReadB { other { id } }";
struct Policy;
#[async_trait]
impl AgqlReceiverPolicy for Policy {
    async fn permits(
        &self,
        delegated: &AuthUser,
        _: &AuthUser,
        _: &ReceiverPurposeMetadata,
        _: &serde_json::Value,
        arguments: &serde_json::Value,
    ) -> AuthResult<bool> {
        Ok(arguments
            .get("tenant")
            .is_none_or(|tenant| tenant.as_str() == delegated.token_claims.tenant_id.as_deref()))
    }
}
#[tokio::test]
async fn exact_signed_delegation_is_admitted_before_context_and_unrelated_read_is_refused() {
    let now = OffsetDateTime::now_utc();
    let user = AuthUser {
        user_id: "owner".into(),
        session_id: Uuid::new_v4(),
        roles: vec!["Reader".into()],
        scopes: vec!["item.read".into()],
        session: SessionContext::default(),
        token_claims: AccessTokenMetadata {
            tenant_id: Some("tenant-a".into()),
            organization_id: Some("organization-a".into()),
            session_family_id: Some(Uuid::new_v4().to_string()),
            grant_kind: Some(AccessTokenGrantKind::UserSession),
            ..Default::default()
        },
    };
    let principal = AuthPrincipal::User(user.clone());
    let resolved = ResolvedPrincipal::new(principal.reference(), principal, now).unwrap();
    let active = Arc::new(ActiveSession {
        user,
        verified_at: now,
    });
    let auth = AuthService::new(
        AuthConfig::new("test-only-secret-that-is-at-least-thirty-two-bytes"),
        Arc::new(NoUsers),
        Arc::new(NoRefreshTokens::default()),
    )
    .unwrap()
    .with_active_user_session_resolver(active.clone());
    let metadata = ReceiverPurposeMetadata {
        receiver: "receiver-a".into(),
        target: "target-a".into(),
        schema: "schema-a".into(),
        capability: "capability-a".into(),
        projection: "projection-a".into(),
        disclosure: "disclosure-a".into(),
        resource_type: "operation".into(),
        resource_id: "read-a".into(),
    };
    let variables = serde_json::json!({"id":"one","tenant":"tenant-a"});
    let purpose =
        ReceiverPurposeV1::bind(metadata.clone(), "ReadA", DOCUMENT, &variables, &variables);
    let binding = SessionBoundDelegationBinding::new(
        ActorIdentity {
            sub: "issuer".into(),
            amr: vec!["service".into()],
        },
        "operation",
        "read-a",
        "correlation-a",
        ExactOperationBinding::new("ReadA", exact_document_hash(DOCUMENT)),
    );
    let request = auth
        .prepare_session_bound_access_token_only(
            &resolved,
            vec!["Reader".into()],
            vec!["item.read".into()],
            binding,
        )
        .await
        .unwrap()
        .with_claim(
            RECEIVER_PURPOSE_CLAIM,
            serde_json::to_value(purpose).unwrap(),
        );
    let grant = auth
        .issue_session_bound_access_token_only(request.clone())
        .await
        .unwrap();
    let accepted =
        AuthPrincipal::User(auth.authenticate_access_token(&grant.access_token).unwrap());
    let mut other_metadata = metadata.clone();
    other_metadata.resource_id = "read-b".into();
    other_metadata.capability = "capability-b".into();
    other_metadata.projection = "projection-b".into();
    let registry = AgqlReceiverRegistry::new(
        vec![
            AgqlReceiverContract::new("ReadA", DOCUMENT, "item", metadata.clone()).unwrap(),
            AgqlReceiverContract::new("ReadB", READ_B, "other", other_metadata).unwrap(),
        ],
        4096,
    )
    .unwrap();
    let body = serde_json::to_vec(
        &serde_json::json!({"query":DOCUMENT,"operationName":"ReadA","variables":variables}),
    )
    .unwrap();
    let admitted = registry
        .admit(
            &body,
            AgqlReceiverTransport::Http,
            accepted.clone(),
            active.as_ref(),
            &Policy,
            &agql_auth::SystemClock,
        )
        .await
        .unwrap();
    assert_eq!(admitted.scopes(), ["item.read"]);
    assert!(
        registry
            .admit(
                &body,
                AgqlReceiverTransport::Http,
                accepted.clone(),
                active.as_ref(),
                &Policy,
                &agql_auth::SystemClock
            )
            .await
            .is_ok()
    );
    // The same public binding function is used by issuance and receiver admission
    // for omission/default/list coercion without collapsing raw input semantics.
    for (document, raw, arguments) in [
        (
            "query ReadA($key: String!){ item(id: $key){ id } }",
            serde_json::json!({"key":"one"}),
            serde_json::json!({"id":"one"}),
        ),
        (
            "query ReadA($id: String = \"one\"){ item(id: $id){ id } }",
            serde_json::json!({}),
            serde_json::json!({"id":"one"}),
        ),
        (
            "query ReadA($id: String){ item(id: $id){ id } }",
            serde_json::json!({"id":null}),
            serde_json::json!({"id":null}),
        ),
        (
            "query ReadA($id: String){ item(id: $id){ id } }",
            serde_json::json!({}),
            serde_json::json!({}),
        ),
        (
            "query ReadA($id: [Int!]){ item(id: $id){ id } }",
            serde_json::json!({"id":[1,2]}),
            serde_json::json!({"id":[1,2]}),
        ),
        (
            "query ReadA($id: [Int!]){ item(id: $id){ id } }",
            serde_json::json!({"id":1}),
            serde_json::json!({"id":[1]}),
        ),
    ] {
        let binding = SessionBoundDelegationBinding::new(
            ActorIdentity {
                sub: "issuer".into(),
                amr: vec![],
            },
            "operation",
            "read-a",
            "correlation-a",
            ExactOperationBinding::new("ReadA", exact_document_hash(document)),
        );
        let purpose =
            ReceiverPurposeV1::bind(metadata.clone(), "ReadA", document, &raw, &arguments);
        let prepared = auth
            .prepare_session_bound_access_token_only(
                &resolved,
                vec!["Reader".into()],
                vec!["item.read".into()],
                binding,
            )
            .await
            .unwrap()
            .with_claim(
                RECEIVER_PURPOSE_CLAIM,
                serde_json::to_value(purpose).unwrap(),
            );
        let token = auth
            .issue_session_bound_access_token_only(prepared)
            .await
            .unwrap();
        let principal =
            AuthPrincipal::User(auth.authenticate_access_token(&token.access_token).unwrap());
        let registry = AgqlReceiverRegistry::new(
            vec![AgqlReceiverContract::new("ReadA", document, "item", metadata.clone()).unwrap()],
            4096,
        )
        .unwrap();
        let body = serde_json::to_vec(
            &serde_json::json!({"query":document,"operationName":"ReadA","variables":raw}),
        )
        .unwrap();
        assert!(
            registry
                .admit(
                    &body,
                    AgqlReceiverTransport::Http,
                    principal,
                    active.as_ref(),
                    &Policy,
                    &agql_auth::SystemClock
                )
                .await
                .is_ok()
        );
    }
    let counters = Arc::new(Effects::default());
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE fixture_items (id TEXT NOT NULL, tenant TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO fixture_items (id,tenant) VALUES (?,?),(?,?)")
        .bind("one")
        .bind("tenant-a")
        .bind("two")
        .bind("tenant-b")
        .execute(&pool)
        .await
        .unwrap();
    let schema = async_graphql::Schema::build(
        Query {
            effects: counters.clone(),
            pool,
        },
        async_graphql::EmptyMutation,
        async_graphql::EmptySubscription,
    )
    .finish();
    // The positive baseline really dispatches, resolves, and reads SQLite.
    counters.dispatch.fetch_add(1, Ordering::SeqCst);
    let response = schema.execute(admitted.into_request().unwrap()).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(counters.values(), [1, 1, 1]);
    counters.reset();
    for query in [
        READ_B.to_string(),
        DOCUMENT.replace("item(", "changed("),
        DOCUMENT.replace("{ id }", "{ id secret }"),
        DOCUMENT.replace("{ id }", "{ id alias:secret }"),
        format!(
            "{} fragment Extra on Item {{ id secret }}",
            DOCUMENT.replace("{ id }", "{ ...Extra }")
        ),
        format!("{} other {{ id }} }}", DOCUMENT.strip_suffix('}').unwrap()),
        format!("{DOCUMENT} query Other {{ other {{ id }} }}"),
        DOCUMENT.replacen("query", "mutation", 1),
        DOCUMENT.replacen("query", "subscription", 1),
    ] {
        let operation_name = if query == READ_B { "ReadB" } else { "ReadA" };
        let request_variables = if query == READ_B {
            serde_json::json!({})
        } else {
            variables.clone()
        };
        let body = serde_json::to_vec(&serde_json::json!({"query":query,"operationName":operation_name,"variables":request_variables})).unwrap();
        refused(
            &registry,
            &body,
            accepted.clone(),
            active.as_ref(),
            &Policy,
            &schema,
            &counters,
        )
        .await;
    }
    for variables in [
        serde_json::json!({"id":"two","tenant":"tenant-a"}),
        serde_json::json!({"id":"one","tenant":"tenant-b"}),
        serde_json::json!({"id":null,"tenant":"tenant-a"}),
        serde_json::json!({"tenant":"tenant-a"}),
        serde_json::json!({"id":"one","tenant":"tenant-a", "unknown":true}),
        serde_json::json!({"id":["one"],"tenant":"tenant-a"}),
    ] {
        let body = serde_json::to_vec(
            &serde_json::json!({"query":DOCUMENT,"operationName":"ReadA","variables":variables}),
        )
        .unwrap();
        refused(
            &registry,
            &body,
            accepted.clone(),
            active.as_ref(),
            &Policy,
            &schema,
            &counters,
        )
        .await;
    }
    for raw in [
        b"[]".to_vec(),
        b"[{}]".to_vec(),
        vec![b' '; 4097],
        b"{\"query\":\"first\",\"query\":\"second\"}".to_vec(),
        body.iter()
            .copied()
            .chain(b" trailing".iter().copied())
            .collect(),
    ] {
        refused(
            &registry,
            &raw,
            accepted.clone(),
            active.as_ref(),
            &Policy,
            &schema,
            &counters,
        )
        .await;
    }
    let duplicate = format!(
        r#"{{"query":{query},"operationName":"ReadA","variables":{{"id":"one","id":"one"}}}}"#,
        query = serde_json::to_string(DOCUMENT).unwrap()
    );
    refused(
        &registry,
        duplicate.as_bytes(),
        accepted.clone(),
        active.as_ref(),
        &Policy,
        &schema,
        &counters,
    )
    .await;
    refused(
        &registry,
        &body,
        accepted.clone(),
        active.as_ref(),
        &Deny,
        &schema,
        &counters,
    )
    .await;
    for change in 0..10 {
        let mut parent = active.as_ref().clone();
        match change {
            0 => parent.user.user_id = "another-owner".into(),
            1 => parent.user.session_id = Uuid::new_v4(),
            2 => parent.user.token_claims.session_family_id = Some(Uuid::new_v4().to_string()),
            3 => parent.user.token_claims.tenant_id = Some("tenant-b".into()),
            4 => parent.user.token_claims.organization_id = Some("organization-b".into()),
            5 => parent.user.token_claims.grant_kind = Some(AccessTokenGrantKind::Sessionless),
            6 => parent.user.roles.clear(),
            7 => parent.user.scopes.clear(),
            8 => parent.verified_at = now - Duration::hours(2),
            _ => parent.user.token_claims.session_version = Some("changed-version".into()),
        }
        if change == 9 {
            refused(
                &registry,
                &body,
                accepted.clone(),
                &ChangedVersion(parent),
                &Policy,
                &schema,
                &counters,
            )
            .await;
        } else {
            refused(
                &registry,
                &body,
                accepted.clone(),
                &parent,
                &Policy,
                &schema,
                &counters,
            )
            .await;
        }
    }
    refused(
        &registry,
        &body,
        accepted.clone(),
        &Revoked,
        &Policy,
        &schema,
        &counters,
    )
    .await;
    let revoking = RevokesAfterRead {
        parent: active.as_ref().clone(),
        reads: AtomicUsize::new(0),
    };
    refused(
        &registry,
        &body,
        accepted.clone(),
        &revoking,
        &Policy,
        &schema,
        &counters,
    )
    .await;
    assert_eq!(revoking.reads.load(Ordering::SeqCst), 2);
    let clock = TestClock(std::sync::Mutex::new(now));
    let result = registry
        .admit(
            &body,
            AgqlReceiverTransport::Http,
            accepted.clone(),
            &ShortParent(active.as_ref().clone()),
            &AdvancePolicy(&clock, now + Duration::seconds(2)),
            &clock,
        )
        .await;
    assert!(result.is_err()); // Parent expires during policy await; own token remains live.
    assert_eq!(counters.values(), [0, 0, 0]);
    *clock.0.lock().unwrap() = now + Duration::hours(2);
    assert!(
        registry
            .admit(
                &body,
                AgqlReceiverTransport::Http,
                accepted.clone(),
                active.as_ref(),
                &Policy,
                &clock
            )
            .await
            .is_err()
    );
    assert_eq!(counters.values(), [0, 0, 0]);
    let mut widened_parent = active.as_ref().clone();
    widened_parent.user.roles.push("AdditionalRole".into());
    widened_parent.user.scopes.push("item.write".into());
    let unchanged = registry
        .admit(
            &body,
            AgqlReceiverTransport::Http,
            accepted.clone(),
            &widened_parent,
            &Policy,
            &agql_auth::SystemClock,
        )
        .await
        .unwrap();
    assert_eq!(unchanged.scopes(), ["item.read"]);
    let unchanged_request = unchanged.into_request().unwrap();
    let identity = unchanged_request
        .data
        .get(&std::any::TypeId::of::<AuthPrincipal>())
        .unwrap()
        .downcast_ref::<AuthPrincipal>()
        .unwrap();
    let AuthPrincipal::User(identity) = identity else {
        panic!("wrong identity class")
    };
    assert_eq!(identity.roles, ["Reader"]);

    let mut other_receiver_metadata = metadata.clone();
    other_receiver_metadata.receiver = "receiver-b".into();
    let other_receiver = AgqlReceiverRegistry::new(
        vec![
            AgqlReceiverContract::new("ReadA", DOCUMENT, "item", other_receiver_metadata).unwrap(),
        ],
        4096,
    )
    .unwrap();
    refused(
        &other_receiver,
        &body,
        accepted.clone(),
        active.as_ref(),
        &Policy,
        &schema,
        &counters,
    )
    .await;
    let empty_registry = AgqlReceiverRegistry::new(vec![], 4096).unwrap();
    refused(
        &empty_registry,
        &body,
        accepted.clone(),
        active.as_ref(),
        &Policy,
        &schema,
        &counters,
    )
    .await;
    for envelope in [
        serde_json::json!({"query":DOCUMENT,"operationName":"Unregistered","variables":{"id":"one"}}),
        serde_json::json!({"query":DOCUMENT,"operationName":"ReadA","variables":{"id":"one"},"extensions":{}}),
        serde_json::json!({"operationName":"ReadA","variables":{"id":"one"}}),
    ] {
        refused(
            &registry,
            &serde_json::to_vec(&envelope).unwrap(),
            accepted.clone(),
            active.as_ref(),
            &Policy,
            &schema,
            &counters,
        )
        .await;
    }
    for document in [
        "mutation ReadA { item { id } }",
        "subscription ReadA { item { id } }",
        "{ item { id } }",
        "query ReadA { item { id } other { id } }",
        "query ReadA { item { id } } query Other { other { id } }",
        "query ReadA { alias:item { id } }",
        "query ReadA { item { ...F } } fragment F on Item { id }",
        "query ReadA($id: CustomInput) { item(id:$id) { id } }",
        "query ReadA { item(id: \"one\") { id } }",
        "query ReadA { item { child(id:1) { id } } }",
    ] {
        assert!(AgqlReceiverContract::new("ReadA", document, "item", metadata.clone()).is_err());
    }
    // Even coherent signed purpose is intent, not current tenant policy.
    let other_tenant_variables = serde_json::json!({"id":"two","tenant":"tenant-b"});
    let other_tenant_purpose = ReceiverPurposeV1::bind(
        metadata.clone(),
        "ReadA",
        DOCUMENT,
        &other_tenant_variables,
        &other_tenant_variables,
    );
    let token = auth
        .issue_session_bound_access_token_only(request.clone().with_claim(
            RECEIVER_PURPOSE_CLAIM,
            serde_json::to_value(other_tenant_purpose).unwrap(),
        ))
        .await
        .unwrap();
    let principal =
        AuthPrincipal::User(auth.authenticate_access_token(&token.access_token).unwrap());
    let other_tenant_body = serde_json::to_vec(&serde_json::json!({"query":DOCUMENT,"operationName":"ReadA","variables":other_tenant_variables})).unwrap();
    refused(
        &registry,
        &other_tenant_body,
        principal,
        active.as_ref(),
        &Policy,
        &schema,
        &counters,
    )
    .await;
    // Mutate real issued wire claims, sign them with the fixture issuer, then
    // pass every token through agql-auth authentication before admission.
    let secret = b"test-only-secret-that-is-at-least-thirty-two-bytes";
    let wire: serde_json::Value = jsonwebtoken::decode(
        &grant.access_token,
        &jsonwebtoken::DecodingKey::from_secret(secret),
        &{
            let mut v = jsonwebtoken::Validation::default();
            v.validate_aud = false;
            v
        },
    )
    .unwrap()
    .claims;
    let mut changes = vec![
        ("/grant_kind", serde_json::json!("sessionless")),
        ("/sid", serde_json::json!(Uuid::new_v4().to_string())),
        (
            "/session_family_id",
            serde_json::json!(Uuid::new_v4().to_string()),
        ),
        ("/session_version", serde_json::json!("changed")),
        ("/tenant_id", serde_json::json!("tenant-b")),
        ("/organization_id", serde_json::json!("organization-b")),
        ("/exp", serde_json::json!(now.unix_timestamp() - 120)),
        ("/roles", serde_json::json!(["Expanded"])),
        ("/scope", serde_json::json!("item.write")),
        ("/resource_id", serde_json::json!("other-resource")),
        ("/resource_type", serde_json::json!("other-class")),
    ];
    changes.extend([
        (
            "/graphql_orm_receiver_purpose/version",
            serde_json::json!(2),
        ),
        (
            "/graphql_orm_receiver_purpose/target_class",
            serde_json::json!("private_routed"),
        ),
        (
            "/graphql_orm_receiver_purpose/metadata/receiver",
            serde_json::json!("receiver-b"),
        ),
        (
            "/graphql_orm_receiver_purpose/metadata/target",
            serde_json::json!("target-b"),
        ),
        (
            "/graphql_orm_receiver_purpose/metadata/schema",
            serde_json::json!("schema-b"),
        ),
        (
            "/graphql_orm_receiver_purpose/metadata/capability",
            serde_json::json!("capability-b"),
        ),
        (
            "/graphql_orm_receiver_purpose/metadata/projection",
            serde_json::json!("projection-b"),
        ),
        (
            "/graphql_orm_receiver_purpose/metadata/disclosure",
            serde_json::json!("disclosure-b"),
        ),
        (
            "/graphql_orm_receiver_purpose/variables_sha256",
            serde_json::json!("bad"),
        ),
        (
            "/graphql_orm_receiver_purpose/arguments_sha256",
            serde_json::json!("bad"),
        ),
        (
            "/graphql_orm_receiver_purpose/document_sha256",
            serde_json::json!("bad"),
        ),
    ]);
    for (path, value) in changes {
        let mut claims = wire.clone();
        *claims.pointer_mut(path).unwrap() = value;
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(secret),
        )
        .unwrap();
        if let Ok(user) = auth.authenticate_access_token(&token) {
            refused(
                &registry,
                &body,
                AuthPrincipal::User(user),
                active.as_ref(),
                &Policy,
                &schema,
                &counters,
            )
            .await;
        }
        assert_eq!(counters.values(), [0, 0, 0], "{path}");
    }
    for value in [
        serde_json::Value::Null,
        serde_json::json!([]),
        serde_json::json!({"version":1}),
        {
            let mut purpose = wire[RECEIVER_PURPOSE_CLAIM].clone();
            purpose["unknown"] = serde_json::json!(true);
            purpose
        },
    ] {
        let mut claims = wire.clone();
        claims[RECEIVER_PURPOSE_CLAIM] = value;
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(secret),
        )
        .unwrap();
        let user = auth.authenticate_access_token(&token).unwrap();
        refused(
            &registry,
            &body,
            AuthPrincipal::User(user),
            active.as_ref(),
            &Policy,
            &schema,
            &counters,
        )
        .await;
    }
    for key in [
        RECEIVER_PURPOSE_CLAIM,
        "operation",
        "actor",
        "correlation_id",
        "session_family_id",
        "session_version",
        "tenant_id",
        "organization_id",
        "grant_kind",
    ] {
        let mut claims = wire.clone();
        claims.as_object_mut().unwrap().remove(key);
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(secret),
        )
        .unwrap();
        if let Ok(user) = auth.authenticate_access_token(&token) {
            refused(
                &registry,
                &body,
                AuthPrincipal::User(user),
                active.as_ref(),
                &Policy,
                &schema,
                &counters,
            )
            .await;
        }
        assert_eq!(counters.values(), [0, 0, 0]);
    }

    assert!(
        registry
            .admit(
                &body,
                AgqlReceiverTransport::WebSocket,
                accepted.clone(),
                active.as_ref(),
                &Policy,
                &agql_auth::SystemClock
            )
            .await
            .is_err()
    );
    assert_eq!(counters.values(), [0, 0, 0]);
    // Opt-out compatibility/reproducer: the generic identity bridge still does
    // not enforce operation purpose. Read B is real and performs a SQLite read.
    let legacy = AgqlAcceptedRequest::new(accepted)
        .inject(async_graphql::Request::new(READ_B))
        .unwrap();
    counters.dispatch.fetch_add(1, Ordering::SeqCst);
    let response = schema.execute(legacy).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(counters.values(), [1, 1, 1]);
}

#[derive(Default)]
struct Effects {
    dispatch: AtomicUsize,
    resolver: AtomicUsize,
    read: AtomicUsize,
}
impl Effects {
    fn values(&self) -> [usize; 3] {
        [
            self.dispatch.load(Ordering::SeqCst),
            self.resolver.load(Ordering::SeqCst),
            self.read.load(Ordering::SeqCst),
        ]
    }
    fn reset(&self) {
        self.dispatch.store(0, Ordering::SeqCst);
        self.resolver.store(0, Ordering::SeqCst);
        self.read.store(0, Ordering::SeqCst);
    }
}
#[derive(async_graphql::SimpleObject)]
struct Item {
    id: String,
    secret: String,
}
struct Query {
    effects: Arc<Effects>,
    pool: sqlx::SqlitePool,
}
#[async_graphql::Object]
impl Query {
    async fn other(&self) -> Item {
        self.effects.resolver.fetch_add(1, Ordering::SeqCst);
        self.effects.read.fetch_add(1, Ordering::SeqCst);
        let (id,): (String,) =
            sqlx::query_as("SELECT id FROM fixture_items WHERE id=? AND tenant=?")
                .bind("two")
                .bind("tenant-b")
                .fetch_one(&self.pool)
                .await
                .unwrap();
        Item {
            id,
            secret: "synthetic-detail".into(),
        }
    }
    async fn item(&self, id: String, tenant: String) -> Item {
        self.effects.resolver.fetch_add(1, Ordering::SeqCst);
        self.effects.read.fetch_add(1, Ordering::SeqCst);
        let (id,): (String,) =
            sqlx::query_as("SELECT id FROM fixture_items WHERE id=? AND tenant=?")
                .bind(id)
                .bind(tenant)
                .fetch_one(&self.pool)
                .await
                .unwrap();
        Item {
            id,
            secret: "synthetic-detail".into(),
        }
    }
}
struct Deny;
#[async_trait]
impl AgqlReceiverPolicy for Deny {
    async fn permits(
        &self,
        _: &AuthUser,
        _: &AuthUser,
        _: &ReceiverPurposeMetadata,
        _: &serde_json::Value,
        _: &serde_json::Value,
    ) -> AuthResult<bool> {
        Ok(false)
    }
}
struct ChangedVersion(ActiveSession);
#[async_trait]
impl VerifiedActiveUserSessionResolver for ChangedVersion {
    async fn resolve_active_user_session(
        &self,
        reference: &PrincipalReference,
    ) -> AuthResult<VerifiedActiveUserSession> {
        VerifiedActiveUserSession::from_authoritative_record(
            reference.clone(),
            self.0.user.clone(),
            "changed-version",
            self.0.verified_at + Duration::hours(1),
            None,
            self.0.verified_at,
        )
    }
}
async fn refused(
    registry: &AgqlReceiverRegistry,
    body: &[u8],
    principal: AuthPrincipal,
    parent: &dyn VerifiedActiveUserSessionResolver,
    policy: &dyn AgqlReceiverPolicy,
    schema: &async_graphql::Schema<
        Query,
        async_graphql::EmptyMutation,
        async_graphql::EmptySubscription,
    >,
    effects: &Effects,
) {
    let result = registry
        .admit(
            body,
            AgqlReceiverTransport::Http,
            principal,
            parent,
            policy,
            &agql_auth::SystemClock,
        )
        .await;
    match result {
        Ok(admitted) => {
            effects.dispatch.fetch_add(1, Ordering::SeqCst);
            schema.execute(admitted.into_request().unwrap()).await;
            panic!("unexpected admission");
        }
        Err(error) => assert_eq!(error.to_string(), "RECEIVER_ADMISSION_REFUSED"),
    }
    assert_eq!(effects.values(), [0, 0, 0]);
}

struct Revoked;
#[async_trait]
impl VerifiedActiveUserSessionResolver for Revoked {
    async fn resolve_active_user_session(
        &self,
        _: &PrincipalReference,
    ) -> AuthResult<VerifiedActiveUserSession> {
        Err(agql_auth::AuthError::Forbidden)
    }
}
#[derive(Debug)]
struct TestClock(std::sync::Mutex<OffsetDateTime>);
impl agql_auth::Clock for TestClock {
    fn now(&self) -> OffsetDateTime {
        *self.0.lock().unwrap()
    }
}
struct ShortParent(ActiveSession);
#[async_trait]
impl VerifiedActiveUserSessionResolver for ShortParent {
    async fn resolve_active_user_session(
        &self,
        reference: &PrincipalReference,
    ) -> AuthResult<VerifiedActiveUserSession> {
        VerifiedActiveUserSession::from_authoritative_record(
            reference.clone(),
            self.0.user.clone(),
            "session-version-1",
            self.0.verified_at + Duration::hours(1),
            Some(self.0.verified_at + Duration::seconds(1)),
            self.0.verified_at,
        )
    }
}
struct AdvancePolicy<'a>(&'a TestClock, OffsetDateTime);
#[async_trait]
impl AgqlReceiverPolicy for AdvancePolicy<'_> {
    async fn permits(
        &self,
        _: &AuthUser,
        _: &AuthUser,
        _: &ReceiverPurposeMetadata,
        _: &serde_json::Value,
        _: &serde_json::Value,
    ) -> AuthResult<bool> {
        *self.0.0.lock().unwrap() = self.1;
        Ok(true)
    }
}

struct RevokesAfterRead {
    parent: ActiveSession,
    reads: AtomicUsize,
}
#[async_trait]
impl VerifiedActiveUserSessionResolver for RevokesAfterRead {
    async fn resolve_active_user_session(
        &self,
        reference: &PrincipalReference,
    ) -> AuthResult<VerifiedActiveUserSession> {
        if self.reads.fetch_add(1, Ordering::SeqCst) != 0 {
            return Err(agql_auth::AuthError::Forbidden);
        }
        self.parent.resolve_active_user_session(reference).await
    }
}
