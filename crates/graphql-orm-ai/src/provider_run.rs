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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiProviderRunInterruptOutcome {
    /// No live resource existed for the exact binding.
    NotActive,
    /// An active provider turn accepted the interruption request.
    Requested,
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
}
