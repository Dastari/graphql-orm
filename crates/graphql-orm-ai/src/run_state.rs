//! Durable run states and fenced worker transitions.

use graphql_orm::graphql::orm::{FencedLeaseState, LeaseError, LeaseProof};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Durable agent-run state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiRunState {
    /// Eligible for a worker claim.
    Queued,
    /// Claimed but provider work has not started.
    Leased,
    /// Provider/orchestration work is active.
    Running,
    /// Waiting for an argument-bound approval.
    WaitingApproval,
    /// Waiting for an application/internal tool result or for one claimed
    /// approval to be freshly consumed.
    WaitingTool,
    /// Waiting for the principal to reauthenticate.
    WaitingReauth,
    /// Waiting for an exactly bound provider background response. This state
    /// has no active worker lease and can only be advanced by reconciliation.
    WaitingProvider,
    /// Eligible after a retry deadline.
    RetryScheduled,
    /// Restore/crash left an uncertain side effect requiring review.
    RecoveryRequired,
    /// Successful terminal state.
    Completed,
    /// Failed terminal state.
    Failed,
    /// Cancelled terminal state.
    Cancelled,
}

/// Canonical owner-visible event emitted when an authoritative run leaves the
/// active worker lifecycle.
///
/// `RecoveryRequired` is included even though it is not a safely deletable
/// terminal state: it closes ordinary worker execution and requires explicit
/// operator reconciliation. This value describes durable UI/replay metadata
/// only. It grants no run mutation, retry, provider, tool, or resource
/// authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiRunTerminalEvent {
    /// The run completed successfully.
    Completed,
    /// The run failed with a redacted server-owned classification.
    Failed,
    /// The run was cancelled.
    Cancelled,
    /// The run stopped because an external effect could not be proven safe.
    RecoveryRequired,
}

impl AiRunTerminalEvent {
    /// Maps a durable run state to its owner-visible closing event.
    pub const fn from_run_state(state: AiRunState) -> Option<Self> {
        match state {
            AiRunState::Completed => Some(Self::Completed),
            AiRunState::Failed => Some(Self::Failed),
            AiRunState::Cancelled => Some(Self::Cancelled),
            AiRunState::RecoveryRequired => Some(Self::RecoveryRequired),
            AiRunState::Queued
            | AiRunState::Leased
            | AiRunState::Running
            | AiRunState::WaitingApproval
            | AiRunState::WaitingTool
            | AiRunState::WaitingReauth
            | AiRunState::WaitingProvider
            | AiRunState::RetryScheduled => None,
        }
    }

    /// Maps one canonical terminal event type back to its closed value.
    pub const fn from_event_type(event_type: &str) -> Option<Self> {
        match event_type.as_bytes() {
            b"run_completed" => Some(Self::Completed),
            b"run_failed" => Some(Self::Failed),
            b"run_cancelled" => Some(Self::Cancelled),
            b"run_recovery_required" => Some(Self::RecoveryRequired),
            _ => None,
        }
    }

    /// Stable owner-visible durable event name.
    pub const fn event_type(self) -> &'static str {
        match self {
            Self::Completed => "run_completed",
            Self::Failed => "run_failed",
            Self::Cancelled => "run_cancelled",
            Self::RecoveryRequired => "run_recovery_required",
        }
    }

    /// Exact durable run state represented by this event.
    pub const fn run_state(self) -> AiRunState {
        match self {
            Self::Completed => AiRunState::Completed,
            Self::Failed => AiRunState::Failed,
            Self::Cancelled => AiRunState::Cancelled,
            Self::RecoveryRequired => AiRunState::RecoveryRequired,
        }
    }
}

