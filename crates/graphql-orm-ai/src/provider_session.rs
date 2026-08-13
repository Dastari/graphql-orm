//! Provider-neutral durable provider-session binding contracts.
//!
//! A provider session is retained provider state addressed by an opaque cursor.
//! It is deliberately distinct from an in-memory warm process: process
//! ownership, pooling, admission, and idle eviction belong to a provider
//! adapter or host, while this module describes the durable, protected binding
//! needed to resume provider state without weakening run fencing.

#![cfg_attr(feature = "mssql", allow(dead_code))]

use std::fmt;

use agql_auth::PrincipalReference;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

#[cfg(any(feature = "sqlite", feature = "postgres"))]
use crate::AiRunLease;
use crate::{AiError, AiRunId, AiScope, AiSessionId, ProviderError, ProviderKind};

const MAXIMUM_CURSOR_BYTES: usize = 64 * 1024;
const MAXIMUM_IDENTIFIER_BYTES: usize = 200;
const MAXIMUM_PROTOCOL_BYTES: usize = 200;
const MAXIMUM_PROVIDER_SESSION_LIFETIME: Duration = Duration::days(30);
const MAXIMUM_PROVIDER_SESSION_IDLE_TTL: Duration = Duration::days(7);
const MAXIMUM_PROVIDER_SESSION_LEASE_TTL: Duration = Duration::hours(1);
const MAXIMUM_PROVIDER_SESSION_RETRY_DELAY: Duration = Duration::days(7);
pub(crate) const MAXIMUM_PROVIDER_SESSION_REBIND_AUTHORIZATION_TTL: Duration = Duration::minutes(1);

/// Opaque provider-issued resume cursor.
///
/// The cursor is accepted only from a trusted provider adapter. It is not a
/// bearer credential, GraphQL input, model value, log field, or authorization
/// proof. Durable services protect it before persistence and bind the envelope
/// to the exact row, field, and application scope.
#[derive(Clone, PartialEq, Eq)]
pub struct AiProviderSessionCursor {
    kind: String,
    value: String,
}

impl AiProviderSessionCursor {
    /// Creates a bounded provider-specific opaque cursor.
    ///
    /// `kind` is a stable provider-adapter format identifier, not a model-
    /// selected capability. The cursor value may contain printable UTF-8 but
    /// no control characters.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for an invalid kind or an empty,
    /// control-bearing, or oversized cursor.
    pub fn new(kind: impl Into<String>, value: impl Into<String>) -> Result<Self, AiError> {
        let cursor = Self {
            kind: kind.into(),
            value: value.into(),
        };
        if !valid_namespaced_identifier(&cursor.kind)
            || cursor.value.is_empty()
            || cursor.value.len() > MAXIMUM_CURSOR_BYTES
            || cursor.value.chars().any(char::is_control)
        {
            return Err(AiError::InvalidInput(
                "invalid provider-session cursor".to_owned(),
            ));
        }
        Ok(cursor)
    }

    /// Stable provider-adapter cursor format.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Returns the opaque value to the trusted provider adapter.
    ///
    /// Callers must not log, serialize into ordinary telemetry, expose through
    /// GraphQL, or use this value as authority.
    pub fn expose_to_provider_adapter(&self) -> &str {
        &self.value
    }

    pub(crate) fn fingerprint(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"graphql-orm-ai/provider-session-cursor/v1\0");
        digest.update((self.kind.len() as u64).to_be_bytes());
        digest.update(self.kind.as_bytes());
        digest.update((self.value.len() as u64).to_be_bytes());
        digest.update(self.value.as_bytes());
        hex::encode(digest.finalize())
    }

    pub(crate) fn into_parts(self) -> (String, String) {
        (self.kind, self.value)
    }
}

impl fmt::Debug for AiProviderSessionCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AiProviderSessionCursor")
            .field("kind", &self.kind)
            .field("value", &"[REDACTED]")
            .field("value_bytes", &self.value.len())
            .finish()
    }
}

/// Exact immutable provider/runtime identity to which a cursor is bound.
///
/// These fields are server-authored deployment facts. Constructing this value
/// validates their shape only; it does not enable provider retention, egress,
/// application tools, or a model route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiProviderSessionDescriptor {
    provider_kind: ProviderKind,
    provider_profile_id: String,
    provider_model: String,
    registration_fingerprint: String,
    protocol_version: String,
    policy_fingerprint: String,
}

impl AiProviderSessionDescriptor {
    /// Creates one exact provider-session identity.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] unless profile/model/protocol
    /// identifiers are bounded and both fingerprints are lowercase SHA-256.
    pub fn new(
        provider_kind: ProviderKind,
        provider_profile_id: impl Into<String>,
        provider_model: impl Into<String>,
        registration_fingerprint: impl Into<String>,
        protocol_version: impl Into<String>,
        policy_fingerprint: impl Into<String>,
    ) -> Result<Self, AiError> {
        let descriptor = Self {
            provider_kind,
            provider_profile_id: provider_profile_id.into(),
            provider_model: provider_model.into(),
            registration_fingerprint: registration_fingerprint.into(),
            protocol_version: protocol_version.into(),
            policy_fingerprint: policy_fingerprint.into(),
        };
        if !valid_identifier(&descriptor.provider_profile_id)
            || !valid_identifier(&descriptor.provider_model)
            || !crate::valid_sha256(&descriptor.registration_fingerprint)
            || descriptor.protocol_version.is_empty()
            || descriptor.protocol_version.len() > MAXIMUM_PROTOCOL_BYTES
            || descriptor.protocol_version.chars().any(char::is_control)
            || !crate::valid_sha256(&descriptor.policy_fingerprint)
        {
            return Err(AiError::InvalidConfiguration(
                "invalid provider-session descriptor".to_owned(),
            ));
        }
        Ok(descriptor)
    }

    /// Exact provider family.
    pub fn provider_kind(&self) -> &ProviderKind {
        &self.provider_kind
    }

    /// Exact logical provider profile.
    pub fn provider_profile_id(&self) -> &str {
        &self.provider_profile_id
    }

    /// Exact model/routing key.
    pub fn provider_model(&self) -> &str {
        &self.provider_model
    }

    /// Fingerprint of executable, fixed arguments, sandbox, and adapter
    /// registration. Remote adapters use an equivalent immutable deployment
    /// registration fingerprint.
    pub fn registration_fingerprint(&self) -> &str {
        &self.registration_fingerprint
    }

    /// Exact negotiated provider-adapter protocol version.
    pub fn protocol_version(&self) -> &str {
        &self.protocol_version
    }

    /// Exact host-authored retention/rule/provider-policy fingerprint.
    pub fn policy_fingerprint(&self) -> &str {
        &self.policy_fingerprint
    }
}

