//! Provider-neutral lifecycle for one exactly fenced provider run.

use uuid::Uuid;

use sha2::{Digest, Sha256};

#[cfg(any(feature = "sqlite", feature = "postgres"))]
use crate::{AiError, AiRunLease};
use crate::{AiRunId, AiSessionId};

/// Exact non-persistent identity of one claimed provider run.
///
/// Construction is crate-owned so a host, provider adapter, or model cannot
/// manufacture a binding for another attempt or lease generation. The value
/// carries no provider, egress, budget, tool, or persistence authority. Those
/// proofs remain mandatory for every individual provider turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AiProviderRunBinding {
    session_id: AiSessionId,
    run_id: AiRunId,
    attempt_id: Uuid,
    lease_generation: i64,
    owner_fingerprint: [u8; 32],
}

impl AiProviderRunBinding {
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn new(
        session_id: AiSessionId,
        run_id: AiRunId,
        attempt_id: Uuid,
        lease_generation: i64,
        owner_fingerprint: [u8; 32],
    ) -> Result<Self, AiError> {
        if session_id.0.is_nil()
            || run_id.0.is_nil()
            || attempt_id.is_nil()
            || lease_generation <= 0
        {
            return Err(AiError::Conflict);
        }
        Ok(Self {
            session_id,
            run_id,
            attempt_id,
            lease_generation,
            owner_fingerprint,
        })
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn from_lease(lease: &AiRunLease) -> Result<Self, AiError> {
        Self::new(
            lease.session_id(),
            lease.run_id(),
            lease.attempt_id(),
            lease.lease_generation(),
            provider_run_owner_fingerprint(lease.principal_reference()),
        )
    }

    #[cfg(all(
        test,
        feature = "provider-codex-app-server",
        any(feature = "sqlite", feature = "postgres")
    ))]
    pub(crate) fn new_for_principal_reference(
        session_id: AiSessionId,
        run_id: AiRunId,
        attempt_id: Uuid,
        lease_generation: i64,
        reference: &agql_auth::PrincipalReference,
    ) -> Result<Self, AiError> {
        Self::new(
            session_id,
            run_id,
            attempt_id,
            lease_generation,
            provider_run_owner_fingerprint(reference),
        )
    }

    #[cfg_attr(feature = "mssql", allow(dead_code))]
    pub(crate) fn matches_principal_reference(
        self,
        reference: &agql_auth::PrincipalReference,
    ) -> bool {
        self.owner_fingerprint == provider_run_owner_fingerprint(reference)
    }

    /// Owning durable AI session.
    pub const fn session_id(self) -> AiSessionId {
        self.session_id
    }

    /// Durable AI run.
    pub const fn run_id(self) -> AiRunId {
        self.run_id
    }

    /// Exact current worker attempt.
    pub const fn attempt_id(self) -> Uuid {
        self.attempt_id
    }

    /// Monotonic lease fencing generation.
    pub const fn lease_generation(self) -> i64 {
        self.lease_generation
    }

    #[cfg(feature = "provider-codex-app-server")]
    pub(crate) const fn owner_fingerprint(self) -> [u8; 32] {
        self.owner_fingerprint
    }
}

#[cfg_attr(feature = "mssql", allow(dead_code))]
fn provider_run_owner_fingerprint(reference: &agql_auth::PrincipalReference) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"graphql-orm-ai/provider-run-owner/v1\0");
    match &reference.kind {
        agql_auth::PrincipalReferenceKind::UserSession => digest.update(b"user_session\0"),
        agql_auth::PrincipalReferenceKind::ApiToken { principal_kind } => {
            digest.update(b"api_token\0");
            digest.update((principal_kind.len() as u64).to_be_bytes());
            digest.update(principal_kind.as_bytes());
        }
    }
    digest.update((reference.subject.len() as u64).to_be_bytes());
    digest.update(reference.subject.as_bytes());
    digest.finalize().into()
}

/// Why a run-scoped provider resource is being closed.
///
/// This value is lifecycle metadata only. It must not be used to infer a
/// durable run outcome; the fenced run service remains authoritative.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiProviderRunCloseReason {
    /// The fenced run completed successfully.
    Completed,
    /// The fenced run failed safely.
    Failed,
    /// Owner cancellation won the durable fence.
    Cancelled,
    /// The run entered privileged recovery.
    RecoveryRequired,
    /// The worker lost its durable lease.
    LeaseLost,
    /// The managed worker or deployment is shutting down.
    Shutdown,
    /// The provider transport violated its reviewed protocol.
    ProtocolViolation,
}

