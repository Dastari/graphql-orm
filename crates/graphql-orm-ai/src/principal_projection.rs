//! Opt-in host-bound namespace projection of authenticated principals.
//!
//! `AuthPrincipal::reference` never carries a user audience, so a host whose
//! [`CurrentPrincipalResolver`](agql_auth::CurrentPrincipalResolver) requires
//! the verified audience on every durable reference cannot resolve the
//! engine's default projection. A trusted host installs one
//! [`AiPrincipalProjection`] on each admitting service; the engine then stamps
//! the host's verified namespace into every reference it persists or resolves.

use crate::AiError;
use agql_auth::{AuthPrincipal, PrincipalReference, PrincipalReferenceKind};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use agql_auth::{Clock, CurrentPrincipalResolver};
use std::fmt;
use std::sync::Arc;

/// Maximum UTF-8 byte length of a projected principal namespace.
pub const MAX_AI_PRINCIPAL_NAMESPACE_BYTES: usize = 256;

/// Validated, nonsecret host namespace such as a verified token audience.
///
/// A namespace is identity metadata, never a credential. It is 1 to
/// [`MAX_AI_PRINCIPAL_NAMESPACE_BYTES`] bytes, has no control characters and
/// no leading or trailing whitespace. `Debug` prints only its length.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AiPrincipalNamespace(String);

impl AiPrincipalNamespace {
    /// Validates a host namespace.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] for an empty, oversized,
    /// whitespace-padded or control-character value. The value is not echoed.
    pub fn new(value: impl Into<String>) -> Result<Self, AiError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_AI_PRINCIPAL_NAMESPACE_BYTES
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(AiError::InvalidConfiguration(
                "invalid AI principal namespace".to_owned(),
            ));
        }
        Ok(Self(value))
    }

    /// Returns the namespace.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AiPrincipalNamespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Abbreviated to the length so logs never carry host realm names.
        write!(f, "AiPrincipalNamespace(<{} bytes>)", self.0.len())
    }
}

/// Trusted host seam naming the namespace an authenticated principal was
/// verified against.
///
/// Implementations are pure functions of the already-authenticated principal
/// and host configuration: no I/O, no request-global or mutable state.
/// Returning an error refuses admission. The projector can only choose a
/// namespace; identity, grant, bindings and expiry are preserved by the engine.
///
/// The engine cannot prove the host verified the namespace. It proves only
/// that nothing else in the reference changed. Current resolution through the
/// host's strict resolver remains authoritative.
pub trait AiPrincipalProjector: Send + Sync {
    /// Returns the verified namespace for this principal.
    ///
    /// # Errors
    ///
    /// Any error refuses admission. The engine reports it as
    /// [`AiError::ReauthorizationFailed`] without its detail.
    fn namespace(&self, principal: &AuthPrincipal) -> Result<AiPrincipalNamespace, AiError>;
}

struct FixedNamespace(AiPrincipalNamespace);

impl AiPrincipalProjector for FixedNamespace {
    fn namespace(&self, _principal: &AuthPrincipal) -> Result<AiPrincipalNamespace, AiError> {
        Ok(self.0.clone())
    }
}

/// Engine-enforced host namespace projection, shared by every admitting
/// service through [`Arc`].
///
/// [`Self::project`] returns `principal.reference()` with only `audience` set:
///
/// - a user-session reference gains the namespace as its audience;
/// - an API-token reference keeps its own audience, which must already equal
///   the namespace (a projection never rebinds a token);
/// - an existing, different audience is refused.
///
/// Kind, subject, session/token/family IDs, grant kind, session version,
/// tenant/resource/actor/operation/correlation bindings and the exact
/// `expires_at` are unchanged. Projection never extends a deadline; compose it
/// with [`crate::AiRunAuthorization`] for bounded run lifetimes.
#[derive(Clone)]
pub struct AiPrincipalProjection {
    projector: Arc<dyn AiPrincipalProjector>,
}

impl AiPrincipalProjection {
    /// Installs a host projector.
    pub fn new(projector: Arc<dyn AiPrincipalProjector>) -> Self {
        Self { projector }
    }

    /// Stamps one fixed namespace, for hosts with a single verified audience.
    pub fn fixed(namespace: AiPrincipalNamespace) -> Self {
        Self::new(Arc::new(FixedNamespace(namespace)))
    }