/// Deployment ceilings for durable provider sessions and cleanup workers.
///
/// These ceilings do not enable provider retention. Current rules, provider
/// capability, egress, scope retention policy, and current-principal access
/// must separately allow every create or resume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiProviderSessionLimits {
    idle_ttl: Duration,
    absolute_lifetime: Duration,
    claim_lease_ttl: Duration,
    cleanup_lease_ttl: Duration,
    maximum_retry_delay: Duration,
    maximum_retries: u32,
    maximum_candidate_scan: usize,
}

impl AiProviderSessionLimits {
    /// Creates validated provider-session hard limits.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] unless all durations are
    /// positive and within compiled ceilings, idle TTL does not exceed the
    /// absolute lifetime, the persisted retry counter saturates at no more
    /// than 100, and candidate scans are in `1..=256`.
    pub fn new(
        idle_ttl: Duration,
        absolute_lifetime: Duration,
        claim_lease_ttl: Duration,
        cleanup_lease_ttl: Duration,
        maximum_retry_delay: Duration,
        maximum_retries: u32,
        maximum_candidate_scan: usize,
    ) -> Result<Self, AiError> {
        if !idle_ttl.is_positive()
            || idle_ttl > MAXIMUM_PROVIDER_SESSION_IDLE_TTL
            || !absolute_lifetime.is_positive()
            || absolute_lifetime > MAXIMUM_PROVIDER_SESSION_LIFETIME
            || idle_ttl > absolute_lifetime
            || !claim_lease_ttl.is_positive()
            || claim_lease_ttl > MAXIMUM_PROVIDER_SESSION_LEASE_TTL
            || !cleanup_lease_ttl.is_positive()
            || cleanup_lease_ttl > MAXIMUM_PROVIDER_SESSION_LEASE_TTL
            || !maximum_retry_delay.is_positive()
            || maximum_retry_delay > MAXIMUM_PROVIDER_SESSION_RETRY_DELAY
            || maximum_retries > 100
            || !(1..=256).contains(&maximum_candidate_scan)
        {
            return Err(AiError::InvalidConfiguration(
                "invalid provider-session limits".to_owned(),
            ));
        }
        Ok(Self {
            idle_ttl,
            absolute_lifetime,
            claim_lease_ttl,
            cleanup_lease_ttl,
            maximum_retry_delay,
            maximum_retries,
            maximum_candidate_scan,
        })
    }

    /// Provider-session idle TTL, separate from a warm-process TTL.
    pub const fn idle_ttl(self) -> Duration {
        self.idle_ttl
    }

    /// Absolute provider-session lifetime.
    pub const fn absolute_lifetime(self) -> Duration {
        self.absolute_lifetime
    }

    /// Run claim lease TTL.
    pub const fn claim_lease_ttl(self) -> Duration {
        self.claim_lease_ttl
    }

    /// Cleanup worker lease TTL.
    pub const fn cleanup_lease_ttl(self) -> Duration {
        self.cleanup_lease_ttl
    }

    /// Maximum rows considered by one cleanup claim.
    pub const fn maximum_candidate_scan(self) -> usize {
        self.maximum_candidate_scan
    }

    pub(crate) const fn maximum_retry_delay(self) -> Duration {
        self.maximum_retry_delay
    }

    pub(crate) const fn maximum_retries(self) -> u32 {
        self.maximum_retries
    }
}

impl Default for AiProviderSessionLimits {
    fn default() -> Self {
        Self {
            idle_ttl: Duration::hours(1),
            absolute_lifetime: Duration::days(7),
            claim_lease_ttl: Duration::minutes(5),
            cleanup_lease_ttl: Duration::minutes(5),
            maximum_retry_delay: Duration::hours(1),
            maximum_retries: 10,
            maximum_candidate_scan: 50,
        }
    }
}

/// Input for binding an already-created empty provider session to a run.
///
/// The provider thread must contain no business content before this request
/// commits. This bounds the crash window between provider creation and durable
/// cursor persistence to an empty orphan. The initiating run may populate the
/// thread only after receiving the returned fenced claim.
pub struct AiProviderSessionBindRequest {
    descriptor: AiProviderSessionDescriptor,
    cursor: AiProviderSessionCursor,
    transcript_fingerprint: String,
    provider_expires_at: Option<OffsetDateTime>,
}

/// Host-planned durable provider-session identity for one provider turn.
///
/// The transcript fingerprint names the authoritative durable prefix before
/// the current input message. The provider executor uses this value only to
/// claim or create an exactly matching binding; it cannot widen provider,
/// tool, egress, or retention policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiProviderSessionTurnPlan {
    descriptor: AiProviderSessionDescriptor,
    transcript_fingerprint: String,
}

impl AiProviderSessionTurnPlan {
    /// Creates one exact provider-session turn binding.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] unless `transcript_fingerprint` is a
    /// canonical lowercase SHA-256 fingerprint of the authoritative durable
    /// message prefix before the current input.
    pub fn new(
        descriptor: AiProviderSessionDescriptor,
        transcript_fingerprint: impl Into<String>,
    ) -> Result<Self, AiError> {
        let plan = Self {
            descriptor,
            transcript_fingerprint: transcript_fingerprint.into(),
        };
        if !crate::valid_sha256(&plan.transcript_fingerprint) {
            return Err(AiError::InvalidInput(
                "invalid provider-session transcript fingerprint".to_owned(),
            ));
        }
        Ok(plan)
    }

    /// Exact immutable provider/runtime identity.
    pub fn descriptor(&self) -> &AiProviderSessionDescriptor {
        &self.descriptor
    }

    /// Canonical authoritative transcript prefix before current input.
    pub fn transcript_fingerprint(&self) -> &str {
        &self.transcript_fingerprint
    }
}

impl AiProviderSessionBindRequest {
    /// Creates a validated bind request for an empty provider session.
    ///
    /// `transcript_fingerprint` identifies the exact durable message prefix
    /// before the initiating user message; an empty prefix still uses a
    /// canonical SHA-256 rather than an empty string.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for a malformed transcript
    /// fingerprint.
    pub fn new(
        descriptor: AiProviderSessionDescriptor,
        cursor: AiProviderSessionCursor,
        transcript_fingerprint: impl Into<String>,
        provider_expires_at: Option<OffsetDateTime>,
    ) -> Result<Self, AiError> {
        let request = Self {
            descriptor,
            cursor,
            transcript_fingerprint: transcript_fingerprint.into(),
            provider_expires_at,
        };
        if !crate::valid_sha256(&request.transcript_fingerprint) {
            return Err(AiError::InvalidInput(
                "invalid provider-session transcript fingerprint".to_owned(),
            ));
        }
        Ok(request)
    }

    /// Exact provider/runtime descriptor.
    pub fn descriptor(&self) -> &AiProviderSessionDescriptor {
        &self.descriptor
    }

    /// Provider-declared expiry, when the protocol supplies one.
    pub const fn provider_expires_at(&self) -> Option<OffsetDateTime> {
        self.provider_expires_at
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        AiProviderSessionDescriptor,
        AiProviderSessionCursor,
        String,
        Option<OffsetDateTime>,
    ) {
        (
            self.descriptor,
            self.cursor,
            self.transcript_fingerprint,
            self.provider_expires_at,
        )
    }
}