impl AiRunState {
    /// Stable durable storage value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Leased => "leased",
            Self::Running => "running",
            Self::WaitingApproval => "waiting_approval",
            Self::WaitingTool => "waiting_tool",
            Self::WaitingReauth => "waiting_reauth",
            Self::WaitingProvider => "waiting_provider",
            Self::RetryScheduled => "retry_scheduled",
            Self::RecoveryRequired => "recovery_required",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "leased" => Some(Self::Leased),
            "running" => Some(Self::Running),
            "waiting_approval" => Some(Self::WaitingApproval),
            "waiting_tool" => Some(Self::WaitingTool),
            "waiting_reauth" => Some(Self::WaitingReauth),
            "waiting_provider" => Some(Self::WaitingProvider),
            "retry_scheduled" => Some(Self::RetryScheduled),
            "recovery_required" => Some(Self::RecoveryRequired),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Returns whether no further worker transition is allowed.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Returns whether a worker may perform the transition.
    pub const fn can_transition_to(self, next: Self) -> bool {
        match self {
            Self::Queued | Self::RetryScheduled => matches!(next, Self::Leased | Self::Cancelled),
            Self::Leased => matches!(next, Self::Running | Self::Cancelled | Self::Failed),
            Self::Running => matches!(
                next,
                Self::WaitingApproval
                    | Self::WaitingTool
                    | Self::WaitingReauth
                    | Self::WaitingProvider
                    | Self::RetryScheduled
                    | Self::Completed
                    | Self::Failed
                    | Self::Cancelled
                    | Self::RecoveryRequired
            ),
            Self::WaitingApproval => matches!(
                next,
                Self::Running
                    | Self::WaitingTool
                    | Self::WaitingReauth
                    | Self::Cancelled
                    | Self::Failed
                    | Self::RecoveryRequired
            ),
            Self::WaitingTool => matches!(
                next,
                Self::Running
                    | Self::RetryScheduled
                    | Self::Cancelled
                    | Self::Failed
                    | Self::RecoveryRequired
            ),
            Self::WaitingReauth => matches!(
                next,
                Self::Queued | Self::Cancelled | Self::Failed | Self::RecoveryRequired
            ),
            Self::WaitingProvider => matches!(next, Self::Cancelled),
            Self::RecoveryRequired | Self::Completed | Self::Failed | Self::Cancelled => false,
        }
    }
}

/// Pure representation of a durable run row's state and fenced lease fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiRunLeaseMachine {
    /// Current run state.
    pub state: AiRunState,
    /// Portable lease/CAS state.
    pub lease: FencedLeaseState,
}

impl AiRunLeaseMachine {
    /// Creates a queued run.
    pub fn queued(run_id: impl Into<String>, row_version: i64) -> Self {
        Self {
            state: AiRunState::Queued,
            lease: FencedLeaseState::new(run_id, row_version),
        }
    }

    /// Claims queued/retry-scheduled work and transitions it to `Leased`.
    pub fn claim(
        &mut self,
        worker_id: impl Into<String>,
        attempt_id: Uuid,
        now_ms: i64,
        lease_ttl_ms: i64,
        expected_row_version: i64,
    ) -> Result<LeaseProof, AiRunTransitionError> {
        if !matches!(self.state, AiRunState::Queued | AiRunState::RetryScheduled) {
            return Err(AiRunTransitionError::InvalidTransition {
                from: self.state,
                to: AiRunState::Leased,
            });
        }
        let proof = self.lease.claim(
            worker_id,
            attempt_id,
            now_ms,
            lease_ttl_ms,
            expected_row_version,
        )?;
        self.state = AiRunState::Leased;
        Ok(proof)
    }

    /// Applies a fenced durable state transition.
    pub fn transition(
        &mut self,
        proof: &LeaseProof,
        next: AiRunState,
        now_ms: i64,
        expected_row_version: i64,
    ) -> Result<i64, AiRunTransitionError> {
        if !self.state.can_transition_to(next) {
            return Err(AiRunTransitionError::InvalidTransition {
                from: self.state,
                to: next,
            });
        }
        let version = self
            .lease
            .commit_fenced_write(proof, now_ms, expected_row_version)?;
        self.state = next;
        Ok(version)
    }

    /// Authorizes and versions a durable event/tool/provider child append
    /// without changing run state.
    pub fn commit_child_write(
        &mut self,
        proof: &LeaseProof,
        now_ms: i64,
        expected_row_version: i64,
    ) -> Result<i64, AiRunTransitionError> {
        Ok(self
            .lease
            .commit_fenced_write(proof, now_ms, expected_row_version)?)
    }
}

/// Run transition error.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum AiRunTransitionError {
    /// State transition is not allowed.
    #[error("invalid AI run transition from {from:?} to {to:?}")]
    InvalidTransition {
        /// Current state.
        from: AiRunState,
        /// Requested state.
        to: AiRunState,
    },
    /// Lease/CAS/fencing validation failed.
    #[error(transparent)]
    Lease(#[from] LeaseError),
}