    /// Projects an authenticated principal into its namespace-bound reference.
    ///
    /// Hosts use this for their own runtime calls so they persist exactly the
    /// reference the services do.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::ReauthorizationFailed`] when the projector refuses
    /// or the namespace contradicts an existing audience binding.
    pub fn project(&self, principal: &AuthPrincipal) -> Result<PrincipalReference, AiError> {
        let namespace = self
            .projector
            .namespace(principal)
            .map_err(|_| AiError::ReauthorizationFailed)?;
        let mut reference = principal.reference();
        match (&reference.kind, reference.audience.as_deref()) {
            (_, Some(existing)) if existing == namespace.as_str() => {}
            (PrincipalReferenceKind::UserSession, None) => {
                reference.audience = Some(namespace.0);
            }
            _ => return Err(AiError::ReauthorizationFailed),
        }
        Ok(reference)
    }
}

impl fmt::Debug for AiPrincipalProjection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiPrincipalProjection")
            .finish_non_exhaustive()
    }
}

/// Stable, safe reason code recorded when queued or waiting work carries a
/// delegated or sessionless stored principal reference.
pub(crate) const DELEGATED_REFERENCE_CODE: &str = "principal_reference_delegated";

/// Whether a stored or request principal reference must not act as the user
/// behind it.
///
/// This is the one predicate behind both request-root refusal and stored
/// reference refusal. A user-session reference is refused when its grant kind
/// is `Sessionless` or `SessionBoundDelegation`, or when it has no grant kind
/// but still carries an actor subject or an exact operation binding. Current
/// resolution maps such a reference to the parent user session, dropping its
/// narrowing. A resource binding alone is not refused: ordinary user sessions
/// carry one. API-token references are never refused here.
pub(crate) fn is_delegated_reference(reference: &PrincipalReference) -> bool {
    if reference.kind != PrincipalReferenceKind::UserSession {
        return false;
    }
    match reference.grant_kind {
        Some(
            agql_auth::AccessTokenGrantKind::Sessionless
            | agql_auth::AccessTokenGrantKind::SessionBoundDelegation,
        ) => true,
        Some(agql_auth::AccessTokenGrantKind::UserSession) => false,
        None => reference.actor_subject.is_some() || reference.operation.is_some(),
    }
}