impl fmt::Debug for AiProviderSessionBindRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AiProviderSessionBindRequest")
            .field("descriptor", &self.descriptor)
            .field("cursor", &self.cursor)
            .field("transcript_fingerprint", &self.transcript_fingerprint)
            .field("provider_expires_at", &self.provider_expires_at)
            .finish()
    }
}

/// Content-free view of one durable provider-session binding.
///
/// The opaque cursor and protection envelope are intentionally absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiProviderSessionBindingView {
    pub(crate) binding_id: Uuid,
    pub(crate) session_id: AiSessionId,
    pub(crate) scope: AiScope,
    pub(crate) descriptor: AiProviderSessionDescriptor,
    pub(crate) state: AiProviderSessionState,
    pub(crate) through_message_sequence: i64,
    pub(crate) transcript_fingerprint: String,
    pub(crate) provider_expires_at: Option<OffsetDateTime>,
    pub(crate) idle_expires_at: OffsetDateTime,
    pub(crate) absolute_expires_at: OffsetDateTime,
    pub(crate) row_version: i64,
}

/// Crate-issued authority to replace one exact deleted provider-session
/// generation during one exact current run fence.
///
/// Consumers cannot construct or alter this value. It contains no provider
/// cursor and grants no provider egress, tool, GraphQL, or application
/// authority. [`AiProviderSessionService::rebind_for_run`] rehydrates the
/// current principal and atomically revalidates every field before replacing
/// the tombstone.
#[derive(Clone, PartialEq, Eq)]
pub struct AiProviderSessionRebindAuthorization {
    pub(crate) binding_id: Uuid,
    pub(crate) session_id: AiSessionId,
    pub(crate) run_id: AiRunId,
    pub(crate) attempt_id: Uuid,
    pub(crate) run_lease_generation: i64,
    pub(crate) binding_row_version: i64,
    pub(crate) binding_claim_generation: i64,
    pub(crate) cleanup_generation: i64,
    pub(crate) provider_absence_observed_at: OffsetDateTime,
    pub(crate) expires_at: OffsetDateTime,
    pub(crate) principal_reference: PrincipalReference,
    pub(crate) scope: AiScope,
    pub(crate) descriptor: AiProviderSessionDescriptor,
    pub(crate) transcript_fingerprint: String,
}

impl AiProviderSessionRebindAuthorization {
    /// Exact run authorized to replace the deleted generation.
    pub const fn run_id(&self) -> AiRunId {
        self.run_id
    }

    /// Short-lived authorization expiry, additionally bounded by the run
    /// lease expiry.
    pub const fn expires_at(&self) -> OffsetDateTime {
        self.expires_at
    }
}

impl fmt::Debug for AiProviderSessionRebindAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AiProviderSessionRebindAuthorization")
            .field("expires_at", &self.expires_at)
            .field("binding", &"[REDACTED]")
            .field("run_fence", &"[REDACTED]")
            .field("principal", &"[REDACTED]")
            .field("scope", &"[REDACTED]")
            .field("provider_descriptor", &"[REDACTED]")
            .field("transcript", &"[REDACTED]")
            .finish()
    }
}

/// Authoritative disposition of one provider session for a planned run.
///
/// The host may create an empty provider session only for [`Self::New`] or
/// [`Self::RebindAllowed`]. Every other state remains unavailable until its
/// owning lifecycle operation completes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiProviderSessionRunDisposition {
    /// No binding or tombstone exists for the AI session.
    New,
    /// The exact active binding can be claimed and resumed.
    Resume(Box<AiProviderSessionBindingView>),
    /// A confirmed wait was already reclaimed for this exact run fence.
    Reclaimed(Box<AiProviderSessionClaim>),
    /// Exact provider absence was proven and this run may replace the
    /// tombstone once.
    RebindAllowed(Box<AiProviderSessionRebindAuthorization>),
    /// A binding exists but cannot safely serve the planned run.
    Unavailable(AiProviderSessionState),
}

/// Closed durable wait kind that may temporarily park one retained provider
/// session.
///
/// This classification is persistence metadata, not provider, approval, or
/// subscription authority. The owning service validates the exact linked
/// durable wait before it confirms or reclaims a parked cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiProviderSessionWaitKind {
    /// One exact human-approval wait.
    Approval,
    /// One exact bounded replayable-subscription wait.
    Subscription,
}

impl AiProviderSessionWaitKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Subscription => "subscription",
        }
    }

    pub(crate) fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "approval" => Some(Self::Approval),
            "subscription" => Some(Self::Subscription),
            _ => None,
        }
    }
}

/// Exact durable wait identity selected by the host-owned approval or
/// subscription persistence service.
///
/// Constructing this value does not authorize parking or resumption. The
/// provider-session service requires a matching durable wait graph and a
/// provider-turn checkpoint before either transition can commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiProviderSessionWaitIdentity {
    kind: AiProviderSessionWaitKind,
    wait_id: Uuid,
}

impl AiProviderSessionWaitIdentity {
    pub(crate) fn from_parts(
        kind: AiProviderSessionWaitKind,
        wait_id: Uuid,
    ) -> Result<Self, AiError> {
        if wait_id.is_nil() {
            return Err(AiError::InvalidInput(
                "invalid provider-session wait identity".to_owned(),
            ));
        }
        Ok(Self { kind, wait_id })
    }

    /// Identifies one exact durable approval.
    #[allow(dead_code)]
    pub(crate) const fn approval(approval_id: crate::AiApprovalId) -> Self {
        Self {
            kind: AiProviderSessionWaitKind::Approval,
            wait_id: approval_id.0,
        }
    }

    /// Identifies one exact durable subscription waiter.
    ///
    /// The UUID must be allocated by the subscription-wait persistence
    /// service. Passing an arbitrary UUID cannot create authority because the
    /// provider-session service verifies the complete durable waiter graph.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for a nil waiter identity.
    #[allow(dead_code)]
    pub(crate) fn subscription(waiter_id: Uuid) -> Result<Self, AiError> {
        Self::from_parts(AiProviderSessionWaitKind::Subscription, waiter_id)
    }

    /// Closed wait kind.
    pub const fn kind(self) -> AiProviderSessionWaitKind {
        self.kind
    }

    /// Exact approval or waiter identity.
    pub const fn wait_id(self) -> Uuid {
        self.wait_id
    }
}

/// Opaque request to park the provider-retained continuation proven by one
/// exact durable provider-turn checkpoint.
///
/// Only the crate's checkpoint/approval/subscription persistence owners can
/// construct this value. Alternate provider-session stores receive bounded
/// getters for exact validation; application hosts cannot inject a wait,
/// cursor, claim, checkpoint, or continuation fingerprint.
#[derive(Clone, PartialEq, Eq)]
pub struct AiProviderSessionWaitParkRequest {
    pub(crate) claim: AiProviderSessionClaim,
    pub(crate) wait: AiProviderSessionWaitIdentity,
    pub(crate) source_checkpoint_id: Uuid,
    pub(crate) source_checkpoint_fingerprint: String,
    pub(crate) continuation_fingerprint: String,
    pub(crate) request_fingerprint: String,
}