/// Result of a bounded provider-run interruption request.
///
/// [`Self::RequestedSettled`] is an adapter-level proof, not an
/// acknowledgement: only an adapter that can show its interrupt leaves the
/// retained provider thread consistent with the durable transcript may report
/// it. Every other adapter keeps the fail-closed
/// [`Self::Requested`]/[`Self::NotActive`] pair, and an unrecognized value is
/// treated as unsettled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiProviderRunInterruptOutcome {
    /// No live resource existed for the exact binding.
    NotActive,
    /// An active provider turn accepted the interruption request.
    Requested,
    /// An active provider turn accepted the interruption request, and the
    /// adapter proved its own settlement conditions for that exact turn.
    ///
    /// An adapter may return this only when all of the following hold:
    ///
    /// 1. the provider acknowledged the interrupt for the exact fenced turn;
    /// 2. no dynamic tool call for that turn is unresolved; and
    /// 3. the adapter has version-observed evidence that the interrupted
    ///    partial turn is discarded from the provider payload, the provider's
    ///    durable thread artifact, and the model's later context.
    ///
    /// This still proves nothing about the *caller's* persistence. The durable
    /// leg — that no output of the interrupted turn was persisted, or may have
    /// been persisted uncertainly — belongs to the caller and is applied
    /// through [`AiRunInterruptSettlement::with_durable_turn_evidence`].
    RequestedSettled,
}

/// What an interrupt request proved about the provider turn it stopped.
///
/// This is deliberately separate from [`AiProviderRunInterruptOutcome`], which
/// reports one adapter's view of one resource. Settlement is a stronger claim:
/// that the provider's retained thread, after interruption, is consistent with
/// the durable transcript.
///
/// [`Self::Settled`] requires three independent legs, and anything short of
/// all three stays [`Self::RequestedUnsettled`]:
///
/// 1. the interrupt was acknowledged by the provider;
/// 2. the adapter proved no unresolved dynamic tool call for that turn and
///    version-observed discard of the interrupted partial turn
///    ([`AiProviderRunInterruptOutcome::RequestedSettled`]); and
/// 3. the caller proved from committed durable rows that the interrupted turn
///    left no uncertain persisted output
///    ([`Self::with_durable_turn_evidence`]).
///
/// Acknowledgement alone is never settlement. Treating it as settlement would
/// let the model carry content the durable transcript never recorded, which is
/// the divergence the retained-session disclosure events exist to expose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiRunInterruptSettlement {
    /// No live provider resource existed for the exact fence.
    NotActive,
    /// Interruption was requested and acknowledged, but the retained thread's
    /// post-interrupt content is not proven to match the durable transcript.
    RequestedUnsettled,
    /// The provider proved the interrupted turn left its retained thread
    /// consistent with the durable transcript, with no unresolved dynamic tool
    /// call and no uncertain persisted output.
    Settled,
}

impl AiRunInterruptSettlement {
    /// Returns whether the retained thread may be kept bound.
    ///
    /// Anything other than proven settlement is false, so an unrecognized or
    /// merely acknowledged interruption fails closed into invalidation.
    ///
    /// A true value permits retention; it does not perform it. The durable
    /// provider-session boundary re-proves the same conditions from committed
    /// rows before it keeps a binding, so a caller that skips
    /// [`Self::with_durable_turn_evidence`] still cannot retain a thread whose
    /// turn persisted output.
    pub const fn retains_thread(self) -> bool {
        matches!(self, Self::Settled)
    }

    /// Applies the caller-owned durable leg of the settlement guard.
    ///
    /// `no_uncertain_persisted_output` must be derived from committed durable
    /// state for the exact interrupted turn: no assistant output, tool result,
    /// or turn checkpoint was persisted, and none may have been persisted
    /// uncertainly. Anything else demotes provider-proven settlement to
    /// [`Self::RequestedUnsettled`], so an adapter can never widen retention
    /// past what the caller's transcript can reproduce.
    #[must_use]
    pub const fn with_durable_turn_evidence(self, no_uncertain_persisted_output: bool) -> Self {
        match self {
            Self::Settled if !no_uncertain_persisted_output => Self::RequestedUnsettled,
            other => other,
        }
    }

    /// Folds one adapter outcome into an aggregate settlement.
    ///
    /// Aggregation is fail-closed: a single acknowledged-but-unsettled adapter
    /// keeps the whole interruption unsettled even when another adapter proved
    /// its own resource settled.
    pub(crate) const fn fold_provider_outcome(
        self,
        outcome: AiProviderRunInterruptOutcome,
    ) -> Self {
        match (self, outcome) {
            (_, AiProviderRunInterruptOutcome::Requested) | (Self::RequestedUnsettled, _) => {
                Self::RequestedUnsettled
            }
            (Self::NotActive, AiProviderRunInterruptOutcome::RequestedSettled)
            | (Self::Settled, _) => Self::Settled,
            (Self::NotActive, AiProviderRunInterruptOutcome::NotActive) => Self::NotActive,
        }
    }
}

