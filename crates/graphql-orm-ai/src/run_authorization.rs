//! Explicit host-issued authorization for durable AI work.

use crate::AiError;
use agql_auth::{AuthPrincipal, Clock, PrincipalReference, PrincipalReferenceKind};
use async_trait::async_trait;
use std::sync::Arc;
use time::Duration;
use uuid::Uuid;

/// Trusted host boundary for admitting a new run beyond its submitting credential.
///
/// Implementations must verify current authoritative session state and policy, cap
/// expiry by the live session lifetime, and preserve every reference binding. This
/// is never called to refresh an existing run. It grants no tool, scope, or egress
/// permission; ordinary current-principal resolution remains mandatory throughout
/// execution. The returned reference must contain no bearer or role snapshot.
#[async_trait]
pub trait AiRunAuthorizationIssuer: Send + Sync {
    /// Issues a bounded reference for this authenticated submission or explicit retry.
    ///
    /// # Errors
    ///
    /// Returns an error if the source credential, authoritative session, owner, or
    /// host work policy does not admit this run. The host must not extend session
    /// idle expiry or silently convert delegated/API credentials to user authority.
    async fn issue(
        &self,
        principal: &AuthPrincipal,
        session_id: Uuid,
        run_id: Uuid,
    ) -> Result<PrincipalReference, AiError>;
}

/// Validates a host-issued run reference before it enters durable run storage.
///
/// This opt-in boundary preserves the original reference by default in the session
/// and disposition services. Its issuer owns authentication policy; this wrapper
/// enforces unchanged identity/bindings, a still-live submitting user credential,
/// and a finite future deadline no later than 24 hours after admission.
pub struct AiRunAuthorization {
    issuer: Arc<dyn AiRunAuthorizationIssuer>,
    clock: Arc<dyn Clock>,
}

impl AiRunAuthorization {
    /// Installs the host's trusted admission boundary and time source.
    pub fn new(issuer: Arc<dyn AiRunAuthorizationIssuer>, clock: Arc<dyn Clock>) -> Self {
        Self { issuer, clock }
    }

    pub(crate) async fn issue(
        &self,
        principal: &AuthPrincipal,
        session_id: Uuid,
        run_id: Uuid,
    ) -> Result<PrincipalReference, AiError> {
        let original = principal.reference();
        let started_at = self.clock.now();
        if original.kind != PrincipalReferenceKind::UserSession
            || original.session_id.is_none()
            || session_id.is_nil()
            || run_id.is_nil()
            || original
                .expires_at
                .is_none_or(|expiry| expiry <= started_at)
            || matches!(
                original.grant_kind,
                Some(
                    agql_auth::AccessTokenGrantKind::Sessionless
                        | agql_auth::AccessTokenGrantKind::SessionBoundDelegation
                )
            )
        {
            return Err(AiError::ReauthorizationFailed);
        }
        let issued = self.issuer.issue(principal, session_id, run_id).await?;
        let now = self.clock.now();
        let mut identity = issued.clone();
        identity.expires_at = original.expires_at;
        if identity != original
            || now < started_at
            || original.expires_at.is_none_or(|expiry| expiry <= now)
            || issued
                .expires_at
                .is_none_or(|expiry| expiry <= now || expiry > started_at + Duration::hours(24))
        {
            return Err(AiError::ReauthorizationFailed);
        }
        Ok(issued)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agql_auth::{AccessTokenMetadata, AuthUser, FixedClock, SessionContext};
    use time::OffsetDateTime;

    struct Issuer(PrincipalReference);
    #[async_trait]
    impl AiRunAuthorizationIssuer for Issuer {
        async fn issue(
            &self,
            _: &AuthPrincipal,
            _: Uuid,
            _: Uuid,
        ) -> Result<PrincipalReference, AiError> {
            Ok(self.0.clone())
        }
    }
    fn principal(now: OffsetDateTime) -> AuthPrincipal {
        AuthPrincipal::User(AuthUser {
            user_id: "owner".into(),
            session_id: Uuid::from_u128(1),
            roles: vec![],
            scopes: vec!["records.read".into()],
            session: SessionContext::default(),
            token_claims: AccessTokenMetadata {
                expires_at: Some(now + Duration::seconds(95)),
                tenant_id: Some("tenant".into()),
                ..Default::default()
            },
        })
    }
    async fn admit(
        principal: &AuthPrincipal,
        reference: PrincipalReference,
        now: OffsetDateTime,
    ) -> Result<PrincipalReference, AiError> {
        AiRunAuthorization::new(Arc::new(Issuer(reference)), Arc::new(FixedClock::new(now)))
            .issue(principal, Uuid::from_u128(2), Uuid::from_u128(3))
            .await
    }
    #[tokio::test]
    async fn admits_an_hour_of_work_without_changing_the_submitting_credential() {
        let now = OffsetDateTime::now_utc();
        let principal = principal(now);
        let source = principal.reference();
        let mut issued = source.clone();
        issued.expires_at = Some(now + Duration::minutes(75));
        let result = admit(&principal, issued.clone(), now).await.unwrap();
        assert_eq!(result, issued);
        assert_eq!(principal.reference(), source);
        assert!(result.expires_at.unwrap() > now + Duration::hours(1));
    }
    #[tokio::test]
    async fn rejects_expired_source_missing_deadlines_and_excessive_lifetimes() {
        let now = OffsetDateTime::now_utc();
        let principal = principal(now);
        let mut issued = principal.reference();
        issued.expires_at = Some(now + Duration::hours(1));
        assert!(
            admit(&principal, issued.clone(), now + Duration::seconds(95))
                .await
                .is_err()
        );
        for expiry in [
            None,
            Some(now),
            Some(now + Duration::hours(24) + Duration::seconds(1)),
        ] {
            issued.expires_at = expiry;
            assert!(admit(&principal, issued.clone(), now).await.is_err());
        }
    }
    #[tokio::test]
    async fn rejects_host_changes_to_identity_or_resource_bindings() {
        let now = OffsetDateTime::now_utc();
        let principal = principal(now);
        let base = principal.reference();
        let mut changed = Vec::new();
        let mut r = base.clone();
        r.subject = "another".into();
        changed.push(r);
        let mut r = base.clone();
        r.session_id = Some(Uuid::new_v4().to_string());
        changed.push(r);
        let mut r = base.clone();
        r.tenant_id = Some("other".into());
        changed.push(r);
        let mut r = base.clone();
        r.resource_id = Some("other".into());
        changed.push(r);
        let mut r = base.clone();
        r.token_id = Some("other".into());
        changed.push(r);
        for mut reference in changed {
            reference.expires_at = Some(now + Duration::hours(1));
            assert!(admit(&principal, reference, now).await.is_err());
        }
    }
}