impl AiProviderSessionWaitParkRequest {
    pub(crate) fn new(
        claim: AiProviderSessionClaim,
        wait: AiProviderSessionWaitIdentity,
        source_checkpoint_id: Uuid,
        source_checkpoint_fingerprint: String,
        continuation_fingerprint: String,
    ) -> Result<Self, AiError> {
        let request_fingerprint = wait_park_request_fingerprint(
            &claim,
            wait,
            source_checkpoint_id,
            &source_checkpoint_fingerprint,
            &continuation_fingerprint,
        )?;
        Ok(Self {
            claim,
            wait,
            source_checkpoint_id,
            source_checkpoint_fingerprint,
            continuation_fingerprint,
            request_fingerprint,
        })
    }

    pub(crate) fn has_valid_fingerprint(&self) -> bool {
        wait_park_request_fingerprint(
            &self.claim,
            self.wait,
            self.source_checkpoint_id,
            &self.source_checkpoint_fingerprint,
            &self.continuation_fingerprint,
        )
        .is_ok_and(|fingerprint| fingerprint == self.request_fingerprint)
    }

    /// Exact provider-session binding.
    pub const fn binding_id(&self) -> Uuid {
        self.claim.binding_id
    }

    /// Exact owning AI session.
    pub const fn session_id(&self) -> AiSessionId {
        self.claim.session_id
    }

    /// Exact source run.
    pub const fn run_id(&self) -> AiRunId {
        self.claim.run_id
    }

    /// Exact source attempt.
    pub const fn attempt_id(&self) -> Uuid {
        self.claim.attempt_id
    }

    /// Exact source run lease generation.
    pub const fn run_lease_generation(&self) -> i64 {
        self.claim.run_lease_generation
    }

    /// Exact binding claim generation.
    pub const fn binding_claim_generation(&self) -> i64 {
        self.claim.binding_claim_generation
    }

    /// Exact wait identity whose durable graph must later confirm parking.
    pub const fn wait(&self) -> AiProviderSessionWaitIdentity {
        self.wait
    }

    /// Exact already-durable provider-turn checkpoint.
    pub const fn source_checkpoint_id(&self) -> Uuid {
        self.source_checkpoint_id
    }

    /// Verified hash of the exact source checkpoint.
    pub fn source_checkpoint_fingerprint(&self) -> &str {
        &self.source_checkpoint_fingerprint
    }

    /// Fingerprint of the complete provider-retained continuation.
    pub fn continuation_fingerprint(&self) -> &str {
        &self.continuation_fingerprint
    }

    /// Exact frozen provider descriptor.
    pub const fn descriptor(&self) -> &AiProviderSessionDescriptor {
        &self.claim.descriptor
    }

    /// Exact durable transcript prefix preceding the parked turn.
    pub fn transcript_fingerprint(&self) -> &str {
        &self.claim.transcript_fingerprint
    }
}

fn wait_park_request_fingerprint(
    claim: &AiProviderSessionClaim,
    wait: AiProviderSessionWaitIdentity,
    source_checkpoint_id: Uuid,
    source_checkpoint_fingerprint: &str,
    continuation_fingerprint: &str,
) -> Result<String, AiError> {
    let value = serde_json::json!({
        "format": "graphql-orm-ai/provider-session-wait-park-request/v1",
        "bindingId": claim.binding_id,
        "sessionId": claim.session_id.0,
        "runId": claim.run_id.0,
        "attemptId": claim.attempt_id,
        "runLeaseGeneration": claim.run_lease_generation,
        "bindingClaimGeneration": claim.binding_claim_generation,
        "bindingRowVersion": claim.binding_row_version,
        "throughMessageSequence": claim.through_message_sequence,
        "transcriptFingerprint": claim.transcript_fingerprint,
        "principalReference": claim.principal_reference,
        "descriptor": claim.descriptor,
        "waitKind": wait.kind.as_str(),
        "waitId": wait.wait_id,
        "sourceCheckpointId": source_checkpoint_id,
        "sourceCheckpointFingerprint": source_checkpoint_fingerprint,
        "continuationFingerprint": continuation_fingerprint,
    });
    let bytes = serde_json::to_vec(&value).map_err(|_| AiError::PersistenceFailed)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

impl fmt::Debug for AiProviderSessionWaitParkRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AiProviderSessionWaitParkRequest")
            .field("wait_kind", &self.wait.kind)
            .field("binding", &"[REDACTED]")
            .field("run_fence", &"[REDACTED]")
            .field("checkpoint", &"[REDACTED]")
            .field("continuation", &"[REDACTED]")
            .finish()
    }
}

/// Opaque proof that one exact claimed provider session entered the bounded
/// wait-parking protocol.
///
/// A host cannot construct or alter this proof. It contains no cursor,
/// provider content, tool arguments, or authority. Confirmation revalidates
/// the exact run, wait row, source provider-turn checkpoint, parked
/// checkpoint, continuation fingerprint, owner, scope, descriptor, and
/// transcript before the parked session becomes reclaimable.
#[derive(Clone, PartialEq, Eq)]
pub struct AiProviderSessionParkedWait {
    pub(crate) binding_id: Uuid,
    pub(crate) session_id: AiSessionId,
    pub(crate) source_run_id: AiRunId,
    pub(crate) source_attempt_id: Uuid,
    pub(crate) source_run_lease_generation: i64,
    pub(crate) source_binding_claim_generation: i64,
    pub(crate) park_generation: i64,
    pub(crate) wait: AiProviderSessionWaitIdentity,
    pub(crate) source_checkpoint_id: Uuid,
    pub(crate) source_checkpoint_fingerprint: String,
    pub(crate) continuation_fingerprint: String,
    pub(crate) binding_row_version: i64,
}

impl AiProviderSessionParkedWait {
    /// Exact durable wait identity.
    pub const fn wait(&self) -> AiProviderSessionWaitIdentity {
        self.wait
    }

    /// Exact source run whose provider cursor is parked.
    pub const fn source_run_id(&self) -> AiRunId {
        self.source_run_id
    }
}

impl fmt::Debug for AiProviderSessionParkedWait {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AiProviderSessionParkedWait")
            .field("wait_kind", &self.wait.kind)
            .field("binding", &"[REDACTED]")
            .field("wait", &"[REDACTED]")
            .field("run_fence", &"[REDACTED]")
            .field("checkpoints", &"[REDACTED]")
            .field("continuation", &"[REDACTED]")
            .finish()
    }
}

impl AiProviderSessionBindingView {
    /// Durable binding identity.
    pub const fn binding_id(&self) -> Uuid {
        self.binding_id
    }