/// Result of closing one exact provider-run resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiProviderRunCloseOutcome {
    /// No resource existed for the exact binding.
    NotActive,
    /// The exact resource was removed and bounded shutdown was attempted.
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_binding_rejects_nil_or_nonpositive_fence_members() {
        let session_id = AiSessionId::new();
        let run_id = AiRunId::new();
        let attempt_id = Uuid::new_v4();

        let owner = [1; 32];
        assert!(AiProviderRunBinding::new(session_id, run_id, attempt_id, 1, owner).is_ok());
        assert!(
            AiProviderRunBinding::new(AiSessionId(Uuid::nil()), run_id, attempt_id, 1, owner)
                .is_err()
        );
        assert!(
            AiProviderRunBinding::new(session_id, AiRunId(Uuid::nil()), attempt_id, 1, owner)
                .is_err()
        );
        assert!(AiProviderRunBinding::new(session_id, run_id, Uuid::nil(), 1, owner).is_err());
        assert!(AiProviderRunBinding::new(session_id, run_id, attempt_id, 0, owner).is_err());
    }

    #[test]
    fn attempt_and_generation_participate_in_identity() {
        let session_id = AiSessionId::new();
        let run_id = AiRunId::new();
        let attempt_id = Uuid::new_v4();
        let binding = AiProviderRunBinding::new(session_id, run_id, attempt_id, 1, [1; 32])
            .expect("test binding should validate");
        let later_attempt =
            AiProviderRunBinding::new(session_id, run_id, Uuid::new_v4(), 1, [1; 32])
                .expect("later attempt should validate");
        let later_generation =
            AiProviderRunBinding::new(session_id, run_id, attempt_id, 2, [1; 32])
                .expect("later generation should validate");
        let another_owner = AiProviderRunBinding::new(session_id, run_id, attempt_id, 1, [2; 32])
            .expect("another owner binding should validate");

        assert_ne!(binding, later_attempt);
        assert_ne!(binding, later_generation);
        assert_ne!(binding, another_owner);
    }

    #[test]
    fn settlement_requires_acknowledgement_adapter_proof_and_durable_evidence() {
        let settled = AiRunInterruptSettlement::NotActive
            .fold_provider_outcome(AiProviderRunInterruptOutcome::RequestedSettled)
            .with_durable_turn_evidence(true);
        assert_eq!(settled, AiRunInterruptSettlement::Settled);
        assert!(settled.retains_thread());

        // Acknowledgement without adapter proof is never settlement.
        let acknowledged = AiRunInterruptSettlement::NotActive
            .fold_provider_outcome(AiProviderRunInterruptOutcome::Requested)
            .with_durable_turn_evidence(true);
        assert_eq!(acknowledged, AiRunInterruptSettlement::RequestedUnsettled);
        assert!(!acknowledged.retains_thread());

        // Uncertain persisted output demotes an adapter-proven settlement.
        let uncertain = AiRunInterruptSettlement::NotActive
            .fold_provider_outcome(AiProviderRunInterruptOutcome::RequestedSettled)
            .with_durable_turn_evidence(false);
        assert_eq!(uncertain, AiRunInterruptSettlement::RequestedUnsettled);
        assert!(!uncertain.retains_thread());

        // An inert adapter still reports no live resource.
        assert_eq!(
            AiRunInterruptSettlement::NotActive
                .fold_provider_outcome(AiProviderRunInterruptOutcome::NotActive)
                .with_durable_turn_evidence(true),
            AiRunInterruptSettlement::NotActive
        );
    }

    #[test]
    fn one_unsettled_adapter_keeps_the_whole_interruption_unsettled() {
        let mixed = AiRunInterruptSettlement::NotActive
            .fold_provider_outcome(AiProviderRunInterruptOutcome::RequestedSettled)
            .fold_provider_outcome(AiProviderRunInterruptOutcome::Requested);
        assert_eq!(mixed, AiRunInterruptSettlement::RequestedUnsettled);

        let reversed = AiRunInterruptSettlement::NotActive
            .fold_provider_outcome(AiProviderRunInterruptOutcome::Requested)
            .fold_provider_outcome(AiProviderRunInterruptOutcome::RequestedSettled);
        assert_eq!(reversed, AiRunInterruptSettlement::RequestedUnsettled);
    }
}