/// The only way the engine resolves a principal reference for authority.
///
/// Releases before 0.130.0 could persist a delegated or sessionless request
/// credential's reference on a run or on session-title work. Workers resolve
/// stored references to the user session behind them, so this refuses the
/// delegated shape before the host resolver is called, with the same
/// non-transient error an expired or revoked reference produces. Callers map
/// the error exactly as they map any other resolution failure, so these rows
/// fail through the expired-reference path.
pub(crate) async fn resolve_current_reference(
    resolver: &dyn agql_auth::CurrentPrincipalResolver,
    reference: &PrincipalReference,
) -> agql_auth::AuthResult<agql_auth::ResolvedPrincipal> {
    if is_delegated_reference(reference) {
        return Err(agql_auth::AuthError::Forbidden);
    }
    resolver.resolve(reference).await
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
/// Refuses a request credential that must not act as the user behind it.
///
/// Current-principal resolution maps a session-bound delegation to the parent
/// user session it is bound to, which drops the delegation's actor, resource
/// and operation narrowing. A sessionless grant has no session to resolve.
/// Approval, proposal, run-control, tool-result preview and stream roots
/// rehydrate their caller this way before deciding or disclosing, so they
/// accept neither grant, as run issuance does. Message submission persists the
/// caller's reference for workers to resolve the same way, so every session
/// root, and the inbox page, refuses them too, whether or not run
/// authorization or submission admission is installed. A credential with no grant kind
/// that still carries an actor or an exact operation binding is narrowed the
/// same way and is refused before it reaches the host resolver. A resource
/// binding alone is not refused: ordinary user sessions carry one. The refusal
/// carries no reason.
pub(crate) fn refuse_delegated_request(principal: &AuthPrincipal) -> Result<(), AiError> {
    if matches!(principal, AuthPrincipal::User(_)) && is_delegated_reference(&principal.reference())
    {
        return Err(AiError::ReauthorizationFailed);
    }
    Ok(())
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
/// The reference a request root resolves for its caller, after
/// `refuse_delegated_request`.
pub(crate) fn request_admission_reference(
    projection: Option<&AiPrincipalProjection>,
    principal: &AuthPrincipal,
) -> Result<PrincipalReference, AiError> {
    refuse_delegated_request(principal)?;
    admission_reference(projection, principal)
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
/// The reference an admitting service persists or resolves for a caller.
pub(crate) fn admission_reference(
    projection: Option<&AiPrincipalProjection>,
    principal: &AuthPrincipal,
) -> Result<PrincipalReference, AiError> {
    match projection {
        Some(projection) => projection.project(principal),
        None => Ok(principal.reference()),
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
/// Admits a durable stream. Delegated and sessionless credentials are refused
/// because the stream refreshes its principal through the resolver. Without a
/// projection the caller's own principal
/// and reference are used, unchanged. With one, the projected reference is
/// strictly resolved before the first page and must come back exactly, for the
/// same principal kind and subject; the stream starts from that current
/// principal and periodically re-resolves the projected reference.
pub(crate) async fn admit_stream(
    projection: Option<&AiPrincipalProjection>,
    resolver: &dyn CurrentPrincipalResolver,
    principal: AuthPrincipal,
) -> Result<(PrincipalReference, AuthPrincipal), AiError> {
    refuse_delegated_request(&principal)?;
    let Some(projection) = projection else {
        return Ok((principal.reference(), principal));
    };
    let reference = projection.project(&principal)?;
    let resolved = resolve_current_reference(resolver, &reference)
        .await
        .map_err(|_| AiError::ReauthorizationFailed)?;
    // Stream services carry no injected clock; the system clock bounds the
    // initial admission like the submission check, with a small skew allowance
    // for resolvers that stamp their own clock.
    let now = time::OffsetDateTime::now_utc();
    if !resolved_identity_matches(&resolved, &reference)
        || resolved.resolved_at() > now + MAXIMUM_STREAM_RESOLUTION_SKEW
        || reference.expires_at.is_some_and(|expiry| expiry <= now)
    {
        return Err(AiError::ReauthorizationFailed);
    }
    Ok((reference, resolved.into_principal()))
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
const MAXIMUM_STREAM_RESOLUTION_SKEW: time::Duration = time::Duration::seconds(5);

/// Whether a resolution is for exactly the requested reference and keeps its
/// principal kind and subject. Periodic stream reauthorization applies it so a
/// resolver cannot switch the principal mid-stream.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub(crate) fn resolved_identity_matches(
    resolved: &agql_auth::ResolvedPrincipal,
    reference: &PrincipalReference,
) -> bool {
    let current = resolved.principal().reference();
    resolved.reference() == reference
        && current.kind == reference.kind
        && current.subject == reference.subject
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
/// Opt-in strict resolution of a new submission's reference before any write.
#[derive(Clone)]
pub(crate) struct AiSubmissionAdmission {
    resolver: Arc<dyn CurrentPrincipalResolver>,
    clock: Arc<dyn Clock>,
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
impl AiSubmissionAdmission {
    pub(crate) fn new(resolver: Arc<dyn CurrentPrincipalResolver>, clock: Arc<dyn Clock>) -> Self {
        Self { resolver, clock }
    }

    /// Requires the host resolver to return exactly this live reference.
    pub(crate) async fn admit(&self, reference: &PrincipalReference) -> Result<(), AiError> {
        let resolved = resolve_current_reference(self.resolver.as_ref(), reference)
            .await
            .map_err(|_| AiError::ReauthorizationFailed)?;
        let now = self.clock.now();
        if resolved.reference() != reference
            || resolved.resolved_at() > now
            || reference.expires_at.is_some_and(|expiry| expiry <= now)
        {
            return Err(AiError::ReauthorizationFailed);
        }
        Ok(())
    }
}

/// Legacy stored references a pre-0.130.0 release could persist, derived from
/// an ordinary user-session reference: a session-bound delegation, a
/// sessionless grant, and a reference with no grant kind but an actor.
#[cfg(test)]
pub(crate) fn test_delegated_references(
    base: &PrincipalReference,
) -> Vec<(&'static str, PrincipalReference)> {
    let mut delegation = base.clone();
    delegation.grant_kind = Some(agql_auth::AccessTokenGrantKind::SessionBoundDelegation);
    delegation.actor_subject = Some("delegated-agent".to_owned());
    let mut sessionless = base.clone();
    sessionless.grant_kind = Some(agql_auth::AccessTokenGrantKind::Sessionless);
    let mut actor_only = base.clone();
    actor_only.grant_kind = None;
    actor_only.actor_subject = Some("delegated-agent".to_owned());
    vec![
        ("session_bound_delegation", delegation),
        ("sessionless", sessionless),
        ("no_grant_kind_with_actor", actor_only),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use agql_auth::{
        AccessTokenGrantKind, AccessTokenMetadata, ApiTokenPrincipal, AuthUser, SessionContext,
    };
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    fn user(audience_like_tenant: &str) -> AuthPrincipal {
        AuthPrincipal::User(AuthUser {
            user_id: "owner".into(),
            session_id: Uuid::from_u128(1),
            roles: vec!["admin".into()],
            scopes: vec!["records.read".into()],
            session: SessionContext::default(),
            token_claims: AccessTokenMetadata {
                jti: Some("token-1".into()),
                session_family_id: Some("family-1".into()),
                grant_kind: Some(AccessTokenGrantKind::UserSession),
                session_version: Some("v3".into()),
                tenant_id: Some(audience_like_tenant.into()),
                resource_type: Some("project".into()),
                resource_id: Some("p1".into()),
                expires_at: Some(OffsetDateTime::UNIX_EPOCH + Duration::days(20_000)),
                correlation_id: Some("corr".into()),
                ..Default::default()
            },
        })
    }

    fn namespace(value: &str) -> AiPrincipalNamespace {
        AiPrincipalNamespace::new(value).unwrap()
    }

    #[test]
    fn namespace_validation_is_bounded_and_does_not_echo_input() {
        for invalid in [
            String::new(),
            " padded".to_owned(),
            "padded ".to_owned(),
            "line\nbreak".to_owned(),
            "x".repeat(MAX_AI_PRINCIPAL_NAMESPACE_BYTES + 1),
        ] {
            let error = AiPrincipalNamespace::new(invalid.clone()).unwrap_err();
            assert!(!error.to_string().contains(invalid.trim()) || invalid.trim().is_empty());
        }
        assert!(AiPrincipalNamespace::new("x".repeat(MAX_AI_PRINCIPAL_NAMESPACE_BYTES)).is_ok());
        assert_eq!(namespace("host:admin").as_str(), "host:admin");
        assert_eq!(
            format!("{:?}", namespace("host:admin")),
            "AiPrincipalNamespace(<10 bytes>)"
        );
    }

    #[test]
    fn projection_changes_only_the_user_audience() {
        let principal = user("tenant");
        let original = principal.reference();
        let projected = AiPrincipalProjection::fixed(namespace("host:admin"))
            .project(&principal)
            .unwrap();
        assert_eq!(projected.audience.as_deref(), Some("host:admin"));
        let mut unstamped = projected.clone();
        unstamped.audience = None;
        assert_eq!(unstamped, original);
        assert_eq!(projected.expires_at, original.expires_at);
        assert_eq!(principal.reference(), original);
    }

    #[test]
    fn api_tokens_keep_their_audience_and_are_never_rebound() {
        let token = |audience: Option<&str>| {
            AuthPrincipal::ApiToken(ApiTokenPrincipal {
                token_id: Uuid::from_u128(9),
                subject: "service".into(),
                principal_kind: agql_auth::ApiTokenPrincipalKind::service(),
                scopes: vec![],
                audience: audience.map(str::to_owned),
                resource_type: None,
                resource_id: None,
                expires_at: OffsetDateTime::UNIX_EPOCH + Duration::days(20_000),
            })
        };
        let projection = AiPrincipalProjection::fixed(namespace("host:api"));
        let same = token(Some("host:api"));
        assert_eq!(projection.project(&same).unwrap(), same.reference());
        for other in [token(Some("host:other")), token(None)] {
            assert!(matches!(
                projection.project(&other),
                Err(AiError::ReauthorizationFailed)
            ));
        }
    }

    #[test]
    fn projector_refusal_is_reported_without_detail() {
        struct Refuse;
        impl AiPrincipalProjector for Refuse {
            fn namespace(&self, _: &AuthPrincipal) -> Result<AiPrincipalNamespace, AiError> {
                Err(AiError::InvalidInput("host detail: secret-ish".into()))
            }
        }
        let error = AiPrincipalProjection::new(Arc::new(Refuse))
            .project(&user("tenant"))
            .unwrap_err();
        assert!(matches!(error, AiError::ReauthorizationFailed));
        assert!(!error.to_string().contains("secret-ish"));
    }

    #[test]
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    fn default_admission_is_the_unprojected_reference() {
        let principal = user("tenant");
        assert_eq!(
            admission_reference(None, &principal).unwrap(),
            principal.reference()
        );
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    #[test]
    fn delegated_or_narrowed_request_credentials_are_refused() {
        let refused = |principal: &AuthPrincipal| {
            matches!(
                refuse_delegated_request(principal),
                Err(AiError::ReauthorizationFailed)
            )
        };
        let with_claims = |edit: &dyn Fn(&mut AccessTokenMetadata)| {
            let AuthPrincipal::User(mut user) = user("tenant-a") else {
                unreachable!()
            };
            edit(&mut user.token_claims);
            AuthPrincipal::User(user)
        };
        let actor = || agql_auth::ActorIdentity {
            sub: "ai-coordinator".into(),
            amr: vec!["service".into()],
        };
        let operation =
            || agql_auth::ExactOperationBinding::new("RecordsCount", "sha256:reviewed-document");

        // Ordinary sessions, with or without a grant kind or a resource binding.
        assert!(!refused(&user("tenant-a")));
        assert!(!refused(&with_claims(&|claims| claims.grant_kind = None)));
        assert!(!refused(&with_claims(&|claims| {
            claims.grant_kind = None;
            claims.resource_type = Some("record".into());
            claims.resource_id = Some("record-1".into());
        })));
        assert!(!refused(&with_claims(&|claims| {
            claims.resource_type = Some("record".into());
            claims.resource_id = Some("record-1".into());
        })));

        // Delegated and sessionless grants.
        for kind in [
            AccessTokenGrantKind::SessionBoundDelegation,
            AccessTokenGrantKind::Sessionless,
        ] {
            assert!(refused(&with_claims(
                &|claims| claims.grant_kind = Some(kind)
            )));
        }

        // No grant kind, but narrowed by an actor or an exact operation.
        assert!(refused(&with_claims(&|claims| {
            claims.grant_kind = None;
            claims.actor = Some(actor());
        })));
        assert!(refused(&with_claims(&|claims| {
            claims.grant_kind = None;
            claims.operation = Some(operation());
        })));
        assert!(refused(&with_claims(&|claims| {
            claims.grant_kind = None;
            claims.actor = Some(actor());
            claims.operation = Some(operation());
            claims.resource_type = Some("graphql_operation".into());
            claims.resource_id = Some("records_count".into());
        })));
    }

    #[test]
    fn stored_reference_predicate_matches_the_request_refusal() {
        let base = user("tenant").reference();
        assert!(!is_delegated_reference(&base));
        let mut unclassified = base.clone();
        unclassified.grant_kind = None;
        assert!(!is_delegated_reference(&unclassified));
        unclassified.resource_type = Some("project".into());
        unclassified.resource_id = Some("p1".into());
        assert!(
            !is_delegated_reference(&unclassified),
            "a resource binding alone is an ordinary user session"
        );
        for (_, reference) in test_delegated_references(&base) {
            assert!(is_delegated_reference(&reference));
        }
        let mut operation_only = base.clone();
        operation_only.grant_kind = None;
        operation_only.operation = Some(agql_auth::ExactOperationBinding::new(
            "RecordsCount",
            "sha256:reviewed-document",
        ));
        assert!(is_delegated_reference(&operation_only));
        // API-token references carry no grant kind and are never refused here.
        let token = AuthPrincipal::ApiToken(ApiTokenPrincipal {
            token_id: Uuid::from_u128(9),
            subject: "service".into(),
            principal_kind: agql_auth::ApiTokenPrincipalKind::service(),
            scopes: vec![],
            audience: None,
            resource_type: None,
            resource_id: None,
            expires_at: OffsetDateTime::UNIX_EPOCH + Duration::days(20_000),
        });
        assert!(!is_delegated_reference(&token.reference()));
    }

    #[tokio::test]
    async fn stored_reference_resolution_refuses_before_the_host_resolver() {
        struct Counting(std::sync::atomic::AtomicUsize);
        #[async_trait::async_trait]
        impl agql_auth::CurrentPrincipalResolver for Counting {
            async fn resolve(
                &self,
                _: &PrincipalReference,
            ) -> agql_auth::AuthResult<agql_auth::ResolvedPrincipal> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(agql_auth::AuthError::AuthServiceUnavailable)
            }
        }
        let resolver = Counting(std::sync::atomic::AtomicUsize::new(0));
        let base = user("tenant").reference();
        for (_, reference) in test_delegated_references(&base) {
            assert!(matches!(
                resolve_current_reference(&resolver, &reference).await,
                Err(agql_auth::AuthError::Forbidden)
            ));
        }
        assert_eq!(resolver.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(resolve_current_reference(&resolver, &base).await.is_err());
        assert_eq!(resolver.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