    /// Exact owning AI session.
    pub const fn session_id(&self) -> AiSessionId {
        self.session_id
    }

    /// Exact application scope.
    pub const fn scope(&self) -> &AiScope {
        &self.scope
    }

    /// Exact provider/runtime descriptor.
    pub const fn descriptor(&self) -> &AiProviderSessionDescriptor {
        &self.descriptor
    }

    /// Durable lifecycle state.
    pub const fn state(&self) -> AiProviderSessionState {
        self.state
    }

    /// Inclusive durable message prefix represented by provider state.
    pub const fn through_message_sequence(&self) -> i64 {
        self.through_message_sequence
    }

    /// Canonical fingerprint of that durable transcript prefix.
    pub fn transcript_fingerprint(&self) -> &str {
        &self.transcript_fingerprint
    }

    /// Provider-declared expiry, when known.
    pub const fn provider_expires_at(&self) -> Option<OffsetDateTime> {
        self.provider_expires_at
    }

    /// Deployment-owned idle expiry.
    pub const fn idle_expires_at(&self) -> OffsetDateTime {
        self.idle_expires_at
    }

    /// Deployment-owned absolute expiry.
    pub const fn absolute_expires_at(&self) -> OffsetDateTime {
        self.absolute_expires_at
    }

    /// Monotonic compare-and-set version of the durable binding.
    pub const fn row_version(&self) -> i64 {
        self.row_version
    }
}

/// Closed durable provider-session lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiProviderSessionState {
    /// Available for one exact future run claim.
    Active,
    /// Held by one exact run/attempt/generation.
    Claimed,
    /// Exact completed provider continuation is parked while a durable
    /// approval or subscription wait owns coordinator progress.
    ParkedWait,
    /// Never reusable; exact provider deletion is required.
    CleanupRequired,
    /// Held by one cleanup worker.
    CleanupInProgress,
    /// Exact deletion is waiting for bounded retry.
    CleanupBackoff,
    /// Provider absence was confirmed and the cursor was cleared; only the
    /// crate-issued rebind lifecycle may replace this tombstone.
    Deleted,
    /// Restored provider state that must never resume automatically.
    RestoreQuarantined,
}

impl AiProviderSessionState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Claimed => "claimed",
            Self::ParkedWait => "parked_wait",
            Self::CleanupRequired => "cleanup_required",
            Self::CleanupInProgress => "cleanup_in_progress",
            Self::CleanupBackoff => "cleanup_backoff",
            Self::Deleted => "deleted",
            Self::RestoreQuarantined => "restore_quarantined",
        }
    }

    pub(crate) fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "claimed" => Some(Self::Claimed),
            "parked_wait" => Some(Self::ParkedWait),
            "cleanup_required" => Some(Self::CleanupRequired),
            "cleanup_in_progress" => Some(Self::CleanupInProgress),
            "cleanup_backoff" => Some(Self::CleanupBackoff),
            "deleted" => Some(Self::Deleted),
            "restore_quarantined" => Some(Self::RestoreQuarantined),
            _ => None,
        }
    }
}

/// Exact provider-session lease bound to one current run fence.
///
/// Fields are private so a host cannot manufacture resume authority. The
/// claim contains no cursor; opening it rehydrates the current principal and
/// rechecks the binding, session, scope, provider descriptor, and run fence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiProviderSessionClaim {
    pub(crate) binding_id: Uuid,
    pub(crate) session_id: AiSessionId,
    pub(crate) run_id: AiRunId,
    pub(crate) attempt_id: Uuid,
    pub(crate) run_lease_generation: i64,
    pub(crate) binding_claim_generation: i64,
    pub(crate) binding_row_version: i64,
    pub(crate) claim_expires_at: OffsetDateTime,
    pub(crate) through_message_sequence: i64,
    pub(crate) transcript_fingerprint: String,
    pub(crate) principal_reference: PrincipalReference,
    pub(crate) descriptor: AiProviderSessionDescriptor,
}

impl AiProviderSessionClaim {
    /// Durable binding identity.
    pub const fn binding_id(&self) -> Uuid {
        self.binding_id
    }

    /// Owning AI session.
    pub const fn session_id(&self) -> AiSessionId {
        self.session_id
    }

    /// Exact run holding the binding.
    pub const fn run_id(&self) -> AiRunId {
        self.run_id
    }

    /// Exact run attempt holding the binding.
    pub const fn attempt_id(&self) -> Uuid {
        self.attempt_id
    }

    /// Exact run fencing generation.
    pub const fn run_lease_generation(&self) -> i64 {
        self.run_lease_generation
    }

    /// Monotonic provider-session claim generation.
    pub const fn binding_claim_generation(&self) -> i64 {
        self.binding_claim_generation
    }

    /// Claim expiry.
    pub const fn claim_expires_at(&self) -> OffsetDateTime {
        self.claim_expires_at
    }

    /// Durable transcript watermark before the current run.
    pub const fn through_message_sequence(&self) -> i64 {
        self.through_message_sequence
    }

    pub(crate) fn transcript_fingerprint(&self) -> &str {
        &self.transcript_fingerprint
    }

    /// Exact provider/runtime descriptor.
    pub const fn descriptor(&self) -> &AiProviderSessionDescriptor {
        &self.descriptor
    }
}

/// Authorized cursor opened for one exact provider-session claim.
#[derive(Clone)]
pub struct AiOpenedProviderSession {
    claim: AiProviderSessionClaim,
    cursor: AiProviderSessionCursor,
    activation: AiProviderSessionActivation,
}

impl AiOpenedProviderSession {
    pub(crate) fn new(claim: AiProviderSessionClaim, cursor: AiProviderSessionCursor) -> Self {
        Self {
            claim,
            cursor,
            activation: AiProviderSessionActivation::ExistingRetained,
        }
    }

    pub(crate) fn activate_newly_bound_empty(
        mut self,
        binding: crate::AiProviderRunBinding,
        created_cursor: &AiProviderSessionCursor,
    ) -> Result<Self, AiError> {
        if self.claim.session_id != binding.session_id()
            || self.claim.run_id != binding.run_id()
            || self.claim.attempt_id != binding.attempt_id()
            || self.claim.run_lease_generation != binding.lease_generation()
            || !binding.matches_principal_reference(&self.claim.principal_reference)
            || self.cursor != *created_cursor
        {
            return Err(AiError::Conflict);
        }
        self.activation = AiProviderSessionActivation::NewlyBoundEmpty;
        Ok(self)
    }

    /// Exact fenced claim receiving provider transport.
    pub const fn claim(&self) -> &AiProviderSessionClaim {
        &self.claim
    }

    /// Opaque cursor for the trusted provider adapter.
    pub const fn cursor(&self) -> &AiProviderSessionCursor {
        &self.cursor
    }

    #[cfg_attr(
        not(any(test, feature = "provider-codex-app-server")),
        allow(dead_code)
    )]
    pub(crate) const fn activation(&self) -> AiProviderSessionActivation {
        self.activation
    }
}

