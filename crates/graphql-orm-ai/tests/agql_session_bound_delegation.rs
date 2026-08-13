use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agql_auth::{
    AccessTokenGrantKind, AccessTokenMetadata, ActorIdentity, AuthConfig, AuthError, AuthPrincipal,
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
        let mut expected = AuthPrincipal::User(self.user.clone()).reference();
        let mut requested = reference.clone();
        expected.session_version = None;
        requested.session_version = None;
        if requested != expected {
            return Err(AuthError::Forbidden);
        }
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

#[tokio::test]
async fn normal_ai_consumer_can_issue_an_exact_session_bound_delegation() {
    let now = OffsetDateTime::now_utc();
    let session_id = Uuid::new_v4();
    let session_family_id = Uuid::new_v4();
    let user = AuthUser {
        user_id: "session-owner".to_owned(),
        session_id,
        roles: vec!["Operator".to_owned()],
        scopes: vec!["records.read".to_owned()],
        session: SessionContext::default(),
        token_claims: AccessTokenMetadata {
            tenant_id: Some("tenant-1".to_owned()),
            session_family_id: Some(session_family_id.to_string()),
            grant_kind: Some(AccessTokenGrantKind::UserSession),
            ..AccessTokenMetadata::default()
        },
    };
    let principal = AuthPrincipal::User(user.clone());
    let resolved = ResolvedPrincipal::new(principal.reference(), principal, now)
        .expect("source user session should resolve");
    let active_session = ActiveSession {
        user,
        verified_at: now,
    };
    let verified = active_session
        .resolve_active_user_session(resolved.reference())
        .await
        .expect("active session resolver should produce opaque verification");
    assert_eq!(verified.user().session_id, session_id);

    let refresh_tokens = Arc::new(NoRefreshTokens::default());
    let auth = AuthService::new(
        AuthConfig::new("test-only-secret-that-is-at-least-thirty-two-bytes"),
        Arc::new(NoUsers),
        refresh_tokens.clone(),
    )
    .expect("test auth service should construct")
    .with_active_user_session_resolver(Arc::new(active_session));
    let binding = SessionBoundDelegationBinding::new(
        ActorIdentity {
            sub: "ai-coordinator".to_owned(),
            amr: vec!["service".to_owned()],
        },
        "graphql_operation",
        "records_count",
        "correlation-1",
        ExactOperationBinding::new("RecordsCount", "sha256:reviewed-document"),
    );
    let request = auth
        .prepare_session_bound_access_token_only(
            &resolved,
            vec!["Operator".to_owned()],
            vec!["records.read".to_owned()],
            binding,
        )
        .await
        .expect("narrowed request should prepare")
        .with_ttl(Duration::minutes(5));
    let grant = auth
        .issue_session_bound_access_token_only(request)
        .await
        .expect("session-bound access-token-only grant should issue");
    let decoded = auth
        .authenticate_access_token(&grant.access_token)
        .expect("issued token should decode through the ordinary auth service");

    assert_eq!(decoded.session_id, session_id);
    assert_eq!(decoded.user_id, "session-owner");
    assert_eq!(decoded.roles, ["Operator"]);
    assert_eq!(decoded.scopes, ["records.read"]);
    assert_eq!(
        decoded.token_claims.grant_kind,
        Some(AccessTokenGrantKind::SessionBoundDelegation)
    );
    assert_eq!(
        decoded.token_claims.actor,
        Some(ActorIdentity {
            sub: "ai-coordinator".to_owned(),
            amr: vec!["service".to_owned()],
        })
    );
    assert_eq!(
        decoded.token_claims.resource_type.as_deref(),
        Some("graphql_operation")
    );
    assert_eq!(
        decoded.token_claims.resource_id.as_deref(),
        Some("records_count")
    );
    assert_eq!(
        decoded.token_claims.correlation_id.as_deref(),
        Some("correlation-1")
    );
    assert_eq!(
        decoded.token_claims.operation,
        Some(ExactOperationBinding::new(
            "RecordsCount",
            "sha256:reviewed-document",
        ))
    );
    assert_eq!(
        AuthPrincipal::User(decoded).reference().session_id,
        Some(session_id.to_string())
    );
    assert_eq!(refresh_tokens.mutations.load(Ordering::SeqCst), 0);
}