impl fmt::Debug for AiOpenedProviderSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AiOpenedProviderSession")
            .field("claim", &self.claim)
            .field("cursor", &self.cursor)
            .field("activation", &self.activation)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AiProviderSessionActivation {
    NewlyBoundEmpty,
    ExistingRetained,
}

/// Exact durable assistant-output proof used to advance a provider session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiProviderSessionCommit {
    assistant_message_id: Uuid,
    through_message_sequence: i64,
    transcript_fingerprint: String,
}

impl AiProviderSessionCommit {
    /// Creates a commit proof supplied after ordinary protected assistant
    /// output persistence.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for a nil message, non-positive
    /// watermark, or malformed transcript fingerprint.
    pub fn new(
        assistant_message_id: Uuid,
        through_message_sequence: i64,
        transcript_fingerprint: impl Into<String>,
    ) -> Result<Self, AiError> {
        let commit = Self {
            assistant_message_id,
            through_message_sequence,
            transcript_fingerprint: transcript_fingerprint.into(),
        };
        if commit.assistant_message_id.is_nil()
            || commit.through_message_sequence <= 0
            || !crate::valid_sha256(&commit.transcript_fingerprint)
        {
            return Err(AiError::InvalidInput(
                "invalid provider-session commit".to_owned(),
            ));
        }
        Ok(commit)
    }

    /// Durable assistant message that closed the provider turn.
    pub const fn assistant_message_id(&self) -> Uuid {
        self.assistant_message_id
    }

    /// Inclusive durable message prefix represented after the turn.
    pub const fn through_message_sequence(&self) -> i64 {
        self.through_message_sequence
    }

    /// Canonical durable transcript-prefix fingerprint.
    pub fn transcript_fingerprint(&self) -> &str {
        &self.transcript_fingerprint
    }
}

/// Fenced cleanup claim containing no opened provider cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiProviderSessionCleanupClaim {
    pub(crate) binding_id: Uuid,
    pub(crate) session_id: AiSessionId,
    pub(crate) scope: AiScope,
    pub(crate) descriptor: AiProviderSessionDescriptor,
    pub(crate) cleanup_worker_id: String,
    pub(crate) cleanup_generation: i64,
    pub(crate) cleanup_expires_at: OffsetDateTime,
    pub(crate) row_version: i64,
}

impl AiProviderSessionCleanupClaim {
    /// Binding selected for cleanup.
    pub const fn binding_id(&self) -> Uuid {
        self.binding_id
    }

    /// Owning session.
    pub const fn session_id(&self) -> AiSessionId {
        self.session_id
    }

    /// Owning application scope.
    pub const fn scope(&self) -> &AiScope {
        &self.scope
    }

    /// Exact provider/runtime descriptor.
    pub const fn descriptor(&self) -> &AiProviderSessionDescriptor {
        &self.descriptor
    }

    /// Cleanup worker owner.
    pub fn cleanup_worker_id(&self) -> &str {
        &self.cleanup_worker_id
    }

    /// Monotonic cleanup generation.
    pub const fn cleanup_generation(&self) -> i64 {
        self.cleanup_generation
    }

    /// Cleanup lease expiry.
    pub const fn cleanup_expires_at(&self) -> OffsetDateTime {
        self.cleanup_expires_at
    }
}

/// Exact deletion request passed only to a registered provider adapter.
pub struct AiProviderSessionDeletionRequest {
    claim: AiProviderSessionCleanupClaim,
    cursor: AiProviderSessionCursor,
}

impl AiProviderSessionDeletionRequest {
    pub(crate) fn new(
        claim: AiProviderSessionCleanupClaim,
        cursor: AiProviderSessionCursor,
    ) -> Self {
        Self { claim, cursor }
    }

    /// Exact cleanup claim.
    pub const fn claim(&self) -> &AiProviderSessionCleanupClaim {
        &self.claim
    }

    /// Opaque provider cursor.
    pub const fn cursor(&self) -> &AiProviderSessionCursor {
        &self.cursor
    }
}

impl fmt::Debug for AiProviderSessionDeletionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AiProviderSessionDeletionRequest")
            .field("claim", &self.claim)
            .field("cursor", &self.cursor)
            .finish()
    }
}

/// Provider adapter proof that the exact retained session is absent.
///
/// The trusted adapter may return this value only after a deletion request or
/// an authoritative provider lookup proves absence. Expiry alone is not
/// absence unless the immutable provider contract defines and verifies it as
/// such.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiProviderSessionAbsenceProof {
    binding_id: Uuid,
    cursor_fingerprint: String,
    observed_at: OffsetDateTime,
}

impl AiProviderSessionAbsenceProof {
    /// Creates an authoritative provider-absence observation for the exact
    /// deletion request handled by a trusted provider adapter.
    pub fn for_request(
        request: &AiProviderSessionDeletionRequest,
        observed_at: OffsetDateTime,
    ) -> Self {
        Self {
            binding_id: request.claim.binding_id,
            cursor_fingerprint: request.cursor.fingerprint(),
            observed_at,
        }
    }

    /// Trusted observation timestamp.
    pub const fn observed_at(&self) -> OffsetDateTime {
        self.observed_at
    }

    pub(crate) const fn binding_id(&self) -> Uuid {
        self.binding_id
    }

    pub(crate) fn cursor_fingerprint(&self) -> &str {
        &self.cursor_fingerprint
    }
}

/// Provider-specific exact deletion/absence boundary.
///
/// Implementations receive no principal credential, application delegated
/// authority, GraphQL target, shell, filesystem, browser, or arbitrary URL.
#[async_trait]
pub trait AiProviderSessionDeletionService: Send + Sync {
    /// Deletes the exact provider session or authoritatively proves it absent.
    async fn delete_or_confirm_absent(
        &self,
        request: &AiProviderSessionDeletionRequest,
    ) -> Result<AiProviderSessionAbsenceProof, ProviderError>;
}

/// Durable provider-session lifecycle owned by the AI persistence boundary.
///
/// None of these operations grants provider egress or application-tool
/// authority. Provider transport still requires current rules, egress,
/// budgets, and coordinator fencing.
#[async_trait]
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait AiProviderSessionService: Send + Sync {
    /// Returns the current owner-visible binding shell, when one exists.
    ///
    /// Current principal, session, scope, and protection readiness are checked
    /// even though the cursor remains unopened. This method distinguishes a
    /// genuinely new session from a stale or incompatible binding; callers
    /// must never treat a mismatched existing row as permission to replace it.
    async fn inspect_for_run(
        &self,
        lease: &AiRunLease,
    ) -> Result<Option<AiProviderSessionBindingView>, AiError>;

    /// Classifies the exact planned provider session without exposing cursor
    /// material or requiring the host to infer replacement authority.
    ///
    /// The default preserves compatibility for alternate stores but never
    /// issues rebind authority. Durable stores that preserve deleted
    /// tombstones override this method and return a crate-owned
    /// [`AiProviderSessionRebindAuthorization`] only after validating exact
    /// provider absence and the current run fence.
    ///
    /// # Errors
    ///
    /// Returns an error when the run fence, current principal/session access,
    /// provider descriptor, transcript prefix, or stored lifecycle evidence
    /// cannot be validated exactly.
    async fn disposition_for_run(
        &self,
        lease: &AiRunLease,
        planned: &AiProviderSessionTurnPlan,
    ) -> Result<AiProviderSessionRunDisposition, AiError> {
        match self.inspect_for_run(lease).await? {
            None => Ok(AiProviderSessionRunDisposition::New),
            Some(binding)
                if binding.state() == AiProviderSessionState::Active
                    && binding.descriptor() == planned.descriptor()
                    && binding.transcript_fingerprint() == planned.transcript_fingerprint() =>
            {
                Ok(AiProviderSessionRunDisposition::Resume(Box::new(binding)))
            }
            Some(binding) => Ok(AiProviderSessionRunDisposition::Unavailable(
                binding.state(),
            )),
        }
    }

    /// Binds an already-created empty provider session and claims it for the
    /// exact current run.
    async fn bind_for_run(
        &self,
        lease: &AiRunLease,
        request: AiProviderSessionBindRequest,
    ) -> Result<AiProviderSessionClaim, AiError>;

    /// Replaces one exact deleted tombstone with an already-created empty
    /// provider session and claims it for the authorized run.
    ///
    /// The default denies so alternate stores do not acquire replacement
    /// semantics implicitly.
    ///
    /// # Errors
    ///
    /// Returns an error when current authority, the run fence, the
    /// crate-issued authorization, deleted generation, provider descriptor,
    /// transcript prefix, or protected replacement cursor is not exact.
    async fn rebind_for_run(
        &self,
        _lease: &AiRunLease,
        _authorization: AiProviderSessionRebindAuthorization,
        _request: AiProviderSessionBindRequest,
    ) -> Result<AiProviderSessionClaim, AiError> {
        Err(AiError::RuntimeNotReady)
    }

    /// Claims an existing active binding for an exact current run, expected
    /// provider/runtime identity, and host-planned canonical transcript.
    ///
    /// The transcript fingerprint must be derived from the same authoritative
    /// durable message/context watermark used to build the next provider
    /// request. A mismatch makes the retained cursor ineligible; it never
    /// causes the service to replay or infer missing content.
    async fn claim_for_run(
        &self,
        lease: &AiRunLease,
        expected: &AiProviderSessionDescriptor,
        expected_transcript_fingerprint: &str,
    ) -> Result<AiProviderSessionClaim, AiError>;

    /// Rehydrates current owner authority and opens the protected cursor for
    /// the exact still-current run claim.
    async fn open_for_run(
        &self,
        lease: &AiRunLease,
        claim: &AiProviderSessionClaim,
    ) -> Result<AiOpenedProviderSession, AiError>;

    /// Renews the provider-session claim under the exact current run fence.
    async fn heartbeat(
        &self,
        lease: &AiRunLease,
        claim: &AiProviderSessionClaim,
    ) -> Result<AiProviderSessionClaim, AiError>;

    /// Parks an exact completed provider-retained turn before the owning run
    /// releases its coordinator lease for a durable wait.
    ///
    /// The default denies. Implementations must require the exact current
    /// claim and already-durable `provider_turn_persisted` checkpoint and must
    /// fingerprint the complete provider-retained continuation. In-flight
    /// streams and synchronous dynamic-tool responders are not suspendable.
    async fn park_for_wait(
        &self,
        _lease: &AiRunLease,
        _request: AiProviderSessionWaitParkRequest,
    ) -> Result<AiProviderSessionParkedWait, AiError> {
        Err(AiError::RuntimeNotReady)
    }

    /// Confirms that the run lease was released into the exact durable wait
    /// graph after [`Self::park_for_wait`].
    ///
    /// Confirmation is idempotent only for the same opaque proof. It grants
    /// no provider authority and exposes no cursor.
    async fn confirm_parked_wait(
        &self,
        _parked: &AiProviderSessionParkedWait,
    ) -> Result<(), AiError> {
        Err(AiError::RuntimeNotReady)
    }

    /// Quarantines one exact parked cursor after its durable wait handoff is
    /// known to have failed.
    async fn require_parked_wait_cleanup(
        &self,
        _parked: &AiProviderSessionParkedWait,
        _reason_code: &str,
    ) -> Result<(), AiError> {
        Err(AiError::RuntimeNotReady)
    }

    /// Quarantines the exact cursor when a wait-parking handoff is known to
    /// have failed but the caller cannot tell whether parking committed.
    ///
    /// The crate-owned request binds both the original claim and intended
    /// wait graph. Implementations may accept only the exact still-claimed
    /// binding or the exact unconfirmed parked binding produced by that
    /// request. This is a convergence boundary, not a generic state reset.
    async fn require_wait_handoff_cleanup(
        &self,
        _request: &AiProviderSessionWaitParkRequest,
        _reason_code: &str,
    ) -> Result<(), AiError> {
        Err(AiError::RuntimeNotReady)
    }

    /// Reclaims one confirmed parked cursor for the exact fresh run fence
    /// after its one-shot approval or subscription adoption remains linked.
    ///
    /// The service derives its crate-private authorization from durable state;
    /// callers cannot manufacture or replay it. After this transition, any
    /// failure before provider continuation must route the returned claim to
    /// [`Self::require_cleanup`].
    async fn reclaim_after_wait(
        &self,
        _lease: &AiRunLease,
    ) -> Result<AiProviderSessionClaim, AiError> {
        Err(AiError::RuntimeNotReady)
    }

    /// Advances the durable watermark only after exact protected assistant
    /// output persistence and canonical terminal run completion, then releases
    /// the binding for a future run.
    async fn commit_turn(
        &self,
        lease: &AiRunLease,
        claim: &AiProviderSessionClaim,
        commit: AiProviderSessionCommit,
    ) -> Result<AiProviderSessionBindingView, AiError>;

    /// Irreversibly removes a claim from reuse after cancellation, transport
    /// ambiguity, cursor rejection, policy drift, or another safe reason.
    async fn require_cleanup(
        &self,
        claim: &AiProviderSessionClaim,
        reason_code: &str,
    ) -> Result<(), AiError>;

    /// Claims one expired, invalidated, or deletion-required binding.
    async fn claim_cleanup(
        &self,
        worker_id: &str,
    ) -> Result<Option<AiProviderSessionCleanupClaim>, AiError>;

    /// Opens a cleanup cursor under an exact current maintenance protection
    /// policy. This does not grant provider deletion; the registered adapter
    /// still owns that boundary.
    async fn open_for_cleanup(
        &self,
        claim: &AiProviderSessionCleanupClaim,
        policy: &crate::AiContentProtectionPolicy,
    ) -> Result<AiProviderSessionDeletionRequest, AiError>;

    /// Records exact provider absence, clears protected cursor material, and
    /// retains a private deleted-generation tombstone eligible only for the
    /// fenced disposition/rebind lifecycle.
    async fn complete_cleanup(
        &self,
        claim: &AiProviderSessionCleanupClaim,
        proof: AiProviderSessionAbsenceProof,
    ) -> Result<(), AiError>;

    /// Releases cleanup under bounded retry backoff.
    async fn schedule_cleanup_retry(
        &self,
        claim: &AiProviderSessionCleanupClaim,
        delay: Duration,
        reason_code: &str,
    ) -> Result<(), AiError>;
}

pub(crate) fn provider_kind_value(kind: &ProviderKind) -> Result<String, AiError> {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or(AiError::PersistenceFailed)
}

pub(crate) fn parse_provider_kind(value: &str) -> Result<ProviderKind, AiError> {
    serde_json::from_value(serde_json::Value::String(value.to_owned()))
        .map_err(|_| AiError::PersistenceFailed)
}

fn valid_identifier(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAXIMUM_IDENTIFIER_BYTES
        && !value.chars().any(char::is_control)
}

fn valid_namespaced_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAXIMUM_IDENTIFIER_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b'-' | b'/')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(marker: char) -> String {
        marker.to_string().repeat(64)
    }

    fn descriptor() -> AiProviderSessionDescriptor {
        AiProviderSessionDescriptor::new(
            ProviderKind::LocalHarness,
            "local-reviewed",
            "reviewed-model",
            sha('a'),
            "codex-app-server/v1",
            sha('b'),
        )
        .expect("descriptor should validate")
    }

    #[test]
    fn opaque_cursor_is_bounded_and_redacted() {
        let cursor = AiProviderSessionCursor::new("codex.thread", "thread-secret")
            .expect("cursor should validate");
        assert_eq!(cursor.kind(), "codex.thread");
        assert_eq!(cursor.expose_to_provider_adapter(), "thread-secret");
        assert!(!format!("{cursor:?}").contains("thread-secret"));
        assert!(AiProviderSessionCursor::new("Bad Kind", "cursor").is_err());
        assert!(AiProviderSessionCursor::new("codex.thread", "bad\nvalue").is_err());
    }

    #[test]
    fn descriptor_binds_every_runtime_identity() {
        let base = descriptor();
        assert_eq!(base.provider_kind(), &ProviderKind::LocalHarness);
        assert_eq!(base.provider_profile_id(), "local-reviewed");
        assert_eq!(base.provider_model(), "reviewed-model");
        assert_eq!(base.registration_fingerprint(), sha('a'));
        assert_eq!(base.protocol_version(), "codex-app-server/v1");
        assert_eq!(base.policy_fingerprint(), sha('b'));
        assert!(
            AiProviderSessionDescriptor::new(
                ProviderKind::LocalHarness,
                "local-reviewed",
                "reviewed-model",
                "not-a-sha",
                "codex-app-server/v1",
                sha('b'),
            )
            .is_err()
        );
    }

    #[test]
    fn limits_keep_provider_retention_separate_and_bounded() {
        let limits = AiProviderSessionLimits::new(
            Duration::minutes(30),
            Duration::hours(12),
            Duration::minutes(2),
            Duration::minutes(3),
            Duration::minutes(30),
            5,
            20,
        )
        .expect("limits should validate");
        assert_eq!(limits.idle_ttl(), Duration::minutes(30));
        assert_eq!(limits.absolute_lifetime(), Duration::hours(12));
        assert!(
            AiProviderSessionLimits::new(
                Duration::hours(13),
                Duration::hours(12),
                Duration::minutes(2),
                Duration::minutes(3),
                Duration::minutes(30),
                5,
                20,
            )
            .is_err()
        );
    }

    #[test]
    fn bind_and_commit_inputs_reject_unbound_evidence() {
        let cursor = AiProviderSessionCursor::new("codex.thread", "thread-1")
            .expect("cursor should validate");
        assert!(
            AiProviderSessionBindRequest::new(descriptor(), cursor, "not-a-fingerprint", None,)
                .is_err()
        );
        assert!(AiProviderSessionCommit::new(Uuid::nil(), 1, sha('c')).is_err());
        assert!(AiProviderSessionCommit::new(Uuid::new_v4(), 0, sha('c')).is_err());
    }

    #[test]
    fn lifecycle_state_is_closed() {
        for (state, persisted) in [
            (AiProviderSessionState::Active, "active"),
            (AiProviderSessionState::Claimed, "claimed"),
            (AiProviderSessionState::CleanupRequired, "cleanup_required"),
            (
                AiProviderSessionState::CleanupInProgress,
                "cleanup_in_progress",
            ),
            (AiProviderSessionState::CleanupBackoff, "cleanup_backoff"),
            (AiProviderSessionState::Deleted, "deleted"),
            (
                AiProviderSessionState::RestoreQuarantined,
                "restore_quarantined",
            ),
        ] {
            assert_eq!(state.as_str(), persisted);
            assert_eq!(
                AiProviderSessionState::from_persisted(persisted),
                Some(state)
            );
        }
        assert_eq!(AiProviderSessionState::from_persisted("unknown"), None);
    }

    #[test]
    fn cursor_fingerprint_changes_with_kind_or_value() {
        let first = AiProviderSessionCursor::new("codex.thread", "thread-1")
            .expect("cursor should validate");
        let second = AiProviderSessionCursor::new("codex.thread", "thread-2")
            .expect("cursor should validate");
        let third = AiProviderSessionCursor::new("other.thread", "thread-1")
            .expect("cursor should validate");
        assert_ne!(first.fingerprint(), second.fingerprint());
        assert_ne!(first.fingerprint(), third.fingerprint());
        assert_eq!(first.fingerprint().len(), 64);
    }

    #[test]
    fn absence_proof_is_bound_to_the_exact_cleanup_request() {
        let binding_id = Uuid::new_v4();
        let cursor = AiProviderSessionCursor::new("codex.thread", "thread-1")
            .expect("cursor should validate");
        let expected_cursor_fingerprint = cursor.fingerprint();
        let request = AiProviderSessionDeletionRequest::new(
            AiProviderSessionCleanupClaim {
                binding_id,
                session_id: AiSessionId(Uuid::new_v4()),
                scope: AiScope::new("workspace", "default"),
                descriptor: descriptor(),
                cleanup_worker_id: "cleanup-1".to_owned(),
                cleanup_generation: 2,
                cleanup_expires_at: OffsetDateTime::UNIX_EPOCH + Duration::hours(1),
                row_version: 4,
            },
            cursor,
        );
        let proof = AiProviderSessionAbsenceProof::for_request(
            &request,
            OffsetDateTime::UNIX_EPOCH + Duration::minutes(30),
        );
        assert_eq!(proof.binding_id(), binding_id);
        assert_eq!(proof.cursor_fingerprint(), expected_cursor_fingerprint);
    }
}
