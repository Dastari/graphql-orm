//! Durable ORM-backed run leasing, fencing, and recovery.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use crate::orm_read_bounds::InternalReadBound as _;
use std::collections::BTreeSet;
use std::sync::Arc;

use agql_auth::{Clock, PrincipalReference};
use graphql_orm::db::Database;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::filters::{Int64Filter, IntFilter, StringFilter, UuidFilter};
use graphql_orm::graphql::orm::{
    ConditionalUpdateOutcome, DefaultWriteBackend, MutationContext, OrderDirection,
    TransactionError, TransactionMode,
};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::orm_inbox::{PreparedAiInboxEvent, append_inbox_event};
use crate::orm_native_approvals::{
    AiPreparedNativeApproval, PreparedNativeApprovalCandidate, PreparedNativeApprovalReceipt,
    PreparedNativeBlockedReceipt,
};
use crate::orm_provider_session::AiProviderSessionBindingRecord;
use crate::persistence::*;
use crate::{
    AiApprovalId, AiBudgetAmounts, AiError, AiRunCancellation, AiRunCancellationHub, AiRunFailure,
    AiRunId, AiRunRetryEvidence, AiRunState, AiRunTerminalEvent, AiScope, AiSessionId,
    AiSessionWakeup, AiToolCallId, ProtectedContentEnvelope, classify_run_retry,
};

pub(crate) mod native_checkpoints;
mod tool_media;

const MAXIMUM_WORKER_ID_BYTES: usize = 256;
const MAXIMUM_SAFE_CODE_BYTES: usize = 200;
const MAXIMUM_PROVIDER_REFERENCE_BYTES: usize = 1_024;

/// Deployment-owned hard limits for durable run workers.
///
/// These limits bound every candidate scan, lease, retry, and serialization
/// retry independently of GraphQL-managed configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRunServiceLimits {
    lease_ttl: Duration,
    maximum_retry_delay: Duration,
    maximum_candidate_scan: usize,
    maximum_run_retries: u32,
    maximum_transaction_retries: usize,
    lease_recovery_grace: Duration,
}

impl AiRunServiceLimits {
    /// Creates validated worker limits.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] when a duration is not
    /// positive, a candidate scan is outside `1..=256`, more than 100 run
    /// retries are allowed, or more than 16 serialization retries are used.
    pub fn new(
        lease_ttl: Duration,
        maximum_retry_delay: Duration,
        maximum_candidate_scan: usize,
        maximum_run_retries: u32,
        maximum_transaction_retries: usize,
    ) -> Result<Self, AiError> {
        if !lease_ttl.is_positive()
            || !maximum_retry_delay.is_positive()
            || !(1..=256).contains(&maximum_candidate_scan)
            || maximum_run_retries > 100
            || maximum_transaction_retries > 16
        {
            return Err(AiError::InvalidConfiguration(
                "invalid deployment run-service limits".to_owned(),
            ));
        }
        Ok(Self {
            lease_ttl,
            maximum_retry_delay,
            maximum_candidate_scan,
            maximum_run_retries,
            maximum_transaction_retries,
            lease_recovery_grace: Duration::ZERO,
        })
    }

    /// Returns these limits with a clock-skew grace for expired-lease
    /// recovery.
    ///
    /// Lease expiry is stamped by the worker that claimed or renewed the
    /// lease, while [`OrmAiRunService::recover_expired_leases`] compares it
    /// with the recovering process's clock. A recovering process whose clock
    /// runs ahead could otherwise close a live lease. Recovery treats a lease
    /// as expired only once it is older than this grace. Ownership never
    /// depends on it: every write remains fenced by attempt, generation and
    /// row version. The default is zero, which preserves the previous timing.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] when the grace is negative or
    /// longer than the lease TTL.
    pub fn with_lease_recovery_grace(mut self, grace: Duration) -> Result<Self, AiError> {
        if grace.is_negative() || grace > self.lease_ttl {
            return Err(AiError::InvalidConfiguration(
                "invalid lease recovery grace".to_owned(),
            ));
        }
        self.lease_recovery_grace = grace;
        Ok(self)
    }

    /// Returns the clock-skew grace applied before an expired lease is
    /// recovered.
    pub const fn lease_recovery_grace(&self) -> Duration {
        self.lease_recovery_grace
    }

    /// Returns the duration of every newly issued or renewed lease.
    pub const fn lease_ttl(&self) -> Duration {
        self.lease_ttl
    }

    /// Returns the maximum number of rows considered by one claim/recovery pass.
    pub const fn maximum_candidate_scan(&self) -> usize {
        self.maximum_candidate_scan
    }
}

/// Exact in-memory proof of one current durable run claim.
///
/// Fields are private so callers cannot manufacture a lease. The value alone
/// does not authorize a write: every service operation re-reads the run and
/// verifies its run ID, attempt ID, generation, owner, expiry, state, linked
/// checkpoint, and row version in a state-machine transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiRunLease {
    run_id: AiRunId,
    session_id: AiSessionId,
    input_message_id: Uuid,
    principal_reference: PrincipalReference,
    attempt_id: Uuid,
    worker_id: String,
    lease_generation: i64,
    lease_expires_at: OffsetDateTime,
    row_version: i64,
    state: AiRunState,
    retry_count: u32,
    latest_checkpoint_id: Option<Uuid>,
    recovery_epoch_id: Option<Uuid>,
}

impl AiRunLease {
    /// Run identifier.
    pub const fn run_id(&self) -> AiRunId {
        self.run_id
    }

    /// Session identifier.
    pub const fn session_id(&self) -> AiSessionId {
        self.session_id
    }

    /// User message that initiated the run.
    pub const fn input_message_id(&self) -> Uuid {
        self.input_message_id
    }

    /// Safe durable principal reference requiring fresh rehydration before
    /// provider egress, tools, approvals, and long-running checkpoints.
    pub fn principal_reference(&self) -> &PrincipalReference {
        &self.principal_reference
    }

    /// Current attempt identifier.
    pub const fn attempt_id(&self) -> Uuid {
        self.attempt_id
    }

    /// Worker owner of the claim.
    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }

    /// Monotonic durable fencing generation.
    pub const fn lease_generation(&self) -> i64 {
        self.lease_generation
    }

    /// Current lease expiry.
    pub const fn lease_expires_at(&self) -> OffsetDateTime {
        self.lease_expires_at
    }

    /// Current run state observed by the service.
    pub const fn state(&self) -> AiRunState {
        self.state
    }

    /// Number of previously scheduled retries.
    pub const fn retry_count(&self) -> u32 {
        self.retry_count
    }

    /// Latest fenced coordinator checkpoint linked to this lease, if any.
    ///
    /// A checkpoint ID alone is not resume authority. It must be validated and
    /// adopted by the protected checkpoint service under the current fence.
    pub const fn latest_checkpoint_id(&self) -> Option<Uuid> {
        self.latest_checkpoint_id
    }

    /// Recovery epoch the lease was granted under.
    ///
    /// Every fenced write re-checks inside its transaction that this equals
    /// the store's current live epoch, and gate-guarded services require it
    /// to equal [`crate::AiRuntimeStartGate::epoch_id`].
    pub const fn recovery_epoch_id(&self) -> Option<Uuid> {
        self.recovery_epoch_id
    }

    #[cfg(test)]
    pub(crate) fn test_running(principal_reference: PrincipalReference) -> Self {
        Self {
            run_id: AiRunId::new(),
            session_id: AiSessionId::new(),
            input_message_id: Uuid::new_v4(),
            principal_reference,
            attempt_id: Uuid::new_v4(),
            worker_id: "coordinator-test-worker".to_owned(),
            lease_generation: 1,
            lease_expires_at: OffsetDateTime::now_utc() + Duration::minutes(5),
            row_version: 1,
            state: AiRunState::Running,
            retry_count: 0,
            latest_checkpoint_id: None,
            recovery_epoch_id: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_with_checkpoint(&self, checkpoint_id: Uuid) -> Self {
        let mut lease = self.clone();
        lease.latest_checkpoint_id = Some(checkpoint_id);
        lease
    }

    #[cfg(test)]
    pub(crate) fn test_without_checkpoint(&self) -> Self {
        let mut lease = self.clone();
        lease.latest_checkpoint_id = None;
        lease
    }

    #[cfg(test)]
    pub(crate) fn test_with_state(&self, state: AiRunState) -> Self {
        let mut lease = self.clone();
        lease.state = state;
        lease
    }
}

/// Server-authored immutable outcome for a claimed run attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiRunCompletion {
    final_state: AiRunState,
    outcome_code: String,
    error_code: Option<String>,
    provider_response_id: Option<String>,
}

impl AiRunCompletion {
    /// Creates a terminal or recovery-required attempt outcome.
    ///
    /// Outcome and error values must be stable, redacted machine codes. They
    /// must never contain provider diagnostics, prompts, tool arguments, or
    /// response content.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for a non-final state, malformed safe
    /// code, oversized provider reference, or inconsistent success/error pair.
    pub fn new(
        final_state: AiRunState,
        outcome_code: impl Into<String>,
        error_code: Option<String>,
        provider_response_id: Option<String>,
    ) -> Result<Self, AiError> {
        let outcome_code = outcome_code.into();
        if !matches!(
            final_state,
            AiRunState::Completed
                | AiRunState::Failed
                | AiRunState::Cancelled
                | AiRunState::RecoveryRequired
        ) || !valid_safe_code(&outcome_code)
            || error_code
                .as_deref()
                .is_some_and(|code| !valid_safe_code(code))
            || provider_response_id.as_ref().is_some_and(|value| {
                value.is_empty() || value.len() > MAXIMUM_PROVIDER_REFERENCE_BYTES
            })
            || (final_state == AiRunState::Completed && error_code.is_some())
        {
            return Err(AiError::InvalidInput(
                "invalid redacted run completion".to_owned(),
            ));
        }
        Ok(Self {
            final_state,
            outcome_code,
            error_code,
            provider_response_id,
        })
    }

    /// Durable run state produced by the outcome.
    pub const fn final_state(&self) -> AiRunState {
        self.final_state
    }

    /// Stable redacted outcome code recorded on the immutable attempt outcome.
    ///
    /// This is server-owned classification, never provider diagnostics, a
    /// prompt, a tool argument, or response content.
    pub fn outcome_code(&self) -> &str {
        &self.outcome_code
    }

    #[cfg(test)]
    pub(crate) fn test_error_code(&self) -> Option<&str> {
        self.error_code.as_deref()
    }
}

/// Bounded result of expired-lease reconciliation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AiRunRecoveryReport {
    /// Claims that expired before provider execution and were safely requeued.
    pub requeued: u32,
    /// Expired running attempts whose exact completed read-only tool batch was
    /// retained for current-principal validation and one fenced adoption.
    pub checkpoint_requeued: u32,
    /// Claims moved to manual/privileged recovery because external execution
    /// may have happened.
    pub recovery_required: u32,
    /// Pre-provider claims failed because their retry ceiling was exhausted.
    pub failed: u32,
    /// Expired attempts finalized successfully because an exact same-transaction
    /// assistant-output checkpoint proved final protected output persistence.
    pub completed: u32,
    /// Expired rows whose durable lease or checkpoint state was malformed.
    /// Each was closed as `RecoveryRequired` with
    /// `lease_recovery_state_invalid` and audited; the pass continued.
    pub quarantined: u32,
    /// Rows or reservations whose reconciliation transaction failed in this
    /// pass. They are unchanged and are retried by the next pass.
    pub skipped: u32,
    /// Expired `reserved` or `uncertain` budget reservations whose attempt no
    /// longer holds a live lease, settled by committing the full reserved
    /// estimate as spend.
    pub budget_reservations_settled: u32,
}

/// Fenced worker handoff for one human-approved waiting run.
///
/// The handoff preserves the original attempt and generation because the
/// approval, provider usage, and staged tool call are bound to them. It still
/// fences the former worker by atomically replacing the lease owner, expiry,
/// heartbeat, and row-version proof. This value is not approval consumption
/// and grants no resolver, rule, egress, or tool authority.
#[derive(Clone, Debug)]
pub struct AiApprovedRunClaim {
    approval_id: AiApprovalId,
    tool_call_id: AiToolCallId,
    lease: AiRunLease,
}

impl AiApprovedRunClaim {
    /// Exact approved, unconsumed one-shot request selected by the handoff.
    pub const fn approval_id(&self) -> AiApprovalId {
        self.approval_id
    }

    /// Exact staged consequential tool selected by the handoff.
    pub const fn tool_call_id(&self) -> AiToolCallId {
        self.tool_call_id
    }

    /// New owner/row-version fence in `WaitingTool` while approval is
    /// revalidated for consumption.
    pub fn lease(&self) -> &AiRunLease {
        &self.lease
    }

    /// Consumes the claim into its exact IDs and waiting lease.
    pub fn into_parts(self) -> (AiApprovalId, AiToolCallId, AiRunLease) {
        (self.approval_id, self.tool_call_id, self.lease)
    }

    #[cfg(test)]
    pub(crate) fn test_claim(lease: AiRunLease) -> Self {
        Self {
            approval_id: AiApprovalId::new(),
            tool_call_id: AiToolCallId::new(),
            lease,
        }
    }
}

/// Durable worker queue implemented only through generated `graphql-orm`
/// repositories and state-machine transactions.
#[derive(Clone)]
pub struct OrmAiRunService {
    database: Database<DefaultWriteBackend>,
    clock: Arc<dyn Clock>,
    limits: AiRunServiceLimits,
    cancellation_hub: Option<Arc<AiRunCancellationHub>>,
}

#[derive(Clone)]
pub(crate) struct PreparedProviderBlock {
    pub id: Uuid,
    pub kind: String,
    pub protected_content: serde_json::Value,
    pub byte_count: i64,
    pub line_count: i64,
}

#[derive(Clone)]
pub(crate) struct PreparedProviderOutput {
    pub message_id: Uuid,
    pub event_id: Uuid,
    pub inbox_event_id: Uuid,
    pub provider_kind: String,
    pub provider_model: String,
    pub protected_preview: serde_json::Value,
    pub protected_event: serde_json::Value,
    pub protected_inbox_event: serde_json::Value,
    pub blocks: Vec<PreparedProviderBlock>,
    pub correlation_id: String,
    pub provider_response_id: Option<String>,
    pub budget_reservation_id: Uuid,
    pub checkpoint_hash: String,
    pub expected_owner_principal_kind: String,
    pub expected_owner_subject: String,
    pub expected_scope_kind: String,
    pub expected_scope_id: String,
    pub expected_tenant_id: Option<String>,
}

#[derive(Clone)]
pub(crate) struct PreparedContextCheckpointSource {
    pub message: AiMessageRecord,
    pub blocks: Vec<AiMessageBlockRecord>,
}

#[derive(Clone)]
pub(crate) struct PreparedContextCheckpoint {
    pub id: Uuid,
    pub through_sequence: i64,
    pub source_hash: String,
    pub token_estimate: i64,
    pub provider_kind: String,
    pub provider_model: String,
    pub protected_summary: serde_json::Value,
    pub expected_parent: Option<AiContextCheckpointRecord>,
    pub sources: Vec<PreparedContextCheckpointSource>,
    pub maximum_checkpoints_per_session: usize,
    pub expected_owner_principal_kind: String,
    pub expected_owner_subject: String,
    pub expected_scope_kind: String,
    pub expected_scope_id: String,
    pub expected_tenant_id: Option<String>,
}

#[derive(Clone)]
pub(crate) struct PreparedCoordinatorCheckpointTool {
    pub id: Uuid,
    pub provider_call_id: String,
    pub tool_id: String,
    pub result_egress_manifest_hash: String,
}

#[derive(Clone)]
pub(crate) enum PreparedNativeCheckpointBinding {
    Completed {
        // Exact row versions whose protected arguments/results and egress were opened.
        tool_versions: Vec<(Uuid, i64, i64)>,
    },
    Source {
        candidate_id: Uuid,
        pending_call_index: usize,
        callback_count: usize,
    },
    Outcome {
        candidate_id: Uuid,
        source_checkpoint_id: Uuid,
        pending_call_index: usize,
        callback_count: usize,
    },
}

#[derive(Clone)]
pub(crate) struct PreparedCoordinatorCheckpoint {
    pub id: Uuid,
    pub checkpoint_kind: String,
    pub provider_kind: String,
    pub provider_model: String,
    pub provider_response_id: Option<String>,
    pub budget_reservation_id: Uuid,
    pub protected_state: serde_json::Value,
    pub checkpoint_hash: String,
    pub completed_tools: Vec<PreparedCoordinatorCheckpointTool>,
    pub native_binding: Option<PreparedNativeCheckpointBinding>,
}

#[derive(Clone)]
pub(crate) struct PreparedLiveDeltaEvent {
    pub id: Uuid,
    pub event_type: String,
    pub protected_payload: serde_json::Value,
    pub correlation_id: String,
    pub provider_kind: String,
    pub provider_model: String,
    pub budget_reservation_id: Uuid,
    pub expected_owner_principal_kind: String,
    pub expected_owner_subject: String,
    pub expected_scope_kind: String,
    pub expected_scope_id: String,
    pub expected_tenant_id: Option<String>,
}

#[derive(Clone)]
pub(crate) struct PreparedProviderActivityEvent {
    pub id: Uuid,
    pub inbox_event_id: Uuid,
    pub protected_payload: serde_json::Value,
    pub protected_inbox_payload: serde_json::Value,
    pub correlation_id: String,
    pub provider_kind: String,
    pub provider_model: String,
    pub budget_reservation_id: Uuid,
    pub expected_owner_principal_kind: String,
    pub expected_owner_subject: String,
    pub expected_scope_kind: String,
    pub expected_scope_id: String,
    pub expected_tenant_id: Option<String>,
}

#[derive(Clone)]
pub(crate) struct PreparedUiIntentEvent {
    pub id: Uuid,
    pub inbox_event_id: Uuid,
    pub protected_payload: serde_json::Value,
    pub protected_inbox_payload: serde_json::Value,
    pub correlation_id: String,
    pub provider_kind: String,
    pub provider_model: String,
    pub provider_response_id: Option<String>,
    pub budget_reservation_id: Uuid,
    pub usage: AiBudgetAmounts,
    pub cached_input_tokens: u64,
    pub expected_owner_principal_kind: String,
    pub expected_owner_subject: String,
    pub expected_scope_kind: String,
    pub expected_scope_id: String,
    pub expected_tenant_id: Option<String>,
}

#[derive(Clone)]
pub(crate) struct PreparedProposal {
    pub id: Uuid,
    pub proposal_type: String,
    pub schema_version: String,
    pub item_count: i64,
    pub protected_payload: serde_json::Value,
    pub source_references: serde_json::Value,
    pub created_by_subject: String,
    pub expires_at: Option<i64>,
    pub event_id: Uuid,
    pub protected_event: serde_json::Value,
    pub correlation_id: String,
    pub expected_owner_principal_kind: String,
    pub expected_owner_subject: String,
    pub expected_scope_kind: String,
    pub expected_scope_id: String,
    pub expected_tenant_id: Option<String>,
}

#[derive(Clone)]
pub(crate) struct PreparedApprovalRequest {
    pub id: Uuid,
    pub tool_call_id: Uuid,
    pub principal_subject: String,
    pub principal_reference_fingerprint: String,
    pub delegated_actor_subject: Option<String>,
    pub delegation_reference: Option<String>,
    pub argument_hash: String,
    pub tool_fingerprint: String,
    pub binding_hash: String,
    pub execution_target_id: String,
    pub target_schema_fingerprint: String,
    pub operation_name: String,
    pub operation_document_hash: String,
    pub result_projection_fingerprint: String,
    pub disclosure_schema_fingerprint: String,
    pub policy_version: String,
    pub authorization_state_digest: String,
    pub protected_resource_bindings: serde_json::Value,
    pub protected_action_preview: serde_json::Value,
    pub action_preview_hash: String,
    pub recent_mfa_required: bool,
    pub expires_at: i64,
    pub event_id: Uuid,
    pub protected_event: serde_json::Value,
    pub correlation_id: String,
    pub expected_owner_principal_kind: String,
    pub expected_owner_subject: String,
    pub expected_scope_kind: String,
    pub expected_scope_id: String,
    pub expected_tenant_id: Option<String>,
    pub parked_provider_wait: Option<PreparedApprovalProviderWait>,
}

#[derive(Clone)]
pub(crate) struct PreparedApprovalProviderWait {
    pub source_checkpoint_id: Uuid,
    pub source_checkpoint_fingerprint: String,
    pub parked_checkpoint_id: Uuid,
    pub parked_checkpoint_fingerprint: String,
    pub protected_parked_checkpoint: serde_json::Value,
    pub provider_kind: String,
    pub provider_model: String,
    pub provider_response_id: Option<String>,
    pub budget_reservation_id: Uuid,
}

#[derive(Clone)]
pub(crate) struct PreparedApprovalConsumption {
    pub approval_id: Uuid,
    pub tool_call_id: Uuid,
    pub binding_hash: String,
    pub expected_approval_version: i64,
    pub event_id: Uuid,
    pub protected_event: serde_json::Value,
    pub started_event: PreparedToolLifecycleEvent,
    pub correlation_id: String,
    pub expected_owner_principal_kind: String,
    pub expected_owner_subject: String,
    pub expected_scope_kind: String,
    pub expected_scope_id: String,
    pub expected_tenant_id: Option<String>,
}

#[derive(Clone)]
pub(crate) enum PreparedApprovalWaitOutcome {
    Cancelled(Box<PreparedApprovalWaitCancellation>),
    RecoveryRequired {
        approval_id: Option<Uuid>,
        tool_call_id: Option<Uuid>,
    },
}

#[derive(Clone)]
pub(crate) struct PreparedApprovalWaitCancellation {
    pub call: AiToolCallRecord,
    pub step: AiRunStepRecord,
    pub approval: AiApprovalRecord,
    pub checkpoint: AiRunCheckpointRecord,
    pub parked_checkpoint: Option<AiRunCheckpointRecord>,
    pub attempt_outcome: Option<AiRunAttemptOutcomeRecord>,
    pub next_approval_state: Option<String>,
    pub call_state: String,
}

#[derive(Clone)]
pub(crate) struct PreparedApprovalWaitReconciliation {
    pub expected_run: AiRunRecord,
    pub expected_owner_principal_kind: String,
    pub expected_owner_subject: String,
    pub expected_scope_kind: String,
    pub expected_scope_id: String,
    pub expected_tenant_id: Option<String>,
    pub outcome: PreparedApprovalWaitOutcome,
    pub outcome_code: String,
    pub policy_version: Option<String>,
    pub event_id: Uuid,
    /// `None` only when the stored principal reference is delegated: no
    /// content-protection policy is resolved for it, so the reconciled event
    /// is omitted and the canonical metadata-only terminal event remains.
    pub protected_event: Option<serde_json::Value>,
    pub worker_id: String,
}

#[derive(Clone)]
pub(crate) struct PreparedToolCallStart {
    pub id: Uuid,
    pub provider_call_key: String,
    pub provider_call_id: String,
    pub provider_kind: String,
    pub provider_model: String,
    pub provider_response_id: Option<String>,
    pub budget_reservation_id: Uuid,
    pub provider_turn_index: i64,
    pub tool_call_index: i64,
    pub tool_id: String,
    pub tool_fingerprint: String,
    pub execution_provenance: Option<crate::AiToolExecutionProvenance>,
    /// Shared rather than cloned: the refusal is a minted-only token, and a
    /// retried transaction must never duplicate it.
    pub native_preflight_refusal: Option<Arc<crate::orm_tools::NativePreflightRefusal>>,
    pub protected_arguments: serde_json::Value,
    pub argument_hash: String,
    pub risk: String,
    pub idempotency_key: Option<String>,
    pub correlation_id: String,
    pub causation_id: String,
    pub delegation_reference: Option<String>,
    pub started_event: Option<PreparedToolLifecycleEvent>,
    pub expected_owner_principal_kind: String,
    pub expected_owner_subject: String,
    pub expected_scope_kind: String,
    pub expected_scope_id: String,
    pub expected_tenant_id: Option<String>,
}

#[derive(Clone)]
struct NativeBlockedCall {
    candidate_id: AiToolCallId,
    receipt: PreparedNativeApprovalReceipt,
    disclosure_schema_fingerprint: String,
}

#[derive(Clone)]
pub(crate) struct PreparedToolLifecycleEvent {
    pub event_id: Uuid,
    pub inbox_event_id: Uuid,
    pub protected_event: serde_json::Value,
    pub protected_inbox_event: serde_json::Value,
}

#[derive(Clone)]
pub(crate) struct PreparedToolCallFinish {
    pub id: Uuid,
    pub state: String,
    pub protected_result: serde_json::Value,
    pub authorization_code: String,
    pub authorization_policy_version: Option<String>,
    pub authorization_state_digest: Option<String>,
    pub disclosure_schema_fingerprint: String,
    pub result_classification: String,
    pub result_egress_decision_id: Option<Uuid>,
    pub result_egress_manifest_hash: Option<String>,
    pub application_audit_ref: Option<String>,
    pub event_id: Uuid,
    pub inbox_event_id: Uuid,
    pub protected_event: serde_json::Value,
    pub protected_inbox_event: serde_json::Value,
    pub correlation_id: String,
    pub expected_provider_call_key: String,
    pub expected_tool_fingerprint: String,
    pub expected_owner_principal_kind: String,
    pub expected_owner_subject: String,
    pub expected_scope_kind: String,
    pub expected_scope_id: String,
    pub expected_tenant_id: Option<String>,
}

impl OrmAiRunService {
    pub(crate) const fn lease_ttl(&self) -> Duration {
        self.limits.lease_ttl
    }

    /// Creates an ORM-backed run service.
    pub fn new(
        database: Database<DefaultWriteBackend>,
        clock: Arc<dyn Clock>,
        limits: AiRunServiceLimits,
    ) -> Self {
        Self {
            database,
            clock,
            limits,
            cancellation_hub: None,
        }
    }

    /// Enables process-local immediate cancellation wakeups. Durable polling
    /// remains authoritative when the notification is absent or missed.
    #[must_use]
    pub fn with_cancellation_hub(mut self, hub: Arc<AiRunCancellationHub>) -> Self {
        self.cancellation_hub = Some(hub);
        self
    }

    /// Returns the ORM database handle for host schema composition.
    pub fn database(&self) -> &Database<DefaultWriteBackend> {
        &self.database
    }

    /// Observes whether an owner cancellation has already won the exact run
    /// attempt/generation fence.
    pub async fn cancellation(
        &self,
        lease: &AiRunLease,
    ) -> Result<Option<AiRunCancellation>, AiError> {
        let current = AiRunRecord::find_by_id(&self.database, &lease.run_id.0)
            .await
            .map_err(|_| AiError::PersistenceFailed)?
            .ok_or(AiError::NotFound)?;
        let stored_reference: PrincipalReference =
            serde_json::from_value(current.principal_reference.clone())
                .map_err(|_| AiError::PersistenceFailed)?;
        if current.session_id != lease.session_id.0
            || current.input_message_id != lease.input_message_id
            || stored_reference != lease.principal_reference
            || current.attempt_id != Some(lease.attempt_id)
            || current.lease_generation != lease.lease_generation
        {
            return Err(AiError::Conflict);
        }
        if current.state != AiRunState::Cancelled.as_str() {
            return Ok(None);
        }
        let request_id = current
            .cancellation_request_id
            .ok_or(AiError::PersistenceFailed)?;
        let requested_at = current
            .cancellation_requested_at
            .ok_or(AiError::PersistenceFailed)?;
        Ok(Some(AiRunCancellation::new(
            lease.session_id,
            lease.run_id,
            request_id,
            requested_at,
        )))
    }

    /// Waits for a process-local wakeup or the bounded durable polling
    /// deadline, then reads the authoritative cancellation state.
    pub async fn wait_for_cancellation(
        &self,
        lease: &AiRunLease,
        maximum_wait: std::time::Duration,
    ) -> Result<Option<AiRunCancellation>, AiError> {
        if let Some(cancellation) = self.cancellation(lease).await? {
            return Ok(Some(cancellation));
        }
        if maximum_wait.is_zero() {
            return Ok(None);
        }
        if let Some(hub) = &self.cancellation_hub {
            hub.wait(lease.run_id, maximum_wait).await;
        } else {
            tokio::time::sleep(maximum_wait).await;
        }
        self.cancellation(lease).await
    }

    /// Claims the oldest eligible queued/retry-scheduled run.
    ///
    /// The claim and immutable attempt fact commit atomically. Concurrent
    /// workers can never receive the same attempt/generation pair.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid worker ID, malformed durable state,
    /// fencing overflow, or persistence failure.
    pub async fn claim_next(&self, worker_id: &str) -> Result<Option<AiRunLease>, AiError> {
        validate_worker_id(worker_id)?;
        let now = canonical_second(self.clock.now());
        for retry in 0..=self.limits.maximum_transaction_retries {
            match self.claim_once(worker_id.to_owned(), now).await {
                Ok(result) => return result,
                Err(TransactionError::Retryable(_))
                    if retry < self.limits.maximum_transaction_retries =>
                {
                    tokio::task::yield_now().await;
                }
                Err(error) => return Err(map_transaction(error)),
            }
        }
        Err(AiError::PersistenceFailed)
    }

    /// Claims one approved, unconsumed `WaitingApproval` run for immediate
    /// fresh validation and one-shot consumption.
    ///
    /// Legacy stateless waits preserve their existing in-attempt handoff.
    /// Retained-provider waits require an exactly confirmed parked-session
    /// graph and atomically create a fresh attempt/generation while refencing
    /// the staged call and step. The completed source attempt and parked
    /// checkpoint remain immutable evidence; an unconfirmed graph stays
    /// unclaimed so maintenance can repair confirmation without replay.
    /// Expired approved rows encountered in the bounded window are atomically
    /// marked expired and audited so they cannot permanently block newer
    /// eligible approvals.
    /// The caller must rehydrate current principal/rules, rebuild the preview,
    /// consume the approval, and execute the ordinary resolver; this claim
    /// authorizes none of those actions.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid worker ID, malformed approval/tool/run
    /// linkage, an invalid durable lease, lease-time overflow, or persistence
    /// failure.
    pub async fn claim_next_approved(
        &self,
        worker_id: &str,
    ) -> Result<Option<AiApprovedRunClaim>, AiError> {
        validate_worker_id(worker_id)?;
        let now = canonical_second(self.clock.now());
        for retry in 0..=self.limits.maximum_transaction_retries {
            match self.claim_approved_once(worker_id.to_owned(), now).await {
                Ok(result) => return Ok(result),
                Err(TransactionError::Retryable(_))
                    if retry < self.limits.maximum_transaction_retries =>
                {
                    tokio::task::yield_now().await;
                }
                Err(error) => return Err(map_transaction(error)),
            }
        }
        Err(AiError::PersistenceFailed)
    }

    pub(crate) async fn reconcile_approval_wait(
        &self,
        reconciliation: PreparedApprovalWaitReconciliation,
    ) -> Result<(), AiError> {
        validate_worker_id(&reconciliation.worker_id)?;
        if !valid_safe_code(&reconciliation.outcome_code)
            || reconciliation
                .policy_version
                .as_deref()
                .is_some_and(|value| {
                    value.trim().is_empty()
                        || value.len() > 1_024
                        || value.chars().any(char::is_control)
                })
        {
            return Err(AiError::InvalidInput(
                "invalid approval-wait reconciliation".to_owned(),
            ));
        }
        let now = canonical_second(self.clock.now());
        for retry in 0..=self.limits.maximum_transaction_retries {
            match self
                .reconcile_approval_wait_once(reconciliation.clone(), now)
                .await
            {
                Ok(()) => return Ok(()),
                Err(TransactionError::Retryable(_))
                    if retry < self.limits.maximum_transaction_retries =>
                {
                    tokio::task::yield_now().await;
                }
                Err(error) => return Err(map_transaction(error)),
            }
        }
        Err(AiError::PersistenceFailed)
    }

    /// Renews a current leased/running claim and returns its new row-version
    /// proof. Classified retryable database transactions use the configured
    /// transaction retry bound; provider requests and tools are never retried.
    ///
    /// # Errors
    ///
    /// Fails closed for an expired, superseded, malformed, non-active, or
    /// otherwise stale fence, and for persistence failures.
    pub async fn heartbeat(&self, lease: &AiRunLease) -> Result<AiRunLease, AiError> {
        self.update_active_lease(lease, LeaseUpdate::Heartbeat)
            .await
    }

    /// Drives `execution` to completion while renewing `lease` at a third of
    /// the lease TTL, and returns its output with the latest lease proof.
    ///
    /// Application tools run outside any transaction and may take up to their
    /// configured execution limit, which can exceed the lease TTL. Without
    /// renewal, lease recovery could close a live attempt mid-tool. A failed
    /// renewal (for example after cancellation) keeps the previous proof; the
    /// later fenced write reports the lost lease as before.
    pub(crate) async fn run_with_lease_heartbeat<T>(
        &self,
        lease: &AiRunLease,
        execution: impl std::future::Future<Output = T>,
    ) -> (T, AiRunLease) {
        let third: Duration = self.limits.lease_ttl / 3;
        let interval = third.max(Duration::SECOND).unsigned_abs();
        let mut current = lease.clone();
        // Boxed so wrapping a large tool future does not grow every caller.
        let mut execution = Box::pin(execution);
        loop {
            tokio::select! {
                output = &mut execution => return (output, current),
                () = tokio::time::sleep(interval) => {
                    if let Ok(renewed) = self.heartbeat(&current).await {
                        current = renewed;
                    }
                }
            }
        }
    }

    /// Transitions a freshly claimed run from `Leased` to `Running`.
    ///
    /// A running run must be treated as externally uncertain after its budget
    /// reservation crosses the provider-transport boundary.
    ///
    /// # Errors
    ///
    /// Fails closed for an expired or stale fence, invalid state, or
    /// persistence failure.
    pub async fn start(&self, lease: &AiRunLease) -> Result<AiRunLease, AiError> {
        self.update_active_lease(lease, LeaseUpdate::Start).await
    }

    /// Completes a current attempt and appends its immutable outcome in the
    /// same fenced transaction.
    ///
    /// Budget usage must be reconciled before a successful/failed terminal
    /// completion. Ambiguous provider execution must use `RecoveryRequired`.
    ///
    /// # Errors
    ///
    /// Fails closed for an expired/stale fence, invalid transition, duplicate
    /// outcome, or persistence failure.
    pub async fn finish(
        &self,
        lease: &AiRunLease,
        completion: AiRunCompletion,
    ) -> Result<(), AiError> {
        let now = canonical_second(self.clock.now());
        retry_transaction(self.limits.maximum_transaction_retries, || {
            self.finish_once(lease.clone(), completion.clone(), now)
        })
        .await
        .map_err(map_transaction)
    }

    /// One attempt of [`Self::finish`]. Every attempt re-validates the exact
    /// lease fence, so a retry after an attempt that committed but whose
    /// acknowledgement was lost finds the run already closed and is refused
    /// with `Conflict` instead of writing a second outcome.
    fn finish_once(
        &self,
        lease: AiRunLease,
        completion: AiRunCompletion,
        now: OffsetDateTime,
    ) -> impl std::future::Future<Output = Result<(), TransactionError>> + '_ {
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let current = load_and_validate_active_lease(tx, &lease, now).await?;
                    let current_state = persisted_state(&current)?;
                    if !current_state.can_transition_to(completion.final_state) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let outcome = tx
                        .compare_and_swap::<AiRunRecord>(
                            &current.id,
                            current.row_version,
                            exact_state(&current.state),
                            UpdateAiRunRecordInput {
                                state: Some(completion.final_state.as_str().to_owned()),
                                lease_owner: Some(None),
                                lease_expires_at: Some(None),
                                lease_heartbeat_at: Some(None),
                                next_attempt_at: Some(None),
                                error_code: Some(completion.error_code.clone()),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?;
                    if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    // The event must classify from the value that lands on the
                    // run row, because that is what the retry mutation will
                    // later re-evaluate. Classifying from `outcome_code` would
                    // let a completion with an outcome code but no error code
                    // advertise a retry the mutation then refuses.
                    let terminal_error_code = completion.error_code.clone();
                    append_attempt_outcome(
                        tx,
                        &lease,
                        completion.final_state,
                        completion.outcome_code,
                        completion.provider_response_id,
                        now,
                    )
                    .await?;
                    append_terminal_run_event(
                        tx,
                        &current,
                        completion.final_state,
                        terminal_error_code.as_deref(),
                        now,
                    )
                    .await
                })
            })
    }

    /// Schedules a bounded retry and relinquishes the current fence.
    ///
    /// # Errors
    ///
    /// Fails for an invalid delay/code, exhausted retry ceiling, an expired or
    /// stale fence, an invalid transition, or persistence failure.
    pub async fn schedule_retry(
        &self,
        lease: &AiRunLease,
        delay: Duration,
        error_code: impl Into<String>,
    ) -> Result<(), AiError> {
        let error_code = error_code.into();
        if delay.is_negative()
            || delay > self.limits.maximum_retry_delay
            || !valid_safe_code(&error_code)
        {
            return Err(AiError::InvalidInput(
                "invalid redacted run retry".to_owned(),
            ));
        }
        if lease.retry_count >= self.limits.maximum_run_retries {
            return Err(AiError::Conflict);
        }
        let now = canonical_second(self.clock.now());
        let eligible_at = now
            .checked_add(delay)
            .ok_or_else(|| AiError::InvalidConfiguration("run retry time overflow".to_owned()))?;
        let lease = lease.clone();
        retry_transaction(self.limits.maximum_transaction_retries, || {
            let error_code = error_code.clone();
            let lease = lease.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let current = load_and_validate_active_lease(tx, &lease, now).await?;
                        let current_state = persisted_state(&current)?;
                        if !current_state.can_transition_to(AiRunState::RetryScheduled)
                            || current.retry_count < 0
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let retry_count = current
                            .retry_count
                            .checked_add(1)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let outcome = tx
                            .compare_and_swap::<AiRunRecord>(
                                &current.id,
                                current.row_version,
                                exact_state(&current.state),
                                UpdateAiRunRecordInput {
                                    state: Some(AiRunState::RetryScheduled.as_str().to_owned()),
                                    attempt_id: Some(None),
                                    lease_owner: Some(None),
                                    lease_expires_at: Some(None),
                                    lease_heartbeat_at: Some(None),
                                    retry_count: Some(retry_count),
                                    next_attempt_at: Some(Some(eligible_at.unix_timestamp())),
                                    error_code: Some(Some(error_code.clone())),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        append_attempt_outcome(
                            tx,
                            &lease,
                            AiRunState::RetryScheduled,
                            error_code,
                            None,
                            now,
                        )
                        .await
                    })
                })
        })
        .await
        .map_err(map_transaction)
    }

    /// Reconciles a bounded window of expired active leases.
    ///
    /// A `Leased` claim is known not to have started provider orchestration and
    /// can be requeued. A `Running` attempt with an exact same-transaction
    /// protected-output checkpoint is safely finalized. An exact completed
    /// read-only tool-batch checkpoint can be requeued for protected adoption;
    /// it remains unusable until current-principal revalidation and is consumed
    /// before the next provider call. When the adopting attempt also loses its
    /// lease before consuming that checkpoint, the checkpoint is validated
    /// against the attempt that wrote it and the run is requeued for adoption
    /// again, bounded by the run retry ceiling. Live `WaitingApproval` runs are
    /// excluded because the bounded approval-wait reconciler owns their current
    /// policy, decision, and cutoff handling without heartbeating the human
    /// wait. Every other running or waiting state becomes `RecoveryRequired`;
    /// it is never silently replayed.
    ///
    /// The scan selects only expired rows, oldest first, so live leases cannot
    /// hide expired ones. Each candidate is reconciled in its own bounded
    /// serialization-retried transaction. A row whose lease or checkpoint
    /// state is malformed is closed as `RecoveryRequired` with
    /// `lease_recovery_state_invalid` and counted in
    /// [`AiRunRecoveryReport::quarantined`]; a row whose transaction still
    /// fails is left unchanged for the next pass and counted in
    /// [`AiRunRecoveryReport::skipped`]. Neither stops the pass. The pass then
    /// settles expired unresolved budget reservations whose attempt no longer
    /// holds a live lease (see
    /// [`AiRunRecoveryReport::budget_reservations_settled`]).
    ///
    /// # Errors
    ///
    /// Returns [`AiError::RuntimeNotReady`] while the runtime recovery epoch is
    /// closed, and an error when the candidate scan itself fails.
    pub async fn recover_expired_leases(&self) -> Result<AiRunRecoveryReport, AiError> {
        let now = canonical_second(self.clock.now());
        let limits = self.limits;
        let cutoff = now
            .checked_sub(limits.lease_recovery_grace)
            .ok_or_else(|| {
                AiError::InvalidConfiguration("lease recovery time overflow".to_owned())
            })?
            .unix_timestamp();
        let scan = limits.maximum_candidate_scan;
        let mut report = AiRunRecoveryReport::default();
        // The first progress-stamp failure other than a stale-row miss. The
        // pass finishes its other work, then fails with it: a row that cannot
        // be stamped would otherwise hold the head of every later page.
        let mut stamp_failure: Option<TransactionError> = None;
        // Decided once per pass: whether any read of this pass can have been
        // filtered. Absence and incompleteness quarantine only when it cannot.
        let filtering_excluded = recovery_filtering_excluded(&self.database).await;
        // Two bounded pages: active rows with no expiry (malformed, so they
        // are quarantined rather than skipped forever) and expired rows, each
        // least recently examined first. A row is recovered, quarantined for
        // a durable defect of its own state, or left unchanged after an
        // operational failure with a fenced progress stamp that moves it
        // behind the rest, so no row can hold the head of the window across
        // passes or restarts.
        for missing_expiry in [true, false] {
            let candidates = retry_transaction(limits.maximum_transaction_retries, || {
                self.database
                    .transaction(TransactionMode::Default, move |tx| {
                        Box::pin(async move {
                            crate::orm_runtime_recovery::require_live_epoch(tx).await?;
                            let filter = || AiRunRecordWhereInput {
                                state: Some(StringFilter {
                                    in_list: Some(
                                        EXPIRED_LEASE_RECOVERY_STATES
                                            .iter()
                                            .map(|state| state.as_str().to_owned())
                                            .collect(),
                                    ),
                                    ..Default::default()
                                }),
                                lease_expires_at: Some(if missing_expiry {
                                    Int64Filter {
                                        is_null: Some(true),
                                        ..Default::default()
                                    }
                                } else {
                                    Int64Filter {
                                        lte: Some(cutoff),
                                        ..Default::default()
                                    }
                                }),
                                ..Default::default()
                            };
                            let rows = tx
                                .query::<AiRunRecord>()
                                .filter(filter())
                                .order_by(AiRunRecordOrderByInput {
                                    recovery_examined_at: Some(OrderDirection::Asc),
                                    ..Default::default()
                                })
                                .internal_limit(scan as i64)
                                .fetch_all()
                                .await
                                .map_err(OrmPublicError::from)?;
                            Ok(rows
                                .into_iter()
                                .map(|run| (run.id, run.row_version, run.state))
                                .collect::<Vec<_>>())
                        })
                    })
            })
            .await
            .map_err(map_transaction)?;
            for (run_id, observed_version, observed_state) in candidates {
                let outcome = retry_transaction(limits.maximum_transaction_retries, || {
                    self.database
                        .transaction(TransactionMode::StateMachine, move |tx| {
                            Box::pin(async move {
                                recover_expired_run(
                                    tx,
                                    run_id,
                                    now,
                                    cutoff,
                                    limits,
                                    filtering_excluded,
                                )
                                .await
                            })
                        })
                })
                .await;
                match outcome {
                    Ok(ExpiredLeaseRecovery::Unchanged) => {}
                    Ok(ExpiredLeaseRecovery::Quarantined) => {
                        report.quarantined = report.quarantined.saturating_add(1);
                    }
                    Ok(ExpiredLeaseRecovery::Recovered(state, code)) => {
                        let counter = match (state, code) {
                            (AiRunState::Completed, _) => &mut report.completed,
                            (AiRunState::Failed, _) => &mut report.failed,
                            (AiRunState::RetryScheduled, "checkpoint_adoption_ready") => {
                                &mut report.checkpoint_requeued
                            }
                            (AiRunState::RetryScheduled, _) => &mut report.requeued,
                            _ => &mut report.recovery_required,
                        };
                        *counter = counter.saturating_add(1);
                    }
                    Err(error)
                        if crate::orm_runtime_recovery::is_epoch_not_live(error.public_error()) =>
                    {
                        return Err(AiError::RuntimeNotReady);
                    }
                    // An operational failure (a host policy refusal, a
                    // refused read, a database error after the bounded
                    // serialization retries). The run stays exactly as it
                    // was; a fenced stamp moves it behind the rest of the
                    // scan so it cannot starve the rows behind it.
                    Err(_) => {
                        report.skipped = report.skipped.saturating_add(1);
                        let observed_state = observed_state.clone();
                        let stamped = retry_transaction(limits.maximum_transaction_retries, || {
                            let observed_state = observed_state.clone();
                            self.database
                                .transaction(TransactionMode::StateMachine, move |tx| {
                                    Box::pin(async move {
                                        stamp_run_recovery_progress(
                                            tx,
                                            run_id,
                                            observed_version,
                                            &observed_state,
                                            now.unix_timestamp(),
                                        )
                                        .await
                                    })
                                })
                        })
                        .await;
                        if let Err(error) = stamped {
                            stamp_failure.get_or_insert(error);
                        }
                    }
                }
            }
        }
        self.settle_stranded_budget_reservations(now, &mut report, &mut stamp_failure)
            .await?;
        if let Some(error) = stamp_failure {
            return Err(map_transaction(error));
        }
        Ok(report)
    }

    /// Settles expired unresolved budget reservations whose attempt no longer
    /// holds a live lease.
    ///
    /// Only the exact worker that reserved a budget can reconcile it with
    /// actual usage: while its attempt is live, or after an owner cancellation
    /// of that attempt. A lease-lost, requeued or otherwise closed attempt, or
    /// a cancelled worker that never reports, would otherwise keep its full
    /// estimate reserved on every counter for the rest of the period. Once
    /// the reservation's own expiry passes, a `reserved` or `uncertain`
    /// reservation of such an attempt is committed at its full reserved
    /// estimate, the same conservative settlement the privileged reclaim
    /// uses, and audited. Dispatch cannot be disproved, so the estimate is
    /// never released. Reservations owned by a background submission keep
    /// their own authoritative reconciliation.
    async fn settle_stranded_budget_reservations(
        &self,
        now: OffsetDateTime,
        report: &mut AiRunRecoveryReport,
        stamp_failure: &mut Option<TransactionError>,
    ) -> Result<(), AiError> {
        let limits = self.limits;
        let scan = limits.maximum_candidate_scan;
        let now_seconds = now.unix_timestamp();
        let candidates = retry_transaction(limits.maximum_transaction_retries, || {
            self.database
                .transaction(TransactionMode::Default, move |tx| {
                    Box::pin(async move {
                        // Least recently examined first: rows a pass leaves
                        // unresolved are stamped and rotate behind the rest,
                        // durably across passes and restarts.
                        let rows = tx
                            .query::<AiBudgetReservationRecord>()
                            .filter(AiBudgetReservationRecordWhereInput {
                                state: Some(StringFilter {
                                    in_list: Some(vec![
                                        "reserved".to_owned(),
                                        "uncertain".to_owned(),
                                    ]),
                                    ..Default::default()
                                }),
                                expires_at: timestamp_at_or_before(now_seconds),
                                ..Default::default()
                            })
                            .order_by(AiBudgetReservationRecordOrderByInput {
                                settlement_examined_at: Some(OrderDirection::Asc),
                                ..Default::default()
                            })
                            .internal_limit(scan as i64)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        Ok(rows
                            .into_iter()
                            .filter(|reservation| reservation.expires_at <= now_seconds)
                            .map(|reservation| {
                                (reservation.id, reservation.row_version, reservation.state)
                            })
                            .collect::<Vec<_>>())
                    })
                })
        })
        .await
        .map_err(map_transaction)?;
        for (reservation_id, observed_version, observed_state) in candidates {
            let outcome = retry_transaction(limits.maximum_transaction_retries, || {
                self.database
                    .transaction(TransactionMode::StateMachine, move |tx| {
                        Box::pin(async move {
                            crate::orm_runtime_recovery::require_live_epoch(tx).await?;
                            settle_stranded_budget_reservation(tx, reservation_id, now).await
                        })
                    })
            })
            .await;
            match outcome {
                Ok(ReservationSettlement::Settled) => {
                    report.budget_reservations_settled =
                        report.budget_reservations_settled.saturating_add(1);
                }
                // Gone left the scan set; Owned was stamped inside the same
                // epoch-checked transaction that observed the owner.
                Ok(ReservationSettlement::Gone | ReservationSettlement::Owned) => {}
                Err(error)
                    if crate::orm_runtime_recovery::is_epoch_not_live(error.public_error()) =>
                {
                    return Err(AiError::RuntimeNotReady);
                }
                Err(_) => {
                    report.skipped = report.skipped.saturating_add(1);
                    // Fenced by the live epoch and the observed version and
                    // state; a refusal leaves the row for the next pass.
                    let observed_state = observed_state.clone();
                    let stamped = retry_transaction(limits.maximum_transaction_retries, || {
                        let observed_state = observed_state.clone();
                        self.database
                            .transaction(TransactionMode::StateMachine, move |tx| {
                                Box::pin(async move {
                                    stamp_reservation_settlement_progress(
                                        tx,
                                        reservation_id,
                                        observed_version,
                                        &observed_state,
                                        now_seconds,
                                    )
                                    .await
                                })
                            })
                    })
                    .await;
                    if let Err(error) = stamped {
                        stamp_failure.get_or_insert(error);
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn append_provider_output(
        &self,
        lease: &AiRunLease,
        output: PreparedProviderOutput,
    ) -> Result<AiRunLease, AiError> {
        let now = canonical_second(self.clock.now());
        let lease_ttl = self.limits.lease_ttl;
        let lease = lease.clone();
        retry_transaction(self.limits.maximum_transaction_retries, || {
            let lease = lease.clone();
            let output = output.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let current = load_and_validate_active_lease(tx, &lease, now).await?;
                        if persisted_state(&current)? != AiRunState::Running
                            || output.blocks.is_empty()
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session.state != "active"
                            || session.deleted_at.is_some()
                            || session.owner_principal_kind != output.expected_owner_principal_kind
                            || session.owner_subject != output.expected_owner_subject
                            || session.scope_kind != output.expected_scope_kind
                            || session.scope_id != output.expected_scope_id
                            || session.tenant_id != output.expected_tenant_id
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let message_sequence = session
                            .message_head
                            .checked_add(1)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let event_sequence = session
                            .stream_head
                            .checked_add(1)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let session_update = tx
                            .compare_and_swap::<AiSessionRecord>(
                                &session.id,
                                session.row_version,
                                AiSessionRecordWhereInput::default(),
                                UpdateAiSessionRecordInput {
                                    message_head: Some(message_sequence),
                                    stream_head: Some(event_sequence),
                                    last_activity_at: Some(now.unix_timestamp()),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(session_update, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let expiry = now
                            .checked_add(lease_ttl)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let run_update = tx
                            .compare_and_swap::<AiRunRecord>(
                                &current.id,
                                current.row_version,
                                exact_state(AiRunState::Running.as_str()),
                                UpdateAiRunRecordInput {
                                    lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                    lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                                    latest_checkpoint_id: Some(Some(output.message_id)),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        let updated_run = match run_update {
                            ConditionalUpdateOutcome::Updated(updated) => updated,
                            ConditionalUpdateOutcome::NotFound
                            | ConditionalUpdateOutcome::Conflict => {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                        };
                        tx.insert::<AiMessageRecord>(CreateAiMessageRecordInput {
                            id: output.message_id,
                            session_id: lease.session_id.0,
                            sequence: message_sequence,
                            message_role: "assistant".to_owned(),
                            author_principal_kind: None,
                            author_subject: None,
                            client_message_id: None,
                            content_hash: None,
                            run_id: Some(lease.run_id.0),
                            provider_kind: Some(output.provider_kind),
                            provider_model: Some(output.provider_model),
                            protected_preview: Some(output.protected_preview),
                            block_count: i64::try_from(output.blocks.len())
                                .map_err(|_| OrmPublicError::new(OrmErrorCode::InvalidInput))?,
                            completion_state: "complete".to_owned(),
                            finalized_at: Some(now.unix_timestamp()),
                            content_purged_at: None,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        tx.insert::<AiRunCheckpointRecord>(CreateAiRunCheckpointRecordInput {
                            id: output.message_id,
                            run_id: lease.run_id.0,
                            attempt_id: lease.attempt_id,
                            lease_generation: lease.lease_generation,
                            checkpoint_kind: "assistant_output_persisted".to_owned(),
                            provider_response_id: output.provider_response_id,
                            budget_reservation_id: Some(output.budget_reservation_id),
                            assistant_message_id: Some(output.message_id),
                            protected_state: None,
                            checkpoint_hash: output.checkpoint_hash,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        for (block_index, block) in output.blocks.into_iter().enumerate() {
                            tx.insert::<AiMessageBlockRecord>(CreateAiMessageBlockRecordInput {
                                id: block.id,
                                message_id: output.message_id,
                                block_index: i64::try_from(block_index)
                                    .map_err(|_| OrmPublicError::new(OrmErrorCode::InvalidInput))?,
                                block_kind: block.kind,
                                protected_content: block.protected_content,
                                byte_count: block.byte_count,
                                line_count: block.line_count,
                            })
                            .await
                            .map_err(OrmPublicError::from)?;
                        }
                        tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                            id: output.event_id,
                            session_id: lease.session_id.0,
                            sequence: event_sequence,
                            event_type: "assistant_message_completed".to_owned(),
                            run_id: Some(lease.run_id.0),
                            causation_id: Some(lease.input_message_id.to_string()),
                            correlation_id: output.correlation_id,
                            protected_payload: output.protected_event,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        tx.queue_event(AiSessionWakeup {
                            session_id: lease.session_id.0,
                            sequence: event_sequence,
                        });
                        append_inbox_event(
                            tx,
                            PreparedAiInboxEvent {
                                id: output.inbox_event_id,
                                principal_kind: output.expected_owner_principal_kind,
                                principal_subject: output.expected_owner_subject,
                                scope: crate::AiScope {
                                    kind: output.expected_scope_kind,
                                    id: output.expected_scope_id,
                                    tenant_id: output.expected_tenant_id,
                                },
                                session_id: lease.session_id.0,
                                event_type: "assistant_message_completed".to_owned(),
                                protected_payload: output.protected_inbox_event,
                                created_at: now.unix_timestamp(),
                            },
                        )
                        .await?;
                        lease_from_record(&updated_run)
                    })
                })
        })
        .await
        .map_err(map_transaction)
    }

    pub(crate) async fn append_context_checkpoint(
        &self,
        lease: &AiRunLease,
        checkpoint: PreparedContextCheckpoint,
    ) -> Result<AiRunLease, AiError> {
        let now = canonical_second(self.clock.now());
        let lease_ttl = self.limits.lease_ttl;
        let lease = lease.clone();
        retry_transaction(self.limits.maximum_transaction_retries, || {
            let checkpoint = checkpoint.clone();
            let lease = lease.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let current = load_and_validate_active_lease(tx, &lease, now).await?;
                        if persisted_state(&current)? != AiRunState::Running
                            || checkpoint.id.is_nil()
                            || checkpoint.through_sequence <= 0
                            || checkpoint.source_hash.len() != 64
                            || !checkpoint
                                .source_hash
                                .bytes()
                                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                            || checkpoint.token_estimate <= 0
                            || checkpoint.provider_kind.trim().is_empty()
                            || checkpoint.provider_model.trim().is_empty()
                            || checkpoint.sources.is_empty()
                            || !(1..=5_000).contains(&checkpoint.maximum_checkpoints_per_session)
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session.state != "active"
                            || session.deleted_at.is_some()
                            || session.message_head < checkpoint.through_sequence
                            || session.owner_principal_kind
                                != checkpoint.expected_owner_principal_kind
                            || session.owner_subject != checkpoint.expected_owner_subject
                            || session.scope_kind != checkpoint.expected_scope_kind
                            || session.scope_id != checkpoint.expected_scope_id
                            || session.tenant_id != checkpoint.expected_tenant_id
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }

                        let checkpoint_limit = i64::try_from(
                            checkpoint.maximum_checkpoints_per_session.saturating_add(1),
                        )
                        .map_err(|_| OrmPublicError::new(OrmErrorCode::InvalidInput))?;
                        let existing = tx
                            .query::<AiContextCheckpointRecord>()
                            .filter(AiContextCheckpointRecordWhereInput {
                                session_id: Some(UuidFilter {
                                    eq: Some(session.id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .default_order()
                            .internal_limit(checkpoint_limit)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        if existing.len() > checkpoint.maximum_checkpoints_per_session {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let current_parent = existing
                            .iter()
                            .find(|candidate| candidate.invalidated_at.is_none());
                        if current_parent != checkpoint.expected_parent.as_ref()
                            || current_parent.is_some_and(|parent| {
                                parent.through_sequence >= checkpoint.through_sequence
                            })
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }

                        let expected_start = current_parent
                            .map_or(1, |parent| parent.through_sequence.saturating_add(1));
                        let expected_source_count = checkpoint
                            .through_sequence
                            .checked_sub(expected_start)
                            .and_then(|value| value.checked_add(1))
                            .and_then(|value| usize::try_from(value).ok())
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        if checkpoint.sources.len() != expected_source_count {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let start = i32::try_from(expected_start)
                            .map_err(|_| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let through = i32::try_from(checkpoint.through_sequence)
                            .map_err(|_| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let message_limit = i64::try_from(expected_source_count.saturating_add(1))
                            .map_err(|_| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let messages = tx
                            .query::<AiMessageRecord>()
                            .filter(AiMessageRecordWhereInput {
                                session_id: Some(UuidFilter {
                                    eq: Some(session.id),
                                    ..Default::default()
                                }),
                                sequence: Some(IntFilter {
                                    gte: Some(start),
                                    lte: Some(through),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .default_order()
                            .internal_limit(message_limit)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        if messages.len() != expected_source_count {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        for (offset, (message, expected)) in
                            messages.iter().zip(&checkpoint.sources).enumerate()
                        {
                            let sequence = expected_start
                                .checked_add(
                                    i64::try_from(offset)
                                        .map_err(|_| OrmPublicError::new(OrmErrorCode::Conflict))?,
                                )
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                            if message != &expected.message
                                || message.sequence != sequence
                                || message.content_purged_at.is_some()
                            {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            let block_limit =
                                i64::try_from(expected.blocks.len().saturating_add(1))
                                    .map_err(|_| OrmPublicError::new(OrmErrorCode::Conflict))?;
                            let blocks = tx
                                .query::<AiMessageBlockRecord>()
                                .filter(AiMessageBlockRecordWhereInput {
                                    message_id: Some(UuidFilter {
                                        eq: Some(message.id),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                })
                                .default_order()
                                .internal_limit(block_limit)
                                .fetch_all()
                                .await
                                .map_err(OrmPublicError::from)?;
                            if blocks != expected.blocks {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                        }

                        let expiry = now
                            .checked_add(lease_ttl)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let run_update = tx
                            .compare_and_swap::<AiRunRecord>(
                                &current.id,
                                current.row_version,
                                exact_state(AiRunState::Running.as_str()),
                                UpdateAiRunRecordInput {
                                    lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                    lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        let updated_run = match run_update {
                            ConditionalUpdateOutcome::Updated(updated) => updated,
                            ConditionalUpdateOutcome::NotFound
                            | ConditionalUpdateOutcome::Conflict => {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                        };
                        tx.insert::<AiContextCheckpointRecord>(
                            CreateAiContextCheckpointRecordInput {
                                id: checkpoint.id,
                                session_id: session.id,
                                through_sequence: checkpoint.through_sequence,
                                source_hash: checkpoint.source_hash,
                                token_estimate: checkpoint.token_estimate,
                                provider_kind: checkpoint.provider_kind,
                                provider_model: checkpoint.provider_model,
                                protected_summary: checkpoint.protected_summary,
                                invalidated_at: None,
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?;
                        lease_from_record(&updated_run)
                    })
                })
        })
        .await
        .map_err(map_transaction)
    }

    pub(crate) async fn append_coordinator_checkpoint(
        &self,
        lease: &AiRunLease,
        checkpoint: PreparedCoordinatorCheckpoint,
    ) -> Result<AiRunLease, AiError> {
        let now = canonical_second(self.clock.now());
        let lease_ttl = self.limits.lease_ttl;
        let lease = lease.clone();
        retry_transaction(self.limits.maximum_transaction_retries, || {
            let checkpoint = checkpoint.clone();
            let lease = lease.clone();
 self.database.transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let current = load_and_validate_active_lease(tx, &lease, now).await?;
                    let valid_kind = match checkpoint.checkpoint_kind.as_str() {
                        "provider_turn_persisted" => checkpoint.completed_tools.is_empty(),
                        native_checkpoints::COMPLETED_KIND => {
                            !checkpoint.completed_tools.is_empty()
                                && matches!(
                                    checkpoint.native_binding,
                                    Some(PreparedNativeCheckpointBinding::Completed { .. })
                                )
                        }
                        "tool_batch_persisted" => !checkpoint.completed_tools.is_empty(),
                        "supervised_tool_batch_persisted" => checkpoint.completed_tools.len() == 1,
                        native_checkpoints::SOURCE_KIND => matches!(
                            checkpoint.native_binding,
                            Some(PreparedNativeCheckpointBinding::Source { .. })
                        ),
                        native_checkpoints::OUTCOME_KIND => {
                            checkpoint.completed_tools.len() == 1
                                && matches!(
                                    checkpoint.native_binding,
                                    Some(PreparedNativeCheckpointBinding::Outcome { .. })
                                )
                        }
                        _ => false,
                    };
                    if (checkpoint.native_binding.is_some()
                        && !matches!(
                            checkpoint.checkpoint_kind.as_str(),
                            native_checkpoints::SOURCE_KIND
                                | native_checkpoints::OUTCOME_KIND
                                | native_checkpoints::COMPLETED_KIND
                        ))
                        || persisted_state(&current)? != AiRunState::Running
                        || !valid_kind
                        || checkpoint.provider_kind.trim().is_empty()
                        || checkpoint.provider_model.trim().is_empty()
                        || checkpoint.checkpoint_hash.len() != 64
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let reservation = tx
                        .find_by_id::<AiBudgetReservationRecord>(&checkpoint.budget_reservation_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    let native_outcome =
                        checkpoint.checkpoint_kind == native_checkpoints::OUTCOME_KIND;
                    if let Some(binding) = &checkpoint.native_binding {
                        let expected_ids = checkpoint
                            .completed_tools
                            .iter()
                            .map(|tool| tool.id)
                            .collect::<BTreeSet<_>>();
                        if expected_ids.len() != checkpoint.completed_tools.len()
                            || coordinator_checkpoint_hash(
                                lease.run_id,
                                lease.attempt_id,
                                lease.lease_generation,
                                checkpoint.id,
                                &checkpoint.checkpoint_kind,
                                &checkpoint.provider_kind,
                                &checkpoint.provider_model,
                                checkpoint.provider_response_id.as_deref(),
                                checkpoint.budget_reservation_id,
                                &checkpoint.protected_state,
                            )
                            .map_err(|_| OrmPublicError::new(OrmErrorCode::Conflict))?
                                != checkpoint.checkpoint_hash
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        match binding {
                            PreparedNativeCheckpointBinding::Completed { tool_versions } => {
                                if tool_versions.len() != checkpoint.completed_tools.len()
                                    || tool_versions
                                        .iter()
                                        .map(|(id, _, _)| *id)
                                        .collect::<BTreeSet<_>>()
                                        != expected_ids
                                {
                                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                                }
                                for (index, ((id, call_version, step_version), expected)) in
                                    tool_versions
                                        .iter()
                                        .zip(&checkpoint.completed_tools)
                                        .enumerate()
                                {
                                    let call = tx
                                        .find_by_id::<AiToolCallRecord>(id)
                                        .await
                                        .map_err(OrmPublicError::from)?
                                        .ok_or_else(OrmPublicError::not_found)?;
                                    let step = tx
                                        .find_by_id::<AiRunStepRecord>(id)
                                        .await
                                        .map_err(OrmPublicError::from)?
                                        .ok_or_else(OrmPublicError::not_found)?;
                                    if *id != expected.id
                                        || call.row_version != *call_version
                                        || step.row_version != *step_version
                                        || call.tool_call_index
                                            != i64::try_from(index).map_err(|_| {
                                                OrmPublicError::new(OrmErrorCode::Conflict)
                                            })?
                                    {
                                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                                    }
                                }
                            }
                            PreparedNativeCheckpointBinding::Source {
                                candidate_id,
                                pending_call_index,
                                callback_count,
                            } => {
                                let candidate =
                                    native_checkpoints::candidate(tx, *candidate_id).await?;
                                let ordinary = native_checkpoints::cohort(
                                    tx,
                                    native_checkpoints::NativeTurn {
                                        run_id: lease.run_id.0,
                                        attempt_id: lease.attempt_id,
                                        generation: lease.lease_generation,
                                        response_id: checkpoint
                                            .provider_response_id
                                            .as_deref()
                                            .ok_or_else(|| {
                                                OrmPublicError::new(OrmErrorCode::Conflict)
                                            })?,
                                        budget: &reservation,
                                    },
                                    &candidate,
                                    native_checkpoints::PendingPhase::Prepared,
                                    Some((*pending_call_index, *callback_count)),
                                )
                                .await?;
                                if ordinary != expected_ids {
                                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                                }
                            }
                            PreparedNativeCheckpointBinding::Outcome {
                                candidate_id,
                                source_checkpoint_id,
                                pending_call_index,
                                callback_count,
                            } => {
                                if expected_ids != BTreeSet::from([*candidate_id]) {
                                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                                }
                                native_checkpoints::outcome(
                                    tx,
                                    lease.run_id.0,
                                    lease.session_id.0,
                                    lease.lease_generation,
                                    &reservation,
                                    checkpoint.provider_response_id.as_deref(),
                                    *candidate_id,
                                    Some(*source_checkpoint_id),
                                    Some((*pending_call_index, *callback_count)),
                                )
                                .await?;
                            }
                        }
                    }
                    if reservation.session_id != lease.session_id.0
                        || reservation.run_id != lease.run_id.0
                        || (!native_outcome
                            && (reservation.attempt_id != lease.attempt_id
                                || reservation.lease_generation != lease.lease_generation))
                        || reservation.provider_kind != checkpoint.provider_kind
                        || reservation.provider_model != checkpoint.provider_model
                        || reservation.state != "committed"
                        || reservation.actual_runs != Some(1)
                        || reservation.reconciled_at.is_none()
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    for expected in &checkpoint.completed_tools {
                        let call = tx
                            .find_by_id::<AiToolCallRecord>(&expected.id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        let step = tx
                            .find_by_id::<AiRunStepRecord>(&expected.id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        let authorization_matches = match checkpoint.checkpoint_kind.as_str() {
                            "tool_batch_persisted" => {
                                call.risk == "read_only" && call.approval_id.is_none()
                            }
                            native_checkpoints::COMPLETED_KIND => {
                                matches!(
                                    call.risk.as_str(),
                                    "read_only" | "low_risk_write" | "non_idempotent_write"
                                ) && call.approval_id.is_none()
                                    && (call.state != "execution_failed"
                                        || call.risk == "read_only"
                                        || crate::orm_tools::native_preflight_failure_code(&call).is_some()
                                        || (call.authorization_code.as_deref().and_then(crate::ToolPreExecutionRejectionReason::from_failure_code).is_some()
                                            && call.result_classification.as_deref() == Some("public")))
                            }
                            native_checkpoints::SOURCE_KIND => {
                                matches!(
                                    call.risk.as_str(),
                                    "read_only" | "low_risk_write" | "non_idempotent_write"
                                ) && call.approval_id.is_none()
                            }
                            "supervised_tool_batch_persisted"
                            | native_checkpoints::OUTCOME_KIND => {
                                let Some(approval_id) = call.approval_id else {
                                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                                };
                                let approval = tx
                                    .find_by_id::<AiApprovalRecord>(&approval_id)
                                    .await
                                    .map_err(OrmPublicError::from)?
                                    .ok_or_else(OrmPublicError::not_found)?;
                                matches!(
                                    call.risk.as_str(),
                                    "low_risk_write" | "non_idempotent_write" | "high_impact"
                                ) && approval.tool_call_id == call.id
                                    && approval.session_id == lease.session_id.0
                                    && approval.state == "consumed"
                                    && approval.maximum_uses == 1
                                    && approval.consumed_uses == 1
                                    && approval.consumed_at.is_some()
                                    && approval.argument_hash == call.argument_hash
                                    && approval.tool_fingerprint == call.tool_fingerprint
                                    && call.authorization_policy_version.as_deref()
                                        == Some(approval.policy_version.as_str())
                                    && call.authorization_state_digest.as_deref()
                                        == Some(approval.authorization_state_digest.as_str())
                            }
                            _ => false,
                        };
                        if call.run_id != lease.run_id.0
                            || call.lease_generation != lease.lease_generation
                            || call.provider_call_id != expected.provider_call_id
                            || call.tool_id != expected.tool_id
                            || call.provider_kind.as_deref()
                                != Some(checkpoint.provider_kind.as_str())
                            || call.provider_model.as_deref()
                                != Some(checkpoint.provider_model.as_str())
                            || call.provider_response_id != checkpoint.provider_response_id
                            || call.budget_reservation_id != Some(checkpoint.budget_reservation_id)
                            || !matches!(call.state.as_str(), "completed" | "execution_failed")
                            || call.protected_result.is_none()
                            || call.result_egress_decision_id.is_none()
                            || call.result_egress_manifest_hash.as_deref()
                                != Some(expected.result_egress_manifest_hash.as_str())
                            || call.completed_at.is_none()
                            || step.run_id != lease.run_id.0
                            || step.lease_generation != lease.lease_generation
                            || step.state != call.state
                            || step.finished_at.is_none()
                            || !authorization_matches
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                    }
                    if matches!(
                        checkpoint.checkpoint_kind.as_str(),
                        "tool_batch_persisted"
                            | "supervised_tool_batch_persisted"
                            | native_checkpoints::COMPLETED_KIND
                    ) {
                        let expected_ids = checkpoint
                            .completed_tools
                            .iter()
                            .map(|tool| tool.id)
                            .collect::<BTreeSet<_>>();
                        let calls = tx
                            .query::<AiToolCallRecord>()
                            .filter(AiToolCallRecordWhereInput {
                                run_id: Some(UuidFilter {
                                    eq: Some(lease.run_id.0),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .internal_limit(4_097)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        if checkpoint.checkpoint_kind == native_checkpoints::COMPLETED_KIND
                            && calls.len() >= 4_097
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let actual_ids = calls
                            .iter()
                            .filter(|call| {
                                if checkpoint.checkpoint_kind == native_checkpoints::COMPLETED_KIND
                                {
                                    return call.budget_reservation_id
                                        == Some(checkpoint.budget_reservation_id);
                                }
                                call.lease_generation == lease.lease_generation
                                    && call.provider_response_id.as_deref()
                                        == checkpoint.provider_response_id.as_deref()
                                    && call.budget_reservation_id
                                        == Some(checkpoint.budget_reservation_id)
                            })
                            .map(|call| call.id)
                            .collect::<BTreeSet<_>>();
                        if actual_ids != expected_ids {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                    }
                    let expiry = now
                        .checked_add(lease_ttl)
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                    let run_update = tx
                        .compare_and_swap::<AiRunRecord>(
                            &current.id,
                            current.row_version,
                            exact_state(AiRunState::Running.as_str()),
                            UpdateAiRunRecordInput {
                                lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                                latest_checkpoint_id: Some(Some(checkpoint.id)),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?;
                    let updated_run = match run_update {
                        ConditionalUpdateOutcome::Updated(updated) => updated,
                        ConditionalUpdateOutcome::NotFound | ConditionalUpdateOutcome::Conflict => {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                    };
                    tx.insert::<AiRunCheckpointRecord>(CreateAiRunCheckpointRecordInput {
                        id: checkpoint.id,
                        run_id: lease.run_id.0,
                        attempt_id: lease.attempt_id,
                        lease_generation: lease.lease_generation,
                        checkpoint_kind: checkpoint.checkpoint_kind,
                        provider_response_id: checkpoint.provider_response_id,
                        budget_reservation_id: Some(checkpoint.budget_reservation_id),
                        assistant_message_id: None,
                        protected_state: Some(checkpoint.protected_state),
                        checkpoint_hash: checkpoint.checkpoint_hash,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    lease_from_record(&updated_run)
                })
            })
})
.await
            .map_err(map_transaction)
    }

    pub(crate) async fn consume_adoption_checkpoint(
        &self,
        lease: &AiRunLease,
        checkpoint_id: Uuid,
        expected_kind: &'static str,
    ) -> Result<AiRunLease, AiError> {
        if !matches!(
            expected_kind,
            "tool_batch_persisted"
                | "supervised_tool_batch_persisted"
                | native_checkpoints::OUTCOME_KIND
        ) {
            return Err(AiError::Conflict);
        }
        let now = canonical_second(self.clock.now());
        let lease_ttl = self.limits.lease_ttl;
        let lease = lease.clone();
        retry_transaction(self.limits.maximum_transaction_retries, || {
            let lease = lease.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let current = load_and_validate_active_lease(tx, &lease, now).await?;
                        if persisted_state(&current)? != AiRunState::Running
                            || current.latest_checkpoint_id != Some(checkpoint_id)
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let checkpoint = tx
                            .query::<AiRunCheckpointRecord>()
                            .filter(AiRunCheckpointRecordWhereInput {
                                id: Some(UuidFilter {
                                    eq: Some(checkpoint_id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .internal_limit(1)
                            .fetch_one()
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if checkpoint.run_id != current.id
                            || checkpoint.checkpoint_kind != expected_kind
                            || checkpoint.budget_reservation_id.is_none()
                            || checkpoint.assistant_message_id.is_some()
                            || checkpoint.protected_state.is_none()
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let expiry = now
                            .checked_add(lease_ttl)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let update = tx
                            .compare_and_swap::<AiRunRecord>(
                                &current.id,
                                current.row_version,
                                exact_state(AiRunState::Running.as_str()),
                                UpdateAiRunRecordInput {
                                    latest_checkpoint_id: Some(None),
                                    lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                    lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        match update {
                            ConditionalUpdateOutcome::Updated(updated) => {
                                lease_from_record(&updated)
                            }
                            ConditionalUpdateOutcome::NotFound
                            | ConditionalUpdateOutcome::Conflict => {
                                Err(OrmPublicError::new(OrmErrorCode::Conflict))
                            }
                        }
                    })
                })
        })
        .await
        .map_err(map_transaction)
    }

    pub(crate) async fn append_live_delta_event(
        &self,
        lease: &AiRunLease,
        event: PreparedLiveDeltaEvent,
    ) -> Result<(), AiError> {
        if event.event_type != "provider_live_delta"
            || !valid_provider_kind(&event.provider_kind)
            || !valid_provider_reference(&event.provider_model)
            || !valid_provider_reference(&event.correlation_id)
            || event.expected_owner_principal_kind.trim().is_empty()
            || event.expected_owner_subject.trim().is_empty()
            || event.expected_scope_kind.trim().is_empty()
            || event.expected_scope_id.trim().is_empty()
        {
            return Err(AiError::InvalidInput(
                "invalid protected live-delta event".to_owned(),
            ));
        }
        let now = canonical_second(self.clock.now());
        let lease = lease.clone();
        retry_transaction(self.limits.maximum_transaction_retries, || {
            let event = event.clone();
            let lease = lease.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let current = tx
                            .find_by_id::<AiRunRecord>(&lease.run_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        require_current_lease_epoch(tx, &current, &lease).await?;
                        let stored_reference: PrincipalReference =
                            serde_json::from_value(current.principal_reference.clone())
                                .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        if persisted_state(&current)? != AiRunState::Running
                            || current.session_id != lease.session_id.0
                            || current.input_message_id != lease.input_message_id
                            || stored_reference != lease.principal_reference
                            || current.attempt_id != Some(lease.attempt_id)
                            || current.lease_owner.as_deref() != Some(lease.worker_id.as_str())
                            || current.lease_generation != lease.lease_generation
                            || current
                                .lease_expires_at
                                .is_none_or(|expires_at| expires_at <= now.unix_timestamp())
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let reservation = tx
                            .find_by_id::<AiBudgetReservationRecord>(&event.budget_reservation_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if reservation.session_id != lease.session_id.0
                            || reservation.run_id != lease.run_id.0
                            || reservation.attempt_id != lease.attempt_id
                            || reservation.lease_generation != lease.lease_generation
                            || reservation.provider_kind != event.provider_kind
                            || reservation.provider_model != event.provider_model
                            || reservation.state != "uncertain"
                            || reservation.actual_runs.is_some()
                            || reservation.reconciled_at.is_none()
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session.state != "active"
                            || session.deleted_at.is_some()
                            || session.owner_principal_kind != event.expected_owner_principal_kind
                            || session.owner_subject != event.expected_owner_subject
                            || session.scope_kind != event.expected_scope_kind
                            || session.scope_id != event.expected_scope_id
                            || session.tenant_id != event.expected_tenant_id
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let event_sequence = session
                            .stream_head
                            .checked_add(1)
                            .filter(|sequence| *sequence <= i64::from(i32::MAX))
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let update = tx
                            .compare_and_swap::<AiSessionRecord>(
                                &session.id,
                                session.row_version,
                                AiSessionRecordWhereInput::default(),
                                UpdateAiSessionRecordInput {
                                    stream_head: Some(event_sequence),
                                    last_activity_at: Some(now.unix_timestamp()),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(update, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                            id: event.id,
                            session_id: session.id,
                            sequence: event_sequence,
                            event_type: event.event_type,
                            run_id: Some(lease.run_id.0),
                            causation_id: Some(lease.input_message_id.to_string()),
                            correlation_id: event.correlation_id,
                            protected_payload: event.protected_payload,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        tx.queue_event(AiSessionWakeup {
                            session_id: session.id,
                            sequence: event_sequence,
                        });
                        Ok(())
                    })
                })
        })
        .await
        .map_err(map_transaction)
    }

    pub(crate) async fn append_provider_activity_event(
        &self,
        lease: &AiRunLease,
        event: PreparedProviderActivityEvent,
    ) -> Result<(), AiError> {
        if !valid_provider_kind(&event.provider_kind)
            || !valid_provider_reference(&event.provider_model)
            || !valid_provider_reference(&event.correlation_id)
            || event.expected_owner_principal_kind.trim().is_empty()
            || event.expected_owner_subject.trim().is_empty()
            || event.expected_scope_kind.trim().is_empty()
            || event.expected_scope_id.trim().is_empty()
        {
            return Err(AiError::InvalidInput(
                "invalid protected provider-activity event".to_owned(),
            ));
        }
        let now = canonical_second(self.clock.now());
        let lease = lease.clone();
        retry_transaction(self.limits.maximum_transaction_retries, || {
            let event = event.clone();
            let lease = lease.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let current = tx
                            .find_by_id::<AiRunRecord>(&lease.run_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        require_current_lease_epoch(tx, &current, &lease).await?;
                        let stored_reference: PrincipalReference =
                            serde_json::from_value(current.principal_reference.clone())
                                .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        if persisted_state(&current)? != AiRunState::Running
                            || current.session_id != lease.session_id.0
                            || current.input_message_id != lease.input_message_id
                            || stored_reference != lease.principal_reference
                            || current.attempt_id != Some(lease.attempt_id)
                            || current.lease_owner.as_deref() != Some(lease.worker_id.as_str())
                            || current.lease_generation != lease.lease_generation
                            || current
                                .lease_expires_at
                                .is_none_or(|expires_at| expires_at <= now.unix_timestamp())
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let reservation = tx
                            .find_by_id::<AiBudgetReservationRecord>(&event.budget_reservation_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if reservation.session_id != lease.session_id.0
                            || reservation.run_id != lease.run_id.0
                            || reservation.attempt_id != lease.attempt_id
                            || reservation.lease_generation != lease.lease_generation
                            || reservation.provider_kind != event.provider_kind
                            || reservation.provider_model != event.provider_model
                            || reservation.state != "uncertain"
                            || reservation.actual_runs.is_some()
                            || reservation.reconciled_at.is_none()
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session.state != "active"
                            || session.deleted_at.is_some()
                            || session.owner_principal_kind != event.expected_owner_principal_kind
                            || session.owner_subject != event.expected_owner_subject
                            || session.scope_kind != event.expected_scope_kind
                            || session.scope_id != event.expected_scope_id
                            || session.tenant_id != event.expected_tenant_id
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let event_sequence = session
                            .stream_head
                            .checked_add(1)
                            .filter(|sequence| *sequence <= i64::from(i32::MAX))
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let update = tx
                            .compare_and_swap::<AiSessionRecord>(
                                &session.id,
                                session.row_version,
                                AiSessionRecordWhereInput::default(),
                                UpdateAiSessionRecordInput {
                                    stream_head: Some(event_sequence),
                                    last_activity_at: Some(now.unix_timestamp()),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(update, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                            id: event.id,
                            session_id: session.id,
                            sequence: event_sequence,
                            event_type: "provider_activity".to_owned(),
                            run_id: Some(lease.run_id.0),
                            causation_id: Some(lease.input_message_id.to_string()),
                            correlation_id: event.correlation_id,
                            protected_payload: event.protected_payload,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        append_inbox_event(
                            tx,
                            PreparedAiInboxEvent {
                                id: event.inbox_event_id,
                                principal_kind: event.expected_owner_principal_kind,
                                principal_subject: event.expected_owner_subject,
                                scope: AiScope {
                                    kind: event.expected_scope_kind,
                                    id: event.expected_scope_id,
                                    tenant_id: event.expected_tenant_id,
                                },
                                session_id: session.id,
                                event_type: "provider_activity".to_owned(),
                                protected_payload: event.protected_inbox_payload,
                                created_at: now.unix_timestamp(),
                            },
                        )
                        .await?;
                        tx.queue_event(AiSessionWakeup {
                            session_id: session.id,
                            sequence: event_sequence,
                        });
                        Ok(())
                    })
                })
        })
        .await
        .map_err(map_transaction)
    }

    pub(crate) async fn append_ui_intent_event(
        &self,
        lease: &AiRunLease,
        event: PreparedUiIntentEvent,
    ) -> Result<(AiRunLease, i64), AiError> {
        if !valid_provider_kind(&event.provider_kind)
            || !valid_provider_reference(&event.provider_model)
            || event
                .provider_response_id
                .as_deref()
                .is_some_and(|value| !valid_provider_reference(value))
            || !valid_provider_reference(&event.correlation_id)
            || event.expected_owner_principal_kind.trim().is_empty()
            || event.expected_owner_subject.trim().is_empty()
            || event.expected_scope_kind.trim().is_empty()
            || event.expected_scope_id.trim().is_empty()
        {
            return Err(AiError::InvalidInput(
                "invalid protected UI intent event".to_owned(),
            ));
        }
        let now = canonical_second(self.clock.now());
        let lease_ttl = self.limits.lease_ttl;
        let lease = lease.clone();
        retry_transaction(self.limits.maximum_transaction_retries, || {
            let event = event.clone();
            let lease = lease.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let current = tx
                            .find_by_id::<AiRunRecord>(&lease.run_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        require_current_lease_epoch(tx, &current, &lease).await?;
                        let stored_reference: PrincipalReference =
                            serde_json::from_value(current.principal_reference.clone())
                                .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        if persisted_state(&current)? != AiRunState::Running
                            || current.session_id != lease.session_id.0
                            || current.input_message_id != lease.input_message_id
                            || stored_reference != lease.principal_reference
                            || current.attempt_id != Some(lease.attempt_id)
                            || current.lease_owner.as_deref() != Some(lease.worker_id.as_str())
                            || current.lease_generation != lease.lease_generation
                            || current.retry_count != i64::from(lease.retry_count)
                            || current.latest_checkpoint_id != lease.latest_checkpoint_id
                            || current
                                .lease_expires_at
                                .is_none_or(|expiry| expiry <= now.unix_timestamp())
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }

                        let checkpoint_id = current
                            .latest_checkpoint_id
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        let checkpoint = tx
                            .query::<AiRunCheckpointRecord>()
                            .filter(AiRunCheckpointRecordWhereInput {
                                id: Some(UuidFilter {
                                    eq: Some(checkpoint_id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .internal_limit(1)
                            .fetch_one()
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                        if checkpoint.run_id != lease.run_id.0
                            || checkpoint.attempt_id != lease.attempt_id
                            || checkpoint.lease_generation != lease.lease_generation
                            || checkpoint.checkpoint_kind != "assistant_output_persisted"
                            || checkpoint.provider_response_id != event.provider_response_id
                            || checkpoint.budget_reservation_id != Some(event.budget_reservation_id)
                            || checkpoint.assistant_message_id != Some(checkpoint_id)
                            || checkpoint.protected_state.is_some()
                            || checkpoint.checkpoint_hash
                                != final_output_checkpoint_hash(
                                    lease.run_id,
                                    lease.attempt_id,
                                    lease.lease_generation,
                                    checkpoint_id,
                                    event.provider_response_id.as_deref(),
                                    event.budget_reservation_id,
                                )
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }

                        if let Some(existing) = tx
                            .find_by_id::<AiSessionEventRecord>(&event.id)
                            .await
                            .map_err(OrmPublicError::from)?
                        {
                            let inbox = tx
                                .find_by_id::<AiInboxEventRecord>(&event.inbox_event_id)
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                            if lease.row_version.checked_add(1) != Some(current.row_version)
                                || existing.session_id != lease.session_id.0
                                || existing.event_type != "ui_intent_suggested"
                                || existing.run_id != Some(lease.run_id.0)
                                || existing.correlation_id != event.correlation_id
                                || inbox.session_id != Some(lease.session_id.0)
                                || inbox.event_type != "ui_intent_suggested"
                                || inbox.principal_kind != event.expected_owner_principal_kind
                                || inbox.principal_subject != event.expected_owner_subject
                            {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            return Ok((lease_from_record(&current)?, existing.sequence));
                        }
                        if current.row_version != lease.row_version {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }

                        let reservation = tx
                            .find_by_id::<AiBudgetReservationRecord>(&event.budget_reservation_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if reservation.scope_kind != event.expected_scope_kind
                            || reservation.scope_id != event.expected_scope_id
                            || reservation.tenant_id != event.expected_tenant_id
                            || reservation.principal_kind != event.expected_owner_principal_kind
                            || reservation.principal_subject != event.expected_owner_subject
                            || reservation.session_id != lease.session_id.0
                            || reservation.run_id != lease.run_id.0
                            || reservation.attempt_id != lease.attempt_id
                            || reservation.lease_generation != lease.lease_generation
                            || reservation.provider_kind != event.provider_kind
                            || reservation.provider_model != event.provider_model
                            || reservation.state != "committed"
                            || reservation.reconciled_at.is_none()
                            || !reservation_usage_matches(
                                &reservation,
                                event.usage,
                                event.cached_input_tokens,
                            )?
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session.state != "active"
                            || session.deleted_at.is_some()
                            || session.owner_principal_kind != event.expected_owner_principal_kind
                            || session.owner_subject != event.expected_owner_subject
                            || session.scope_kind != event.expected_scope_kind
                            || session.scope_id != event.expected_scope_id
                            || session.tenant_id != event.expected_tenant_id
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let event_sequence = session
                            .stream_head
                            .checked_add(1)
                            .filter(|sequence| *sequence <= i64::from(i32::MAX))
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        if !matches!(
                            tx.compare_and_swap::<AiSessionRecord>(
                                &session.id,
                                session.row_version,
                                AiSessionRecordWhereInput::default(),
                                UpdateAiSessionRecordInput {
                                    stream_head: Some(event_sequence),
                                    last_activity_at: Some(now.unix_timestamp()),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?,
                            ConditionalUpdateOutcome::Updated(_)
                        ) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let expiry = now
                            .checked_add(lease_ttl)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let updated_run = match tx
                            .compare_and_swap::<AiRunRecord>(
                                &current.id,
                                current.row_version,
                                exact_state(AiRunState::Running.as_str()),
                                UpdateAiRunRecordInput {
                                    lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                    lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?
                        {
                            ConditionalUpdateOutcome::Updated(updated) => updated,
                            ConditionalUpdateOutcome::NotFound
                            | ConditionalUpdateOutcome::Conflict => {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                        };
                        tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                            id: event.id,
                            session_id: session.id,
                            sequence: event_sequence,
                            event_type: "ui_intent_suggested".to_owned(),
                            run_id: Some(lease.run_id.0),
                            causation_id: Some(lease.input_message_id.to_string()),
                            correlation_id: event.correlation_id.clone(),
                            protected_payload: event.protected_payload,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        tx.queue_event(AiSessionWakeup {
                            session_id: session.id,
                            sequence: event_sequence,
                        });
                        append_inbox_event(
                            tx,
                            PreparedAiInboxEvent {
                                id: event.inbox_event_id,
                                principal_kind: event.expected_owner_principal_kind,
                                principal_subject: event.expected_owner_subject.clone(),
                                scope: crate::AiScope {
                                    kind: event.expected_scope_kind,
                                    id: event.expected_scope_id,
                                    tenant_id: event.expected_tenant_id,
                                },
                                session_id: session.id,
                                event_type: "ui_intent_suggested".to_owned(),
                                protected_payload: event.protected_inbox_payload,
                                created_at: now.unix_timestamp(),
                            },
                        )
                        .await?;
                        tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
                            actor_principal_kind: reservation.principal_kind,
                            actor_subject: event.expected_owner_subject,
                            action: "ai.ui_intent.persist".to_owned(),
                            resource_kind: "ai_ui_intent".to_owned(),
                            resource_reference: event.id.to_string(),
                            outcome: "allowed".to_owned(),
                            reason_code: "validated_ui_intent_suggested".to_owned(),
                            correlation_id: event.correlation_id,
                            causation_id: Some(lease.input_message_id.to_string()),
                            policy_version: None,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        Ok((lease_from_record(&updated_run)?, event_sequence))
                    })
                })
        })
        .await
        .map_err(map_transaction)
    }

    pub(crate) async fn begin_tool_call(
        &self,
        lease: &AiRunLease,
        call: PreparedToolCallStart,
    ) -> Result<AiRunLease, AiError> {
        self.begin_tool_call_inner(lease, call, None, None).await
    }

    pub(crate) async fn prepare_native_approval(
        &self,
        lease: &AiRunLease,
        mut call: PreparedToolCallStart,
        candidate: PreparedNativeApprovalCandidate,
    ) -> Result<AiPreparedNativeApproval, AiError> {
        if !candidate.valid() || call.risk == "read_only" || call.started_event.is_some() {
            return Err(AiError::InvalidInput(
                "invalid native approval preparation".to_owned(),
            ));
        }
        let tool_call_id = AiToolCallId(call.id);
        let provider_call_id = call.provider_call_id.clone();
        let PreparedNativeApprovalCandidate {
            binding_hash,
            preview_hash,
            protected_preparation,
            prepared_event,
        } = candidate;
        call.started_event = Some(prepared_event);
        let lease = self
            .begin_tool_call_inner(
                lease,
                call,
                Some((binding_hash, preview_hash, protected_preparation)),
                None,
            )
            .await?;
        Ok(AiPreparedNativeApproval {
            tool_call_id,
            provider_call_id,
            lease,
        })
    }

    pub(crate) async fn persist_blocked_native_call(
        &self,
        lease: &AiRunLease,
        mut call: PreparedToolCallStart,
        blocked: PreparedNativeBlockedReceipt,
    ) -> Result<AiRunLease, AiError> {
        if call.risk == "read_only"
            || call.started_event.is_some()
            || blocked.candidate_id.0 == call.id
            || !valid_digest(&blocked.disclosure_schema_fingerprint)
            || blocked.receipt.egress_decision_id.is_nil()
            || !valid_digest(&blocked.receipt.receipt_hash)
            || !valid_digest(&blocked.receipt.egress_manifest_hash)
            || !blocked.receipt.protected_receipt.is_object()
            || serde_json::to_vec(&blocked.receipt.protected_receipt)
                .map_or(true, |bytes| bytes.len() > 256 * 1024)
        {
            return Err(AiError::Conflict);
        }
        call.started_event = Some(blocked.event);
        self.begin_tool_call_inner(
            lease,
            call,
            None,
            Some(NativeBlockedCall {
                candidate_id: blocked.candidate_id,
                receipt: blocked.receipt,
                disclosure_schema_fingerprint: blocked.disclosure_schema_fingerprint,
            }),
        )
        .await
    }

    async fn begin_tool_call_inner(
        &self,
        lease: &AiRunLease,
        call: PreparedToolCallStart,
        native: Option<(String, String, serde_json::Value)>,
        blocked: Option<NativeBlockedCall>,
    ) -> Result<AiRunLease, AiError> {
        if call.execution_provenance.is_some() && call.native_preflight_refusal.is_some() {
            return Err(AiError::Conflict);
        }
        if call
            .execution_provenance
            .as_ref()
            .is_some_and(|provenance| {
                provenance.session_id() != lease.session_id()
                    || provenance.run_id() != lease.run_id()
                    || provenance.tool_call_id().0 != call.id
                    || provenance.attempt_id() != lease.attempt_id()
                    || provenance.lease_generation() != lease.lease_generation()
                    || provenance.provider_kind().as_str() != call.provider_kind
                    || provenance.provider_model() != call.provider_model
                    || provenance.provider_call_id() != call.provider_call_id
                    || provenance.provider_response_id() != call.provider_response_id.as_deref()
                    || provenance.budget_reservation_id().0 != call.budget_reservation_id
                    || provenance.tool_fingerprint() != call.tool_fingerprint
                    || provenance.argument_hash() != call.argument_hash
                    || provenance.approval_id().is_some()
            })
        {
            return Err(AiError::Conflict);
        }
        let execution_provenance = call
            .execution_provenance
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| AiError::PersistenceFailed)?
            .or_else(|| {
                call.native_preflight_refusal
                    .as_ref()
                    .map(|refusal| refusal.to_json())
            });
        let now = canonical_second(self.clock.now());
        let lease_ttl = self.limits.lease_ttl;
        let lease = lease.clone();
        retry_transaction(self.limits.maximum_transaction_retries, || {
            let blocked = blocked.clone();
            let call = call.clone();
            let execution_provenance = execution_provenance.clone();
            let lease = lease.clone();
            let native = native.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let current = load_and_validate_active_lease(tx, &lease, now).await?;
                        if persisted_state(&current)? != AiRunState::Running {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if !session_matches_tool_start(&session, &call) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        if call.risk != "read_only" {
                            let pending = tx
                                .query::<AiNativeApprovalCandidateRecord>()
                                .filter(AiNativeApprovalCandidateRecordWhereInput {
                                    run_id: Some(UuidFilter {
                                        eq: Some(lease.run_id.0),
                                        ..Default::default()
                                    }),
                                    state: Some(StringFilter {
                                        eq: Some("prepared".to_owned()),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                })
                                .internal_limit(2)
                                .fetch_all()
                                .await
                                .map_err(OrmPublicError::from)?;
                            if let Some(blocked) = &blocked {
                                if pending.len() != 1 || pending[0].id != blocked.candidate_id.0 {
                                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                                }
                                let candidate =
                                    load_native_candidate(tx, &lease, blocked.candidate_id).await?;
                                if candidate.budget_reservation_id != call.budget_reservation_id
                                    || candidate.protected_control_receipt.is_none()
                                {
                                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                                }
                            } else if !pending.is_empty() {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                        }
                        if native.is_some() || blocked.is_some() {
                            let budget = tx
                                .find_by_id::<AiBudgetReservationRecord>(
                                    &call.budget_reservation_id,
                                )
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(OrmPublicError::not_found)?;
                            if budget.state != "uncertain"
                                || budget.run_id != current.id
                                || budget.session_id != session.id
                                || budget.attempt_id != lease.attempt_id
                                || budget.lease_generation != lease.lease_generation
                                || budget.principal_kind != session.owner_principal_kind
                                || budget.principal_subject != lease.principal_reference.subject
                                || budget.provider_kind != call.provider_kind
                                || budget.provider_model != call.provider_model
                                || budget.scope_kind != session.scope_kind
                                || budget.scope_id != session.scope_id
                                || budget.tenant_id != session.tenant_id
                            {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                        }
                        let lifecycle_kind = if blocked.is_some() {
                            "native_tool_control"
                        } else if native.is_some() {
                            "native_approval_prepared"
                        } else {
                            "application_tool_started"
                        };
                        let call_state = if blocked.is_some() {
                            "control_blocked"
                        } else if native.is_some() {
                            "approval_prepared"
                        } else {
                            "executing"
                        };
                        let step_state = if blocked.is_some() {
                            "control_blocked"
                        } else if native.is_some() {
                            "approval_prepared"
                        } else {
                            "running"
                        };
                        let (updated_run, event_sequence) = if call.started_event.is_some() {
                            let event_sequence = session
                                .stream_head
                                .checked_add(1)
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            if !matches!(
                                tx.compare_and_swap::<AiSessionRecord>(
                                    &session.id,
                                    session.row_version,
                                    AiSessionRecordWhereInput::default(),
                                    UpdateAiSessionRecordInput {
                                        stream_head: Some(event_sequence),
                                        last_activity_at: Some(now.unix_timestamp()),
                                        ..Default::default()
                                    },
                                )
                                .await
                                .map_err(OrmPublicError::from)?,
                                ConditionalUpdateOutcome::Updated(_)
                            ) {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            let expiry = now
                                .checked_add(lease_ttl)
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            let updated_run = match tx
                                .compare_and_swap::<AiRunRecord>(
                                    &current.id,
                                    current.row_version,
                                    exact_state(AiRunState::Running.as_str()),
                                    UpdateAiRunRecordInput {
                                        lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                        lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                                        ..Default::default()
                                    },
                                )
                                .await
                                .map_err(OrmPublicError::from)?
                            {
                                ConditionalUpdateOutcome::Updated(updated) => updated,
                                ConditionalUpdateOutcome::NotFound
                                | ConditionalUpdateOutcome::Conflict => {
                                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                                }
                            };
                            (updated_run, Some(event_sequence))
                        } else {
                            (current, None)
                        };
                        tx.insert::<AiRunStepRecord>(CreateAiRunStepRecordInput {
                            id: call.id,
                            run_id: lease.run_id.0,
                            step_index: call
                                .provider_turn_index
                                .checked_mul(64)
                                .and_then(|value| value.checked_add(call.tool_call_index))
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InvalidInput))?,
                            step_kind: "application_tool".to_owned(),
                            state: step_state.to_owned(),
                            lease_generation: lease.lease_generation,
                            started_at: Some(now.unix_timestamp()),
                            finished_at: blocked.as_ref().map(|_| now.unix_timestamp()),
                            error_code: None,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        tx.insert::<AiToolCallRecord>(CreateAiToolCallRecordInput {
                            id: call.id,
                            run_id: lease.run_id.0,
                            provider_call_key: call.provider_call_key,
                            provider_call_id: call.provider_call_id,
                            provider_kind: Some(call.provider_kind),
                            provider_model: Some(call.provider_model),
                            provider_response_id: call.provider_response_id,
                            budget_reservation_id: Some(call.budget_reservation_id),
                            provider_turn_index: call.provider_turn_index,
                            tool_call_index: call.tool_call_index,
                            tool_id: call.tool_id,
                            tool_fingerprint: call.tool_fingerprint,
                            execution_provenance,
                            protected_arguments: Some(call.protected_arguments),
                            argument_hash: call.argument_hash,
                            protected_result: blocked
                                .as_ref()
                                .map(|blocked| blocked.receipt.protected_receipt.clone()),
                            protected_result_source: None,
                            payload_purged_at: None,
                            risk: call.risk,
                            authorization_code: blocked
                                .as_ref()
                                .map(|_| "native_consequential_paused".to_owned()),
                            authorization_policy_version: None,
                            authorization_state_digest: None,
                            disclosure_schema_fingerprint: blocked
                                .as_ref()
                                .map(|blocked| blocked.disclosure_schema_fingerprint.clone()),
                            result_classification: blocked.as_ref().map(|_| "internal".to_owned()),
                            result_egress_decision_id: blocked
                                .as_ref()
                                .map(|blocked| blocked.receipt.egress_decision_id),
                            result_egress_manifest_hash: blocked
                                .as_ref()
                                .map(|blocked| blocked.receipt.egress_manifest_hash.clone()),
                            application_audit_ref: None,
                            approval_id: None,
                            idempotency_key: call.idempotency_key,
                            correlation_id: Some(call.correlation_id.clone()),
                            causation_id: Some(call.causation_id),
                            delegation_reference: call.delegation_reference,
                            lease_generation: lease.lease_generation,
                            state: call_state.to_owned(),
                            completed_at: blocked.as_ref().map(|_| now.unix_timestamp()),
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        if let Some((binding_hash, preview_hash, protected_preparation)) = native {
                            tx.insert::<AiNativeApprovalCandidateRecord>(
                                CreateAiNativeApprovalCandidateRecordInput {
                                    id: call.id,
                                    run_id: lease.run_id.0,
                                    attempt_id: lease.attempt_id,
                                    lease_generation: lease.lease_generation,
                                    budget_reservation_id: call.budget_reservation_id,
                                    binding_hash,
                                    preview_hash,
                                    protected_preparation: Some(protected_preparation),
                                    protected_control_receipt: None,
                                    control_receipt_hash: None,
                                    control_egress_decision_id: None,
                                    control_egress_manifest_hash: None,
                                    state: "prepared".to_owned(),
                                    final_approval_id: None,
                                    settled_checkpoint_id: None,
                                    payload_purged_at: None,
                                    finalized_at: None,
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        }
                        if let (Some(event), Some(event_sequence)) =
                            (call.started_event, event_sequence)
                        {
                            tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                                id: event.event_id,
                                session_id: lease.session_id.0,
                                sequence: event_sequence,
                                event_type: lifecycle_kind.to_owned(),
                                run_id: Some(lease.run_id.0),
                                causation_id: Some(call.id.to_string()),
                                correlation_id: call.correlation_id.clone(),
                                protected_payload: event.protected_event,
                            })
                            .await
                            .map_err(OrmPublicError::from)?;
                            append_inbox_event(
                                tx,
                                PreparedAiInboxEvent {
                                    id: event.inbox_event_id,
                                    principal_kind: call.expected_owner_principal_kind,
                                    principal_subject: call.expected_owner_subject,
                                    scope: AiScope {
                                        kind: call.expected_scope_kind,
                                        id: call.expected_scope_id,
                                        tenant_id: call.expected_tenant_id,
                                    },
                                    session_id: lease.session_id.0,
                                    event_type: lifecycle_kind.to_owned(),
                                    protected_payload: event.protected_inbox_event,
                                    created_at: now.unix_timestamp(),
                                },
                            )
                            .await?;
                            tx.queue_event(AiSessionWakeup {
                                session_id: lease.session_id.0,
                                sequence: event_sequence,
                            });
                        }
                        lease_from_record(&updated_run)
                    })
                })
        })
        .await
        .map_err(map_transaction)
    }

    pub(crate) async fn load_prepared_native_approval(
        &self,
        lease: &AiRunLease,
        id: AiToolCallId,
    ) -> Result<AiNativeApprovalCandidateRecord, AiError> {
        let now = canonical_second(self.clock.now());
        let lease = lease.clone();
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let run = load_and_validate_active_lease(tx, &lease, now).await?;
                    if run.state != "running" {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    load_native_candidate(tx, &lease, id).await
                })
            })
            .await
            .map_err(map_transaction)
    }

    pub(crate) async fn persist_native_approval_receipt(
        &self,
        lease: &AiRunLease,
        id: AiToolCallId,
        receipt: PreparedNativeApprovalReceipt,
    ) -> Result<AiRunLease, AiError> {
        if receipt.egress_decision_id.is_nil()
            || !valid_digest(&receipt.receipt_hash)
            || !valid_digest(&receipt.egress_manifest_hash)
            || !receipt.protected_receipt.is_object()
            || serde_json::to_vec(&receipt.protected_receipt)
                .map_or(true, |bytes| bytes.len() > 256 * 1024)
        {
            return Err(AiError::InvalidInput(
                "invalid native control receipt".to_owned(),
            ));
        }
        let now = canonical_second(self.clock.now());
        let lease = lease.clone();
        let ttl = self.limits.lease_ttl;
        retry_transaction(self.limits.maximum_transaction_retries, || {
            let lease = lease.clone();
            let receipt = receipt.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let current = load_and_validate_active_lease(tx, &lease, now).await?;
                        if current.state != "running" {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let candidate = load_native_candidate(tx, &lease, id).await?;
                        if let Some(hash) = candidate.control_receipt_hash {
                            if hash == receipt.receipt_hash
                                && candidate.control_egress_decision_id
                                    == Some(receipt.egress_decision_id)
                                && candidate.control_egress_manifest_hash.as_deref()
                                    == Some(receipt.egress_manifest_hash.as_str())
                                && candidate.protected_control_receipt.as_ref()
                                    == Some(&receipt.protected_receipt)
                            {
                                return lease_from_record(&current);
                            }
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        if candidate.protected_control_receipt.is_some()
                            || candidate.control_egress_decision_id.is_some()
                            || candidate.control_egress_manifest_hash.is_some()
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        if !matches!(
                            tx.compare_and_swap::<AiNativeApprovalCandidateRecord>(
                                &candidate.id,
                                candidate.row_version,
                                AiNativeApprovalCandidateRecordWhereInput::default(),
                                UpdateAiNativeApprovalCandidateRecordInput {
                                    protected_control_receipt: Some(Some(
                                        receipt.protected_receipt
                                    )),
                                    control_receipt_hash: Some(Some(receipt.receipt_hash)),
                                    control_egress_decision_id: Some(Some(
                                        receipt.egress_decision_id
                                    )),
                                    control_egress_manifest_hash: Some(Some(
                                        receipt.egress_manifest_hash
                                    )),
                                    ..Default::default()
                                }
                            )
                            .await
                            .map_err(OrmPublicError::from)?,
                            ConditionalUpdateOutcome::Updated(_)
                        ) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let expiry = now
                            .checked_add(ttl)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        match tx
                            .compare_and_swap::<AiRunRecord>(
                                &current.id,
                                current.row_version,
                                exact_state("running"),
                                UpdateAiRunRecordInput {
                                    lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                    lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?
                        {
                            ConditionalUpdateOutcome::Updated(run) => lease_from_record(&run),
                            _ => Err(OrmPublicError::new(OrmErrorCode::Conflict)),
                        }
                    })
                })
        })
        .await
        .map_err(map_transaction)
    }

    pub(crate) async fn append_proposal(
        &self,
        lease: &AiRunLease,
        proposal: PreparedProposal,
    ) -> Result<AiRunLease, AiError> {
        let now = canonical_second(self.clock.now());
        let lease_ttl = self.limits.lease_ttl;
        let lease = lease.clone();
        retry_transaction(self.limits.maximum_transaction_retries, || {
            let lease = lease.clone();
            let proposal = proposal.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let current = load_and_validate_active_lease(tx, &lease, now).await?;
                        if persisted_state(&current)? != AiRunState::Running {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session.state != "active"
                            || session.deleted_at.is_some()
                            || session.owner_principal_kind
                                != proposal.expected_owner_principal_kind
                            || session.owner_subject != proposal.expected_owner_subject
                            || session.scope_kind != proposal.expected_scope_kind
                            || session.scope_id != proposal.expected_scope_id
                            || session.tenant_id != proposal.expected_tenant_id
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let event_sequence = session
                            .stream_head
                            .checked_add(1)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let session_update = tx
                            .compare_and_swap::<AiSessionRecord>(
                                &session.id,
                                session.row_version,
                                AiSessionRecordWhereInput::default(),
                                UpdateAiSessionRecordInput {
                                    stream_head: Some(event_sequence),
                                    last_activity_at: Some(now.unix_timestamp()),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(session_update, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let expiry = now
                            .checked_add(lease_ttl)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let run_update = tx
                            .compare_and_swap::<AiRunRecord>(
                                &current.id,
                                current.row_version,
                                exact_state(AiRunState::Running.as_str()),
                                UpdateAiRunRecordInput {
                                    lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                    lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        let updated_run = match run_update {
                            ConditionalUpdateOutcome::Updated(updated) => updated,
                            ConditionalUpdateOutcome::NotFound
                            | ConditionalUpdateOutcome::Conflict => {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                        };
                        tx.insert::<AiProposalRecord>(CreateAiProposalRecordInput {
                            id: proposal.id,
                            session_id: lease.session_id.0,
                            run_id: lease.run_id.0,
                            scope_kind: proposal.expected_scope_kind,
                            scope_id: proposal.expected_scope_id,
                            proposal_type: proposal.proposal_type,
                            schema_version: proposal.schema_version,
                            item_count: proposal.item_count,
                            protected_payload: Some(proposal.protected_payload),
                            source_references: Some(proposal.source_references),
                            payload_purged_at: None,
                            state: "pending_review".to_owned(),
                            created_by_subject: proposal.created_by_subject,
                            reviewed_by_subject: None,
                            applied_resource_ref: None,
                            application_audit_ref: None,
                            reviewed_at: None,
                            expires_at: proposal.expires_at,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                            id: proposal.event_id,
                            session_id: lease.session_id.0,
                            sequence: event_sequence,
                            event_type: "proposal_created".to_owned(),
                            run_id: Some(lease.run_id.0),
                            causation_id: Some(lease.input_message_id.to_string()),
                            correlation_id: proposal.correlation_id,
                            protected_payload: proposal.protected_event,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        tx.queue_event(AiSessionWakeup {
                            session_id: lease.session_id.0,
                            sequence: event_sequence,
                        });
                        lease_from_record(&updated_run)
                    })
                })
        })
        .await
        .map_err(map_transaction)
    }

    pub(crate) async fn request_approval(
        &self,
        lease: &AiRunLease,
        approval: PreparedApprovalRequest,
    ) -> Result<AiRunLease, AiError> {
        self.request_approval_inner(lease, approval, None).await
    }

    pub(crate) async fn request_prepared_native_approval(
        &self,
        lease: &AiRunLease,
        approval: PreparedApprovalRequest,
        candidate_id: AiToolCallId,
    ) -> Result<AiRunLease, AiError> {
        if approval.parked_provider_wait.is_none() {
            return Err(AiError::Conflict);
        }
        self.request_approval_inner(lease, approval, Some(candidate_id))
            .await
    }

    async fn request_approval_inner(
        &self,
        lease: &AiRunLease,
        approval: PreparedApprovalRequest,
        native_candidate_id: Option<AiToolCallId>,
    ) -> Result<AiRunLease, AiError> {
        let now = canonical_second(self.clock.now());
        let lease_ttl = self.limits.lease_ttl;
        let lease = lease.clone();
        retry_transaction(self.limits.maximum_transaction_retries, || {
            let approval = approval.clone();
            let lease = lease.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let current = load_and_validate_active_lease(tx, &lease, now).await?;
                        if persisted_state(&current)? != AiRunState::Running
                            || approval.expires_at <= now.unix_timestamp()
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session.state != "active"
                            || session.deleted_at.is_some()
                            || session.owner_principal_kind
                                != approval.expected_owner_principal_kind
                            || session.owner_subject != approval.expected_owner_subject
                            || session.scope_kind != approval.expected_scope_kind
                            || session.scope_id != approval.expected_scope_id
                            || session.tenant_id != approval.expected_tenant_id
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let call = tx
                            .find_by_id::<AiToolCallRecord>(&approval.tool_call_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if call.run_id != lease.run_id.0
                            || call.lease_generation != lease.lease_generation
                            || call.state
                                != if native_candidate_id.is_some() {
                                    "approval_prepared"
                                } else {
                                    "executing"
                                }
                            || call.approval_id.is_some()
                            || call.argument_hash != approval.argument_hash
                            || call.tool_fingerprint != approval.tool_fingerprint
                            || call.risk == "read_only"
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let native_candidate = if let Some(id) = native_candidate_id {
                            if id.0 != call.id {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            let candidate = load_native_candidate(tx, &lease, id).await?;
                            let checkpoint_id = current
                                .latest_checkpoint_id
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                            let checkpoint = tx
                                .query::<AiRunCheckpointRecord>()
                                .filter(AiRunCheckpointRecordWhereInput {
                                    id: Some(UuidFilter {
                                        eq: Some(checkpoint_id),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                })
                                .internal_limit(1)
                                .fetch_one()
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(OrmPublicError::not_found)?;
                            let budget = tx
                                .find_by_id::<AiBudgetReservationRecord>(
                                    &candidate.budget_reservation_id,
                                )
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(OrmPublicError::not_found)?;
                            if candidate.binding_hash != approval.binding_hash
                                || candidate.preview_hash != approval.action_preview_hash
                                || candidate.protected_control_receipt.is_none()
                                || candidate
                                    .control_receipt_hash
                                    .as_deref()
                                    .is_none_or(|hash| !valid_digest(hash))
                                || candidate
                                    .control_egress_decision_id
                                    .is_none_or(|id| id.is_nil())
                                || candidate
                                    .control_egress_manifest_hash
                                    .as_deref()
                                    .is_none_or(|hash| !valid_digest(hash))
                                || checkpoint.run_id != current.id
                                || checkpoint.attempt_id != lease.attempt_id
                                || checkpoint.lease_generation != lease.lease_generation
                                || checkpoint.checkpoint_kind
                                    != "native_approval_provider_turn_persisted"
                                || checkpoint.budget_reservation_id
                                    != Some(candidate.budget_reservation_id)
                                || checkpoint.provider_response_id != call.provider_response_id
                                || checkpoint.protected_state.is_none()
                                || !valid_digest(&checkpoint.checkpoint_hash)
                                || budget.state != "committed"
                                || budget.run_id != current.id
                                || budget.session_id != session.id
                                || budget.attempt_id != lease.attempt_id
                                || budget.lease_generation != lease.lease_generation
                                || budget.principal_kind != session.owner_principal_kind
                                || budget.principal_subject != lease.principal_reference.subject
                                || Some(budget.provider_kind.as_str())
                                    != call.provider_kind.as_deref()
                                || Some(budget.provider_model.as_str())
                                    != call.provider_model.as_deref()
                                || budget.scope_kind != session.scope_kind
                                || budget.scope_id != session.scope_id
                                || budget.tenant_id != session.tenant_id
                                || budget.reconciled_at.is_none()
                                || budget.actual_runs != Some(1)
                                || [
                                    budget.actual_input_tokens,
                                    budget.actual_cached_input_tokens,
                                    budget.actual_output_tokens,
                                    budget.actual_tool_units,
                                    budget.actual_image_units,
                                    budget.actual_cost_microunits,
                                ]
                                .iter()
                                .any(|amount| amount.is_none_or(|value| value < 0))
                                || budget.actual_cached_input_tokens > budget.actual_input_tokens
                            {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            Some((candidate, checkpoint_id))
                        } else {
                            None
                        };
                        let parked_provider_wait = if let Some(parked) =
                            approval.parked_provider_wait.clone()
                        {
                            let source = tx
                                .query::<AiRunCheckpointRecord>()
                                .filter(AiRunCheckpointRecordWhereInput {
                                    id: Some(UuidFilter {
                                        eq: Some(parked.source_checkpoint_id),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                })
                                .internal_limit(1)
                                .fetch_one()
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(OrmPublicError::not_found)?;
                            let expected_hash = coordinator_checkpoint_hash(
                                lease.run_id,
                                lease.attempt_id,
                                lease.lease_generation,
                                parked.parked_checkpoint_id,
                                "approval_wait_parked",
                                &parked.provider_kind,
                                &parked.provider_model,
                                parked.provider_response_id.as_deref(),
                                parked.budget_reservation_id,
                                &parked.protected_parked_checkpoint,
                            )
                            .map_err(|_| OrmPublicError::new(OrmErrorCode::Conflict))?;
                            if current.latest_checkpoint_id != Some(source.id)
                                || source.id != parked.source_checkpoint_id
                                || source.run_id != current.id
                                || source.attempt_id != lease.attempt_id
                                || source.lease_generation != lease.lease_generation
                                || source.checkpoint_kind
                                    != if native_candidate_id.is_some() {
                                        "native_approval_provider_turn_persisted"
                                    } else {
                                        "provider_turn_persisted"
                                    }
                                || source.checkpoint_hash != parked.source_checkpoint_fingerprint
                                || source.provider_response_id != parked.provider_response_id
                                || source.budget_reservation_id
                                    != Some(parked.budget_reservation_id)
                                || call.provider_kind.as_deref()
                                    != Some(parked.provider_kind.as_str())
                                || call.provider_model.as_deref()
                                    != Some(parked.provider_model.as_str())
                                || call.provider_response_id != parked.provider_response_id
                                || call.budget_reservation_id != Some(parked.budget_reservation_id)
                                || expected_hash != parked.parked_checkpoint_fingerprint
                            {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            Some((parked, source))
                        } else {
                            None
                        };
                        let event_sequence = session
                            .stream_head
                            .checked_add(1)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let session_update = tx
                            .compare_and_swap::<AiSessionRecord>(
                                &session.id,
                                session.row_version,
                                AiSessionRecordWhereInput::default(),
                                UpdateAiSessionRecordInput {
                                    stream_head: Some(event_sequence),
                                    last_activity_at: Some(now.unix_timestamp()),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(session_update, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        if let Some((candidate, checkpoint_id)) = native_candidate {
                            let step = tx
                                .find_by_id::<AiRunStepRecord>(&candidate.id)
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(OrmPublicError::not_found)?;
                            if step.run_id != current.id
                                || step.lease_generation != lease.lease_generation
                                || step.state != "approval_prepared"
                                || step.finished_at.is_some()
                            {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            if !matches!(
                                tx.compare_and_swap::<AiRunStepRecord>(
                                    &step.id,
                                    step.row_version,
                                    AiRunStepRecordWhereInput::default(),
                                    UpdateAiRunStepRecordInput {
                                        state: Some("running".to_owned()),
                                        ..Default::default()
                                    }
                                )
                                .await
                                .map_err(OrmPublicError::from)?,
                                ConditionalUpdateOutcome::Updated(_)
                            ) {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            if !matches!(
                                tx.compare_and_swap::<AiNativeApprovalCandidateRecord>(
                                    &candidate.id,
                                    candidate.row_version,
                                    AiNativeApprovalCandidateRecordWhereInput::default(),
                                    UpdateAiNativeApprovalCandidateRecordInput {
                                        state: Some("finalized".to_owned()),
                                        final_approval_id: Some(Some(approval.id)),
                                        settled_checkpoint_id: Some(Some(checkpoint_id)),
                                        finalized_at: Some(Some(now.unix_timestamp())),
                                        ..Default::default()
                                    }
                                )
                                .await
                                .map_err(OrmPublicError::from)?,
                                ConditionalUpdateOutcome::Updated(_)
                            ) {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                        }
                        let call_update = tx
                            .compare_and_swap::<AiToolCallRecord>(
                                &call.id,
                                call.row_version,
                                AiToolCallRecordWhereInput::default(),
                                UpdateAiToolCallRecordInput {
                                    approval_id: Some(Some(approval.id)),
                                    state: Some("waiting_approval".to_owned()),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(call_update, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let has_parked_provider_wait = parked_provider_wait.is_some();
                        let run_update_input =
                            if let Some((parked, _)) = parked_provider_wait.as_ref() {
                                UpdateAiRunRecordInput {
                                    state: Some(AiRunState::WaitingApproval.as_str().to_owned()),
                                    attempt_id: Some(None),
                                    lease_owner: Some(None),
                                    lease_expires_at: Some(None),
                                    lease_heartbeat_at: Some(None),
                                    next_attempt_at: Some(None),
                                    latest_checkpoint_id: Some(Some(parked.parked_checkpoint_id)),
                                    error_code: Some(Some("approval_wait_parked".to_owned())),
                                    ..Default::default()
                                }
                            } else {
                                let expiry = now.checked_add(lease_ttl).ok_or_else(|| {
                                    OrmPublicError::new(OrmErrorCode::InternalError)
                                })?;
                                UpdateAiRunRecordInput {
                                    state: Some(AiRunState::WaitingApproval.as_str().to_owned()),
                                    lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                    lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                                    ..Default::default()
                                }
                            };
                        let run_update = tx
                            .compare_and_swap::<AiRunRecord>(
                                &current.id,
                                current.row_version,
                                exact_state(AiRunState::Running.as_str()),
                                run_update_input,
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        let updated_run = match run_update {
                            ConditionalUpdateOutcome::Updated(updated) => updated,
                            ConditionalUpdateOutcome::NotFound
                            | ConditionalUpdateOutcome::Conflict => {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                        };
                        tx.insert::<AiApprovalRecord>(CreateAiApprovalRecordInput {
                            id: approval.id,
                            tool_call_id: approval.tool_call_id,
                            session_id: lease.session_id.0,
                            principal_subject: approval.principal_subject,
                            principal_reference_fingerprint: approval
                                .principal_reference_fingerprint,
                            delegated_actor_subject: approval.delegated_actor_subject,
                            delegation_reference: approval.delegation_reference,
                            argument_hash: approval.argument_hash,
                            tool_fingerprint: approval.tool_fingerprint,
                            binding_hash: approval.binding_hash,
                            execution_target_id: approval.execution_target_id,
                            target_schema_fingerprint: approval.target_schema_fingerprint,
                            operation_name: approval.operation_name,
                            operation_document_hash: approval.operation_document_hash,
                            result_projection_fingerprint: approval.result_projection_fingerprint,
                            disclosure_schema_fingerprint: approval.disclosure_schema_fingerprint,
                            policy_version: approval.policy_version,
                            authorization_state_digest: approval.authorization_state_digest,
                            protected_resource_bindings: Some(approval.protected_resource_bindings),
                            protected_action_preview: Some(approval.protected_action_preview),
                            payload_purged_at: None,
                            action_preview_hash: approval.action_preview_hash,
                            state: "pending".to_owned(),
                            recent_mfa_required: approval.recent_mfa_required,
                            approver_subject: None,
                            expires_at: approval.expires_at,
                            decided_at: None,
                            maximum_uses: 1,
                            consumed_uses: 0,
                            consumed_at: None,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        if let Some((parked, source)) = parked_provider_wait {
                            tx.insert::<AiRunCheckpointRecord>(CreateAiRunCheckpointRecordInput {
                                id: parked.parked_checkpoint_id,
                                run_id: current.id,
                                attempt_id: lease.attempt_id,
                                lease_generation: lease.lease_generation,
                                checkpoint_kind: "approval_wait_parked".to_owned(),
                                provider_response_id: parked.provider_response_id.clone(),
                                budget_reservation_id: Some(parked.budget_reservation_id),
                                assistant_message_id: None,
                                protected_state: Some(parked.protected_parked_checkpoint.clone()),
                                checkpoint_hash: parked.parked_checkpoint_fingerprint.clone(),
                            })
                            .await
                            .map_err(OrmPublicError::from)?;
                            append_attempt_outcome(
                                tx,
                                &lease,
                                AiRunState::WaitingApproval,
                                "approval_wait_parked".to_owned(),
                                source.provider_response_id,
                                now,
                            )
                            .await?;
                        }
                        tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                            id: approval.event_id,
                            session_id: session.id,
                            sequence: event_sequence,
                            event_type: "approval_requested".to_owned(),
                            run_id: Some(lease.run_id.0),
                            causation_id: Some(approval.tool_call_id.to_string()),
                            correlation_id: approval.correlation_id,
                            protected_payload: approval.protected_event,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        tx.queue_event(AiSessionWakeup {
                            session_id: session.id,
                            sequence: event_sequence,
                        });
                        if has_parked_provider_wait {
                            approval_wait_proof_from_record(&lease, &updated_run)
                        } else {
                            lease_from_record(&updated_run)
                        }
                    })
                })
        })
        .await
        .map_err(map_transaction)
    }

    pub(crate) async fn consume_approval(
        &self,
        lease: &AiRunLease,
        consumption: PreparedApprovalConsumption,
    ) -> Result<AiRunLease, AiError> {
        let now = canonical_second(self.clock.now());
        let lease_ttl = self.limits.lease_ttl;
        let lease = lease.clone();
        retry_transaction(self.limits.maximum_transaction_retries, || {
            let consumption = consumption.clone();
            let lease = lease.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let current = load_and_validate_active_lease(tx, &lease, now).await?;
                        if !matches!(
                            persisted_state(&current)?,
                            AiRunState::WaitingApproval | AiRunState::WaitingTool
                        ) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if session.state != "active"
                            || session.deleted_at.is_some()
                            || session.owner_principal_kind
                                != consumption.expected_owner_principal_kind
                            || session.owner_subject != consumption.expected_owner_subject
                            || session.scope_kind != consumption.expected_scope_kind
                            || session.scope_id != consumption.expected_scope_id
                            || session.tenant_id != consumption.expected_tenant_id
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let call = tx
                            .find_by_id::<AiToolCallRecord>(&consumption.tool_call_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if call.run_id != lease.run_id.0
                            || call.lease_generation != lease.lease_generation
                            || call.approval_id != Some(consumption.approval_id)
                            || call.state != "waiting_approval"
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let approval = tx
                            .find_by_id::<AiApprovalRecord>(&consumption.approval_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if approval.tool_call_id != call.id
                            || approval.session_id != session.id
                            || approval.row_version != consumption.expected_approval_version
                            || !matches!(approval.state.as_str(), "approved" | "resume_claimed")
                            || approval.binding_hash != consumption.binding_hash
                            || approval.maximum_uses != 1
                            || approval.consumed_uses != 0
                            || approval.consumed_at.is_some()
                            || approval.decided_at.is_none()
                            || approval.expires_at <= now.unix_timestamp()
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let approval_event_sequence = session
                            .stream_head
                            .checked_add(1)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let started_event_sequence = approval_event_sequence
                            .checked_add(1)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let session_update = tx
                            .compare_and_swap::<AiSessionRecord>(
                                &session.id,
                                session.row_version,
                                AiSessionRecordWhereInput::default(),
                                UpdateAiSessionRecordInput {
                                    stream_head: Some(started_event_sequence),
                                    last_activity_at: Some(now.unix_timestamp()),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(session_update, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let approval_update = tx
                            .compare_and_swap::<AiApprovalRecord>(
                                &approval.id,
                                approval.row_version,
                                AiApprovalRecordWhereInput::default(),
                                UpdateAiApprovalRecordInput {
                                    state: Some("consumed".to_owned()),
                                    consumed_uses: Some(1),
                                    consumed_at: Some(Some(now.unix_timestamp())),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(approval_update, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let call_update = tx
                            .compare_and_swap::<AiToolCallRecord>(
                                &call.id,
                                call.row_version,
                                AiToolCallRecordWhereInput::default(),
                                UpdateAiToolCallRecordInput {
                                    state: Some("executing".to_owned()),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(call_update, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let expiry = now
                            .checked_add(lease_ttl)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let run_update = tx
                            .compare_and_swap::<AiRunRecord>(
                                &current.id,
                                current.row_version,
                                exact_state(&current.state),
                                UpdateAiRunRecordInput {
                                    state: Some(AiRunState::Running.as_str().to_owned()),
                                    lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                    lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                                    error_code: Some(None),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        let updated_run = match run_update {
                            ConditionalUpdateOutcome::Updated(updated) => updated,
                            ConditionalUpdateOutcome::NotFound
                            | ConditionalUpdateOutcome::Conflict => {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                        };
                        tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                            id: consumption.event_id,
                            session_id: session.id,
                            sequence: approval_event_sequence,
                            event_type: "approval_consumed".to_owned(),
                            run_id: Some(lease.run_id.0),
                            causation_id: Some(consumption.approval_id.to_string()),
                            correlation_id: consumption.correlation_id.clone(),
                            protected_payload: consumption.protected_event,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                            id: consumption.started_event.event_id,
                            session_id: session.id,
                            sequence: started_event_sequence,
                            event_type: "application_tool_started".to_owned(),
                            run_id: Some(lease.run_id.0),
                            causation_id: Some(consumption.tool_call_id.to_string()),
                            correlation_id: consumption.correlation_id.clone(),
                            protected_payload: consumption.started_event.protected_event,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        append_inbox_event(
                            tx,
                            PreparedAiInboxEvent {
                                id: consumption.started_event.inbox_event_id,
                                principal_kind: consumption.expected_owner_principal_kind,
                                principal_subject: consumption.expected_owner_subject,
                                scope: AiScope {
                                    kind: consumption.expected_scope_kind,
                                    id: consumption.expected_scope_id,
                                    tenant_id: consumption.expected_tenant_id,
                                },
                                session_id: session.id,
                                event_type: "application_tool_started".to_owned(),
                                protected_payload: consumption.started_event.protected_inbox_event,
                                created_at: now.unix_timestamp(),
                            },
                        )
                        .await?;
                        tx.queue_event(AiSessionWakeup {
                            session_id: session.id,
                            sequence: approval_event_sequence,
                        });
                        tx.queue_event(AiSessionWakeup {
                            session_id: session.id,
                            sequence: started_event_sequence,
                        });
                        lease_from_record(&updated_run)
                    })
                })
        })
        .await
        .map_err(map_transaction)
    }

    pub(crate) async fn finish_tool_call(
        &self,
        lease: &AiRunLease,
        finish: PreparedToolCallFinish,
    ) -> Result<AiRunLease, AiError> {
        self.finish_tool_call_inner(lease, finish, None).await
    }

    pub(crate) async fn finish_tool_call_with_source(
        &self,
        lease: &AiRunLease,
        finish: PreparedToolCallFinish,
        protected_source: Option<serde_json::Value>,
    ) -> Result<AiRunLease, AiError> {
        self.finish_tool_call_inner(lease, finish, protected_source)
            .await
    }

    async fn finish_tool_call_inner(
        &self,
        lease: &AiRunLease,
        finish: PreparedToolCallFinish,
        retained_source: Option<serde_json::Value>,
    ) -> Result<AiRunLease, AiError> {
        let now = canonical_second(self.clock.now());
        let lease_ttl = self.limits.lease_ttl;
        let lease = lease.clone();
        retry_transaction(self.limits.maximum_transaction_retries, || {
            let finish = finish.clone();
            let lease = lease.clone();
            let retained_source = retained_source.clone();
            self.database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        let current = load_and_validate_active_lease(tx, &lease, now).await?;
                        if persisted_state(&current)? != AiRunState::Running {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&lease.session_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if !session_matches_tool_finish(&session, &finish) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let call = tx
                            .find_by_id::<AiToolCallRecord>(&finish.id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if call.run_id != lease.run_id.0
                            || call.lease_generation != lease.lease_generation
                            || call.provider_call_key != finish.expected_provider_call_key
                            || call.tool_fingerprint != finish.expected_tool_fingerprint
                            || match &retained_source {
                                Some(source) => {
                                    call.state != "source_retained"
                                        || call.protected_result.as_ref()
                                            != Some(&finish.protected_result)
                                        || call.protected_result_source.as_ref() != Some(source)
                                }
                                None => {
                                    call.state != "executing"
                                        || call.protected_result.is_some()
                                        || call.protected_result_source.is_some()
                                }
                            }
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let step = tx
                            .find_by_id::<AiRunStepRecord>(&finish.id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if step.run_id != lease.run_id.0
                            || step.lease_generation != lease.lease_generation
                            || step.state != "running"
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let call_update = tx
                            .compare_and_swap::<AiToolCallRecord>(
                                &call.id,
                                call.row_version,
                                AiToolCallRecordWhereInput::default(),
                                UpdateAiToolCallRecordInput {
                                    protected_result: Some(Some(finish.protected_result)),
                                    authorization_code: Some(Some(
                                        finish.authorization_code.clone(),
                                    )),
                                    authorization_policy_version: Some(
                                        finish.authorization_policy_version,
                                    ),
                                    authorization_state_digest: Some(
                                        finish.authorization_state_digest,
                                    ),
                                    disclosure_schema_fingerprint: Some(Some(
                                        finish.disclosure_schema_fingerprint,
                                    )),
                                    result_classification: Some(Some(finish.result_classification)),
                                    result_egress_decision_id: Some(
                                        finish.result_egress_decision_id,
                                    ),
                                    result_egress_manifest_hash: Some(
                                        finish.result_egress_manifest_hash,
                                    ),
                                    application_audit_ref: Some(finish.application_audit_ref),
                                    state: Some(finish.state.clone()),
                                    completed_at: Some(Some(now.unix_timestamp())),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(call_update, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let step_update = tx
                            .compare_and_swap::<AiRunStepRecord>(
                                &step.id,
                                step.row_version,
                                AiRunStepRecordWhereInput::default(),
                                UpdateAiRunStepRecordInput {
                                    state: Some(finish.state.clone()),
                                    finished_at: Some(Some(now.unix_timestamp())),
                                    error_code: (finish.state != "completed")
                                        .then_some(Some(finish.authorization_code)),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(step_update, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let event_sequence = session
                            .stream_head
                            .checked_add(1)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let session_update = tx
                            .compare_and_swap::<AiSessionRecord>(
                                &session.id,
                                session.row_version,
                                AiSessionRecordWhereInput::default(),
                                UpdateAiSessionRecordInput {
                                    stream_head: Some(event_sequence),
                                    last_activity_at: Some(now.unix_timestamp()),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(session_update, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                            id: finish.event_id,
                            session_id: lease.session_id.0,
                            sequence: event_sequence,
                            event_type: "application_tool_completed".to_owned(),
                            run_id: Some(lease.run_id.0),
                            causation_id: Some(finish.id.to_string()),
                            correlation_id: finish.correlation_id.clone(),
                            protected_payload: finish.protected_event,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        append_inbox_event(
                            tx,
                            PreparedAiInboxEvent {
                                id: finish.inbox_event_id,
                                principal_kind: finish.expected_owner_principal_kind.clone(),
                                principal_subject: finish.expected_owner_subject.clone(),
                                scope: AiScope {
                                    kind: finish.expected_scope_kind.clone(),
                                    id: finish.expected_scope_id.clone(),
                                    tenant_id: finish.expected_tenant_id.clone(),
                                },
                                session_id: lease.session_id.0,
                                event_type: "application_tool_completed".to_owned(),
                                protected_payload: finish.protected_inbox_event,
                                created_at: now.unix_timestamp(),
                            },
                        )
                        .await?;
                        tx.queue_event(AiSessionWakeup {
                            session_id: lease.session_id.0,
                            sequence: event_sequence,
                        });
                        let expiry = now
                            .checked_add(lease_ttl)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let run_update = tx
                            .compare_and_swap::<AiRunRecord>(
                                &current.id,
                                current.row_version,
                                exact_state(AiRunState::Running.as_str()),
                                UpdateAiRunRecordInput {
                                    lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                    lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        match run_update {
                            ConditionalUpdateOutcome::Updated(updated) => {
                                lease_from_record(&updated)
                            }
                            ConditionalUpdateOutcome::NotFound
                            | ConditionalUpdateOutcome::Conflict => {
                                Err(OrmPublicError::new(OrmErrorCode::Conflict))
                            }
                        }
                    })
                })
        })
        .await
        .map_err(map_transaction)
    }

    async fn claim_once(
        &self,
        worker_id: String,
        now: OffsetDateTime,
    ) -> Result<Result<Option<AiRunLease>, AiError>, TransactionError> {
        let database = self.database.clone();
        let limits = self.limits;
        database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let epoch = crate::orm_runtime_recovery::require_live_epoch(tx).await?;
                    let expiry = match now.checked_add(limits.lease_ttl) {
                        Some(expiry) => expiry,
                        None => return Ok(Err(AiError::PersistenceFailed)),
                    };
                    // Select only eligible rows, oldest first, in SQL so rows
                    // not yet due cannot fill the bounded window.
                    let mut candidates = Vec::new();
                    for state in [AiRunState::Queued, AiRunState::RetryScheduled] {
                        for eligible in [
                            Int64Filter {
                                is_null: Some(true),
                                ..Default::default()
                            },
                            Int64Filter {
                                lte: Some(now.unix_timestamp()),
                                ..Default::default()
                            },
                        ] {
                            let mut rows = tx
                                .query::<AiRunRecord>()
                                .filter(AiRunRecordWhereInput {
                                    state: Some(StringFilter {
                                        eq: Some(state.as_str().to_owned()),
                                        ..Default::default()
                                    }),
                                    next_attempt_at: Some(eligible),
                                    ..Default::default()
                                })
                                .default_order()
                                .internal_limit(limits.maximum_candidate_scan as i64)
                                .fetch_all()
                                .await
                                .map_err(OrmPublicError::from)?;
                            candidates.append(&mut rows);
                        }
                    }
                    candidates.retain(|run| {
                        run.next_attempt_at
                            .is_none_or(|eligible_at| eligible_at <= now.unix_timestamp())
                    });
                    candidates.sort_by_key(|run| (run.created_at, run.id));
                    candidates.dedup_by_key(|run| run.id);
                    candidates.truncate(limits.maximum_candidate_scan);
                    for current in candidates {
                        let generation = current.lease_generation.checked_add(1).filter(|_| {
                            current.lease_generation >= 0
                                && current.retry_count >= 0
                                && current.attempt_id.is_none()
                                && current.lease_owner.is_none()
                                && current.lease_expires_at.is_none()
                                && (current.error_code.as_deref()
                                    != Some("checkpoint_adoption_ready")
                                    || current.latest_checkpoint_id.is_some())
                        });
                        let reference = serde_json::from_value::<PrincipalReference>(
                            current.principal_reference.clone(),
                        );
                        let (Some(generation), Ok(reference)) = (generation, reference) else {
                            // One malformed queued row must not block every
                            // other run. Close it without a lease or attempt;
                            // it is never replayed.
                            quarantine_unclaimable_run(tx, &current, &worker_id).await?;
                            continue;
                        };
                        let delegated =
                            crate::principal_projection::is_delegated_reference(&reference);
                        let attempt = tx
                            .insert::<AiRunAttemptRecord>(CreateAiRunAttemptRecordInput {
                                run_id: current.id,
                                lease_generation: generation,
                                worker_id: worker_id.clone(),
                                claimed_at: now.unix_timestamp(),
                                finished_at: None,
                                provider_response_id: None,
                                outcome_code: None,
                            })
                            .await
                            .map_err(OrmPublicError::from)?;
                        let outcome = tx
                            .compare_and_swap::<AiRunRecord>(
                                &current.id,
                                current.row_version,
                                exact_state(&current.state),
                                UpdateAiRunRecordInput {
                                    state: Some(AiRunState::Leased.as_str().to_owned()),
                                    attempt_id: Some(Some(attempt.id)),
                                    lease_owner: Some(Some(worker_id.clone())),
                                    recovery_epoch_id: Some(Some(epoch)),
                                    lease_generation: Some(generation),
                                    lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                    lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                                    next_attempt_at: Some(None),
                                    error_code: Some(None),
                                    latest_checkpoint_id: Some(
                                        (current.error_code.as_deref()
                                            == Some("checkpoint_adoption_ready"))
                                        .then_some(current.latest_checkpoint_id)
                                        .flatten(),
                                    ),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        let updated = match outcome {
                            ConditionalUpdateOutcome::Updated(updated) => updated,
                            ConditionalUpdateOutcome::NotFound
                            | ConditionalUpdateOutcome::Conflict => {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                        };
                        let lease = match lease_from_record(&updated) {
                            Ok(lease) => lease,
                            Err(_) => return Ok(Err(AiError::PersistenceFailed)),
                        };
                        if !delegated {
                            return Ok(Ok(Some(lease)));
                        }
                        refuse_delegated_claim(tx, &updated, &lease, now).await?;
                    }
                    Ok(Ok(None))
                })
            })
            .await
    }

    async fn claim_approved_once(
        &self,
        worker_id: String,
        now: OffsetDateTime,
    ) -> Result<Option<AiApprovedRunClaim>, TransactionError> {
        let database = self.database.clone();
        let limits = self.limits;
        database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let epoch = crate::orm_runtime_recovery::require_live_epoch(tx).await?;
                    let mut approvals = tx
                        .query::<AiApprovalRecord>()
                        .filter(AiApprovalRecordWhereInput {
                            state: Some(StringFilter {
                                eq: Some("approved".to_owned()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .default_order()
                        .internal_limit(limits.maximum_candidate_scan as i64)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    approvals.sort_by_key(|approval| (approval.created_at, approval.id));

                    for approval in approvals {
                        if approval.expires_at <= now.unix_timestamp() {
                            let expired = tx
                                .compare_and_swap::<AiApprovalRecord>(
                                    &approval.id,
                                    approval.row_version,
                                    AiApprovalRecordWhereInput {
                                        state: Some(StringFilter {
                                            eq: Some("approved".to_owned()),
                                            ..Default::default()
                                        }),
                                        ..Default::default()
                                    },
                                    UpdateAiApprovalRecordInput {
                                        state: Some("expired".to_owned()),
                                        ..Default::default()
                                    },
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            if !matches!(expired, ConditionalUpdateOutcome::Updated(_)) {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
                                actor_principal_kind: "ai_worker".to_owned(),
                                actor_subject: worker_id.clone(),
                                action: "ai.approval.expired".to_owned(),
                                resource_kind: "ai_approval".to_owned(),
                                resource_reference: approval.id.to_string(),
                                outcome: "denied".to_owned(),
                                reason_code: "approval_expired_before_handoff".to_owned(),
                                correlation_id: approval.id.to_string(),
                                causation_id: Some(approval.tool_call_id.to_string()),
                                policy_version: Some(approval.policy_version),
                            })
                            .await
                            .map_err(OrmPublicError::from)?;
                            continue;
                        }
                        let validated = async {
                        if approval.maximum_uses != 1
                            || approval.consumed_uses != 0
                            || approval.consumed_at.is_some()
                            || approval.decided_at.is_none()
                            || approval.approver_subject.is_none()
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::InternalError));
                        }
                        let call = tx
                            .find_by_id::<AiToolCallRecord>(&approval.tool_call_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let run = tx
                            .find_by_id::<AiRunRecord>(&call.run_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        if approval.session_id == run.session_id
                            && call.approval_id == Some(approval.id)
                            && AiRunState::from_persisted(&run.state).is_some_and(|state| {
                                state != AiRunState::WaitingApproval
                                    && AiRunTerminalEvent::from_run_state(state).is_some()
                            })
                        {
                            // The run left its wait (for example the wait
                            // reconciler closed it) while the approval stayed
                            // open. The approval can never be consumed.
                            return Ok(ApprovedHandoff::Expire("approval_run_closed"));
                        }
                        if approval.session_id != run.session_id
                            || call.approval_id != Some(approval.id)
                            || call.state != "waiting_approval"
                            || call.completed_at.is_some()
                            || call.protected_result.is_some()
                            || call.risk == "read_only"
                            || persisted_state(&run)? != AiRunState::WaitingApproval
                            || run.lease_generation <= 0
                            || call.lease_generation != run.lease_generation
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::InternalError));
                        }
                        let reference: PrincipalReference =
                            serde_json::from_value(run.principal_reference.clone())
                                .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        if crate::principal_projection::is_delegated_reference(&reference) {
                            // A legacy run whose stored reference is delegated
                            // or sessionless never resumes. Expire the approval
                            // so it cannot be consumed; the approval-wait
                            // reconciler then closes the run with the same
                            // safe code, without resolving the reference.
                            return Ok(ApprovedHandoff::Expire(
                                crate::principal_projection::DELEGATED_REFERENCE_CODE,
                            ));
                        }
                        let parked_checkpoint =
                            if run.error_code.as_deref() == Some("approval_wait_parked") {
                                let checkpoint_id = run.latest_checkpoint_id.ok_or_else(|| {
                                    OrmPublicError::new(OrmErrorCode::InternalError)
                                })?;
                                Some(
                                    tx.query::<AiRunCheckpointRecord>()
                                        .filter(AiRunCheckpointRecordWhereInput {
                                            id: Some(UuidFilter {
                                                eq: Some(checkpoint_id),
                                                ..Default::default()
                                            }),
                                            ..Default::default()
                                        })
                                        .internal_limit(1)
                                        .fetch_one()
                                        .await
                                        .map_err(OrmPublicError::from)?
                                        .ok_or_else(|| {
                                            OrmPublicError::new(OrmErrorCode::InternalError)
                                        })?,
                                )
                            } else {
                                None
                            };
                        let legacy_wait = run.attempt_id.is_some()
                            && run
                                .lease_owner
                                .as_deref()
                                .is_some_and(|owner| validate_worker_id(owner).is_ok())
                            && run.lease_expires_at.is_some()
                            && run.lease_heartbeat_at.is_some()
                            && run.error_code.is_none()
                            && parked_checkpoint.is_none();
                        let parked_wait = run.attempt_id.is_none()
                            && run.lease_owner.is_none()
                            && run.lease_expires_at.is_none()
                            && run.lease_heartbeat_at.is_none()
                            && parked_checkpoint.is_some();
                        if !legacy_wait && !parked_wait {
                            return Err(OrmPublicError::new(OrmErrorCode::InternalError));
                        }
                        if let Some(checkpoint) = parked_checkpoint.as_ref() {
                            let protected_state = checkpoint
                                .protected_state
                                .as_ref()
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            let reservation_id = checkpoint
                                .budget_reservation_id
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            let reservation = tx
                                .find_by_id::<AiBudgetReservationRecord>(&reservation_id)
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            let expected_hash = coordinator_checkpoint_hash(
                                AiRunId(run.id),
                                checkpoint.attempt_id,
                                checkpoint.lease_generation,
                                checkpoint.id,
                                &checkpoint.checkpoint_kind,
                                &reservation.provider_kind,
                                &reservation.provider_model,
                                checkpoint.provider_response_id.as_deref(),
                                reservation_id,
                                protected_state,
                            )
                            .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            let outcome = tx
                                .query::<AiRunAttemptOutcomeRecord>()
                                .filter(AiRunAttemptOutcomeRecordWhereInput {
                                    attempt_id: Some(UuidFilter {
                                        eq: Some(checkpoint.attempt_id),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                })
                                .internal_limit(1)
                                .fetch_one()
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            if checkpoint.run_id != run.id
                                || checkpoint.lease_generation != run.lease_generation
                                || checkpoint.checkpoint_kind != "approval_wait_parked"
                                || checkpoint.assistant_message_id.is_some()
                                || checkpoint.provider_response_id != call.provider_response_id
                                || checkpoint.budget_reservation_id != call.budget_reservation_id
                                || checkpoint.checkpoint_hash != expected_hash
                                || reservation.run_id != run.id
                                || reservation.attempt_id != checkpoint.attempt_id
                                || reservation.lease_generation != checkpoint.lease_generation
                                || reservation.state != "committed"
                                || outcome.run_id != run.id
                                || outcome.attempt_id != checkpoint.attempt_id
                                || outcome.lease_generation != checkpoint.lease_generation
                                || outcome.final_state != AiRunState::WaitingApproval.as_str()
                                || outcome.outcome_code != "approval_wait_parked"
                                || outcome.provider_response_id != checkpoint.provider_response_id
                            {
                                return Err(OrmPublicError::new(OrmErrorCode::InternalError));
                            }
                            let bindings = tx
                                .query::<AiProviderSessionBindingRecord>()
                                .filter(
                                    crate::orm_provider_session::AiProviderSessionBindingRecordWhereInput {
                                        session_id: Some(UuidFilter {
                                            eq: Some(run.session_id),
                                            ..Default::default()
                                        }),
                                        ..Default::default()
                                    },
                                )
                                .internal_limit(2)
                                .fetch_all()
                                .await
                                .map_err(OrmPublicError::from)?;
                            if bindings.len() != 1 {
                                return Err(OrmPublicError::new(OrmErrorCode::InternalError));
                            }
                            let binding = &bindings[0];
                            if binding.state != "parked_wait"
                                || binding.parked_wait_kind.as_deref() != Some("approval")
                                || binding.parked_wait_id != Some(approval.id)
                                || binding.claimed_run_id != Some(run.id)
                            {
                                return Err(OrmPublicError::new(OrmErrorCode::InternalError));
                            }
                            if binding.parked_confirmed_at.is_none() {
                                // Confirmation is repairable only while the
                                // exact lease-free wait graph remains current.
                                // Leave the approved row eligible so provider
                                // maintenance can confirm that graph before a
                                // fresh run attempt is claimed.
                                return Ok(ApprovedHandoff::Skip);
                            }
                            if binding.parked_checkpoint_id != Some(checkpoint.id)
                                || binding.parked_checkpoint_fingerprint.as_deref()
                                    != Some(checkpoint.checkpoint_hash.as_str())
                                || binding
                                    .parked_expires_at
                                    .is_none_or(|expires_at| expires_at <= now.unix_timestamp())
                            {
                                return Err(OrmPublicError::new(OrmErrorCode::InternalError));
                            }
                        }
                        if let Some(candidate) = tx.find_by_id::<AiNativeApprovalCandidateRecord>(&call.id)
                            .await.map_err(OrmPublicError::from)?
                        {
                            if !parked_wait || candidate.final_approval_id != Some(approval.id)
                                || candidate.binding_hash != approval.binding_hash
                                || candidate.preview_hash != approval.action_preview_hash
                            { return Err(OrmPublicError::new(OrmErrorCode::Conflict)); }
                            let reservation = tx.find_by_id::<AiBudgetReservationRecord>(
                                &call.budget_reservation_id.ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?)
                                .await.map_err(OrmPublicError::from)?.ok_or_else(OrmPublicError::not_found)?;
                            let source = native_checkpoints::source(tx, &candidate, &reservation,
                                call.provider_response_id.as_deref()).await?;
                            if source.lease_generation != run.lease_generation || source.run_id != run.id
                                || reservation.session_id != run.session_id
                            { return Err(OrmPublicError::new(OrmErrorCode::Conflict)); }
                            native_checkpoints::cohort(tx, native_checkpoints::NativeTurn {
                                run_id: run.id, attempt_id: source.attempt_id, generation: source.lease_generation,
                                response_id: source.provider_response_id.as_deref().ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?,
                                budget: &reservation,
                            }, &candidate, native_checkpoints::PendingPhase::Waiting, None).await?;
                        }
                            Ok(ApprovedHandoff::Proceed {
                                call: Box::new(call),
                                run: Box::new(run),
                                parked_wait,
                            })
                        }
                        .await;
                        let (call, run, parked_wait) = match validated {
                            Ok(ApprovedHandoff::Proceed {
                                call,
                                run,
                                parked_wait,
                            }) => (*call, *run, parked_wait),
                            Ok(ApprovedHandoff::Skip) => continue,
                            Ok(ApprovedHandoff::Expire(reason)) => {
                                expire_unclaimable_approval(tx, &approval, &worker_id, reason)
                                    .await?;
                                continue;
                            }
                            Err(error)
                                if matches!(
                                    error.code,
                                    OrmErrorCode::InternalError | OrmErrorCode::Conflict
                                ) =>
                            {
                                // One malformed approved row must not fail every
                                // later approved claim. It is expired, so it can
                                // never be consumed, and audited.
                                expire_unclaimable_approval(
                                    tx,
                                    &approval,
                                    &worker_id,
                                    "approval_handoff_invalid",
                                )
                                .await?;
                                continue;
                            }
                            Err(error) => return Err(error),
                        };
                        let expiry = now
                            .checked_add(limits.lease_ttl)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let resumed_attempt = if parked_wait {
                            let generation = run
                                .lease_generation
                                .checked_add(1)
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            let attempt = tx
                                .insert::<AiRunAttemptRecord>(CreateAiRunAttemptRecordInput {
                                    run_id: run.id,
                                    lease_generation: generation,
                                    worker_id: worker_id.clone(),
                                    claimed_at: now.unix_timestamp(),
                                    finished_at: None,
                                    provider_response_id: None,
                                    outcome_code: None,
                                })
                                .await
                                .map_err(OrmPublicError::from)?;
                            let call_update = tx
                                .compare_and_swap::<AiToolCallRecord>(
                                    &call.id,
                                    call.row_version,
                                    AiToolCallRecordWhereInput::default(),
                                    UpdateAiToolCallRecordInput {
                                        lease_generation: Some(generation),
                                        ..Default::default()
                                    },
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            if !matches!(call_update, ConditionalUpdateOutcome::Updated(_)) {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            let step = tx
                                .find_by_id::<AiRunStepRecord>(&call.id)
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            if step.run_id != run.id
                                || step.lease_generation != run.lease_generation
                                || step.state != "running"
                                || step.finished_at.is_some()
                            {
                                return Err(OrmPublicError::new(OrmErrorCode::InternalError));
                            }
                            let step_update = tx
                                .compare_and_swap::<AiRunStepRecord>(
                                    &step.id,
                                    step.row_version,
                                    AiRunStepRecordWhereInput::default(),
                                    UpdateAiRunStepRecordInput {
                                        lease_generation: Some(generation),
                                        ..Default::default()
                                    },
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            if !matches!(step_update, ConditionalUpdateOutcome::Updated(_)) {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            Some((attempt.id, generation))
                        } else {
                            None
                        };
                        let approval_update = tx
                            .compare_and_swap::<AiApprovalRecord>(
                                &approval.id,
                                approval.row_version,
                                AiApprovalRecordWhereInput::default(),
                                UpdateAiApprovalRecordInput {
                                    state: Some("resume_claimed".to_owned()),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if !matches!(approval_update, ConditionalUpdateOutcome::Updated(_)) {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let outcome = tx
                            .compare_and_swap::<AiRunRecord>(
                                &run.id,
                                run.row_version,
                                exact_state(AiRunState::WaitingApproval.as_str()),
                                UpdateAiRunRecordInput {
                                    state: Some(AiRunState::WaitingTool.as_str().to_owned()),
                                    attempt_id: resumed_attempt
                                        .map(|(attempt_id, _)| Some(attempt_id)),
                                    lease_generation: resumed_attempt
                                        .map(|(_, generation)| generation),
                                    lease_owner: Some(Some(worker_id.clone())),
                                    recovery_epoch_id: Some(Some(epoch)),
                                    lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                    lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                                    error_code: Some(Some("approval_resume_claimed".to_owned())),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        let updated = match outcome {
                            ConditionalUpdateOutcome::Updated(updated) => updated,
                            ConditionalUpdateOutcome::NotFound
                            | ConditionalUpdateOutcome::Conflict => {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                        };
                        tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
                            actor_principal_kind: "ai_worker".to_owned(),
                            actor_subject: worker_id.clone(),
                            action: "ai.run.approval_resume_claimed".to_owned(),
                            resource_kind: "ai_run".to_owned(),
                            resource_reference: run.id.to_string(),
                            outcome: "allowed".to_owned(),
                            reason_code: "approved_wait_handoff".to_owned(),
                            correlation_id: approval.id.to_string(),
                            causation_id: Some(call.id.to_string()),
                            policy_version: Some(approval.policy_version),
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                        return Ok(Some(AiApprovedRunClaim {
                            approval_id: AiApprovalId(approval.id),
                            tool_call_id: AiToolCallId(call.id),
                            lease: lease_from_record(&updated)?,
                        }));
                    }
                    Ok(None)
                })
            })
            .await
    }

    async fn reconcile_approval_wait_once(
        &self,
        reconciliation: PreparedApprovalWaitReconciliation,
        now: OffsetDateTime,
    ) -> Result<(), TransactionError> {
        let database = self.database.clone();
        database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let terminal_outcome_code = reconciliation.outcome_code.clone();
                    let current = tx
                        .find_by_id::<AiRunRecord>(&reconciliation.expected_run.id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if current != reconciliation.expected_run
                        || persisted_state(&current)? != AiRunState::WaitingApproval
                        || current.lease_generation <= 0
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let parked_wait = current.attempt_id.is_none()
                        && current.lease_owner.is_none()
                        && current.lease_expires_at.is_none()
                        && current.lease_heartbeat_at.is_none()
                        && current.error_code.as_deref() == Some("approval_wait_parked")
                        && current.latest_checkpoint_id.is_some();
                    let active_wait = current.attempt_id.is_some()
                        && current
                            .lease_owner
                            .as_deref()
                            .is_some_and(|owner| validate_worker_id(owner).is_ok())
                        && current.lease_expires_at.is_some()
                        && current.lease_heartbeat_at.is_some()
                        && current.error_code.is_none();
                    if !parked_wait && !active_wait {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let lease = active_wait
                        .then(|| lease_from_record(&current))
                        .transpose()?;
                    let session = tx
                        .find_by_id::<AiSessionRecord>(&current.session_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if session.owner_principal_kind != reconciliation.expected_owner_principal_kind
                        || session.owner_subject != reconciliation.expected_owner_subject
                        || session.scope_kind != reconciliation.expected_scope_kind
                        || session.scope_id != reconciliation.expected_scope_id
                        || session.tenant_id != reconciliation.expected_tenant_id
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }

                    let (
                        final_state,
                        approval_id,
                        tool_call_id,
                        provider_response_id,
                        audit_outcome,
                    ) = match reconciliation.outcome {
                        PreparedApprovalWaitOutcome::Cancelled(cancellation) => {
                            let PreparedApprovalWaitCancellation {
                                call,
                                step,
                                approval,
                                checkpoint,
                                parked_checkpoint,
                                attempt_outcome,
                                next_approval_state,
                                call_state,
                            } = *cancellation;
                            let current_call = tx
                                .find_by_id::<AiToolCallRecord>(&call.id)
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(OrmPublicError::not_found)?;
                            let current_step = tx
                                .find_by_id::<AiRunStepRecord>(&step.id)
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(OrmPublicError::not_found)?;
                            let current_approval = tx
                                .find_by_id::<AiApprovalRecord>(&approval.id)
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(OrmPublicError::not_found)?;
                            let current_checkpoint = tx
                                .query::<AiRunCheckpointRecord>()
                                .filter(AiRunCheckpointRecordWhereInput {
                                    id: Some(UuidFilter {
                                        eq: Some(checkpoint.id),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                })
                                .internal_limit(1)
                                .fetch_one()
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(OrmPublicError::not_found)?;
                            let current_parked_checkpoint = match parked_checkpoint.as_ref() {
                                Some(parked) => Some(
                                    tx.query::<AiRunCheckpointRecord>()
                                        .filter(AiRunCheckpointRecordWhereInput {
                                            id: Some(UuidFilter {
                                                eq: Some(parked.id),
                                                ..Default::default()
                                            }),
                                            ..Default::default()
                                        })
                                        .internal_limit(1)
                                        .fetch_one()
                                        .await
                                        .map_err(OrmPublicError::from)?
                                        .ok_or_else(OrmPublicError::not_found)?,
                                ),
                                None => None,
                            };
                            let current_attempt_outcome = match attempt_outcome.as_ref() {
                                Some(outcome) => Some(
                                    tx.query::<AiRunAttemptOutcomeRecord>()
                                        .filter(AiRunAttemptOutcomeRecordWhereInput {
                                            attempt_id: Some(UuidFilter {
                                                eq: Some(outcome.attempt_id),
                                                ..Default::default()
                                            }),
                                            ..Default::default()
                                        })
                                        .internal_limit(1)
                                        .fetch_one()
                                        .await
                                        .map_err(OrmPublicError::from)?
                                        .ok_or_else(OrmPublicError::not_found)?,
                                ),
                                None => None,
                            };
                            if current_call != call
                                || current_step != step
                                || current_approval != approval
                                || current_checkpoint != checkpoint
                                || current_parked_checkpoint != parked_checkpoint
                                || current_attempt_outcome != attempt_outcome
                            {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            let provider_response_id = checkpoint
                                .provider_response_id
                                .as_deref()
                                .filter(|value| valid_provider_reference(value))
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            let budget_reservation_id = checkpoint
                                .budget_reservation_id
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            let protected_state = checkpoint
                                .protected_state
                                .as_ref()
                                .filter(|state| {
                                    serde_json::to_vec(state)
                                        .is_ok_and(|encoded| encoded.len() <= 64 * 1024 * 1024)
                                })
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            let reservation = tx
                                .find_by_id::<AiBudgetReservationRecord>(&budget_reservation_id)
                                .await
                                .map_err(OrmPublicError::from)?
                                .ok_or_else(OrmPublicError::not_found)?;
                            let principal_reference: PrincipalReference =
                                serde_json::from_value(current.principal_reference.clone())
                                    .map_err(|_| {
                                        OrmPublicError::new(OrmErrorCode::InternalError)
                                    })?;
                            let principal_fingerprint = hex::encode(Sha256::digest(
                                serde_json::to_vec(&principal_reference).map_err(|_| {
                                    OrmPublicError::new(OrmErrorCode::InternalError)
                                })?,
                            ));
                            let expected_hash = coordinator_checkpoint_hash(
                                AiRunId(current.id),
                                checkpoint.attempt_id,
                                checkpoint.lease_generation,
                                checkpoint.id,
                                &checkpoint.checkpoint_kind,
                                &reservation.provider_kind,
                                &reservation.provider_model,
                                Some(provider_response_id),
                                budget_reservation_id,
                                protected_state,
                            )
                            .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            let calls = tx
                                .query::<AiToolCallRecord>()
                                .filter(AiToolCallRecordWhereInput {
                                    run_id: Some(UuidFilter {
                                        eq: Some(current.id),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                })
                                .internal_limit(4_097)
                                .fetch_all()
                                .await
                                .map_err(OrmPublicError::from)?;
                            let relevant = calls
                                .iter()
                                .filter(|candidate| {
                                    candidate.lease_generation == current.lease_generation
                                        && candidate.provider_response_id.as_deref()
                                            == Some(provider_response_id)
                                        && candidate.budget_reservation_id
                                            == Some(budget_reservation_id)
                                })
                                .collect::<Vec<_>>();
                            let approval_state_is_terminal =
                                matches!(approval.state.as_str(), "denied" | "revoked" | "expired")
                                    && next_approval_state.is_none();
                            let approval_state_expires =
                                matches!(approval.state.as_str(), "pending" | "approved")
                                    && next_approval_state.as_deref() == Some("expired");
                            let parked_graph_valid =
                                match (parked_checkpoint.as_ref(), attempt_outcome.as_ref()) {
                                    (Some(parked), Some(outcome)) => {
                                        let Some(parked_state) = parked.protected_state.as_ref()
                                        else {
                                            return Err(OrmPublicError::new(
                                                OrmErrorCode::InternalError,
                                            ));
                                        };
                                        let parked_hash = coordinator_checkpoint_hash(
                                            AiRunId(current.id),
                                            checkpoint.attempt_id,
                                            checkpoint.lease_generation,
                                            parked.id,
                                            &parked.checkpoint_kind,
                                            &reservation.provider_kind,
                                            &reservation.provider_model,
                                            Some(provider_response_id),
                                            budget_reservation_id,
                                            parked_state,
                                        )
                                        .map_err(|_| {
                                            OrmPublicError::new(OrmErrorCode::InternalError)
                                        })?;
                                        parked_wait
                                            && current.latest_checkpoint_id == Some(parked.id)
                                            && parked.run_id == current.id
                                            && parked.attempt_id == checkpoint.attempt_id
                                            && parked.lease_generation
                                                == checkpoint.lease_generation
                                            && parked.checkpoint_kind == "approval_wait_parked"
                                            && parked.provider_response_id
                                                == checkpoint.provider_response_id
                                            && parked.budget_reservation_id
                                                == checkpoint.budget_reservation_id
                                            && parked.assistant_message_id.is_none()
                                            && parked.checkpoint_hash == parked_hash
                                            && outcome.run_id == current.id
                                            && outcome.attempt_id == checkpoint.attempt_id
                                            && outcome.lease_generation
                                                == checkpoint.lease_generation
                                            && outcome.final_state
                                                == AiRunState::WaitingApproval.as_str()
                                            && outcome.outcome_code == "approval_wait_parked"
                                            && outcome.provider_response_id
                                                == checkpoint.provider_response_id
                                    }
                                    (None, None) => {
                                        active_wait
                                            && current.latest_checkpoint_id == Some(checkpoint.id)
                                    }
                                    _ => false,
                                };
                            let native_source =
                                checkpoint.checkpoint_kind == native_checkpoints::SOURCE_KIND;
                            if native_source {
                                let candidate = native_checkpoints::candidate(tx, call.id).await?;
                                let source = native_checkpoints::source(
                                    tx,
                                    &candidate,
                                    &reservation,
                                    Some(provider_response_id),
                                )
                                .await?;
                                if source.id != checkpoint.id
                                    || candidate.final_approval_id != Some(approval.id)
                                    || candidate.binding_hash != approval.binding_hash
                                    || candidate.preview_hash != approval.action_preview_hash
                                {
                                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                                }
                                native_checkpoints::cohort(
                                    tx,
                                    native_checkpoints::NativeTurn {
                                        run_id: current.id,
                                        attempt_id: checkpoint.attempt_id,
                                        generation: checkpoint.lease_generation,
                                        response_id: provider_response_id,
                                        budget: &reservation,
                                    },
                                    &candidate,
                                    native_checkpoints::PendingPhase::Waiting,
                                    None,
                                )
                                .await?;
                            }
                            if !parked_graph_valid
                                || checkpoint.run_id != current.id
                                || checkpoint.lease_generation != current.lease_generation
                                || (!native_source
                                    && checkpoint.checkpoint_kind != "provider_turn_persisted")
                                || checkpoint.assistant_message_id.is_some()
                                || checkpoint.checkpoint_hash != expected_hash
                                || reservation.session_id != session.id
                                || reservation.scope_kind != reconciliation.expected_scope_kind
                                || reservation.scope_id != reconciliation.expected_scope_id
                                || reservation.tenant_id != reconciliation.expected_tenant_id
                                || reservation.principal_kind
                                    != reconciliation.expected_owner_principal_kind
                                || reservation.principal_subject
                                    != reconciliation.expected_owner_subject
                                || reservation.run_id != current.id
                                || reservation.attempt_id != checkpoint.attempt_id
                                || reservation.lease_generation != checkpoint.lease_generation
                                || reservation.state != "committed"
                                || reservation.actual_runs != Some(1)
                                || reservation.reconciled_at.is_none()
                                || calls.len() >= 4_097
                                || (!native_source
                                    && (relevant.len() != 1 || relevant[0].id != call.id))
                                || call.run_id != current.id
                                || call.lease_generation != current.lease_generation
                                || call.provider_kind.as_deref()
                                    != Some(reservation.provider_kind.as_str())
                                || call.provider_model.as_deref()
                                    != Some(reservation.provider_model.as_str())
                                || call.provider_response_id.as_deref()
                                    != Some(provider_response_id)
                                || call.budget_reservation_id != Some(budget_reservation_id)
                                || (!native_source && call.tool_call_index != 0)
                                || call.state != "waiting_approval"
                                || call.completed_at.is_some()
                                || call.protected_result.is_some()
                                || call.approval_id != Some(approval.id)
                                || call.risk == "read_only"
                                || step.id != call.id
                                || step.run_id != current.id
                                || step.lease_generation != current.lease_generation
                                || step.step_kind != "application_tool"
                                || step.state != "running"
                                || step.finished_at.is_some()
                                || approval.tool_call_id != call.id
                                || approval.session_id != session.id
                                || approval.principal_subject != principal_reference.subject
                                || approval.principal_reference_fingerprint != principal_fingerprint
                                || approval.argument_hash != call.argument_hash
                                || approval.tool_fingerprint != call.tool_fingerprint
                                || approval.maximum_uses != 1
                                || approval.consumed_uses != 0
                                || approval.consumed_at.is_some()
                                || !(approval_state_is_terminal || approval_state_expires)
                                || !matches!(
                                    call_state.as_str(),
                                    "approval_denied" | "approval_revoked" | "approval_expired"
                                )
                            {
                                return Err(OrmPublicError::new(OrmErrorCode::InternalError));
                            }
                            if let Some(next_state) = next_approval_state {
                                let approval_update = tx
                                    .compare_and_swap::<AiApprovalRecord>(
                                        &approval.id,
                                        approval.row_version,
                                        AiApprovalRecordWhereInput::default(),
                                        UpdateAiApprovalRecordInput {
                                            state: Some(next_state),
                                            ..Default::default()
                                        },
                                    )
                                    .await
                                    .map_err(OrmPublicError::from)?;
                                if !matches!(approval_update, ConditionalUpdateOutcome::Updated(_))
                                {
                                    return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                                }
                            }
                            let call_update = tx
                                .compare_and_swap::<AiToolCallRecord>(
                                    &call.id,
                                    call.row_version,
                                    AiToolCallRecordWhereInput::default(),
                                    UpdateAiToolCallRecordInput {
                                        state: Some(call_state.clone()),
                                        completed_at: Some(Some(now.unix_timestamp())),
                                        ..Default::default()
                                    },
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            if !matches!(call_update, ConditionalUpdateOutcome::Updated(_)) {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            let step_update = tx
                                .compare_and_swap::<AiRunStepRecord>(
                                    &step.id,
                                    step.row_version,
                                    AiRunStepRecordWhereInput::default(),
                                    UpdateAiRunStepRecordInput {
                                        state: Some(call_state),
                                        finished_at: Some(Some(now.unix_timestamp())),
                                        error_code: Some(Some(reconciliation.outcome_code.clone())),
                                        ..Default::default()
                                    },
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            if !matches!(step_update, ConditionalUpdateOutcome::Updated(_)) {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            (
                                AiRunState::Cancelled,
                                Some(approval.id),
                                Some(call.id),
                                Some(provider_response_id.to_owned()),
                                "cancelled",
                            )
                        }
                        PreparedApprovalWaitOutcome::RecoveryRequired {
                            approval_id,
                            tool_call_id,
                        } => {
                            // Every open approval of the run closes in the terminal
                            // event below, including any this reconciliation could not
                            // identify.
                            (
                                AiRunState::RecoveryRequired,
                                approval_id,
                                tool_call_id,
                                None,
                                "recovery_required",
                            )
                        }
                    };

                    let reconciled_event = match reconciliation.protected_event {
                        Some(protected_event) => {
                            let event_sequence = session
                                .stream_head
                                .checked_add(1)
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                            let session_update = tx
                                .compare_and_swap::<AiSessionRecord>(
                                    &session.id,
                                    session.row_version,
                                    AiSessionRecordWhereInput::default(),
                                    UpdateAiSessionRecordInput {
                                        stream_head: Some(event_sequence),
                                        last_activity_at: Some(now.unix_timestamp()),
                                        ..Default::default()
                                    },
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            if !matches!(session_update, ConditionalUpdateOutcome::Updated(_)) {
                                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                            }
                            Some((event_sequence, protected_event))
                        }
                        None => None,
                    };
                    let run_update = tx
                        .compare_and_swap::<AiRunRecord>(
                            &current.id,
                            current.row_version,
                            exact_state(AiRunState::WaitingApproval.as_str()),
                            UpdateAiRunRecordInput {
                                state: Some(final_state.as_str().to_owned()),
                                lease_owner: Some(None),
                                lease_expires_at: Some(None),
                                lease_heartbeat_at: Some(None),
                                next_attempt_at: Some(None),
                                error_code: Some(Some(reconciliation.outcome_code.clone())),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?;
                    if !matches!(run_update, ConditionalUpdateOutcome::Updated(_)) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    if let Some((event_sequence, protected_event)) = &reconciled_event {
                        tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
                            id: reconciliation.event_id,
                            session_id: session.id,
                            sequence: *event_sequence,
                            event_type: "approval_wait_reconciled".to_owned(),
                            run_id: Some(current.id),
                            causation_id: tool_call_id.or(approval_id).map(|id| id.to_string()),
                            correlation_id: approval_id.unwrap_or(current.id).to_string(),
                            protected_payload: protected_event.clone(),
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                    }
                    tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
                        actor_principal_kind: "ai_worker".to_owned(),
                        actor_subject: reconciliation.worker_id,
                        action: "ai.run.approval_wait_reconcile".to_owned(),
                        resource_kind: "ai_run".to_owned(),
                        resource_reference: current.id.to_string(),
                        outcome: audit_outcome.to_owned(),
                        reason_code: terminal_outcome_code.clone(),
                        correlation_id: approval_id.unwrap_or(current.id).to_string(),
                        causation_id: tool_call_id.map(|id| id.to_string()),
                        policy_version: reconciliation.policy_version,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    if let Some(lease) = lease.as_ref() {
                        append_attempt_outcome(
                            tx,
                            lease,
                            final_state,
                            terminal_outcome_code.clone(),
                            provider_response_id,
                            now,
                        )
                        .await?;
                    }
                    append_terminal_run_event(
                        tx,
                        &current,
                        final_state,
                        Some(terminal_outcome_code.as_str()),
                        now,
                    )
                    .await?;
                    if let Some((event_sequence, _)) = reconciled_event {
                        tx.queue_event(AiSessionWakeup {
                            session_id: session.id,
                            sequence: event_sequence,
                        });
                    }
                    Ok(())
                })
            })
            .await
    }

    async fn update_active_lease(
        &self,
        lease: &AiRunLease,
        update: LeaseUpdate,
    ) -> Result<AiRunLease, AiError> {
        for retry in 0..=self.limits.maximum_transaction_retries {
            match self.update_active_lease_once(lease, update).await {
                Ok(renewed) => return Ok(renewed),
                Err(TransactionError::Retryable(_))
                    if retry < self.limits.maximum_transaction_retries =>
                {
                    // Only the rolled-back fenced state transition repeats.
                    // A small bounded delay lets the competing writer finish.
                    tokio::time::sleep(std::time::Duration::from_millis(5 << retry.min(5))).await;
                }
                Err(error) => return Err(map_transaction(error)),
            }
        }
        Err(AiError::PersistenceFailed)
    }

    async fn update_active_lease_once(
        &self,
        lease: &AiRunLease,
        update: LeaseUpdate,
    ) -> Result<AiRunLease, TransactionError> {
        let lease = lease.clone();
        let lease_ttl = self.limits.lease_ttl;
        let clock = self.clock.clone();
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    // Lock admission may itself wait. Check the current time
                    // after acquiring it, so contention cannot revive expiry.
                    let now = canonical_second(clock.now());
                    let current = load_and_validate_active_lease(tx, &lease, now).await?;
                    let current_state = persisted_state(&current)?;
                    let next_state = match update {
                        LeaseUpdate::Heartbeat
                            if matches!(
                                current_state,
                                AiRunState::Leased | AiRunState::Running
                            ) =>
                        {
                            current_state
                        }
                        LeaseUpdate::Start if current_state == AiRunState::Leased => {
                            AiRunState::Running
                        }
                        _ => return Err(OrmPublicError::new(OrmErrorCode::Conflict)),
                    };
                    let expiry = now
                        .checked_add(lease_ttl)
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                    let outcome = tx
                        .compare_and_swap::<AiRunRecord>(
                            &current.id,
                            current.row_version,
                            exact_state(&current.state),
                            UpdateAiRunRecordInput {
                                state: Some(next_state.as_str().to_owned()),
                                lease_expires_at: Some(Some(expiry.unix_timestamp())),
                                lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?;
                    match outcome {
                        ConditionalUpdateOutcome::Updated(updated) => lease_from_record(&updated),
                        ConditionalUpdateOutcome::NotFound | ConditionalUpdateOutcome::Conflict => {
                            Err(OrmPublicError::new(OrmErrorCode::Conflict))
                        }
                    }
                })
            })
            .await
    }
}

#[derive(Clone, Copy)]
enum LeaseUpdate {
    Heartbeat,
    Start,
}

/// Exact transition chosen for one expired active lease.
struct ExpiredLeaseTransition {
    next_state: AiRunState,
    outcome_code: &'static str,
    next_retry_count: i64,
    next_attempt_at: Option<i64>,
    provider_response_id: Option<String>,
}

/// Outcome of reconciling one expired-lease candidate in its own transaction.
enum ExpiredLeaseRecovery {
    /// The row changed or renewed after the scan; nothing was written.
    Unchanged,
    /// The row moved to the transition's state.
    Recovered(AiRunState, &'static str),
    /// The row's durable lease or checkpoint state was malformed and it was
    /// closed as `RecoveryRequired`.
    Quarantined,
}

/// Classifies one expired lease from committed rows only. Every write happens
/// in [`recover_expired_run`]; malformed linked state returns `InternalError`.
async fn classify_expired_lease(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    current: &AiRunRecord,
    lease: &AiRunLease,
    now: OffsetDateTime,
    limits: AiRunServiceLimits,
    filtering_excluded: bool,
) -> Result<ExpiredLeaseTransition, OrmPublicError> {
    // A missing linked row, or a check that needs a complete set of rows, is
    // proof of a stored defect only when no read can have been filtered
    // (see [`recovery_filtering_excluded`]). Field-to-field contradictions
    // between rows that were read are durable regardless.
    let current_state = persisted_state(current)?;
    let (final_checkpoint, adoptable_tool_batch, inherited_checkpoint) =
        if let Some(checkpoint_id) = current.latest_checkpoint_id {
            let checkpoint = tx
                .query::<AiRunCheckpointRecord>()
                .filter(AiRunCheckpointRecordWhereInput {
                    id: Some(UuidFilter {
                        eq: Some(checkpoint_id),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .internal_limit(1)
                .fetch_one()
                .await
                .map_err(OrmPublicError::from)?
                .ok_or_else(|| missing_linked_row(filtering_excluded))?;
            // A checkpoint written by an earlier attempt is inherited only by an
            // adoption requeue: `claim_once` keeps the pointer for the adopting
            // attempt, which clears it when it consumes the batch. Validate it
            // against the attempt and generation that wrote it.
            let source_attempt_id = checkpoint.attempt_id;
            let source_generation = checkpoint.lease_generation;
            let inherited = source_generation < lease.lease_generation;
            if checkpoint.run_id != current.id
                || source_generation > lease.lease_generation
                || (!inherited && source_attempt_id != lease.attempt_id)
                || (inherited
                    && !matches!(
                        checkpoint.checkpoint_kind.as_str(),
                        "tool_batch_persisted"
                            | "supervised_tool_batch_persisted"
                            | native_checkpoints::OUTCOME_KIND
                    ))
            {
                return Err(recovery_state_invalid());
            }
            if checkpoint.checkpoint_kind == "assistant_output_persisted" {
                let message_id = checkpoint
                    .assistant_message_id
                    .ok_or_else(recovery_state_invalid)?;
                let budget_reservation_id = checkpoint
                    .budget_reservation_id
                    .ok_or_else(recovery_state_invalid)?;
                let reservation = tx
                    .find_by_id::<AiBudgetReservationRecord>(&budget_reservation_id)
                    .await
                    .map_err(OrmPublicError::from)?
                    .ok_or_else(|| missing_linked_row(filtering_excluded))?;
                if reservation.session_id != current.session_id
                    || reservation.run_id != current.id
                    || reservation.attempt_id != source_attempt_id
                    || reservation.lease_generation != source_generation
                    || reservation.state != "committed"
                    || reservation.actual_runs != Some(1)
                    || reservation.reconciled_at.is_none()
                {
                    return Err(recovery_state_invalid());
                }
                let expected_hash = final_output_checkpoint_hash(
                    lease.run_id,
                    source_attempt_id,
                    source_generation,
                    message_id,
                    checkpoint.provider_response_id.as_deref(),
                    budget_reservation_id,
                );
                if checkpoint.checkpoint_hash != expected_hash {
                    return Err(recovery_state_invalid());
                }
                let message = tx
                    .find_by_id::<AiMessageRecord>(&message_id)
                    .await
                    .map_err(OrmPublicError::from)?
                    .ok_or_else(|| missing_linked_row(filtering_excluded))?;
                if message.session_id != current.session_id
                    || message.run_id != Some(current.id)
                    || message.message_role != "assistant"
                    || message.provider_kind.as_deref() != Some(reservation.provider_kind.as_str())
                    || message.provider_model.as_deref()
                        != Some(reservation.provider_model.as_str())
                    || message.completion_state != "complete"
                    || message.finalized_at.is_none()
                    || !(1..=4_096).contains(&message.block_count)
                {
                    return Err(recovery_state_invalid());
                }
                let mut blocks = tx
                    .query::<AiMessageBlockRecord>()
                    .filter(AiMessageBlockRecordWhereInput {
                        message_id: Some(UuidFilter {
                            eq: Some(message_id),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .internal_limit(message.block_count + 1)
                    .fetch_all()
                    .await
                    .map_err(OrmPublicError::from)?;
                blocks.sort_by_key(|block| block.block_index);
                // Completeness: the count and the index sequence are derived
                // from enumerating the visible blocks.
                if i64::try_from(blocks.len()).ok() != Some(message.block_count)
                    || blocks
                        .iter()
                        .enumerate()
                        .any(|(index, block)| i64::try_from(index).ok() != Some(block.block_index))
                {
                    return Err(missing_linked_row(filtering_excluded));
                }
                if blocks
                    .iter()
                    .any(|block| block.byte_count < 0 || block.line_count < 1)
                {
                    return Err(recovery_state_invalid());
                }
                (Some(checkpoint.provider_response_id), false, inherited)
            } else if matches!(
                checkpoint.checkpoint_kind.as_str(),
                "tool_batch_persisted"
                    | "supervised_tool_batch_persisted"
                    | native_checkpoints::OUTCOME_KIND
            ) {
                let native_outcome = checkpoint.checkpoint_kind == native_checkpoints::OUTCOME_KIND;
                let supervised = native_outcome
                    || checkpoint.checkpoint_kind == "supervised_tool_batch_persisted";
                let provider_response_id = checkpoint.provider_response_id.as_deref();
                if provider_response_id.is_some_and(|value| !valid_provider_reference(value)) {
                    return Err(recovery_state_invalid());
                }
                let budget_reservation_id = checkpoint
                    .budget_reservation_id
                    .ok_or_else(recovery_state_invalid)?;
                let protected_state = checkpoint
                    .protected_state
                    .as_ref()
                    .filter(|state| {
                        serde_json::to_vec(state)
                            .is_ok_and(|encoded| encoded.len() <= 64 * 1024 * 1024)
                    })
                    .ok_or_else(recovery_state_invalid)?;
                if checkpoint.assistant_message_id.is_some() {
                    return Err(recovery_state_invalid());
                }
                let reservation = tx
                    .find_by_id::<AiBudgetReservationRecord>(&budget_reservation_id)
                    .await
                    .map_err(OrmPublicError::from)?
                    .ok_or_else(|| missing_linked_row(filtering_excluded))?;
                if reservation.session_id != current.session_id
                    || reservation.run_id != current.id
                    || (!native_outcome
                        && (reservation.attempt_id != source_attempt_id
                            || reservation.lease_generation != source_generation))
                    || reservation.state != "committed"
                    || reservation.actual_runs != Some(1)
                    || reservation.reconciled_at.is_none()
                    || reservation.provider_kind.trim().is_empty()
                    || reservation.provider_model.trim().is_empty()
                {
                    return Err(recovery_state_invalid());
                }
                let expected_hash = coordinator_checkpoint_hash(
                    lease.run_id,
                    source_attempt_id,
                    source_generation,
                    checkpoint.id,
                    &checkpoint.checkpoint_kind,
                    &reservation.provider_kind,
                    &reservation.provider_model,
                    provider_response_id,
                    budget_reservation_id,
                    protected_state,
                )
                .map_err(|_| recovery_state_invalid())?;
                if checkpoint.checkpoint_hash != expected_hash {
                    return Err(recovery_state_invalid());
                }
                let calls = tx
                    .query::<AiToolCallRecord>()
                    .filter(AiToolCallRecordWhereInput {
                        run_id: Some(UuidFilter {
                            eq: Some(current.id),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .internal_limit(4_097)
                    .fetch_all()
                    .await
                    .map_err(OrmPublicError::from)?;
                let relevant = calls
                    .iter()
                    .filter(|call| {
                        call.lease_generation == source_generation
                            && call.provider_response_id.as_deref() == provider_response_id
                            && call.budget_reservation_id == Some(budget_reservation_id)
                    })
                    .collect::<Vec<_>>();
                // Completeness: counts derived from enumerating the batch.
                if relevant.is_empty()
                    || relevant.len() > 4_096
                    || (supervised && relevant.len() != 1)
                {
                    return Err(missing_linked_row(filtering_excluded));
                }
                if relevant.iter().any(|call| {
                    !matches!(call.state.as_str(), "completed" | "execution_failed")
                        || call.protected_result.is_none()
                        || call.result_egress_decision_id.is_none()
                        || call.result_egress_manifest_hash.is_none()
                        || call.completed_at.is_none()
                }) {
                    return Err(recovery_state_invalid());
                }
                if native_outcome {
                    native_checkpoints::outcome(
                        tx,
                        current.id,
                        current.session_id,
                        source_generation,
                        &reservation,
                        provider_response_id,
                        relevant[0].id,
                        None,
                        None,
                    )
                    .await
                    .map_err(|error| match error {
                        native_checkpoints::NativeCheckError::Contradiction => {
                            recovery_state_invalid()
                        }
                        native_checkpoints::NativeCheckError::Incomplete
                        | native_checkpoints::NativeCheckError::Missing => {
                            missing_linked_row(filtering_excluded)
                        }
                        native_checkpoints::NativeCheckError::Operational(error) => error,
                    })?;
                }
                for call in relevant {
                    let authorization_matches = if supervised {
                        let Some(approval_id) = call.approval_id else {
                            return Err(recovery_state_invalid());
                        };
                        let approval = tx
                            .find_by_id::<AiApprovalRecord>(&approval_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(|| missing_linked_row(filtering_excluded))?;
                        matches!(
                            call.risk.as_str(),
                            "low_risk_write" | "non_idempotent_write" | "high_impact"
                        ) && approval.tool_call_id == call.id
                            && approval.session_id == current.session_id
                            && approval.state == "consumed"
                            && approval.maximum_uses == 1
                            && approval.consumed_uses == 1
                            && approval.consumed_at.is_some()
                            && approval.argument_hash == call.argument_hash
                            && approval.tool_fingerprint == call.tool_fingerprint
                            && call.authorization_policy_version.as_deref()
                                == Some(approval.policy_version.as_str())
                            && call.authorization_state_digest.as_deref()
                                == Some(approval.authorization_state_digest.as_str())
                    } else {
                        call.risk == "read_only" && call.approval_id.is_none()
                    };
                    let step = tx
                        .find_by_id::<AiRunStepRecord>(&call.id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(|| missing_linked_row(filtering_excluded))?;
                    if step.run_id != current.id
                        || step.lease_generation != source_generation
                        || step.state != call.state
                        || step.finished_at.is_none()
                        || !authorization_matches
                    {
                        return Err(recovery_state_invalid());
                    }
                }
                // Read-only provider-retained/stateless batches
                // and the narrow supervised provider-retained
                // batch have complete durable
                // tool/approval/step/budget proof here. The
                // adopter still opens and validates every
                // protected row under current authority before
                // transport.
                (None, true, inherited)
            } else {
                (None, false, inherited)
            }
        } else {
            (None, false, false)
        };
    if (final_checkpoint.is_some() && current_state != AiRunState::Running)
        || (adoptable_tool_batch
            && current_state != AiRunState::Running
            && !(inherited_checkpoint && current_state == AiRunState::Leased))
    {
        return Err(recovery_state_invalid());
    }
    let (next_state, outcome_code, next_retry_count, next_attempt_at, provider_response_id) =
        if let Some(provider_response_id) = final_checkpoint {
            (
                AiRunState::Completed,
                "lease_expired_after_output_persisted",
                current.retry_count,
                None,
                provider_response_id,
            )
        } else if adoptable_tool_batch {
            let retry_count =
                u32::try_from(current.retry_count).map_err(|_| recovery_state_invalid())?;
            if retry_count >= limits.maximum_run_retries {
                (
                    AiRunState::RecoveryRequired,
                    "checkpoint_adoption_retry_exhausted",
                    current.retry_count,
                    None,
                    None,
                )
            } else {
                (
                    AiRunState::RetryScheduled,
                    "checkpoint_adoption_ready",
                    current
                        .retry_count
                        .checked_add(1)
                        .ok_or_else(recovery_state_invalid)?,
                    Some(now.unix_timestamp()),
                    None,
                )
            }
        } else if current_state == AiRunState::Leased {
            let retry_count =
                u32::try_from(current.retry_count).map_err(|_| recovery_state_invalid())?;
            if retry_count >= limits.maximum_run_retries {
                (
                    AiRunState::Failed,
                    "lease_expired_retry_exhausted",
                    current.retry_count,
                    None,
                    None,
                )
            } else {
                (
                    AiRunState::RetryScheduled,
                    "lease_expired_before_start",
                    current
                        .retry_count
                        .checked_add(1)
                        .ok_or_else(recovery_state_invalid)?,
                    Some(now.unix_timestamp()),
                    None,
                )
            }
        } else {
            (
                AiRunState::RecoveryRequired,
                "lease_expired_after_start",
                current.retry_count,
                None,
                None,
            )
        };
    Ok(ExpiredLeaseTransition {
        next_state,
        outcome_code,
        next_retry_count,
        next_attempt_at,
        provider_response_id,
    })
}

/// Reconciles one scanned expired-lease candidate.
///
/// Malformed linked state cannot abort the whole pass: the row is closed as
/// `RecoveryRequired` with `lease_recovery_state_invalid`, which is never
/// replayed, and the caller continues with the next candidate.
async fn recover_expired_run(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    run_id: Uuid,
    now: OffsetDateTime,
    cutoff: i64,
    limits: AiRunServiceLimits,
    filtering_excluded: bool,
) -> Result<ExpiredLeaseRecovery, OrmPublicError> {
    crate::orm_runtime_recovery::require_live_epoch(tx).await?;
    let Some(current) = tx
        .find_by_id::<AiRunRecord>(&run_id)
        .await
        .map_err(OrmPublicError::from)?
    else {
        return Ok(ExpiredLeaseRecovery::Unchanged);
    };
    if !EXPIRED_LEASE_RECOVERY_STATES
        .iter()
        .any(|state| state.as_str() == current.state)
        || current
            .lease_expires_at
            .is_some_and(|expires_at| expires_at > cutoff)
    {
        return Ok(ExpiredLeaseRecovery::Unchanged);
    }
    let lease = current
        .lease_expires_at
        .ok_or_else(recovery_state_invalid)
        .and_then(|_| lease_from_record(&current).map_err(|_| recovery_state_invalid()));
    let classified = match &lease {
        Ok(lease) => {
            classify_expired_lease(tx, &current, lease, now, limits, filtering_excluded).await
        }
        Err(error) => Err(error.clone()),
    };
    // Only an explicitly identified durable failure of the run's own stored
    // state quarantines it: a malformed lease, checkpoint, reservation, call,
    // step or approval linkage found by classification, or terminal-event
    // linkage (a missing or inconsistent session, an unreadable principal
    // reference) that the terminal event would refuse. Every other error (a
    // host policy refusal, a refused read, a database error) returns to the
    // caller, which leaves the run untouched.
    let classified = match classified {
        Ok(transition) if AiRunTerminalEvent::from_run_state(transition.next_state).is_some() => {
            match terminal_event_session(tx, &current).await? {
                Ok(_) => Ok(transition),
                // A session a registered row policy hides is not proven absent.
                Err(defect) if defect.code == OrmErrorCode::NotFound && !filtering_excluded => {
                    Err(missing_linked_row(false))
                }
                Err(_) => Err(recovery_state_invalid()),
            }
        }
        other => other,
    };
    let (transition, quarantined) = match classified {
        Ok(transition) => (transition, false),
        Err(error) if is_recovery_state_invalid(&error) => (
            ExpiredLeaseTransition {
                next_state: AiRunState::RecoveryRequired,
                outcome_code: "lease_recovery_state_invalid",
                next_retry_count: current.retry_count,
                next_attempt_at: None,
                provider_response_id: None,
            },
            true,
        ),
        Err(error) => return Err(error),
    };
    let update = tx
        .compare_and_swap::<AiRunRecord>(
            &current.id,
            current.row_version,
            exact_state(&current.state),
            UpdateAiRunRecordInput {
                state: Some(transition.next_state.as_str().to_owned()),
                attempt_id: Some(None),
                lease_owner: Some(None),
                lease_expires_at: Some(None),
                lease_heartbeat_at: Some(None),
                retry_count: Some(transition.next_retry_count),
                next_attempt_at: Some(transition.next_attempt_at),
                error_code: Some(
                    (transition.next_state != AiRunState::Completed)
                        .then_some(transition.outcome_code.to_owned()),
                ),
                ..Default::default()
            },
        )
        .await
        .map_err(OrmPublicError::from)?;
    if !matches!(update, ConditionalUpdateOutcome::Updated(_)) {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    if quarantined {
        // No attempt outcome or terminal event is written: both validate the
        // linkage that failed classification. The audit row and the run state
        // are the durable record; clients observe the state on their next read.
        // The run's approvals and parked provider session still close here.
        close_run_dependents(tx, &current).await?;
        tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
            actor_principal_kind: "system".to_owned(),
            actor_subject: "run-lease-recovery".to_owned(),
            action: "ai.run.lease_recovery_quarantined".to_owned(),
            resource_kind: "ai_run".to_owned(),
            resource_reference: current.id.to_string(),
            outcome: "denied".to_owned(),
            reason_code: "lease_recovery_state_invalid".to_owned(),
            correlation_id: current.id.to_string(),
            causation_id: current.attempt_id.map(|attempt| attempt.to_string()),
            policy_version: None,
        })
        .await
        .map_err(OrmPublicError::from)?;
        return Ok(ExpiredLeaseRecovery::Quarantined);
    }
    let lease = lease?;
    append_attempt_outcome(
        tx,
        &lease,
        transition.next_state,
        transition.outcome_code.to_owned(),
        transition.provider_response_id,
        now,
    )
    .await?;
    if AiRunTerminalEvent::from_run_state(transition.next_state).is_some() {
        append_terminal_run_event(
            tx,
            &current,
            transition.next_state,
            Some(transition.outcome_code),
            now,
        )
        .await?;
    }
    Ok(ExpiredLeaseRecovery::Recovered(
        transition.next_state,
        transition.outcome_code,
    ))
}

/// Commits one expired unresolved reservation whose attempt is closed.
/// Returns `false` without writing when the reservation is still owned.
/// Stamps a reservation the sweep must leave to its owner, inside the
/// settlement transaction that checked the live epoch and observed the owner.
async fn stamp_owned_reservation(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    reservation: &AiBudgetReservationRecord,
    now: OffsetDateTime,
) -> Result<(), OrmPublicError> {
    if !stamp_reservation_settlement_progress(
        tx,
        reservation.id,
        reservation.row_version,
        &reservation.state,
        now.unix_timestamp(),
    )
    .await?
    {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    Ok(())
}

/// Outcome of examining one stranded-reservation candidate.
enum ReservationSettlement {
    /// Committed at its reserved estimate.
    Settled,
    /// No longer unresolved or expired; it left the scan set.
    Gone,
    /// Still owned by a live attempt or a background submission.
    Owned,
}

async fn settle_stranded_budget_reservation(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    reservation_id: Uuid,
    now: OffsetDateTime,
) -> Result<ReservationSettlement, OrmPublicError> {
    let Some(reservation) = tx
        .find_by_id::<AiBudgetReservationRecord>(&reservation_id)
        .await
        .map_err(OrmPublicError::from)?
    else {
        return Ok(ReservationSettlement::Gone);
    };
    if !matches!(reservation.state.as_str(), "reserved" | "uncertain")
        || reservation.expires_at > now.unix_timestamp()
    {
        return Ok(ReservationSettlement::Gone);
    }
    let Some(run) = tx
        .find_by_id::<AiRunRecord>(&reservation.run_id)
        .await
        .map_err(OrmPublicError::from)?
    else {
        return Err(OrmPublicError::new(OrmErrorCode::InternalError));
    };
    if run.session_id != reservation.session_id {
        return Err(OrmPublicError::new(OrmErrorCode::InternalError));
    }
    let attempt_live = run.attempt_id == Some(reservation.attempt_id)
        && run.lease_generation == reservation.lease_generation
        && run.lease_expires_at.is_some();
    if attempt_live {
        stamp_owned_reservation(tx, &reservation, now).await?;
        return Ok(ReservationSettlement::Owned);
    }
    let submissions = tx
        .query::<AiProviderBackgroundSubmissionRecord>()
        .filter(AiProviderBackgroundSubmissionRecordWhereInput {
            run_id: Some(UuidFilter {
                eq: Some(run.id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .internal_limit(4_097)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    if submissions.len() > 4_096
        || submissions
            .iter()
            .any(|submission| submission.budget_reservation_id == reservation.id)
    {
        stamp_owned_reservation(tx, &reservation, now).await?;
        return Ok(ReservationSettlement::Owned);
    }
    crate::orm_budget::commit_stranded_reservation(tx, &reservation, now).await?;
    tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
        actor_principal_kind: "system".to_owned(),
        actor_subject: "run-lease-recovery".to_owned(),
        action: "ai.budget.reservation_settled".to_owned(),
        resource_kind: "ai_budget_reservation".to_owned(),
        resource_reference: reservation.id.to_string(),
        outcome: "allowed".to_owned(),
        reason_code: "budget_reservation_attempt_closed".to_owned(),
        correlation_id: run.id.to_string(),
        causation_id: Some(reservation.attempt_id.to_string()),
        policy_version: Some(reservation.pricing_policy_version.clone()),
    })
    .await
    .map_err(OrmPublicError::from)?;
    Ok(ReservationSettlement::Settled)
}

/// Row security the backend applies to the AI tables for the current role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BackendRowSecurity {
    /// The backend has no row security (SQLite).
    None,
    /// Row security exists but is not active for this role on any AI table.
    Inactive,
    /// Row security is active for this role on at least one AI table.
    Active,
    /// The state could not be established.
    Unknown,
}

/// Whether recovery can rule out that a read was filtered: no ORM row policy
/// is registered and the backend cannot be filtering the AI tables for the
/// current role.
fn filtering_excluded_decision(row_policy_registered: bool, backend: BackendRowSecurity) -> bool {
    !row_policy_registered
        && matches!(
            backend,
            BackendRowSecurity::None | BackendRowSecurity::Inactive
        )
}

/// Decides, once per recovery pass, whether reads can have been filtered.
///
/// A host row policy, or PostgreSQL row-level security active for the
/// recovery role on any AI table, can hide an existing row, and no read
/// tells hidden from absent. Then a missing linked row or an incomplete set
/// of rows is an operational skip, never a quarantine. The PostgreSQL state
/// is read through the ORM's row-security introspection
/// (`row_security_active` for each AI table and the current role); a failed
/// or incomplete introspection counts as "cannot be excluded". The run
/// service is compiled only for SQLite and PostgreSQL.
pub(crate) async fn recovery_filtering_excluded(database: &Database<DefaultWriteBackend>) -> bool {
    use graphql_orm::graphql::orm::{OrmBackend, RlsIntrospectionBackend, RlsTableProbe};
    let backend =
        match <DefaultWriteBackend as RlsIntrospectionBackend>::introspect_rls_enforcement(
            database.pool(),
        )
        .await
        {
            Ok(None) => BackendRowSecurity::None,
            Err(_) => BackendRowSecurity::Unknown,
            Ok(Some(_)) => {
                let probes = <crate::AiSchemaModule as graphql_orm::graphql::orm::OrmSchemaModule>::entities(
                &crate::AiSchemaModule,
            )
            .iter()
            .map(|entity| RlsTableProbe {
                table_sql: <DefaultWriteBackend as OrmBackend>::DIALECT
                    .render_identifier_path(entity.table_name)
                    .into_owned(),
                expressions: Vec::new(),
                render: false,
            })
            .collect::<Vec<_>>();
                match <DefaultWriteBackend as RlsIntrospectionBackend>::introspect_rls_target(
                    database.pool(),
                    &probes,
                )
                .await
                {
                    Ok(target)
                        if target.tables.len() == probes.len()
                            && target.tables.iter().all(|table| table.relation.is_some()) =>
                    {
                        if target.tables.iter().any(|table| table.row_security_active) {
                            BackendRowSecurity::Active
                        } else {
                            BackendRowSecurity::Inactive
                        }
                    }
                    _ => BackendRowSecurity::Unknown,
                }
            }
        };
    filtering_excluded_decision(database.row_policy().is_some(), backend)
}

/// Safe message marking a durable validation failure of a run's own stored
/// recovery state, as opposed to an operational failure.
const RECOVERY_STATE_INVALID: &str = "AI run recovery state is invalid";

/// A durable validation failure of the run's own stored state. Only this
/// error quarantines an expired run.
fn recovery_state_invalid() -> OrmPublicError {
    OrmPublicError::with_message(OrmErrorCode::InternalError, RECOVERY_STATE_INVALID)
}

/// Safe message for a linked row that recovery could not read while a row
/// policy is registered: an operational skip, never a quarantine.
const LINKED_ROW_NOT_VISIBLE: &str = "AI run recovery could not read a linked row";

/// A linked row recovery could not read, or a check that needs a complete
/// set of rows failed. Durable only when `filtering_excluded`.
fn missing_linked_row(filtering_excluded: bool) -> OrmPublicError {
    if filtering_excluded {
        recovery_state_invalid()
    } else {
        OrmPublicError::with_message(OrmErrorCode::ServiceUnavailable, LINKED_ROW_NOT_VISIBLE)
    }
}

fn is_recovery_state_invalid(error: &OrmPublicError) -> bool {
    error.code == OrmErrorCode::InternalError && error.message == RECOVERY_STATE_INVALID
}

/// Loads and validates the session a terminal run event is written to.
///
/// The outer error is operational (a read failed or was refused). The inner
/// error is a durable defect of the run's stored linkage: an unreadable
/// principal reference, a missing session, or a session that does not match
/// the run's owner, scope or tenant.
async fn terminal_event_session(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    run: &AiRunRecord,
) -> Result<Result<AiSessionRecord, OrmPublicError>, OrmPublicError> {
    let Ok(principal_reference) =
        serde_json::from_value::<PrincipalReference>(run.principal_reference.clone())
    else {
        return Ok(Err(OrmPublicError::new(OrmErrorCode::InternalError)));
    };
    let expected_principal_kind = match &principal_reference.kind {
        agql_auth::PrincipalReferenceKind::UserSession => "user".to_owned(),
        agql_auth::PrincipalReferenceKind::ApiToken { principal_kind } => {
            format!("api_token:{principal_kind}")
        }
    };
    let Some(session) = tx
        .find_by_id::<AiSessionRecord>(&run.session_id)
        .await
        .map_err(OrmPublicError::from)?
    else {
        return Ok(Err(OrmPublicError::not_found()));
    };
    let valid_session_state = match session.state.as_str() {
        "active" | "archived" => session.deleted_at.is_none(),
        "deleting" => session.deleted_at.is_some(),
        _ => false,
    };
    if run.id.is_nil()
        || run.lease_generation <= 0
        || session.id != run.session_id
        || !valid_session_state
        || session.owner_principal_kind != expected_principal_kind
        || session.owner_subject != principal_reference.subject
        || session.scope_kind.trim().is_empty()
        || session.scope_id.trim().is_empty()
        || session.stream_head < 0
        || session.stream_head == i64::MAX
        || principal_reference
            .tenant_id
            .as_ref()
            .is_some_and(|tenant_id| session.tenant_id.as_ref() != Some(tenant_id))
    {
        return Ok(Err(OrmPublicError::new(OrmErrorCode::InternalError)));
    }
    Ok(Ok(session))
}

/// Records that recovery examined `run_id` and left it unchanged, so the run
/// rotates behind the rest of the scan. Fenced like every runtime write: it
/// requires the live recovery epoch and the exact row version and state the
/// scan observed, and changes nothing else. The stamp is
/// `max(now, previous + 1)`, so repeated passes within one second still
/// rotate. Returns `false` when the run changed since it was scanned (someone
/// else progressed it); any other failure is returned as an error.
async fn stamp_run_recovery_progress(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    run_id: Uuid,
    observed_version: i64,
    observed_state: &str,
    now: i64,
) -> Result<bool, OrmPublicError> {
    crate::orm_runtime_recovery::require_live_epoch(tx).await?;
    let Some(current) = tx
        .find_by_id::<AiRunRecord>(&run_id)
        .await
        .map_err(OrmPublicError::from)?
    else {
        return Ok(false);
    };
    if current.row_version != observed_version || current.state != observed_state {
        return Ok(false);
    }
    let stamp = now.max(current.recovery_examined_at.saturating_add(1));
    let outcome = tx
        .compare_and_swap::<AiRunRecord>(
            &run_id,
            observed_version,
            exact_state(observed_state),
            UpdateAiRunRecordInput {
                recovery_examined_at: Some(stamp),
                ..Default::default()
            },
        )
        .await
        .map_err(OrmPublicError::from)?;
    Ok(matches!(outcome, ConditionalUpdateOutcome::Updated(_)))
}

/// Records that the settlement sweep examined a reservation and left it
/// unresolved, under the same fencing and stamp rule as
/// [`stamp_run_recovery_progress`].
async fn stamp_reservation_settlement_progress(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    reservation_id: Uuid,
    observed_version: i64,
    observed_state: &str,
    now: i64,
) -> Result<bool, OrmPublicError> {
    crate::orm_runtime_recovery::require_live_epoch(tx).await?;
    let Some(current) = tx
        .find_by_id::<AiBudgetReservationRecord>(&reservation_id)
        .await
        .map_err(OrmPublicError::from)?
    else {
        return Ok(false);
    };
    if current.row_version != observed_version || current.state != observed_state {
        return Ok(false);
    }
    let stamp = now.max(current.settlement_examined_at.saturating_add(1));
    let outcome = tx
        .compare_and_swap::<AiBudgetReservationRecord>(
            &reservation_id,
            observed_version,
            AiBudgetReservationRecordWhereInput {
                state: Some(StringFilter {
                    eq: Some(observed_state.to_owned()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            UpdateAiBudgetReservationRecordInput {
                settlement_examined_at: Some(stamp),
                ..Default::default()
            },
        )
        .await
        .map_err(OrmPublicError::from)?;
    Ok(matches!(outcome, ConditionalUpdateOutcome::Updated(_)))
}

/// Active states whose expired lease the recovery pass reconciles.
const EXPIRED_LEASE_RECOVERY_STATES: [AiRunState; 4] = [
    AiRunState::Leased,
    AiRunState::Running,
    AiRunState::WaitingTool,
    AiRunState::WaitingReauth,
];

/// Read-only validation result for one approved approval handoff.
enum ApprovedHandoff {
    /// The exact wait graph is current and may be claimed.
    Proceed {
        call: Box<AiToolCallRecord>,
        run: Box<AiRunRecord>,
        parked_wait: bool,
    },
    /// The graph is repairable by other maintenance; leave it eligible.
    Skip,
    /// The approval can never be consumed; expire it with this safe reason.
    Expire(&'static str),
}

/// Closes everything that only a live run may still use, in the caller's
/// transaction: every unconsumed approval, and a provider session parked on
/// the run's wait.
///
/// Every terminal run transition calls this through
/// [`append_terminal_run_event`]; cancellation and the quarantine paths, which
/// write no terminal event, call it directly.
pub(crate) async fn close_run_dependents(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    run: &AiRunRecord,
) -> Result<(), OrmPublicError> {
    expire_open_approvals_of_run(tx, run.id).await?;
    close_parked_provider_session_of_run(tx, run).await
}

/// Moves a provider session parked on `run`'s wait to exact-reference cleanup
/// because the run closed.
///
/// Without this, cleanup could learn that the run closed only by reading the
/// run of every parked binding, which no bounded SQL predicate can express.
/// The binding now reaches the ordinary `cleanup_required` scan bucket in the
/// same transaction that closes the run.
async fn close_parked_provider_session_of_run(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    run: &AiRunRecord,
) -> Result<(), OrmPublicError> {
    const REASON: &str = "provider_session_parked_wait_closed";
    let bindings = tx
        .query::<AiProviderSessionBindingRecord>()
        .filter(
            crate::orm_provider_session::AiProviderSessionBindingRecordWhereInput {
                session_id: Some(UuidFilter {
                    eq: Some(run.session_id),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .internal_limit(2)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    for binding in bindings {
        if binding.state != "parked_wait" || binding.claimed_run_id != Some(run.id) {
            continue;
        }
        let outcome = tx
            .compare_and_swap::<AiProviderSessionBindingRecord>(
                &binding.id,
                binding.row_version,
                crate::orm_provider_session::AiProviderSessionBindingRecordWhereInput {
                    state: Some(StringFilter {
                        eq: Some("parked_wait".to_owned()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                crate::orm_provider_session::UpdateAiProviderSessionBindingRecordInput {
                    state: Some("cleanup_required".to_owned()),
                    last_run_id: Some(Some(run.id)),
                    claimed_run_id: Some(None),
                    claimed_attempt_id: Some(None),
                    claimed_run_lease_generation: Some(None),
                    claim_owner: Some(None),
                    claim_expires_at: Some(None),
                    cleanup_owner: Some(None),
                    cleanup_lease_expires_at: Some(None),
                    cleanup_next_attempt_at: Some(Some(0)),
                    cleanup_reason_code: Some(Some(REASON.to_owned())),
                    ..Default::default()
                },
            )
            .await
            .map_err(OrmPublicError::from)?;
        if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
        tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
            actor_principal_kind: "system".to_owned(),
            actor_subject: "provider-session-lifecycle".to_owned(),
            action: "ai.provider_session.cleanup_required".to_owned(),
            resource_kind: "ai_provider_session".to_owned(),
            resource_reference: binding.id.to_string(),
            outcome: "allowed".to_owned(),
            reason_code: REASON.to_owned(),
            correlation_id: binding.id.to_string(),
            causation_id: Some(run.id.to_string()),
            policy_version: None,
        })
        .await
        .map_err(OrmPublicError::from)?;
    }
    Ok(())
}

/// Expires every unconsumed (`pending`, `approved` or `resume_claimed`)
/// approval of a run that is closing for good, in the caller's transaction.
///
/// `resume_claimed` is included: an approved handoff whose worker dies before
/// consumption is closed by lease recovery, and the approval must not stay
/// claimable state forever.
///
/// Every terminal run transition calls this through
/// [`append_terminal_run_event`], and cancellation calls it directly, so no
/// exit from `WaitingApproval` can leave an approval that a later decision or
/// approved claim would act on.
pub(crate) async fn expire_open_approvals_of_run(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    run_id: Uuid,
) -> Result<(), OrmPublicError> {
    const MAXIMUM_CALLS: usize = 4_096;
    let calls = tx
        .query::<AiToolCallRecord>()
        .filter(AiToolCallRecordWhereInput {
            run_id: Some(UuidFilter {
                eq: Some(run_id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .internal_limit(MAXIMUM_CALLS as i64 + 1)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    if calls.len() > MAXIMUM_CALLS {
        return Err(OrmPublicError::new(OrmErrorCode::InternalError));
    }
    for call in calls {
        let Some(approval_id) = call.approval_id else {
            continue;
        };
        let Some(approval) = tx
            .find_by_id::<AiApprovalRecord>(&approval_id)
            .await
            .map_err(OrmPublicError::from)?
        else {
            continue;
        };
        if approval.tool_call_id != call.id
            || approval.consumed_uses != 0
            || approval.consumed_at.is_some()
            || !matches!(
                approval.state.as_str(),
                "pending" | "approved" | "resume_claimed"
            )
        {
            continue;
        }
        let expired = tx
            .compare_and_swap::<AiApprovalRecord>(
                &approval.id,
                approval.row_version,
                AiApprovalRecordWhereInput {
                    state: Some(StringFilter {
                        eq: Some(approval.state.clone()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                UpdateAiApprovalRecordInput {
                    state: Some("expired".to_owned()),
                    ..Default::default()
                },
            )
            .await
            .map_err(OrmPublicError::from)?;
        if !matches!(expired, ConditionalUpdateOutcome::Updated(_)) {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
    }
    Ok(())
}

/// Expires an approved approval that can never be handed off and audits it.
async fn expire_unclaimable_approval(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    approval: &AiApprovalRecord,
    worker_id: &str,
    reason_code: &str,
) -> Result<(), OrmPublicError> {
    let expired = tx
        .compare_and_swap::<AiApprovalRecord>(
            &approval.id,
            approval.row_version,
            AiApprovalRecordWhereInput {
                state: Some(StringFilter {
                    eq: Some("approved".to_owned()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            UpdateAiApprovalRecordInput {
                state: Some("expired".to_owned()),
                ..Default::default()
            },
        )
        .await
        .map_err(OrmPublicError::from)?;
    if !matches!(expired, ConditionalUpdateOutcome::Updated(_)) {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
        actor_principal_kind: "ai_worker".to_owned(),
        actor_subject: worker_id.to_owned(),
        action: "ai.approval.expired".to_owned(),
        resource_kind: "ai_approval".to_owned(),
        resource_reference: approval.id.to_string(),
        outcome: "denied".to_owned(),
        reason_code: reason_code.to_owned(),
        correlation_id: approval.id.to_string(),
        causation_id: Some(approval.tool_call_id.to_string()),
        policy_version: Some(approval.policy_version.clone()),
    })
    .await
    .map_err(OrmPublicError::from)?;
    Ok(())
}

/// Closes a queued or retry-scheduled run whose durable claim state is
/// malformed, so it cannot block the bounded claim window.
async fn quarantine_unclaimable_run(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    current: &AiRunRecord,
    worker_id: &str,
) -> Result<(), OrmPublicError> {
    const CODE: &str = "claim_state_invalid";
    let update = tx
        .compare_and_swap::<AiRunRecord>(
            &current.id,
            current.row_version,
            exact_state(&current.state),
            UpdateAiRunRecordInput {
                state: Some(AiRunState::RecoveryRequired.as_str().to_owned()),
                attempt_id: Some(None),
                lease_owner: Some(None),
                lease_expires_at: Some(None),
                lease_heartbeat_at: Some(None),
                next_attempt_at: Some(None),
                error_code: Some(Some(CODE.to_owned())),
                ..Default::default()
            },
        )
        .await
        .map_err(OrmPublicError::from)?;
    if !matches!(update, ConditionalUpdateOutcome::Updated(_)) {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    close_run_dependents(tx, current).await?;
    // No terminal event or attempt outcome is written: both validate the very
    // linkage that is malformed here. The audit row and the run state are the
    // durable record; clients observe the state on their next read.
    tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
        actor_principal_kind: "ai_worker".to_owned(),
        actor_subject: worker_id.to_owned(),
        action: "ai.run.claim_quarantined".to_owned(),
        resource_kind: "ai_run".to_owned(),
        resource_reference: current.id.to_string(),
        outcome: "denied".to_owned(),
        reason_code: CODE.to_owned(),
        correlation_id: current.id.to_string(),
        causation_id: None,
        policy_version: None,
    })
    .await
    .map_err(OrmPublicError::from)?;
    Ok(())
}

pub(crate) async fn abandon_native_approval_candidates(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    run_id: Uuid,
    final_state: AiRunState,
    now: i64,
) -> Result<(), OrmPublicError> {
    let candidates = tx
        .query::<AiNativeApprovalCandidateRecord>()
        .filter(AiNativeApprovalCandidateRecordWhereInput {
            run_id: Some(UuidFilter {
                eq: Some(run_id),
                ..Default::default()
            }),
            state: Some(StringFilter {
                eq: Some("prepared".to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        })
        .internal_limit(2)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    if candidates.len() > 1 || (!candidates.is_empty() && final_state == AiRunState::Completed) {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    for candidate in candidates {
        let call = tx
            .find_by_id::<AiToolCallRecord>(&candidate.id)
            .await
            .map_err(OrmPublicError::from)?
            .ok_or_else(OrmPublicError::not_found)?;
        let step = tx
            .find_by_id::<AiRunStepRecord>(&candidate.id)
            .await
            .map_err(OrmPublicError::from)?
            .ok_or_else(OrmPublicError::not_found)?;
        if call.run_id != run_id
            || step.run_id != run_id
            || call.lease_generation != candidate.lease_generation
            || step.lease_generation != candidate.lease_generation
            || call.state != "approval_prepared"
            || step.state != "approval_prepared"
            || call.approval_id.is_some()
            || candidate.final_approval_id.is_some()
        {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
        if !matches!(
            tx.compare_and_swap::<AiToolCallRecord>(
                &call.id,
                call.row_version,
                AiToolCallRecordWhereInput::default(),
                UpdateAiToolCallRecordInput {
                    state: Some("cancelled".to_owned()),
                    authorization_code: Some(Some("native_approval_abandoned".to_owned())),
                    completed_at: Some(Some(now)),
                    ..Default::default()
                }
            )
            .await
            .map_err(OrmPublicError::from)?,
            ConditionalUpdateOutcome::Updated(_)
        ) || !matches!(
            tx.compare_and_swap::<AiRunStepRecord>(
                &step.id,
                step.row_version,
                AiRunStepRecordWhereInput::default(),
                UpdateAiRunStepRecordInput {
                    state: Some("cancelled".to_owned()),
                    finished_at: Some(Some(now)),
                    error_code: Some(Some("native_approval_abandoned".to_owned())),
                    ..Default::default()
                }
            )
            .await
            .map_err(OrmPublicError::from)?,
            ConditionalUpdateOutcome::Updated(_)
        ) || !matches!(
            tx.compare_and_swap::<AiNativeApprovalCandidateRecord>(
                &candidate.id,
                candidate.row_version,
                AiNativeApprovalCandidateRecordWhereInput::default(),
                UpdateAiNativeApprovalCandidateRecordInput {
                    state: Some("abandoned".to_owned()),
                    ..Default::default()
                }
            )
            .await
            .map_err(OrmPublicError::from)?,
            ConditionalUpdateOutcome::Updated(_)
        ) {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
    }
    Ok(())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

async fn load_native_candidate(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    lease: &AiRunLease,
    id: AiToolCallId,
) -> Result<AiNativeApprovalCandidateRecord, OrmPublicError> {
    let candidate = tx
        .find_by_id::<AiNativeApprovalCandidateRecord>(&id.0)
        .await
        .map_err(OrmPublicError::from)?
        .ok_or_else(OrmPublicError::not_found)?;
    let call = tx
        .find_by_id::<AiToolCallRecord>(&id.0)
        .await
        .map_err(OrmPublicError::from)?
        .ok_or_else(OrmPublicError::not_found)?;
    if candidate.run_id != lease.run_id.0
        || candidate.attempt_id != lease.attempt_id
        || candidate.lease_generation != lease.lease_generation
        || candidate.state != "prepared"
        || candidate.protected_preparation.is_none()
        || candidate.payload_purged_at.is_some()
        || candidate.final_approval_id.is_some()
        || candidate.settled_checkpoint_id.is_some()
        || candidate.finalized_at.is_some()
        || call.run_id != lease.run_id.0
        || call.lease_generation != lease.lease_generation
        || call.state != "approval_prepared"
        || call.approval_id.is_some()
        || call.protected_arguments.is_none()
        || call.payload_purged_at.is_some()
        || call.budget_reservation_id != Some(candidate.budget_reservation_id)
    {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    Ok(candidate)
}

fn session_matches_tool_start(session: &AiSessionRecord, call: &PreparedToolCallStart) -> bool {
    session.state == "active"
        && session.deleted_at.is_none()
        && session.owner_principal_kind == call.expected_owner_principal_kind
        && session.owner_subject == call.expected_owner_subject
        && session.scope_kind == call.expected_scope_kind
        && session.scope_id == call.expected_scope_id
        && session.tenant_id == call.expected_tenant_id
}

fn session_matches_tool_finish(session: &AiSessionRecord, finish: &PreparedToolCallFinish) -> bool {
    session.state == "active"
        && session.deleted_at.is_none()
        && session.owner_principal_kind == finish.expected_owner_principal_kind
        && session.owner_subject == finish.expected_owner_subject
        && session.scope_kind == finish.expected_scope_kind
        && session.scope_id == finish.expected_scope_id
        && session.tenant_id == finish.expected_tenant_id
}

pub(crate) fn exact_state(state: &str) -> AiRunRecordWhereInput {
    AiRunRecordWhereInput {
        state: Some(StringFilter {
            eq: Some(state.to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Requires, inside the caller's transaction, that the store's current epoch
/// is live and that both the run row and `lease` were granted under it.
///
/// Every lease-fenced write calls this, either through
/// [`load_and_validate_active_lease`] or beside its own inline lease check.
pub(crate) async fn require_current_lease_epoch(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    run: &AiRunRecord,
    lease: &AiRunLease,
) -> Result<(), OrmPublicError> {
    let epoch = crate::orm_runtime_recovery::require_live_epoch(tx).await?;
    if run.recovery_epoch_id != Some(epoch) || lease.recovery_epoch_id != Some(epoch) {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    Ok(())
}

/// Closes a freshly leased run whose stored principal reference is delegated
/// or sessionless, in the claim transaction that leased it.
///
/// Releases before 0.130.0 could queue such a run. Its worker would resolve
/// the reference to the user session behind it and act with the full user's
/// authority, so the run never reaches a worker: the lease is finished as
/// `Failed` with [`crate::principal_projection::DELEGATED_REFERENCE_CODE`],
/// which retry admission refuses, exactly as an ordinary fenced completion
/// would record it. Nothing is resolved, decrypted, or sent anywhere.
async fn refuse_delegated_claim(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    leased: &AiRunRecord,
    lease: &AiRunLease,
    now: OffsetDateTime,
) -> Result<(), OrmPublicError> {
    let code = crate::principal_projection::DELEGATED_REFERENCE_CODE;
    let outcome = tx
        .compare_and_swap::<AiRunRecord>(
            &leased.id,
            leased.row_version,
            exact_state(AiRunState::Leased.as_str()),
            UpdateAiRunRecordInput {
                state: Some(AiRunState::Failed.as_str().to_owned()),
                lease_owner: Some(None),
                lease_expires_at: Some(None),
                lease_heartbeat_at: Some(None),
                next_attempt_at: Some(None),
                error_code: Some(Some(code.to_owned())),
                ..Default::default()
            },
        )
        .await
        .map_err(OrmPublicError::from)?;
    if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    append_attempt_outcome(tx, lease, AiRunState::Failed, code.to_owned(), None, now).await?;
    tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
        actor_principal_kind: "ai_worker".to_owned(),
        actor_subject: lease.worker_id().to_owned(),
        action: "ai.run.principal_refused".to_owned(),
        resource_kind: "ai_run".to_owned(),
        resource_reference: leased.id.to_string(),
        outcome: "denied".to_owned(),
        reason_code: code.to_owned(),
        correlation_id: leased.id.to_string(),
        causation_id: leased.attempt_id.map(|attempt_id| attempt_id.to_string()),
        policy_version: None,
    })
    .await
    .map_err(OrmPublicError::from)?;
    append_terminal_run_event(tx, leased, AiRunState::Failed, Some(code), now).await
}

pub(crate) async fn load_and_validate_active_lease(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    lease: &AiRunLease,
    now: OffsetDateTime,
) -> Result<AiRunRecord, OrmPublicError> {
    let current = tx
        .find_by_id::<AiRunRecord>(&lease.run_id.0)
        .await
        .map_err(OrmPublicError::from)?
        .ok_or_else(OrmPublicError::not_found)?;
    require_current_lease_epoch(tx, &current, lease).await?;
    let state = persisted_state(&current)?;
    let stored_reference: PrincipalReference =
        serde_json::from_value(current.principal_reference.clone())
            .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
    if current.session_id != lease.session_id.0
        || current.input_message_id != lease.input_message_id
        || stored_reference != lease.principal_reference
        || current.attempt_id != Some(lease.attempt_id)
        || current.lease_owner.as_deref() != Some(lease.worker_id.as_str())
        || current.lease_generation != lease.lease_generation
        || current.row_version != lease.row_version
        || state != lease.state
        || current.retry_count != i64::from(lease.retry_count)
        || current.latest_checkpoint_id != lease.latest_checkpoint_id
        || current
            .lease_expires_at
            .is_none_or(|expires_at| expires_at <= now.unix_timestamp())
    {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    Ok(current)
}

pub(crate) fn lease_from_record(record: &AiRunRecord) -> Result<AiRunLease, OrmPublicError> {
    let principal_reference = serde_json::from_value(record.principal_reference.clone())
        .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
    let attempt_id = record
        .attempt_id
        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
    let worker_id = record
        .lease_owner
        .clone()
        .filter(|worker| validate_worker_id(worker).is_ok())
        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
    let expires_at = record
        .lease_expires_at
        .and_then(|value| OffsetDateTime::from_unix_timestamp(value).ok())
        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
    if record.lease_generation <= 0 || record.row_version < 0 || record.retry_count < 0 {
        return Err(OrmPublicError::new(OrmErrorCode::InternalError));
    }
    Ok(AiRunLease {
        run_id: AiRunId(record.id),
        session_id: AiSessionId(record.session_id),
        input_message_id: record.input_message_id,
        principal_reference,
        attempt_id,
        worker_id,
        lease_generation: record.lease_generation,
        lease_expires_at: expires_at,
        row_version: record.row_version,
        state: persisted_state(record)?,
        retry_count: u32::try_from(record.retry_count)
            .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?,
        latest_checkpoint_id: record.latest_checkpoint_id,
        recovery_epoch_id: record.recovery_epoch_id,
    })
}

fn approval_wait_proof_from_record(
    source: &AiRunLease,
    record: &AiRunRecord,
) -> Result<AiRunLease, OrmPublicError> {
    if record.id != source.run_id.0
        || record.session_id != source.session_id.0
        || record.input_message_id != source.input_message_id
        || record.attempt_id.is_some()
        || record.lease_owner.is_some()
        || record.lease_expires_at.is_some()
        || record.lease_heartbeat_at.is_some()
        || record.state != AiRunState::WaitingApproval.as_str()
        || record.latest_checkpoint_id.is_none()
        || record.lease_generation != source.lease_generation
    {
        return Err(OrmPublicError::new(OrmErrorCode::InternalError));
    }
    let mut proof = source.clone();
    proof.row_version = record.row_version;
    proof.state = AiRunState::WaitingApproval;
    proof.latest_checkpoint_id = record.latest_checkpoint_id;
    Ok(proof)
}

fn persisted_state(record: &AiRunRecord) -> Result<AiRunState, OrmPublicError> {
    AiRunState::from_persisted(&record.state)
        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))
}

fn reservation_usage_matches(
    reservation: &AiBudgetReservationRecord,
    usage: AiBudgetAmounts,
    cached_input_tokens: u64,
) -> Result<bool, OrmPublicError> {
    let as_i64 = |value: u64| {
        i64::try_from(value).map_err(|_| OrmPublicError::new(OrmErrorCode::InvalidInput))
    };
    Ok(
        reservation.actual_input_tokens == Some(as_i64(usage.input_tokens)?)
            && reservation.actual_cached_input_tokens == Some(as_i64(cached_input_tokens)?)
            && reservation.actual_output_tokens == Some(as_i64(usage.output_tokens)?)
            && reservation.actual_tool_units == Some(as_i64(usage.tool_units)?)
            && reservation.actual_image_units == Some(as_i64(usage.image_units)?)
            && reservation.actual_cost_microunits == Some(as_i64(usage.cost_microunits)?)
            && reservation.actual_runs == Some(as_i64(usage.runs)?),
    )
}

const TERMINAL_EVENT_METADATA_FORMAT: &str = "graphql-orm-ai-run-terminal-event-v1";
const TERMINAL_EVENT_METADATA_FORMAT_V2: &str = "graphql-orm-ai-run-terminal-event-v2";
pub(crate) const PROVIDER_SESSION_EVENT_METADATA_FORMAT: &str =
    "graphql-orm-ai-provider-session-event-v1";

/// Stable event type disclosing that a retained provider thread stopped being
/// usable, so the model's context was reset even though the durable transcript
/// is continuous.
pub(crate) const PROVIDER_SESSION_RESET_EVENT: &str = "provider_session_reset";
/// Stable event type disclosing that a new provider session binding replaced a
/// prior one for the same session.
pub(crate) const PROVIDER_SESSION_REBOUND_EVENT: &str = "provider_session_rebound";

/// Opens any content-free tagged event metadata without consulting a scope
/// content key.
///
/// Returns `Ok(None)` for an event whose payload is ordinary protected content,
/// which the caller must open through the configured protector.
pub(crate) fn open_metadata_only_event(
    event_type: &str,
    protected_payload: &serde_json::Value,
) -> Result<Option<serde_json::Value>, AiError> {
    if matches!(
        event_type,
        PROVIDER_SESSION_RESET_EVENT | PROVIDER_SESSION_REBOUND_EVENT
    ) {
        return open_provider_session_event_metadata(protected_payload);
    }
    open_terminal_event_metadata(event_type, protected_payload)
}

/// Opens the content-free provider-session lifecycle envelope.
///
/// The reason class is a server-owned safe code describing *why* a retained
/// thread was dropped. It never carries provider, prompt, tool, cursor, or
/// authorization content, so it needs no scope content key.
fn open_provider_session_event_metadata(
    protected_payload: &serde_json::Value,
) -> Result<Option<serde_json::Value>, AiError> {
    let Some(value) = protected_payload
        .as_object()
        .and_then(|envelope| envelope.get("value"))
        .and_then(serde_json::Value::as_object)
    else {
        return Ok(None);
    };
    if value.get("format").and_then(serde_json::Value::as_str)
        != Some(PROVIDER_SESSION_EVENT_METADATA_FORMAT)
    {
        return Ok(None);
    }
    if protected_payload
        .get("protection")
        .and_then(serde_json::Value::as_str)
        != Some("database_managed")
        || protected_payload
            .as_object()
            .is_none_or(|object| object.len() != 2)
        || value.len() != 2
        || !value
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .is_some_and(valid_safe_code)
    {
        return Err(AiError::PersistenceFailed);
    }
    Ok(Some(serde_json::Value::Object(value.clone())))
}

/// Builds the content-free provider-session lifecycle envelope.
pub(crate) fn provider_session_event_metadata(
    reason_code: &str,
) -> Result<serde_json::Value, OrmPublicError> {
    if !valid_safe_code(reason_code) {
        return Err(OrmPublicError::new(OrmErrorCode::InvalidInput));
    }
    serde_json::to_value(ProtectedContentEnvelope::DatabaseManaged {
        value: serde_json::json!({
            "format": PROVIDER_SESSION_EVENT_METADATA_FORMAT,
            "reason": reason_code,
        }),
    })
    .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))
}

/// Opens the deliberately content-free metadata envelope used by canonical
/// run terminal events without consulting a scope content key.
///
/// The run state and event type are already ordinary row metadata. The exact
/// tagged envelope adds no prompt, output, provider, tool, authorization, or
/// error content. All other event payloads continue through the configured
/// content-protection policy and protector.
pub(crate) fn open_terminal_event_metadata(
    event_type: &str,
    protected_payload: &serde_json::Value,
) -> Result<Option<serde_json::Value>, AiError> {
    let Some(value) = protected_payload
        .as_object()
        .and_then(|envelope| envelope.get("value"))
        .and_then(serde_json::Value::as_object)
    else {
        return Ok(None);
    };
    // Rows written before the failure record used the v1 shape and must stay
    // readable. Anything that is neither tagged shape falls through to the
    // configured content protector; anything that claims a tagged shape but
    // does not match it exactly fails closed.
    let expected_len = match value.get("format").and_then(serde_json::Value::as_str) {
        Some(TERMINAL_EVENT_METADATA_FORMAT) => 2,
        Some(TERMINAL_EVENT_METADATA_FORMAT_V2) => 3,
        _ => return Ok(None),
    };
    let terminal =
        AiRunTerminalEvent::from_event_type(event_type).ok_or(AiError::PersistenceFailed)?;
    if protected_payload
        .get("protection")
        .and_then(serde_json::Value::as_str)
        != Some("database_managed")
        || protected_payload
            .as_object()
            .is_none_or(|object| object.len() != 2)
        || value.len() != expected_len
        || value.get("state").and_then(serde_json::Value::as_str)
            != Some(terminal.run_state().as_str())
    {
        return Err(AiError::PersistenceFailed);
    }
    if expected_len == 3 && !valid_terminal_failure_record(value.get("failure")) {
        return Err(AiError::PersistenceFailed);
    }
    Ok(Some(serde_json::Value::Object(value.clone())))
}

/// Validates the bounded failure record without opening any protected content.
///
/// `null` is the successful-terminal case. Every other shape must be the exact
/// versioned record this crate writes.
fn valid_terminal_failure_record(failure: Option<&serde_json::Value>) -> bool {
    let Some(failure) = failure else {
        return false;
    };
    if failure.is_null() {
        return true;
    }
    let Some(record) = failure.as_object() else {
        return false;
    };
    record.len() == 5
        && record.get("version").and_then(serde_json::Value::as_u64)
            == Some(u64::from(crate::AI_RUN_FAILURE_VERSION))
        && record.get("ok") == Some(&serde_json::Value::Bool(false))
        && record
            .get("retryable")
            .is_some_and(serde_json::Value::is_boolean)
        && record
            .get("admission")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|admission| {
                matches!(
                    admission,
                    "allowed" | "refused_uncertain" | "refused_already_answered"
                )
            })
        && record
            .get("code")
            .is_some_and(|code| code.is_null() || code.as_str().is_some_and(valid_safe_code))
}

fn terminal_event_metadata(
    terminal: AiRunTerminalEvent,
    failure: Option<&AiRunFailure>,
) -> Result<serde_json::Value, OrmPublicError> {
    serde_json::to_value(ProtectedContentEnvelope::DatabaseManaged {
        value: serde_json::json!({
            "format": TERMINAL_EVENT_METADATA_FORMAT_V2,
            "state": terminal.run_state().as_str(),
            "failure": failure.map(AiRunFailure::to_json),
        }),
    })
    .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))
}

/// Appends the canonical session and owner-inbox terminal event in the
/// caller's exact run-transition transaction.
pub(crate) async fn append_terminal_run_event(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    run: &AiRunRecord,
    final_state: AiRunState,
    outcome_code: Option<&str>,
    now: OffsetDateTime,
) -> Result<(), OrmPublicError> {
    abandon_native_approval_candidates(tx, run.id, final_state, now.unix_timestamp()).await?;
    close_run_dependents(tx, run).await?;
    let terminal = AiRunTerminalEvent::from_run_state(final_state)
        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InvalidInput))?;
    let session = terminal_event_session(tx, run).await??;
    let sequence = session
        .stream_head
        .checked_add(1)
        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
    if !matches!(
        tx.compare_and_swap::<AiSessionRecord>(
            &session.id,
            session.row_version,
            AiSessionRecordWhereInput::default(),
            UpdateAiSessionRecordInput {
                stream_head: Some(sequence),
                last_activity_at: Some(now.unix_timestamp()),
                ..Default::default()
            },
        )
        .await
        .map_err(OrmPublicError::from)?,
        ConditionalUpdateOutcome::Updated(_)
    ) {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    let scope = AiScope {
        kind: session.scope_kind,
        id: session.scope_id,
        tenant_id: session.tenant_id,
    };
    let event_id = Uuid::new_v4();
    let inbox_event_id = Uuid::new_v4();
    // Retry admission is decided from committed rows only. The caller supplies
    // the outcome code from the completion it is committing, because `run` is
    // the pre-transition record and its `error_code` is still the previous
    // attempt's value.
    let failure = match terminal {
        AiRunTerminalEvent::Completed => None,
        AiRunTerminalEvent::Failed
        | AiRunTerminalEvent::Cancelled
        | AiRunTerminalEvent::RecoveryRequired => {
            let evidence = AiRunRetryEvidence {
                terminal,
                produced_assistant_output: run_produced_assistant_output(
                    tx,
                    run.session_id,
                    run.id,
                )
                .await?,
                provider_dispatch_possible: run_provider_dispatch_possible(tx, run.id).await?,
                consequential_tool_dispatch_possible: run_consequential_tool_dispatch_possible(
                    tx, run.id,
                )
                .await?,
            };
            Some(AiRunFailure::new(
                classify_run_retry(evidence, outcome_code),
                outcome_code.map(str::to_owned),
            ))
        }
    };
    let protected_event = terminal_event_metadata(terminal, failure.as_ref())?;
    let protected_inbox_event = terminal_event_metadata(terminal, failure.as_ref())?;
    tx.insert::<AiSessionEventRecord>(CreateAiSessionEventRecordInput {
        id: event_id,
        session_id: session.id,
        sequence,
        event_type: terminal.event_type().to_owned(),
        run_id: Some(run.id),
        causation_id: run.attempt_id.map(|attempt_id| attempt_id.to_string()),
        correlation_id: run.id.to_string(),
        protected_payload: protected_event,
    })
    .await
    .map_err(OrmPublicError::from)?;
    tx.queue_event(AiSessionWakeup {
        session_id: session.id,
        sequence,
    });
    append_inbox_event(
        tx,
        PreparedAiInboxEvent {
            id: inbox_event_id,
            principal_kind: session.owner_principal_kind,
            principal_subject: session.owner_subject,
            scope,
            session_id: session.id,
            event_type: terminal.event_type().to_owned(),
            protected_payload: protected_inbox_event,
            created_at: now.unix_timestamp(),
        },
    )
    .await?;
    Ok(())
}

pub(crate) async fn append_attempt_outcome(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    lease: &AiRunLease,
    final_state: AiRunState,
    outcome_code: String,
    provider_response_id: Option<String>,
    now: OffsetDateTime,
) -> Result<(), OrmPublicError> {
    tx.insert::<AiRunAttemptOutcomeRecord>(CreateAiRunAttemptOutcomeRecordInput {
        attempt_id: lease.attempt_id,
        run_id: lease.run_id.0,
        lease_generation: lease.lease_generation,
        worker_id: lease.worker_id.clone(),
        final_state: final_state.as_str().to_owned(),
        outcome_code,
        provider_response_id,
        finished_at: now.unix_timestamp(),
    })
    .await
    .map_err(OrmPublicError::from)?;
    Ok(())
}

pub(crate) fn validate_worker_id(worker_id: &str) -> Result<(), AiError> {
    if worker_id.trim().is_empty()
        || worker_id.len() > MAXIMUM_WORKER_ID_BYTES
        || worker_id.chars().any(char::is_control)
    {
        return Err(AiError::InvalidInput("invalid AI worker ID".to_owned()));
    }
    Ok(())
}

fn valid_provider_reference(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAXIMUM_PROVIDER_REFERENCE_BYTES
        && value.bytes().all(|byte| !byte.is_ascii_control())
}

fn valid_provider_kind(value: &str) -> bool {
    matches!(
        value,
        "openai"
            | "anthropic"
            | "xai"
            | "ollama"
            | "openai_compatible"
            | "local_harness"
            | "chat_completions"
    )
}

pub(crate) fn final_output_checkpoint_hash(
    run_id: AiRunId,
    attempt_id: Uuid,
    lease_generation: i64,
    message_id: Uuid,
    provider_response_id: Option<&str>,
    budget_reservation_id: Uuid,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(run_id.0.as_bytes());
    hasher.update([0]);
    hasher.update(attempt_id.as_bytes());
    hasher.update([0]);
    hasher.update(lease_generation.to_be_bytes());
    hasher.update([0]);
    hasher.update(message_id.as_bytes());
    hasher.update([0]);
    if let Some(response_id) = provider_response_id {
        hasher.update(response_id.as_bytes());
    }
    hasher.update([0]);
    hasher.update(budget_reservation_id.as_bytes());
    hex::encode(hasher.finalize())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn coordinator_checkpoint_hash(
    run_id: AiRunId,
    attempt_id: Uuid,
    lease_generation: i64,
    checkpoint_id: Uuid,
    kind: &str,
    provider_kind: &str,
    provider_model: &str,
    provider_response_id: Option<&str>,
    budget_reservation_id: Uuid,
    protected_state: &serde_json::Value,
) -> Result<String, AiError> {
    let provider_kind_hash_value = match provider_kind {
        "openai" => "open_ai",
        "openai_compatible" => "open_ai_compatible",
        "anthropic" | "xai" | "ollama" | "local_harness" | "chat_completions" => provider_kind,
        _ => return Err(AiError::PersistenceFailed),
    };
    let protected_state_hash = hex::encode(Sha256::digest(
        serde_json::to_vec(protected_state).map_err(|_| AiError::PersistenceFailed)?,
    ));
    let redacted = serde_json::json!({
        "checkpointId": checkpoint_id,
        "runId": run_id.0,
        "attemptId": attempt_id,
        "leaseGeneration": lease_generation,
        "kind": kind,
        // Preserve the Serde representation used by the original 0.6.0
        // checkpoint writer, while persistence/configuration use `as_str()`.
        "providerKind": provider_kind_hash_value,
        "providerModel": provider_model,
        "providerResponseId": provider_response_id,
        "budgetReservationId": budget_reservation_id,
        "protectedStateHash": protected_state_hash,
    });
    let encoded = serde_json::to_vec(&redacted).map_err(|_| AiError::PersistenceFailed)?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

fn valid_safe_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= MAXIMUM_SAFE_CODE_BYTES
        && code.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.' | b':')
        })
}

pub(crate) fn canonical_second(value: OffsetDateTime) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(value.unix_timestamp())
        .expect("an existing OffsetDateTime timestamp remains representable")
}

/// Runs one state-machine transaction, retrying the whole callback when the
/// database classifies the failure as retryable (SQLite busy, PostgreSQL
/// serialization failure or deadlock).
///
/// `attempt` must build a fresh transaction each time. Every callback in this
/// crate re-reads and re-validates its fence inside the transaction, so a
/// retry can never act on state observed by an aborted attempt.
pub(crate) async fn retry_transaction<T, F, Fut>(
    maximum_retries: usize,
    mut attempt: F,
) -> Result<T, TransactionError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, TransactionError>>,
{
    let mut retry = 0;
    loop {
        match attempt().await {
            Err(TransactionError::Retryable(_)) if retry < maximum_retries => {
                retry += 1;
                tokio::task::yield_now().await;
            }
            outcome => return outcome,
        }
    }
}

/// Serialization-retry bound for services without their own retry limit.
pub(crate) const DEFAULT_TRANSACTION_RETRIES: usize = 8;

/// Builds an `IntFilter` selecting timestamps at or before `timestamp`.
///
/// ORM integer filters carry `i32` values. After 2038 the predicate cannot be
/// expressed, so the filter is omitted and the caller's in-memory check
/// remains authoritative instead of the scan failing.
pub(crate) fn timestamp_at_or_before(timestamp: i64) -> Option<IntFilter> {
    i32::try_from(timestamp).ok().map(|timestamp| IntFilter {
        lte: Some(timestamp),
        ..Default::default()
    })
}

fn map_transaction(error: TransactionError) -> AiError {
    let public = error.public_error();
    if crate::orm_runtime_recovery::is_epoch_not_live(public) {
        return AiError::RuntimeNotReady;
    }
    match public.code {
        OrmErrorCode::InvalidInput
        | OrmErrorCode::CursorInvalid
        | OrmErrorCode::PageLimitExceeded => AiError::InvalidInput(public.message.clone()),
        OrmErrorCode::Unauthenticated | OrmErrorCode::Forbidden => AiError::Forbidden,
        OrmErrorCode::NotFound => AiError::NotFound,
        OrmErrorCode::Conflict | OrmErrorCode::ConstraintViolation => AiError::Conflict,
        OrmErrorCode::ServiceUnavailable
        | OrmErrorCode::InternalError
        | OrmErrorCode::AuthorizationMisconfigured => AiError::PersistenceFailed,
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;
    use agql_auth::{AccessTokenMetadata, AuthPrincipal, AuthUser, FixedClock, SessionContext};
    use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule};
    use graphql_orm::prelude::{Database, SqliteBackend};

    struct Fixture {
        service: OrmAiRunService,
        database: Database<SqliteBackend>,
        clock: FixedClock,
        principal_reference: PrincipalReference,
    }

    async fn fixture() -> Fixture {
        let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .expect("in-memory SQLite should open");
        let module = crate::AiSchemaModule;
        let plan = database
            .schema()
            .plan_migration_to_entities("ai-run-test-v1", "AI run service test", module.entities())
            .await
            .expect("AI schema should plan");
        database
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await
            .expect("AI schema should apply");
        crate::orm_runtime_recovery::open_test_epoch(&database).await;

        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)
            .expect("test timestamp should be valid");
        let clock = FixedClock::new(now);
        let principal = AuthPrincipal::User(AuthUser {
            user_id: "run-user".to_owned(),
            session_id: Uuid::new_v4(),
            roles: vec![],
            scopes: vec![],
            session: SessionContext::default(),
            token_claims: AccessTokenMetadata {
                tenant_id: Some("tenant-run".to_owned()),
                ..AccessTokenMetadata::default()
            },
        });
        let limits = AiRunServiceLimits::new(Duration::seconds(60), Duration::hours(1), 16, 2, 8)
            .expect("test run limits should validate");
        let service = OrmAiRunService::new(database.clone(), Arc::new(clock.clone()), limits);
        Fixture {
            service,
            database,
            clock,
            principal_reference: principal.reference(),
        }
    }

    async fn seed_queued(fixture: &Fixture) -> AiRunId {
        let run_id = AiRunId::new();
        let session_id = AiSessionId::new();
        AiSessionRecord::insert(
            &fixture.database,
            CreateAiSessionRecordInput {
                execution_selection: None,
                id: session_id.0,
                owner_principal_kind: "user".to_owned(),
                owner_subject: "run-user".to_owned(),
                tenant_id: Some("tenant-run".to_owned()),
                scope_kind: "tenant".to_owned(),
                scope_id: "tenant-run".to_owned(),
                title: "Run service test".to_owned(),
                title_revision: 0,
                title_source: "default".to_owned(),
                state: "active".to_owned(),
                stream_head: 0,
                message_head: 0,
                last_activity_at: fixture.clock.now().unix_timestamp(),
                archived_at: None,
                deleted_at: None,
            },
        )
        .await
        .expect("test session should insert");
        AiRunRecord::insert(
            &fixture.database,
            CreateAiRunRecordInput {
                recovery_epoch_id: None,
                execution_selection: None,
                id: run_id.0,
                session_id: session_id.0,
                input_message_id: Uuid::new_v4(),
                principal_reference: serde_json::to_value(&fixture.principal_reference)
                    .expect("principal reference should serialize"),
                state: AiRunState::Queued.as_str().to_owned(),
                attempt_id: None,
                lease_owner: None,
                lease_generation: 0,
                lease_expires_at: None,
                lease_heartbeat_at: None,
                retry_count: 0,
                next_attempt_at: Some(fixture.clock.now().unix_timestamp()),
                error_code: None,
                latest_checkpoint_id: None,
                cancellation_request_id: None,
                cancellation_requested_at: None,
                recovery_examined_at: 0,
            },
        )
        .await
        .expect("queued test run should insert");
        run_id
    }

    async fn run_record(fixture: &Fixture, run_id: AiRunId) -> AiRunRecord {
        AiRunRecord::find_by_id(&fixture.database, &run_id.0)
            .await
            .expect("run lookup should succeed")
            .expect("run should exist")
    }

    async fn attempt_outcomes(fixture: &Fixture) -> Vec<AiRunAttemptOutcomeRecord> {
        fixture
            .database
            .transaction(TransactionMode::Default, |tx| {
                Box::pin(async move {
                    tx.query::<AiRunAttemptOutcomeRecord>()
                        .limit(16)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
            .expect("attempt outcomes should query")
    }

    async fn session_events(fixture: &Fixture) -> Vec<AiSessionEventRecord> {
        fixture
            .database
            .transaction(TransactionMode::Default, |tx| {
                Box::pin(async move {
                    tx.query::<AiSessionEventRecord>()
                        .default_order()
                        .limit(32)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
            .expect("session events should query")
    }

    async fn inbox_events(fixture: &Fixture) -> Vec<AiInboxEventRecord> {
        fixture
            .database
            .transaction(TransactionMode::Default, |tx| {
                Box::pin(async move {
                    tx.query::<AiInboxEventRecord>()
                        .default_order()
                        .limit(32)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
            .expect("inbox events should query")
    }

    async fn provider_dispatch_possible(fixture: &Fixture, run_id: AiRunId) -> bool {
        fixture
            .database
            .transaction(TransactionMode::Default, move |tx| {
                Box::pin(async move { run_provider_dispatch_possible(tx, run_id.0).await })
            })
            .await
            .expect("provider-dispatch evidence should query")
    }

    #[tokio::test]
    async fn retry_evidence_treats_only_absent_or_released_reservations_as_no_dispatch() {
        let empty = fixture().await;
        let empty_run = seed_queued(&empty).await;
        assert!(!provider_dispatch_possible(&empty, empty_run).await);

        for (state, expected) in [
            ("released", false),
            ("expired", false),
            ("reserved", true),
            ("committed", true),
            ("uncertain", true),
            ("future_state", true),
        ] {
            let fixture = fixture().await;
            let run_id = seed_queued(&fixture).await;
            let run = run_record(&fixture, run_id).await;
            AiBudgetReservationRecord::insert(
                &fixture.database,
                CreateAiBudgetReservationRecordInput {
                    budget_counter_ids: serde_json::json!([]),
                    scope_kind: "tenant".to_owned(),
                    scope_id: "tenant-run".to_owned(),
                    tenant_id: Some("tenant-run".to_owned()),
                    principal_kind: "user".to_owned(),
                    principal_subject: "run-user".to_owned(),
                    session_id: run.session_id,
                    run_id: run.id,
                    attempt_id: Uuid::new_v4(),
                    lease_generation: 1,
                    provider_kind: "openai".to_owned(),
                    provider_model: "reviewed-model".to_owned(),
                    reasoning_effort: "unspecified".to_owned(),
                    pricing_policy_version: "pricing-v1".to_owned(),
                    reserved_input_tokens: 1,
                    reserved_output_tokens: 1,
                    reserved_tool_units: 0,
                    reserved_image_units: 0,
                    reserved_cost_microunits: 1,
                    reserved_runs: 1,
                    actual_input_tokens: None,
                    actual_cached_input_tokens: None,
                    actual_output_tokens: None,
                    actual_tool_units: None,
                    actual_image_units: None,
                    actual_cost_microunits: None,
                    actual_runs: None,
                    idempotency_key: format!("retry-evidence-{state}"),
                    state: state.to_owned(),
                    expires_at: (fixture.clock.now() + Duration::minutes(1)).unix_timestamp(),
                    reconciled_at: None,
                    settlement_examined_at: 0,
                },
            )
            .await
            .expect("test reservation should insert");
            assert_eq!(
                provider_dispatch_possible(&fixture, run_id).await,
                expected,
                "unexpected dispatch evidence for {state}",
            );
        }
    }

    #[tokio::test]
    async fn concurrent_workers_cannot_claim_the_same_run() {
        let fixture = fixture().await;
        let run_id = seed_queued(&fixture).await;

        let (first, second) = tokio::join!(
            fixture.service.claim_next("worker-a"),
            fixture.service.claim_next("worker-b"),
        );
        let first = first.expect("first claim should not fail");
        let second = second.expect("second claim should not fail");
        let claims: Vec<_> = [first, second].into_iter().flatten().collect();
        assert_eq!(claims.len(), 1, "only one worker may own the run");
        assert_eq!(claims[0].run_id(), run_id);
        assert_eq!(claims[0].lease_generation(), 1);

        let attempts = fixture
            .database
            .transaction(TransactionMode::Default, |tx| {
                Box::pin(async move {
                    tx.query::<AiRunAttemptRecord>()
                        .limit(4)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
            .expect("attempt history should query");
        assert_eq!(attempts.len(), 1, "one immutable claim fact is written");
        assert_eq!(attempts[0].id, claims[0].attempt_id());
    }

    #[tokio::test]
    async fn heartbeat_rotates_row_version_and_rejects_the_old_lease() {
        let fixture = fixture().await;
        seed_queued(&fixture).await;
        let old = fixture
            .service
            .claim_next("worker-heartbeat")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        fixture.clock.advance_seconds(10);
        let renewed = fixture
            .service
            .heartbeat(&old)
            .await
            .expect("current lease should renew");
        assert!(renewed.lease_expires_at() > old.lease_expires_at());
        assert!(matches!(
            fixture.service.start(&old).await,
            Err(AiError::Conflict)
        ));
        let running = fixture
            .service
            .start(&renewed)
            .await
            .expect("renewed lease should start");
        assert_eq!(running.state(), AiRunState::Running);
    }

    #[tokio::test]
    async fn recovery_requeues_only_pre_provider_claims_and_fences_old_workers() {
        let fixture = fixture().await;
        let safe_run_id = seed_queued(&fixture).await;
        let safe_lease = fixture
            .service
            .claim_next("worker-safe")
            .await
            .expect("safe claim should succeed")
            .expect("safe run should be eligible");
        let uncertain_run_id = seed_queued(&fixture).await;
        let uncertain_lease = fixture
            .service
            .claim_next("worker-uncertain")
            .await
            .expect("uncertain claim should succeed")
            .expect("uncertain run should be eligible");
        let uncertain_lease = fixture
            .service
            .start(&uncertain_lease)
            .await
            .expect("uncertain claim should start");

        fixture.clock.advance_seconds(61);
        let report = fixture
            .service
            .recover_expired_leases()
            .await
            .expect("expired leases should reconcile");
        assert_eq!(report.requeued, 1);
        assert_eq!(report.recovery_required, 1);
        assert_eq!(report.failed, 0);
        assert_eq!(report.completed, 0);
        assert_eq!(
            run_record(&fixture, safe_run_id).await.state,
            AiRunState::RetryScheduled.as_str()
        );
        assert_eq!(
            run_record(&fixture, uncertain_run_id).await.state,
            AiRunState::RecoveryRequired.as_str()
        );
        assert!(matches!(
            fixture.service.heartbeat(&safe_lease).await,
            Err(AiError::Conflict)
        ));
        assert!(matches!(
            fixture.service.heartbeat(&uncertain_lease).await,
            Err(AiError::Conflict)
        ));

        let reclaimed = fixture
            .service
            .claim_next("worker-replacement")
            .await
            .expect("reclaim should succeed")
            .expect("safe retry should be eligible");
        assert_eq!(reclaimed.run_id(), safe_run_id);
        assert_eq!(reclaimed.lease_generation(), 2);
        let outcomes = attempt_outcomes(&fixture).await;
        assert_eq!(outcomes.len(), 2);
        assert!(
            outcomes
                .iter()
                .any(|outcome| outcome.outcome_code == "lease_expired_before_start")
        );
        assert!(
            outcomes
                .iter()
                .any(|outcome| outcome.outcome_code == "lease_expired_after_start")
        );
    }

    #[tokio::test]
    async fn recovery_finalizes_an_exact_persisted_output_checkpoint() {
        let fixture = fixture().await;
        let run_id = seed_queued(&fixture).await;
        let lease = fixture
            .service
            .claim_next("worker-output-checkpoint")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        let lease = fixture
            .service
            .start(&lease)
            .await
            .expect("claim should start");
        let message_id = Uuid::new_v4();
        let provider_response_id = "response-checkpoint";
        let budget_reservation = AiBudgetReservationRecord::insert(
            &fixture.database,
            CreateAiBudgetReservationRecordInput {
                budget_counter_ids: serde_json::json!([]),
                scope_kind: "tenant".to_owned(),
                scope_id: "tenant-run".to_owned(),
                tenant_id: Some("tenant-run".to_owned()),
                principal_kind: "user".to_owned(),
                principal_subject: "run-user".to_owned(),
                session_id: lease.session_id().0,
                run_id: lease.run_id().0,
                attempt_id: lease.attempt_id(),
                lease_generation: lease.lease_generation(),
                provider_kind: "mock".to_owned(),
                provider_model: "checkpoint-test".to_owned(),
                reasoning_effort: "unspecified".to_owned(),
                pricing_policy_version: "checkpoint-pricing-v1".to_owned(),
                reserved_input_tokens: 1,
                reserved_output_tokens: 1,
                reserved_tool_units: 0,
                reserved_image_units: 0,
                reserved_cost_microunits: 1,
                reserved_runs: 1,
                actual_input_tokens: Some(1),
                actual_cached_input_tokens: Some(0),
                actual_output_tokens: Some(1),
                actual_tool_units: Some(0),
                actual_image_units: Some(0),
                actual_cost_microunits: Some(1),
                actual_runs: Some(1),
                idempotency_key: "output-checkpoint-test".to_owned(),
                state: "committed".to_owned(),
                expires_at: (fixture.clock.now() + Duration::minutes(5)).unix_timestamp(),
                reconciled_at: Some(fixture.clock.now().unix_timestamp()),
                settlement_examined_at: 0,
            },
        )
        .await
        .expect("committed test budget should insert");
        let budget_reservation_id = budget_reservation.id;
        let checkpoint_hash = final_output_checkpoint_hash(
            lease.run_id(),
            lease.attempt_id(),
            lease.lease_generation(),
            message_id,
            Some(provider_response_id),
            budget_reservation_id,
        );
        fixture
            .service
            .append_provider_output(
                &lease,
                PreparedProviderOutput {
                    message_id,
                    event_id: Uuid::new_v4(),
                    inbox_event_id: Uuid::new_v4(),
                    provider_kind: "mock".to_owned(),
                    provider_model: "checkpoint-test".to_owned(),
                    protected_preview: serde_json::json!({"protected": true}),
                    protected_event: serde_json::json!({"protected": true}),
                    protected_inbox_event: serde_json::json!({"protected": true}),
                    blocks: vec![PreparedProviderBlock {
                        id: Uuid::new_v4(),
                        kind: "text".to_owned(),
                        protected_content: serde_json::json!({"protected": true}),
                        byte_count: 4,
                        line_count: 1,
                    }],
                    correlation_id: budget_reservation_id.to_string(),
                    provider_response_id: Some(provider_response_id.to_owned()),
                    budget_reservation_id,
                    checkpoint_hash,
                    expected_owner_principal_kind: "user".to_owned(),
                    expected_owner_subject: "run-user".to_owned(),
                    expected_scope_kind: "tenant".to_owned(),
                    expected_scope_id: "tenant-run".to_owned(),
                    expected_tenant_id: Some("tenant-run".to_owned()),
                },
            )
            .await
            .expect("output and checkpoint should commit atomically");

        fixture.clock.advance_seconds(61);
        let report = fixture
            .service
            .recover_expired_leases()
            .await
            .expect("exact output checkpoint should reconcile");

        assert_eq!(report.completed, 1);
        assert_eq!(report.recovery_required, 0);
        assert_eq!(run_record(&fixture, run_id).await.state, "completed");
        let outcomes = attempt_outcomes(&fixture).await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0].outcome_code,
            "lease_expired_after_output_persisted"
        );
        assert_eq!(
            outcomes[0].provider_response_id.as_deref(),
            Some(provider_response_id)
        );
    }

    async fn assert_recovery_requeues_completed_tool_batch(provider_response_id: Option<&str>) {
        let fixture = fixture().await;
        let (run_id, checkpoint_id) =
            seed_completed_tool_batch(&fixture, provider_response_id).await;

        fixture.clock.advance_seconds(61);
        let report = fixture
            .service
            .recover_expired_leases()
            .await
            .expect("exact tool checkpoint should requeue for adoption");
        assert_eq!(report.checkpoint_requeued, 1);
        assert_eq!(report.recovery_required, 0);
        let retry = run_record(&fixture, run_id).await;
        assert_eq!(retry.state, AiRunState::RetryScheduled.as_str());
        assert_eq!(retry.latest_checkpoint_id, Some(checkpoint_id));
        assert_eq!(
            retry.error_code.as_deref(),
            Some("checkpoint_adoption_ready")
        );

        let replacement = fixture
            .service
            .claim_next("worker-tool-adopter")
            .await
            .expect("replacement claim should succeed")
            .expect("checkpoint retry should be eligible");
        assert_eq!(replacement.lease_generation(), 2);
        assert_eq!(replacement.latest_checkpoint_id(), Some(checkpoint_id));
    }

    /// Seeds a started run whose current attempt committed an exact completed
    /// read-only tool-batch checkpoint, and returns its run and checkpoint IDs.
    async fn seed_completed_tool_batch(
        fixture: &Fixture,
        provider_response_id: Option<&str>,
    ) -> (AiRunId, Uuid) {
        let run_id = seed_queued(fixture).await;
        let lease = fixture
            .service
            .claim_next("worker-tool-checkpoint")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        let lease = fixture
            .service
            .start(&lease)
            .await
            .expect("claim should start");
        let reservation = AiBudgetReservationRecord::insert(
            &fixture.database,
            CreateAiBudgetReservationRecordInput {
                budget_counter_ids: serde_json::json!([]),
                scope_kind: "tenant".to_owned(),
                scope_id: "tenant-run".to_owned(),
                tenant_id: Some("tenant-run".to_owned()),
                principal_kind: "user".to_owned(),
                principal_subject: "run-user".to_owned(),
                session_id: lease.session_id().0,
                run_id: lease.run_id().0,
                attempt_id: lease.attempt_id(),
                lease_generation: lease.lease_generation(),
                provider_kind: "openai".to_owned(),
                provider_model: "checkpoint-test".to_owned(),
                reasoning_effort: "unspecified".to_owned(),
                pricing_policy_version: "checkpoint-pricing-v1".to_owned(),
                reserved_input_tokens: 1,
                reserved_output_tokens: 1,
                reserved_tool_units: 1,
                reserved_image_units: 0,
                reserved_cost_microunits: 1,
                reserved_runs: 1,
                actual_input_tokens: Some(1),
                actual_cached_input_tokens: Some(0),
                actual_output_tokens: Some(1),
                actual_tool_units: Some(1),
                actual_image_units: Some(0),
                actual_cost_microunits: Some(1),
                actual_runs: Some(1),
                idempotency_key: "tool-checkpoint-test".to_owned(),
                state: "committed".to_owned(),
                expires_at: (fixture.clock.now() + Duration::minutes(5)).unix_timestamp(),
                reconciled_at: Some(fixture.clock.now().unix_timestamp()),
                settlement_examined_at: 0,
            },
        )
        .await
        .expect("committed test budget should insert");
        let tool_call_id = Uuid::new_v4();
        AiRunStepRecord::insert(
            &fixture.database,
            CreateAiRunStepRecordInput {
                id: tool_call_id,
                run_id: lease.run_id().0,
                step_index: 0,
                step_kind: "application_tool".to_owned(),
                state: "completed".to_owned(),
                lease_generation: lease.lease_generation(),
                started_at: Some(fixture.clock.now().unix_timestamp()),
                finished_at: Some(fixture.clock.now().unix_timestamp()),
                error_code: None,
            },
        )
        .await
        .expect("completed tool step should insert");
        AiToolCallRecord::insert(
            &fixture.database,
            CreateAiToolCallRecordInput {
                id: tool_call_id,
                run_id: lease.run_id().0,
                provider_call_key: "tool-checkpoint-call-key".to_owned(),
                provider_call_id: "call-tool-checkpoint".to_owned(),
                provider_kind: Some("openai".to_owned()),
                provider_model: Some("checkpoint-test".to_owned()),
                provider_response_id: provider_response_id.map(str::to_owned),
                budget_reservation_id: Some(reservation.id),
                provider_turn_index: 0,
                tool_call_index: 0,
                tool_id: "records.read".to_owned(),
                tool_fingerprint: "tool-fingerprint".to_owned(),
                execution_provenance: None,
                protected_arguments: Some(serde_json::json!({"protected": true})),
                argument_hash: "argument-hash".to_owned(),
                protected_result: Some(serde_json::json!({"protected": true})),
                protected_result_source: None,
                payload_purged_at: None,
                risk: "read_only".to_owned(),
                authorization_code: Some("allowed".to_owned()),
                authorization_policy_version: Some("tool-policy-v1".to_owned()),
                authorization_state_digest: Some("authorization-state".to_owned()),
                disclosure_schema_fingerprint: Some("disclosure-v1".to_owned()),
                result_classification: Some("internal".to_owned()),
                result_egress_decision_id: Some(Uuid::new_v4()),
                result_egress_manifest_hash: Some("manifest-hash".to_owned()),
                application_audit_ref: Some("application-audit".to_owned()),
                approval_id: None,
                idempotency_key: None,
                correlation_id: Some("tool-checkpoint-correlation".to_owned()),
                causation_id: Some(lease.input_message_id().to_string()),
                delegation_reference: None,
                lease_generation: lease.lease_generation(),
                state: "completed".to_owned(),
                completed_at: Some(fixture.clock.now().unix_timestamp()),
            },
        )
        .await
        .expect("completed tool call should insert");
        let checkpoint_id = Uuid::new_v4();
        let protected_state = serde_json::json!({
            "protection": "database_managed",
            "value": {"bounded": true},
        });
        let checkpoint_hash = coordinator_checkpoint_hash(
            lease.run_id(),
            lease.attempt_id(),
            lease.lease_generation(),
            checkpoint_id,
            "tool_batch_persisted",
            "openai",
            "checkpoint-test",
            provider_response_id,
            reservation.id,
            &protected_state,
        )
        .expect("checkpoint hash should encode");
        let checkpointed = fixture
            .service
            .append_coordinator_checkpoint(
                &lease,
                PreparedCoordinatorCheckpoint {
                    id: checkpoint_id,
                    checkpoint_kind: "tool_batch_persisted".to_owned(),
                    provider_kind: "openai".to_owned(),
                    provider_model: "checkpoint-test".to_owned(),
                    provider_response_id: provider_response_id.map(str::to_owned),
                    budget_reservation_id: reservation.id,
                    protected_state,
                    checkpoint_hash,
                    native_binding: None,
                    completed_tools: vec![PreparedCoordinatorCheckpointTool {
                        id: tool_call_id,
                        provider_call_id: "call-tool-checkpoint".to_owned(),
                        tool_id: "records.read".to_owned(),
                        result_egress_manifest_hash: "manifest-hash".to_owned(),
                    }],
                },
            )
            .await
            .expect("tool batch checkpoint should commit");
        assert_eq!(checkpointed.latest_checkpoint_id(), Some(checkpoint_id));
        (run_id, checkpoint_id)
    }

    /// Inserts a queued run with a chosen ID into a fresh session.
    async fn seed_queued_with_id(fixture: &Fixture, run_id: Uuid) -> AiRunId {
        let session_id = AiSessionId::new();
        AiSessionRecord::insert(
            &fixture.database,
            CreateAiSessionRecordInput {
                execution_selection: None,
                id: session_id.0,
                owner_principal_kind: "user".to_owned(),
                owner_subject: "run-user".to_owned(),
                tenant_id: Some("tenant-run".to_owned()),
                scope_kind: "tenant".to_owned(),
                scope_id: "tenant-run".to_owned(),
                title: "Run service test".to_owned(),
                title_revision: 0,
                title_source: "default".to_owned(),
                state: "active".to_owned(),
                stream_head: 0,
                message_head: 0,
                last_activity_at: fixture.clock.now().unix_timestamp(),
                archived_at: None,
                deleted_at: None,
            },
        )
        .await
        .expect("test session should insert");
        AiRunRecord::insert(
            &fixture.database,
            CreateAiRunRecordInput {
                recovery_epoch_id: None,
                execution_selection: None,
                id: run_id,
                session_id: session_id.0,
                input_message_id: Uuid::new_v4(),
                principal_reference: serde_json::to_value(&fixture.principal_reference)
                    .expect("principal reference should serialize"),
                state: AiRunState::Queued.as_str().to_owned(),
                attempt_id: None,
                lease_owner: None,
                lease_generation: 0,
                lease_expires_at: None,
                lease_heartbeat_at: None,
                retry_count: 0,
                next_attempt_at: Some(fixture.clock.now().unix_timestamp()),
                error_code: None,
                latest_checkpoint_id: None,
                cancellation_request_id: None,
                cancellation_requested_at: None,
                recovery_examined_at: 0,
            },
        )
        .await
        .expect("queued test run should insert");
        AiRunId(run_id)
    }

    #[tokio::test]
    async fn second_lease_expiry_before_adoption_requeues_again_without_aborting_the_pass() {
        for start_replacement in [false, true] {
            let fixture = fixture().await;
            let (run_id, checkpoint_id) =
                seed_completed_tool_batch(&fixture, Some("response-adoption-retry")).await;
            fixture.clock.advance_seconds(61);
            let first = fixture
                .service
                .recover_expired_leases()
                .await
                .expect("first recovery should requeue for adoption");
            assert_eq!(first.checkpoint_requeued, 1);
            let replacement = fixture
                .service
                .claim_next("worker-adopter")
                .await
                .expect("adoption claim should succeed")
                .expect("adoption retry should be eligible");
            assert_eq!(replacement.latest_checkpoint_id(), Some(checkpoint_id));
            if start_replacement {
                fixture
                    .service
                    .start(&replacement)
                    .await
                    .expect("adoption attempt should start");
            }
            // An unrelated run whose lease expires in the same pass.
            let other_run = seed_queued(&fixture).await;
            fixture
                .service
                .claim_next("worker-other")
                .await
                .expect("unrelated claim should succeed")
                .expect("unrelated run should be eligible");

            // The adopting attempt dies before consuming the checkpoint.
            fixture.clock.advance_seconds(61);
            let second = fixture
                .service
                .recover_expired_leases()
                .await
                .expect("a pre-adoption expiry must not abort the recovery pass");
            assert_eq!(second.checkpoint_requeued, 1, "start={start_replacement}");
            assert_eq!(second.requeued, 1, "start={start_replacement}");
            assert_eq!(second.quarantined, 0, "start={start_replacement}");
            let retry = run_record(&fixture, run_id).await;
            assert_eq!(retry.state, AiRunState::RetryScheduled.as_str());
            assert_eq!(retry.latest_checkpoint_id, Some(checkpoint_id));
            assert_eq!(
                retry.error_code.as_deref(),
                Some("checkpoint_adoption_ready")
            );
            assert_eq!(
                run_record(&fixture, other_run).await.state,
                AiRunState::RetryScheduled.as_str()
            );
        }
    }

    #[tokio::test]
    async fn recovery_quarantines_a_malformed_row_and_still_recovers_the_rest() {
        let fixture = fixture().await;
        let broken = seed_queued(&fixture).await;
        let broken_lease = fixture
            .service
            .claim_next("worker-broken")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        fixture
            .service
            .start(&broken_lease)
            .await
            .expect("broken run should start");
        let healthy = seed_queued(&fixture).await;
        fixture
            .service
            .claim_next("worker-healthy")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        // Corrupt the running row: its checkpoint pointer names no checkpoint.
        let current = run_record(&fixture, broken).await;
        let outcome = fixture
            .database
            .transaction(TransactionMode::Default, move |tx| {
                Box::pin(async move {
                    tx.compare_and_swap::<AiRunRecord>(
                        &current.id,
                        current.row_version,
                        AiRunRecordWhereInput::default(),
                        UpdateAiRunRecordInput {
                            latest_checkpoint_id: Some(Some(Uuid::new_v4())),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(OrmPublicError::from)
                })
            })
            .await
            .expect("test corruption should commit");
        assert!(matches!(outcome, ConditionalUpdateOutcome::Updated(_)));

        fixture.clock.advance_seconds(61);
        let report = fixture
            .service
            .recover_expired_leases()
            .await
            .expect("one malformed row must not abort the recovery pass");
        assert_eq!(report.requeued, 1);
        assert_eq!(report.quarantined, 1);
        assert_eq!(
            run_record(&fixture, healthy).await.state,
            AiRunState::RetryScheduled.as_str()
        );
        let quarantined = run_record(&fixture, broken).await;
        assert_eq!(quarantined.state, AiRunState::RecoveryRequired.as_str());
        assert_eq!(
            quarantined.error_code.as_deref(),
            Some("lease_recovery_state_invalid")
        );
        assert!(quarantined.lease_owner.is_none());
        assert!(quarantined.lease_expires_at.is_none());
        // The quarantined row leaves the scan, so the next pass is clean.
        let next = fixture
            .service
            .recover_expired_leases()
            .await
            .expect("the next pass should succeed");
        assert_eq!(next, AiRunRecoveryReport::default());
    }

    #[tokio::test]
    async fn a_malformed_queued_row_cannot_block_every_claim() {
        let fixture = fixture().await;
        // The malformed row sorts first by primary key and creation time.
        let broken = seed_queued_with_id(&fixture, Uuid::from_u128(1)).await;
        let current = run_record(&fixture, broken).await;
        fixture
            .database
            .transaction(TransactionMode::Default, move |tx| {
                Box::pin(async move {
                    tx.compare_and_swap::<AiRunRecord>(
                        &current.id,
                        current.row_version,
                        AiRunRecordWhereInput::default(),
                        UpdateAiRunRecordInput {
                            lease_owner: Some(Some("ghost-worker".to_owned())),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(OrmPublicError::from)
                })
            })
            .await
            .expect("test corruption should commit");
        let healthy = seed_queued_with_id(&fixture, Uuid::from_u128(2)).await;
        let lease = fixture
            .service
            .claim_next("worker-claim")
            .await
            .expect("one malformed row must not fail every claim")
            .expect("the healthy run should be claimed");
        assert_eq!(lease.run_id(), healthy);
        let quarantined = run_record(&fixture, broken).await;
        assert_eq!(quarantined.state, AiRunState::RecoveryRequired.as_str());
        assert_eq!(
            quarantined.error_code.as_deref(),
            Some("claim_state_invalid")
        );
    }

    async fn insert_tool_call(
        fixture: &Fixture,
        lease: &AiRunLease,
        risk: &str,
        state: &str,
    ) -> Uuid {
        let id = Uuid::new_v4();
        AiToolCallRecord::insert(
            &fixture.database,
            CreateAiToolCallRecordInput {
                id,
                run_id: lease.run_id().0,
                provider_call_key: format!("call-key-{id}"),
                provider_call_id: format!("call-{id}"),
                provider_kind: Some("openai".to_owned()),
                provider_model: Some("retry-evidence".to_owned()),
                provider_response_id: Some("response-retry-evidence".to_owned()),
                budget_reservation_id: None,
                provider_turn_index: 0,
                tool_call_index: 0,
                tool_id: "records.update".to_owned(),
                tool_fingerprint: "tool-fingerprint".to_owned(),
                execution_provenance: None,
                protected_arguments: None,
                argument_hash: "argument-hash".to_owned(),
                protected_result: None,
                protected_result_source: None,
                payload_purged_at: None,
                risk: risk.to_owned(),
                authorization_code: Some("allowed".to_owned()),
                authorization_policy_version: None,
                authorization_state_digest: None,
                disclosure_schema_fingerprint: None,
                result_classification: None,
                result_egress_decision_id: None,
                result_egress_manifest_hash: None,
                application_audit_ref: None,
                approval_id: None,
                idempotency_key: None,
                correlation_id: None,
                causation_id: None,
                delegation_reference: None,
                lease_generation: lease.lease_generation(),
                state: state.to_owned(),
                completed_at: None,
            },
        )
        .await
        .expect("tool call should insert");
        id
    }

    #[tokio::test]
    async fn a_cancelled_run_whose_mutation_may_have_run_is_not_retryable() {
        for (risk, state, admission) in [
            ("read_only", "completed", "allowed"),
            ("non_idempotent_write", "waiting_approval", "allowed"),
            ("non_idempotent_write", "completed", "refused_uncertain"),
            ("low_risk_write", "executing", "refused_uncertain"),
            // Cancellation of an executing call records `cancelled` with no
            // consumed approval proving it never reached the resolver.
            ("high_impact", "cancelled", "refused_uncertain"),
        ] {
            let fixture = fixture().await;
            seed_queued(&fixture).await;
            let lease = fixture
                .service
                .claim_next("worker-cancel-evidence")
                .await
                .expect("claim should succeed")
                .expect("run should be eligible");
            let lease = fixture
                .service
                .start(&lease)
                .await
                .expect("run should start");
            insert_tool_call(&fixture, &lease, risk, state).await;
            fixture
                .service
                .finish(
                    &lease,
                    AiRunCompletion::new(
                        AiRunState::Cancelled,
                        "owner_cancelled",
                        Some("owner_cancelled".to_owned()),
                        None,
                    )
                    .expect("cancellation should validate"),
                )
                .await
                .expect("cancellation should commit");
            let events = session_events(&fixture).await;
            assert_eq!(events.len(), 1);
            assert_eq!(
                events[0].protected_payload["value"]["failure"]["admission"],
                serde_json::json!(admission),
                "risk={risk} state={state}"
            );
        }
    }

    #[tokio::test]
    async fn tool_execution_renews_the_lease_until_it_finishes() {
        let fixture = fixture().await;
        seed_queued(&fixture).await;
        // A three-second lease renews every second.
        let service = OrmAiRunService::new(
            fixture.database.clone(),
            Arc::new(fixture.clock.clone()),
            AiRunServiceLimits::new(Duration::seconds(3), Duration::hours(1), 16, 2, 8)
                .expect("short lease limits should validate"),
        );
        let lease = service
            .claim_next("worker-tool-heartbeat")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        let lease = service.start(&lease).await.expect("run should start");
        let clock = fixture.clock.clone();
        // A resolver that outlasts several heartbeat intervals. The clock
        // moves once, so any renewal extends the lease past its original
        // expiry; no timing race with the simulated clock is involved.
        let execution = async move {
            clock.advance_seconds(1);
            tokio::time::sleep(std::time::Duration::from_millis(4_500)).await;
            "done"
        };
        let (output, renewed) = service.run_with_lease_heartbeat(&lease, execution).await;
        assert_eq!(output, "done");
        assert!(renewed.lease_expires_at() > lease.lease_expires_at());
        // The renewal rotated the fence, so the pre-tool proof is stale.
        assert!(matches!(
            service.heartbeat(&lease).await,
            Err(AiError::Conflict)
        ));
        service
            .heartbeat(&renewed)
            .await
            .expect("the renewed proof stays current");
    }

    #[tokio::test]
    async fn lease_recovery_grace_tolerates_a_fast_recovery_clock() {
        let fixture = fixture().await;
        let run_id = seed_queued(&fixture).await;
        fixture
            .service
            .claim_next("worker-grace")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        let limits = fixture
            .service
            .limits
            .with_lease_recovery_grace(Duration::seconds(30))
            .expect("grace within the lease TTL should validate");
        assert!(
            fixture
                .service
                .limits
                .with_lease_recovery_grace(Duration::seconds(61))
                .is_err()
        );
        let recovering = OrmAiRunService::new(
            fixture.database.clone(),
            Arc::new(fixture.clock.clone()),
            limits,
        );
        // The recovering process's clock runs 20 seconds past the lease.
        fixture.clock.advance_seconds(80);
        assert_eq!(
            recovering
                .recover_expired_leases()
                .await
                .expect("recovery should succeed")
                .requeued,
            0
        );
        assert_eq!(
            run_record(&fixture, run_id).await.state,
            AiRunState::Leased.as_str()
        );
        fixture.clock.advance_seconds(11);
        assert_eq!(
            recovering
                .recover_expired_leases()
                .await
                .expect("recovery should succeed")
                .requeued,
            1
        );
    }

    #[tokio::test]
    async fn a_retry_after_a_lost_commit_acknowledgement_does_not_repeat_the_transition() {
        let fixture = fixture().await;
        let run_id = seed_queued(&fixture).await;
        let lease = fixture
            .service
            .claim_next("worker-lost-ack")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        let lease = fixture
            .service
            .start(&lease)
            .await
            .expect("run should start");
        let completion = AiRunCompletion::new(
            AiRunState::Failed,
            "provider_failed",
            Some("provider_failed".to_owned()),
            None,
        )
        .expect("completion should validate");
        let now = canonical_second(fixture.clock.now());
        let mut attempts = 0;
        let result = retry_transaction(2, || {
            attempts += 1;
            let first = attempts == 1;
            let attempt = fixture
                .service
                .finish_once(lease.clone(), completion.clone(), now);
            async move {
                let outcome = attempt.await;
                if first {
                    // The first attempt committed, but its acknowledgement
                    // was lost and surfaced as a retryable failure.
                    outcome?;
                    return Err(TransactionError::Retryable(OrmPublicError::new(
                        OrmErrorCode::ServiceUnavailable,
                    )));
                }
                outcome
            }
        })
        .await;
        assert_eq!(attempts, 2, "the lost acknowledgement must be retried");
        assert!(
            matches!(result.map_err(map_transaction), Err(AiError::Conflict)),
            "the retried transition must be refused by its fence"
        );
        assert_eq!(
            run_record(&fixture, run_id).await.state,
            AiRunState::Failed.as_str()
        );
        assert_eq!(attempt_outcomes(&fixture).await.len(), 1);
        assert_eq!(session_events(&fixture).await.len(), 1);
        assert_eq!(inbox_events(&fixture).await.len(), 1);
    }

    #[tokio::test]
    async fn runs_whose_session_is_gone_are_quarantined_without_holding_the_window() {
        let fixture = fixture().await;
        let scan = fixture.service.limits.maximum_candidate_scan();
        // A full window of the oldest expired rows whose recovery always
        // fails: their sessions are gone, so the terminal event cannot write.
        let mut broken = Vec::new();
        for index in 0..scan {
            let run = seed_queued_with_id(&fixture, Uuid::from_u128(index as u128 + 1)).await;
            let lease = fixture
                .service
                .claim_next("worker-broken")
                .await
                .expect("claim should succeed")
                .expect("run should be eligible");
            fixture
                .service
                .start(&lease)
                .await
                .expect("run should start");
            broken.push(run);
        }
        let healthy = seed_queued_with_id(&fixture, Uuid::from_u128(u128::MAX)).await;
        let lease = fixture
            .service
            .claim_next("worker-healthy")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        fixture
            .service
            .start(&lease)
            .await
            .expect("run should start");
        for run in &broken {
            let session = run_record(&fixture, *run).await.session_id;
            AiSessionRecord::delete_by_id(&fixture.database, &session)
                .await
                .expect("test session delete should succeed");
        }
        fixture.clock.advance_seconds(61);
        let first = fixture
            .service
            .recover_expired_leases()
            .await
            .expect("failing rows must not fail the pass");
        // A missing session is a durable defect of the run's own linkage, so
        // they are closed for manual recovery instead of holding the window.
        assert_eq!(first.quarantined as usize, scan, "{first:?}");
        assert_eq!(first.skipped, 0);
        for run in &broken {
            let record = run_record(&fixture, *run).await;
            assert_eq!(record.state, AiRunState::RecoveryRequired.as_str());
            assert_eq!(
                record.error_code.as_deref(),
                Some("lease_recovery_state_invalid")
            );
        }
        // A fresh service, as after a restart, carries no scan state.
        let restarted = OrmAiRunService::new(
            fixture.database.clone(),
            Arc::new(fixture.clock.clone()),
            fixture.service.limits,
        );
        let second = restarted
            .recover_expired_leases()
            .await
            .expect("the next pass should reach the healthy row");
        assert_eq!(second.recovery_required, 1, "{second:?}");
        assert_eq!(
            run_record(&fixture, healthy).await.state,
            AiRunState::RecoveryRequired.as_str()
        );
    }

    /// A host entity policy that refuses session-event writes while `refuse`
    /// is set, as a host callback might during an outage.
    #[derive(Clone)]
    struct RefuseSessionEvents {
        refuse: Arc<std::sync::atomic::AtomicBool>,
    }

    impl graphql_orm::graphql::orm::EntityPolicy<DefaultWriteBackend> for RefuseSessionEvents {
        fn can_access_entity<'a>(
            &'a self,
            _context: Option<&'a async_graphql::Context<'_>>,
            _database: &'a Database<DefaultWriteBackend>,
            entity_name: &'static str,
            _policy_key: Option<&'static str>,
            kind: graphql_orm::graphql::orm::EntityAccessKind,
            surface: graphql_orm::graphql::orm::EntityAccessSurface,
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            let refused = entity_name == "AiSessionEventRecord"
                && kind == graphql_orm::graphql::orm::EntityAccessKind::Write
                && self.refuse.load(std::sync::atomic::Ordering::SeqCst);
            let allowed =
                surface == graphql_orm::graphql::orm::EntityAccessSurface::Repository && !refused;
            Box::pin(async move { Ok(allowed) })
        }
    }

    fn service_on(
        fixture: &Fixture,
        database: Database<SqliteBackend>,
        scan: usize,
    ) -> OrmAiRunService {
        OrmAiRunService::new(
            database,
            Arc::new(fixture.clock.clone()),
            AiRunServiceLimits::new(Duration::seconds(60), Duration::hours(1), scan, 2, 8)
                .expect("run limits should validate"),
        )
    }

    async fn started_run(fixture: &Fixture, id: Uuid) -> AiRunId {
        let run = seed_queued_with_id(fixture, id).await;
        let lease = fixture
            .service
            .claim_next("worker-policy")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        fixture
            .service
            .start(&lease)
            .await
            .expect("run should start");
        run
    }

    #[tokio::test]
    async fn a_refusing_host_callback_never_quarantines_a_recoverable_run() {
        let fixture = fixture().await;
        let run = started_run(&fixture, Uuid::new_v4()).await;
        let refuse = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let mut database = fixture.database.clone();
        database.set_entity_policy(RefuseSessionEvents {
            refuse: refuse.clone(),
        });
        fixture.clock.advance_seconds(61);
        let refused = service_on(&fixture, database.clone(), 16)
            .recover_expired_leases()
            .await
            .expect("an operational refusal must not fail the pass");
        assert_eq!(refused.skipped, 1, "{refused:?}");
        assert_eq!(refused.quarantined, 0);
        let untouched = run_record(&fixture, run).await;
        assert_eq!(untouched.state, AiRunState::Running.as_str());
        assert!(untouched.lease_owner.is_some());
        assert!(untouched.recovery_examined_at > 0);
        // The host recovers; a fresh service recovers the run normally.
        refuse.store(false, std::sync::atomic::Ordering::SeqCst);
        let recovered = service_on(&fixture, database, 16)
            .recover_expired_leases()
            .await
            .expect("recovery should succeed");
        assert_eq!(recovered.recovery_required, 1, "{recovered:?}");
        assert_eq!(recovered.quarantined, 0);
        assert_eq!(
            run_record(&fixture, run).await.error_code.as_deref(),
            Some("lease_expired_after_start")
        );
    }

    #[tokio::test]
    async fn a_permanently_refused_run_rotates_behind_healthy_runs_across_restarts() {
        let fixture = fixture().await;
        // The refused run sorts first; its terminal event is always refused.
        let refused_run = started_run(&fixture, Uuid::from_u128(1)).await;
        // The healthy run is only leased, so its requeue writes no event.
        let healthy = seed_queued_with_id(&fixture, Uuid::from_u128(u128::MAX)).await;
        fixture
            .service
            .claim_next("worker-healthy")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        let mut database = fixture.database.clone();
        database.set_entity_policy(RefuseSessionEvents {
            refuse: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        });
        fixture.clock.advance_seconds(61);
        let mut requeued = 0;
        for pass in 0..4 {
            // One candidate per pass, and a new service each time.
            let report = service_on(&fixture, database.clone(), 1)
                .recover_expired_leases()
                .await
                .expect("an operational refusal must not fail the pass");
            assert_eq!(report.quarantined, 0, "pass {pass}: {report:?}");
            requeued += report.requeued;
            fixture.clock.advance_seconds(1);
        }
        assert_eq!(requeued, 1);
        assert_eq!(
            run_record(&fixture, healthy).await.state,
            AiRunState::RetryScheduled.as_str()
        );
        let refused = run_record(&fixture, refused_run).await;
        assert_eq!(refused.state, AiRunState::Running.as_str());
        assert!(refused.error_code.is_none());
    }

    #[tokio::test]
    async fn progress_stamps_are_refused_after_the_epoch_or_row_changes() {
        let fixture = fixture().await;
        let run = started_run(&fixture, Uuid::new_v4()).await;
        let record = run_record(&fixture, run).await;
        let stamp = |version: i64, state: String| {
            fixture
                .database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        stamp_run_recovery_progress(tx, run.0, version, &state, 7).await
                    })
                })
        };
        // A stale row version or state writes nothing.
        assert!(
            !stamp(record.row_version + 1, record.state.clone())
                .await
                .unwrap()
        );
        assert!(
            !stamp(record.row_version, "leased".to_owned())
                .await
                .unwrap()
        );
        assert_eq!(run_record(&fixture, run).await.recovery_examined_at, 0);
        let reservation = AiBudgetReservationRecord::insert(
            &fixture.database,
            CreateAiBudgetReservationRecordInput {
                budget_counter_ids: serde_json::json!([]),
                scope_kind: "tenant".to_owned(),
                scope_id: "tenant-run".to_owned(),
                tenant_id: Some("tenant-run".to_owned()),
                principal_kind: "user".to_owned(),
                principal_subject: "run-user".to_owned(),
                session_id: record.session_id,
                run_id: record.id,
                attempt_id: record.attempt_id.unwrap(),
                lease_generation: record.lease_generation,
                provider_kind: "openai".to_owned(),
                provider_model: "stamp-test".to_owned(),
                reasoning_effort: "unspecified".to_owned(),
                pricing_policy_version: "pricing-v1".to_owned(),
                reserved_input_tokens: 1,
                reserved_output_tokens: 1,
                reserved_tool_units: 0,
                reserved_image_units: 0,
                reserved_cost_microunits: 1,
                reserved_runs: 1,
                actual_input_tokens: None,
                actual_cached_input_tokens: None,
                actual_output_tokens: None,
                actual_tool_units: None,
                actual_image_units: None,
                actual_cost_microunits: None,
                actual_runs: None,
                idempotency_key: "stamp-test".to_owned(),
                state: "reserved".to_owned(),
                expires_at: fixture.clock.now().unix_timestamp(),
                reconciled_at: None,
                settlement_examined_at: 0,
            },
        )
        .await
        .expect("test reservation should insert");
        let stamp_reservation = |version: i64| {
            let id = reservation.id;
            fixture
                .database
                .transaction(TransactionMode::StateMachine, move |tx| {
                    Box::pin(async move {
                        stamp_reservation_settlement_progress(tx, id, version, "reserved", 7).await
                    })
                })
        };
        assert!(
            !stamp_reservation(reservation.row_version + 1)
                .await
                .unwrap()
        );
        // A restore closes the runtime between settlement and its stamp: both
        // stamps are refused outright.
        crate::orm_runtime_recovery::close_test_epoch(&fixture.database).await;
        let closed_reservation = stamp_reservation(reservation.row_version).await;
        assert!(closed_reservation.is_err_and(|error| {
            crate::orm_runtime_recovery::is_epoch_not_live(error.public_error())
        }));
        let unstamped = AiBudgetReservationRecord::find_by_id(&fixture.database, &reservation.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unstamped.settlement_examined_at, 0);
        assert_eq!(unstamped.row_version, reservation.row_version);
        let closed = stamp(record.row_version, record.state.clone()).await;
        assert!(
            closed.is_err_and(|error| crate::orm_runtime_recovery::is_epoch_not_live(
                error.public_error()
            ))
        );
        let after = run_record(&fixture, run).await;
        assert_eq!(after.recovery_examined_at, 0);
        assert_eq!(after.row_version, record.row_version);
    }

    /// A host row policy that decides AI run rows in Rust, row by row.
    struct CallbackRunRows;

    impl graphql_orm::graphql::orm::RowPolicy<DefaultWriteBackend> for CallbackRunRows {
        fn can_read_row<'a>(
            &'a self,
            _ctx: Option<&'a async_graphql::Context<'_>>,
            _db: &'a Database<DefaultWriteBackend>,
            _entity_name: &'static str,
            _policy_key: Option<&'static str>,
            _surface: graphql_orm::graphql::orm::EntityAccessSurface,
            _row: &'a (dyn std::any::Any + Send + Sync),
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(true) })
        }

        fn can_write_row<'a>(
            &'a self,
            _ctx: Option<&'a async_graphql::Context<'_>>,
            _db: &'a Database<DefaultWriteBackend>,
            _entity_name: &'static str,
            _policy_key: Option<&'static str>,
            _surface: graphql_orm::graphql::orm::EntityAccessSurface,
            _row: &'a (dyn std::any::Any + Send + Sync),
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(true) })
        }
    }

    #[tokio::test]
    async fn a_scan_refused_by_a_host_row_policy_budget_is_an_operational_failure() {
        let fixture = fixture().await;
        let mut runs = Vec::new();
        for _ in 0..3 {
            runs.push(started_run(&fixture, Uuid::new_v4()).await);
        }
        fixture.clock.advance_seconds(61);
        let mut database = fixture.database.clone();
        database.set_row_policy(CallbackRunRows);
        let narrow =
            database
                .clone()
                .with_pagination_config(graphql_orm::graphql::orm::PaginationConfig {
                    max_scanned_rows: Some(1),
                    ..Default::default()
                });
        // The candidate budget refuses the scan: the pass reports the failure
        // and changes and quarantines nothing.
        assert!(
            service_on(&fixture, narrow, 16)
                .recover_expired_leases()
                .await
                .is_err()
        );
        for run in &runs {
            assert_eq!(
                run_record(&fixture, *run).await.state,
                AiRunState::Running.as_str()
            );
        }
        // With the default budget the same row policy lets recovery proceed.
        let report = service_on(&fixture, database, 16)
            .recover_expired_leases()
            .await
            .expect("recovery under a callback row policy should succeed");
        assert_eq!(report.recovery_required, 3, "{report:?}");
        assert_eq!(report.quarantined, 0);
    }

    /// A host row policy that hides session rows while `hide` is set.
    struct HideSessions {
        hide: Arc<std::sync::atomic::AtomicBool>,
    }

    impl graphql_orm::graphql::orm::RowPolicy<DefaultWriteBackend> for HideSessions {
        fn can_read_row<'a>(
            &'a self,
            _ctx: Option<&'a async_graphql::Context<'_>>,
            _db: &'a Database<DefaultWriteBackend>,
            entity_name: &'static str,
            _policy_key: Option<&'static str>,
            _surface: graphql_orm::graphql::orm::EntityAccessSurface,
            _row: &'a (dyn std::any::Any + Send + Sync),
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            let visible = !(entity_name == "AiSessionRecord"
                && self.hide.load(std::sync::atomic::Ordering::SeqCst));
            Box::pin(async move { Ok(visible) })
        }

        fn can_write_row<'a>(
            &'a self,
            _ctx: Option<&'a async_graphql::Context<'_>>,
            _db: &'a Database<DefaultWriteBackend>,
            _entity_name: &'static str,
            _policy_key: Option<&'static str>,
            _surface: graphql_orm::graphql::orm::EntityAccessSurface,
            _row: &'a (dyn std::any::Any + Send + Sync),
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(true) })
        }
    }

    #[tokio::test]
    async fn a_session_hidden_by_a_row_policy_is_skipped_not_quarantined() {
        let fixture = fixture().await;
        let run = started_run(&fixture, Uuid::new_v4()).await;
        let hide = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let mut database = fixture.database.clone();
        database.set_row_policy(HideSessions { hide: hide.clone() });
        fixture.clock.advance_seconds(61);
        for _ in 0..3 {
            let report = service_on(&fixture, database.clone(), 16)
                .recover_expired_leases()
                .await
                .expect("a hidden linked row must not fail the pass");
            assert_eq!(report.skipped, 1, "{report:?}");
            assert_eq!(report.quarantined, 0);
            let untouched = run_record(&fixture, run).await;
            assert_eq!(untouched.state, AiRunState::Running.as_str());
            assert!(untouched.error_code.is_none());
        }
        hide.store(false, std::sync::atomic::Ordering::SeqCst);
        let recovered = service_on(&fixture, database, 16)
            .recover_expired_leases()
            .await
            .expect("recovery should succeed once the session is visible");
        assert_eq!(recovered.recovery_required, 1, "{recovered:?}");
        assert_eq!(recovered.quarantined, 0);
    }

    /// A host update hook that refuses every recovery progress stamp.
    struct RefuseRecoveryStamps;

    impl graphql_orm::graphql::orm::WriteInputTransform<DefaultWriteBackend> for RefuseRecoveryStamps {
        fn before_update<'a>(
            &'a self,
            _ctx: Option<&'a async_graphql::Context<'_>>,
            _db: &'a Database<DefaultWriteBackend>,
            _entity_name: &'static str,
            _existing_row: Option<&'a (dyn std::any::Any + Send + Sync)>,
            input: &'a mut (dyn std::any::Any + Send + Sync),
        ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
            let stamp = input
                .downcast_ref::<UpdateAiRunRecordInput>()
                .is_some_and(|update| update.recovery_examined_at.is_some());
            Box::pin(async move {
                if stamp {
                    Err(async_graphql::Error::new("stamp refused"))
                } else {
                    Ok(())
                }
            })
        }
    }

    #[tokio::test]
    async fn a_refused_progress_stamp_fails_the_pass_after_its_other_work() {
        let fixture = fixture().await;
        let refused = started_run(&fixture, Uuid::from_u128(1)).await;
        let healthy = seed_queued_with_id(&fixture, Uuid::from_u128(u128::MAX)).await;
        fixture
            .service
            .claim_next("worker-healthy")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        let mut database = fixture.database.clone();
        database.set_entity_policy(RefuseSessionEvents {
            refuse: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        });
        database.set_write_input_transform(RefuseRecoveryStamps);
        fixture.clock.advance_seconds(61);
        let result = service_on(&fixture, database, 16)
            .recover_expired_leases()
            .await;
        assert!(result.is_err(), "a refused stamp must fail the pass");
        // The rest of the page was still processed.
        assert_eq!(
            run_record(&fixture, healthy).await.state,
            AiRunState::RetryScheduled.as_str()
        );
        let untouched = run_record(&fixture, refused).await;
        assert_eq!(untouched.state, AiRunState::Running.as_str());
        assert_eq!(untouched.recovery_examined_at, 0);
    }

    #[tokio::test]
    async fn progress_stamps_rotate_every_row_within_one_second() {
        let fixture = fixture().await;
        let mut runs = Vec::new();
        for index in 1..=3u128 {
            runs.push(started_run(&fixture, Uuid::from_u128(index)).await);
        }
        let mut database = fixture.database.clone();
        database.set_entity_policy(RefuseSessionEvents {
            refuse: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        });
        fixture.clock.advance_seconds(61);
        // The clock stays fixed for every pass; each pass sees two rows.
        let mut examined = [0u32; 3];
        for _ in 0..6 {
            let mut before = Vec::new();
            for run in &runs {
                before.push(run_record(&fixture, *run).await.recovery_examined_at);
            }
            let report = service_on(&fixture, database.clone(), 2)
                .recover_expired_leases()
                .await
                .expect("an operational refusal must not fail the pass");
            assert_eq!(report.skipped, 2, "{report:?}");
            for (index, run) in runs.iter().enumerate() {
                if run_record(&fixture, *run).await.recovery_examined_at != before[index] {
                    examined[index] += 1;
                }
            }
        }
        assert_eq!(examined.iter().sum::<u32>(), 12);
        assert!(examined.iter().all(|count| *count >= 3), "{examined:?}");
    }

    #[test]
    fn filtering_is_excluded_only_without_any_possible_filter() {
        use BackendRowSecurity::{Active, Inactive, None as NoRowSecurity, Unknown};
        for (row_policy, backend, excluded) in [
            (false, NoRowSecurity, true),
            (false, Inactive, true),
            (false, Active, false),
            (false, Unknown, false),
            (true, NoRowSecurity, false),
            (true, Inactive, false),
            (true, Active, false),
            (true, Unknown, false),
        ] {
            assert_eq!(
                filtering_excluded_decision(row_policy, backend),
                excluded,
                "row_policy={row_policy} backend={backend:?}"
            );
        }
    }

    #[tokio::test]
    async fn sqlite_recovery_excludes_filtering_only_without_a_row_policy() {
        let fixture = fixture().await;
        assert!(recovery_filtering_excluded(&fixture.database).await);
        let mut database = fixture.database.clone();
        database.set_row_policy(CallbackRunRows);
        assert!(!recovery_filtering_excluded(&database).await);
    }

    #[test]
    fn native_check_kinds_keep_their_previous_outward_errors() {
        use native_checkpoints::NativeCheckError;
        for (error, code) in [
            (NativeCheckError::Contradiction, OrmErrorCode::Conflict),
            (NativeCheckError::Incomplete, OrmErrorCode::Conflict),
            (NativeCheckError::Missing, OrmErrorCode::NotFound),
            (
                NativeCheckError::Operational(OrmPublicError::new(
                    OrmErrorCode::ServiceUnavailable,
                )),
                OrmErrorCode::ServiceUnavailable,
            ),
        ] {
            let outward = OrmPublicError::from(error);
            assert_eq!(outward.code, code);
            assert_eq!(outward.message, OrmPublicError::new(code).message);
        }
    }

    #[tokio::test]
    async fn recovery_scan_finds_an_expired_lease_behind_a_full_window_of_live_leases() {
        let fixture = fixture().await;
        // The expired run sorts after every live run by primary key.
        let expired = seed_queued_with_id(&fixture, Uuid::from_u128(u128::MAX)).await;
        fixture
            .service
            .claim_next("worker-expired")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        fixture.clock.advance_seconds(30);
        let scan = fixture.service.limits.maximum_candidate_scan();
        for index in 0..=scan {
            seed_queued_with_id(&fixture, Uuid::from_u128(index as u128 + 1)).await;
            fixture
                .service
                .claim_next("worker-live")
                .await
                .expect("claim should succeed")
                .expect("run should be eligible");
        }
        fixture.clock.advance_seconds(31);
        let report = fixture
            .service
            .recover_expired_leases()
            .await
            .expect("recovery should succeed");
        assert_eq!(report.requeued, 1);
        assert_eq!(
            run_record(&fixture, expired).await.state,
            AiRunState::RetryScheduled.as_str()
        );
    }

    #[tokio::test]
    async fn recovery_requeues_stateful_and_stateless_completed_tool_batches() {
        assert_recovery_requeues_completed_tool_batch(Some("response-tool-checkpoint")).await;
        assert_recovery_requeues_completed_tool_batch(None).await;
    }

    #[tokio::test]
    async fn terminal_completion_is_fenced_and_records_one_immutable_outcome() {
        let fixture = fixture().await;
        let run_id = seed_queued(&fixture).await;
        let leased = fixture
            .service
            .claim_next("worker-complete")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        let running = fixture
            .service
            .start(&leased)
            .await
            .expect("run should start");
        let completion = AiRunCompletion::new(
            AiRunState::Completed,
            "provider_completed",
            None,
            Some("response-safe-reference".to_owned()),
        )
        .expect("completion should validate");
        fixture
            .service
            .finish(&running, completion)
            .await
            .expect("current fence should finish");

        let record = run_record(&fixture, run_id).await;
        assert_eq!(record.state, AiRunState::Completed.as_str());
        assert!(record.lease_owner.is_none());
        let outcomes = attempt_outcomes(&fixture).await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].attempt_id, running.attempt_id());
        assert_eq!(outcomes[0].final_state, AiRunState::Completed.as_str());
        assert_eq!(
            outcomes[0].provider_response_id.as_deref(),
            Some("response-safe-reference")
        );
        let events = session_events(&fixture).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "run_completed");
        assert_eq!(events[0].run_id, Some(run_id.0));
        assert_eq!(events[0].sequence, 1);
        assert_eq!(
            open_terminal_event_metadata(&events[0].event_type, &events[0].protected_payload)
                .expect("terminal metadata should validate")
                .expect("terminal event should use metadata-only envelope"),
            serde_json::json!({
                "format": TERMINAL_EVENT_METADATA_FORMAT_V2,
                "state": "completed",
                "failure": serde_json::Value::Null,
            })
        );
        let inbox = inbox_events(&fixture).await;
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].event_type, "run_completed");
        assert_eq!(inbox[0].session_id, Some(running.session_id().0));
        assert!(matches!(
            fixture
                .service
                .finish(
                    &running,
                    AiRunCompletion::new(AiRunState::Failed, "late_failure", None, None)
                        .expect("failure should validate")
                )
                .await,
            Err(AiError::Conflict)
        ));
        assert_eq!(session_events(&fixture).await.len(), 1);
        assert_eq!(inbox_events(&fixture).await.len(), 1);
        assert_eq!(attempt_outcomes(&fixture).await.len(), 1);
    }

    #[tokio::test]
    async fn failed_and_recovery_completions_publish_exact_canonical_events() {
        for (state, event_type, code) in [
            (AiRunState::Failed, "run_failed", "provider_failed"),
            (
                AiRunState::RecoveryRequired,
                "run_recovery_required",
                "provider_ambiguous",
            ),
        ] {
            let fixture = fixture().await;
            let run_id = seed_queued(&fixture).await;
            let lease = fixture
                .service
                .claim_next("worker-terminal-event")
                .await
                .expect("claim should succeed")
                .expect("run should be eligible");
            let lease = fixture
                .service
                .start(&lease)
                .await
                .expect("run should start");
            fixture
                .service
                .finish(
                    &lease,
                    AiRunCompletion::new(state, code, Some(code.to_owned()), None)
                        .expect("terminal completion should validate"),
                )
                .await
                .expect("terminal completion should commit");

            assert_eq!(run_record(&fixture, run_id).await.state, state.as_str());
            let events = session_events(&fixture).await;
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].event_type, event_type);
            assert_eq!(events[0].run_id, Some(run_id.0));
            let inbox = inbox_events(&fixture).await;
            assert_eq!(inbox.len(), 1);
            assert_eq!(inbox[0].event_type, event_type);
            assert_eq!(attempt_outcomes(&fixture).await.len(), 1);
        }
    }

    #[tokio::test]
    async fn terminal_event_failure_rolls_back_run_outcome_and_watermarks() {
        let fixture = fixture().await;
        let run_id = seed_queued(&fixture).await;
        let lease = fixture
            .service
            .claim_next("worker-terminal-rollback")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        let lease = fixture
            .service
            .start(&lease)
            .await
            .expect("run should start");
        let session = AiSessionRecord::find_by_id(&fixture.database, &lease.session_id().0)
            .await
            .expect("session lookup should succeed")
            .expect("session should exist");
        fixture
            .database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let outcome = tx
                        .compare_and_swap::<AiSessionRecord>(
                            &session.id,
                            session.row_version,
                            AiSessionRecordWhereInput::default(),
                            UpdateAiSessionRecordInput {
                                stream_head: Some(i64::MAX),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?;
                    if matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                        Ok(())
                    } else {
                        Err(OrmPublicError::new(OrmErrorCode::Conflict))
                    }
                })
            })
            .await
            .expect("test should inject a terminal-event sequence overflow");

        assert!(matches!(
            fixture
                .service
                .finish(
                    &lease,
                    AiRunCompletion::new(
                        AiRunState::RecoveryRequired,
                        "terminal_event_failure",
                        Some("terminal_event_failure".to_owned()),
                        None,
                    )
                    .expect("completion should validate"),
                )
                .await,
            Err(AiError::PersistenceFailed)
        ));
        assert_eq!(
            run_record(&fixture, run_id).await.state,
            AiRunState::Running.as_str()
        );
        assert!(attempt_outcomes(&fixture).await.is_empty());
        assert!(session_events(&fixture).await.is_empty());
        assert!(inbox_events(&fixture).await.is_empty());
    }

    #[tokio::test]
    async fn scheduled_retry_relinquishes_fence_and_uses_a_new_generation() {
        let fixture = fixture().await;
        let run_id = seed_queued(&fixture).await;
        let running = fixture
            .service
            .claim_next("worker-retry")
            .await
            .expect("claim should succeed")
            .expect("run should be eligible");
        let running = fixture
            .service
            .start(&running)
            .await
            .expect("run should start");
        fixture
            .service
            .schedule_retry(&running, Duration::seconds(30), "provider_unavailable")
            .await
            .expect("bounded retry should schedule");
        assert_eq!(
            run_record(&fixture, run_id).await.state,
            AiRunState::RetryScheduled.as_str()
        );
        assert!(
            fixture
                .service
                .claim_next("worker-too-early")
                .await
                .expect("early claim should query")
                .is_none()
        );
        fixture.clock.advance_seconds(30);
        let retry = fixture
            .service
            .claim_next("worker-retry-next")
            .await
            .expect("retry claim should succeed")
            .expect("retry should now be eligible");
        assert_eq!(retry.run_id(), run_id);
        assert_eq!(retry.retry_count(), 1);
        assert_eq!(retry.lease_generation(), 2);
    }
    async fn native_checkpoint_fixture() -> (Fixture, AiRunLease, Uuid, Uuid) {
        let fixture = fixture().await;
        seed_queued(&fixture).await;
        let claim = fixture
            .service
            .claim_next("native-checkpoint-worker")
            .await
            .unwrap()
            .unwrap();
        let lease = fixture.service.start(&claim).await.unwrap();
        let budget = native_budget(&fixture, &lease).await;
        let prepared = fixture
            .service
            .prepare_native_approval(
                &lease,
                native_call(budget, "non_idempotent_write"),
                native_candidate(),
            )
            .await
            .unwrap();
        let id = prepared.tool_call_id().0;
        let decision_id = Uuid::new_v4();
        AiEgressEventRecord::insert(
            &fixture.database,
            CreateAiEgressEventRecordInput {
                id: decision_id,
                run_id: Some(lease.run_id().0),
                principal_subject: "run-user".to_owned(),
                scope_kind: "tenant".to_owned(),
                scope_id: "tenant-run".to_owned(),
                manifest_hash: "d".repeat(64),
                destination: "provider".to_owned(),
                capability: "tool_result".to_owned(),
                classification: "internal".to_owned(),
                outcome: "allow".to_owned(),
                reason_code: "fixture".to_owned(),
                policy_version: "fixture-v1".to_owned(),
                estimated_bytes: 16,
                estimated_tokens: 4,
            },
        )
        .await
        .unwrap();
        let renewed = fixture
            .service
            .persist_native_approval_receipt(
                prepared.lease(),
                prepared.tool_call_id(),
                PreparedNativeApprovalReceipt {
                    receipt_hash: "e".repeat(64),
                    protected_receipt: serde_json::json!({"protected":"receipt"}),
                    egress_decision_id: decision_id,
                    egress_manifest_hash: "d".repeat(64),
                },
            )
            .await
            .unwrap();
        fixture
            .database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    tx.update_by_id::<AiBudgetReservationRecord>(
                        &budget,
                        UpdateAiBudgetReservationRecordInput {
                            state: Some("committed".to_owned()),
                            actual_input_tokens: Some(Some(1)),
                            actual_cached_input_tokens: Some(Some(0)),
                            actual_output_tokens: Some(Some(1)),
                            actual_tool_units: Some(Some(1)),
                            actual_image_units: Some(Some(0)),
                            actual_cost_microunits: Some(Some(1)),
                            actual_runs: Some(Some(1)),
                            reconciled_at: Some(Some(1_700_000_000)),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(OrmPublicError::from)?;
                    Ok(())
                })
            })
            .await
            .unwrap();
        (fixture, renewed, id, budget)
    }

    fn native_source_checkpoint(
        lease: &AiRunLease,
        candidate_id: Uuid,
        budget: Uuid,
    ) -> PreparedCoordinatorCheckpoint {
        let id = Uuid::new_v4();
        let protected_state = serde_json::json!({"protected":"native-source-fixture"});
        let checkpoint_hash = coordinator_checkpoint_hash(
            lease.run_id(),
            lease.attempt_id(),
            lease.lease_generation(),
            id,
            native_checkpoints::SOURCE_KIND,
            "openai",
            "reviewed-model",
            Some("native-response"),
            budget,
            &protected_state,
        )
        .unwrap();
        PreparedCoordinatorCheckpoint {
            id,
            checkpoint_kind: native_checkpoints::SOURCE_KIND.to_owned(),
            provider_kind: "openai".to_owned(),
            provider_model: "reviewed-model".to_owned(),
            provider_response_id: Some("native-response".to_owned()),
            budget_reservation_id: budget,
            protected_state,
            checkpoint_hash,
            completed_tools: vec![],
            native_binding: Some(PreparedNativeCheckpointBinding::Source {
                candidate_id,
                pending_call_index: 0,
                callback_count: 1,
            }),
        }
    }

    #[tokio::test]
    async fn native_source_checkpoint_is_never_generic_recovery_or_consumption_work() {
        let (fixture, lease, candidate_id, budget) = native_checkpoint_fixture().await;
        let source = native_source_checkpoint(&lease, candidate_id, budget);
        let id = source.id;
        let checkpointed = fixture
            .service
            .append_coordinator_checkpoint(&lease, source)
            .await
            .unwrap();
        assert!(matches!(
            fixture
                .service
                .consume_adoption_checkpoint(&checkpointed, id, native_checkpoints::SOURCE_KIND)
                .await,
            Err(AiError::Conflict)
        ));
        fixture.clock.advance_seconds(61);
        let report = fixture.service.recover_expired_leases().await.unwrap();
        assert_eq!(report.checkpoint_requeued, 0);
        assert_eq!(report.recovery_required, 1);
        assert_eq!(
            run_record(&fixture, lease.run_id()).await.state,
            "recovery_required"
        );
        assert!(
            fixture
                .service
                .claim_next("must-not-replay")
                .await
                .unwrap()
                .is_none()
        );
        let candidate =
            AiNativeApprovalCandidateRecord::find_by_id(&fixture.database, &candidate_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(candidate.state, "abandoned");
        assert!(candidate.final_approval_id.is_none());
    }

    #[tokio::test]
    async fn native_source_append_rejects_changed_receipt_candidate_and_callback_shape() {
        let (fixture, lease, candidate_id, budget) = native_checkpoint_fixture().await;
        for (id, index, count) in [
            (Uuid::new_v4(), 0, 1),
            (candidate_id, 1, 1),
            (candidate_id, 0, 2),
        ] {
            let mut source = native_source_checkpoint(&lease, candidate_id, budget);
            source.native_binding = Some(PreparedNativeCheckpointBinding::Source {
                candidate_id: id,
                pending_call_index: index,
                callback_count: count,
            });
            assert!(
                fixture
                    .service
                    .append_coordinator_checkpoint(&lease, source)
                    .await
                    .is_err()
            );
        }
        fixture
            .database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    tx.update_by_id::<AiNativeApprovalCandidateRecord>(
                        &candidate_id,
                        UpdateAiNativeApprovalCandidateRecordInput {
                            control_receipt_hash: Some(None),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(OrmPublicError::from)?;
                    Ok(())
                })
            })
            .await
            .unwrap();
        assert!(
            fixture
                .service
                .append_coordinator_checkpoint(
                    &lease,
                    native_source_checkpoint(&lease, candidate_id, budget)
                )
                .await
                .is_err()
        );
        assert!(
            run_record(&fixture, lease.run_id())
                .await
                .latest_checkpoint_id
                .is_none()
        );
        assert_eq!(
            AiRunCheckpointRecord::query(fixture.database.pool())
                .limit(2)
                .fetch_all()
                .await
                .unwrap()
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn native_cancel_or_archive_before_finalization_keeps_completed_effect_evidence() {
        for archive in [false, true] {
            let (fixture, lease, candidate_id, budget) = native_checkpoint_fixture().await;
            // A completed application result is durable evidence, not work to replay.
            let mut prior = native_call(budget, "read_only");
            prior.tool_call_index = 1;
            let prior_id = prior.id;
            let key = prior.provider_call_key.clone();
            let fingerprint = prior.tool_fingerprint.clone();
            let lease = fixture
                .service
                .begin_tool_call(&lease, prior)
                .await
                .unwrap();
            let lease = fixture
                .service
                .finish_tool_call(
                    &lease,
                    PreparedToolCallFinish {
                        id: prior_id,
                        state: "completed".to_owned(),
                        protected_result: serde_json::json!({"protected":"prior-result"}),
                        authorization_code: "allowed".to_owned(),
                        authorization_policy_version: Some("policy-v1".to_owned()),
                        authorization_state_digest: Some("f".repeat(64)),
                        disclosure_schema_fingerprint: "a".repeat(64),
                        result_classification: "internal".to_owned(),
                        result_egress_decision_id: Some(Uuid::new_v4()),
                        result_egress_manifest_hash: Some("b".repeat(64)),
                        application_audit_ref: Some("prior-effect".to_owned()),
                        event_id: Uuid::new_v4(),
                        inbox_event_id: Uuid::new_v4(),
                        protected_event: serde_json::json!({"protected":true}),
                        protected_inbox_event: serde_json::json!({"protected":true}),
                        correlation_id: "fixture".to_owned(),
                        expected_provider_call_key: key,
                        expected_tool_fingerprint: fingerprint,
                        expected_owner_principal_kind: "user".to_owned(),
                        expected_owner_subject: "run-user".to_owned(),
                        expected_scope_kind: "tenant".to_owned(),
                        expected_scope_id: "tenant-run".to_owned(),
                        expected_tenant_id: Some("tenant-run".to_owned()),
                    },
                )
                .await
                .unwrap();
            let before = AiToolCallRecord::find_by_id(&fixture.database, &prior_id)
                .await
                .unwrap()
                .unwrap();
            if archive {
                let session_id = lease.session_id().0;
                fixture
                    .database
                    .transaction(TransactionMode::StateMachine, move |tx| {
                        Box::pin(async move {
                            tx.update_by_id::<AiSessionRecord>(
                                &session_id,
                                UpdateAiSessionRecordInput {
                                    state: Some("archived".to_owned()),
                                    archived_at: Some(Some(1_700_000_000)),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                            Ok(())
                        })
                    })
                    .await
                    .unwrap();
            }
            let completion = AiRunCompletion::new(
                if archive {
                    AiRunState::RecoveryRequired
                } else {
                    AiRunState::Cancelled
                },
                if archive {
                    "session_archived"
                } else {
                    "owner_cancelled"
                },
                None,
                None,
            )
            .unwrap();
            fixture.service.finish(&lease, completion).await.unwrap();
            assert_eq!(
                AiToolCallRecord::find_by_id(&fixture.database, &prior_id)
                    .await
                    .unwrap()
                    .unwrap(),
                before
            );
            let candidate =
                AiNativeApprovalCandidateRecord::find_by_id(&fixture.database, &candidate_id)
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(candidate.state, "abandoned");
            assert!(candidate.final_approval_id.is_none());
            assert!(
                fixture
                    .service
                    .append_coordinator_checkpoint(
                        &lease,
                        native_source_checkpoint(&lease, candidate_id, budget)
                    )
                    .await
                    .is_err()
            );
            assert!(
                fixture
                    .service
                    .claim_next_approved("must-not-execute")
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }

    async fn native_budget(fixture: &Fixture, lease: &AiRunLease) -> Uuid {
        AiBudgetReservationRecord::insert(
            &fixture.database,
            CreateAiBudgetReservationRecordInput {
                budget_counter_ids: serde_json::json!([]),
                scope_kind: "tenant".to_owned(),
                scope_id: "tenant-run".to_owned(),
                tenant_id: Some("tenant-run".to_owned()),
                principal_kind: "user".to_owned(),
                principal_subject: "run-user".to_owned(),
                session_id: lease.session_id().0,
                run_id: lease.run_id().0,
                attempt_id: lease.attempt_id(),
                lease_generation: lease.lease_generation(),
                provider_kind: "openai".to_owned(),
                provider_model: "reviewed-model".to_owned(),
                reasoning_effort: "unspecified".to_owned(),
                pricing_policy_version: "pricing-v1".to_owned(),
                reserved_input_tokens: 10,
                reserved_output_tokens: 10,
                reserved_tool_units: 3,
                reserved_image_units: 0,
                reserved_cost_microunits: 10,
                reserved_runs: 1,
                actual_input_tokens: None,
                actual_cached_input_tokens: None,
                actual_output_tokens: None,
                actual_tool_units: None,
                actual_image_units: None,
                actual_cost_microunits: None,
                actual_runs: None,
                idempotency_key: Uuid::new_v4().to_string(),
                state: "uncertain".to_owned(),
                expires_at: (fixture.clock.now() + Duration::minutes(1)).unix_timestamp(),
                reconciled_at: None,
                settlement_examined_at: 0,
            },
        )
        .await
        .expect("budget")
        .id
    }

    fn native_call(budget: Uuid, risk: &str) -> PreparedToolCallStart {
        let id = Uuid::new_v4();
        PreparedToolCallStart {
            id,
            provider_call_key: id.to_string(),
            provider_call_id: id.to_string(),
            provider_kind: "openai".to_owned(),
            provider_model: "reviewed-model".to_owned(),
            provider_response_id: Some("native-response".to_owned()),
            budget_reservation_id: budget,
            provider_turn_index: 0,
            tool_call_index: 0,
            tool_id: "reviewed-action".to_owned(),
            tool_fingerprint: "descriptor-v1".to_owned(),
            execution_provenance: None,
            native_preflight_refusal: None,
            protected_arguments: serde_json::json!({"ciphertext":"arguments"}),
            argument_hash: "b".repeat(64),
            risk: risk.to_owned(),
            idempotency_key: None,
            correlation_id: "native-test".to_owned(),
            causation_id: "native-test".to_owned(),
            delegation_reference: None,
            started_event: None,
            expected_owner_principal_kind: "user".to_owned(),
            expected_owner_subject: "run-user".to_owned(),
            expected_scope_kind: "tenant".to_owned(),
            expected_scope_id: "tenant-run".to_owned(),
            expected_tenant_id: Some("tenant-run".to_owned()),
        }
    }

    fn native_candidate() -> PreparedNativeApprovalCandidate {
        PreparedNativeApprovalCandidate {
            binding_hash: "a".repeat(64),
            preview_hash: "c".repeat(64),
            protected_preparation: serde_json::json!({"ciphertext":"preparation"}),
            prepared_event: PreparedToolLifecycleEvent {
                event_id: Uuid::new_v4(),
                inbox_event_id: Uuid::new_v4(),
                protected_event: serde_json::json!({"ciphertext":"prepared-event"}),
                protected_inbox_event: serde_json::json!({"ciphertext":"prepared-inbox"}),
            },
        }
    }

    #[tokio::test]
    async fn native_preparation_keeps_lease_and_blocks_later_effect_admission() {
        let fixture = fixture().await;
        seed_queued(&fixture).await;
        let original = fixture
            .service
            .claim_next("native-worker")
            .await
            .unwrap()
            .unwrap();
        let original = fixture.service.start(&original).await.unwrap();
        let budget = native_budget(&fixture, &original).await;
        let call = native_call(budget, "consequential");
        let id = call.id;
        let prepared = fixture
            .service
            .prepare_native_approval(&original, call, native_candidate())
            .await
            .unwrap();
        assert_eq!(prepared.lease().state(), AiRunState::Running);
        assert_eq!(prepared.lease().attempt_id(), original.attempt_id());
        assert!(prepared.lease().row_version > original.row_version);
        let record = AiToolCallRecord::find_by_id(&fixture.database, &id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.state, "approval_prepared");
        assert!(record.approval_id.is_none());
        assert!(record.protected_result.is_none());
        let run = run_record(&fixture, original.run_id()).await;
        assert!(run.lease_owner.is_some());
        assert!(matches!(
            fixture
                .service
                .prepare_native_approval(
                    &original,
                    native_call(budget, "consequential"),
                    native_candidate()
                )
                .await,
            Err(AiError::Conflict)
        ));
        assert!(matches!(
            fixture
                .service
                .begin_tool_call(prepared.lease(), native_call(budget, "consequential"))
                .await,
            Err(AiError::Conflict)
        ));
        assert!(matches!(
            fixture
                .service
                .prepare_native_approval(
                    prepared.lease(),
                    native_call(budget, "consequential"),
                    native_candidate()
                )
                .await,
            Err(AiError::Conflict)
        ));
        let mut read = native_call(budget, "read_only");
        read.tool_call_index = 1;
        assert!(
            fixture
                .service
                .begin_tool_call(prepared.lease(), read)
                .await
                .is_ok()
        );
        let events = session_events(&fixture).await;
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "native_approval_prepared")
        );
        assert!(
            !events
                .iter()
                .any(|event| event.event_type == "application_tool_started")
        );
    }

    #[tokio::test]
    async fn native_preparation_requires_exact_live_provider_budget_and_rolls_back() {
        let fixture = fixture().await;
        seed_queued(&fixture).await;
        let lease = fixture
            .service
            .claim_next("native-worker")
            .await
            .unwrap()
            .unwrap();
        let lease = fixture.service.start(&lease).await.unwrap();
        let call = native_call(Uuid::new_v4(), "consequential");
        let id = call.id;
        assert!(
            fixture
                .service
                .prepare_native_approval(&lease, call, native_candidate())
                .await
                .is_err()
        );
        assert!(
            AiToolCallRecord::find_by_id(&fixture.database, &id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            AiNativeApprovalCandidateRecord::find_by_id(&fixture.database, &id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            run_record(&fixture, lease.run_id()).await.row_version,
            lease.row_version
        );
    }

    #[tokio::test]
    async fn native_receipt_is_exactly_idempotent_and_remains_non_approvable() {
        let fixture = fixture().await;
        seed_queued(&fixture).await;
        let lease = fixture
            .service
            .claim_next("native-worker")
            .await
            .unwrap()
            .unwrap();
        let lease = fixture.service.start(&lease).await.unwrap();
        let budget = native_budget(&fixture, &lease).await;
        let prepared = fixture
            .service
            .prepare_native_approval(
                &lease,
                native_call(budget, "consequential"),
                native_candidate(),
            )
            .await
            .unwrap();
        let egress = Uuid::new_v4();
        let receipt = || PreparedNativeApprovalReceipt {
            protected_receipt: serde_json::json!({"ciphertext":"closed-control-receipt"}),
            receipt_hash: "d".repeat(64),
            egress_decision_id: egress,
            egress_manifest_hash: "e".repeat(64),
        };
        let renewed = fixture
            .service
            .persist_native_approval_receipt(prepared.lease(), prepared.tool_call_id(), receipt())
            .await
            .unwrap();
        let retried = fixture
            .service
            .persist_native_approval_receipt(&renewed, prepared.tool_call_id(), receipt())
            .await
            .unwrap();
        assert_eq!(renewed, retried);
        let mut wrong = receipt();
        wrong.receipt_hash = "f".repeat(64);
        assert!(matches!(
            fixture
                .service
                .persist_native_approval_receipt(&renewed, prepared.tool_call_id(), wrong)
                .await,
            Err(AiError::Conflict)
        ));
        let candidate = fixture
            .service
            .load_prepared_native_approval(&renewed, prepared.tool_call_id())
            .await
            .unwrap();
        assert_eq!(candidate.state, "prepared");
        assert!(candidate.final_approval_id.is_none());
        assert!(candidate.settled_checkpoint_id.is_none());
        assert_eq!(run_record(&fixture, lease.run_id()).await.state, "running");
    }
    #[tokio::test]
    async fn native_later_mutation_records_closed_control_without_approval_or_execution() {
        let fixture = fixture().await;
        seed_queued(&fixture).await;
        let claimed = fixture
            .service
            .claim_next("native-worker")
            .await
            .unwrap()
            .unwrap();
        let lease = fixture.service.start(&claimed).await.unwrap();
        let budget = native_budget(&fixture, &lease).await;
        let prepared = fixture
            .service
            .prepare_native_approval(
                &lease,
                native_call(budget, "consequential"),
                native_candidate(),
            )
            .await
            .unwrap();
        let receipt = || PreparedNativeApprovalReceipt {
            protected_receipt: serde_json::json!({"ciphertext":"control"}),
            receipt_hash: "d".repeat(64),
            egress_decision_id: Uuid::new_v4(),
            egress_manifest_hash: "e".repeat(64),
        };
        let renewed = fixture
            .service
            .persist_native_approval_receipt(prepared.lease(), prepared.tool_call_id(), receipt())
            .await
            .unwrap();
        let mut call = native_call(budget, "consequential");
        call.tool_call_index = 1;
        let id = call.id;
        let blocked = PreparedNativeBlockedReceipt {
            candidate_id: prepared.tool_call_id(),
            receipt: receipt(),
            disclosure_schema_fingerprint: "f".repeat(64),
            event: native_candidate().prepared_event,
        };
        let renewed = fixture
            .service
            .persist_blocked_native_call(&renewed, call, blocked)
            .await
            .unwrap();
        let call = AiToolCallRecord::find_by_id(&fixture.database, &id)
            .await
            .unwrap()
            .unwrap();
        let step = AiRunStepRecord::find_by_id(&fixture.database, &id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(call.state, "control_blocked");
        assert_eq!(step.state, "control_blocked");
        assert!(call.approval_id.is_none());
        assert!(call.protected_result.is_some());
        assert!(call.completed_at.is_some() && step.finished_at.is_some());
        assert_eq!(
            call.authorization_code.as_deref(),
            Some("native_consequential_paused")
        );
        assert!(
            fixture
                .service
                .begin_tool_call(&renewed, native_call(budget, "consequential"))
                .await
                .is_err()
        );
        assert_eq!(
            fixture
                .service
                .load_prepared_native_approval(&renewed, prepared.tool_call_id())
                .await
                .unwrap()
                .state,
            "prepared"
        );
        assert_eq!(renewed.state(), AiRunState::Running);
    }

    fn native_approval_request(fixture: &Fixture, id: AiToolCallId) -> PreparedApprovalRequest {
        PreparedApprovalRequest {
            id: Uuid::new_v4(),
            tool_call_id: id.0,
            principal_subject: "run-user".to_owned(),
            principal_reference_fingerprint: "1".repeat(64),
            delegated_actor_subject: None,
            delegation_reference: None,
            argument_hash: "b".repeat(64),
            tool_fingerprint: "descriptor-v1".to_owned(),
            binding_hash: "a".repeat(64),
            execution_target_id: "local-graphql".to_owned(),
            target_schema_fingerprint: "2".repeat(64),
            operation_name: "UpdateRecord".to_owned(),
            operation_document_hash: "3".repeat(64),
            result_projection_fingerprint: "4".repeat(64),
            disclosure_schema_fingerprint: "5".repeat(64),
            policy_version: "policy-v1".to_owned(),
            authorization_state_digest: "6".repeat(64),
            protected_resource_bindings: serde_json::json!({"ciphertext":"resources"}),
            protected_action_preview: serde_json::json!({"ciphertext":"preview"}),
            action_preview_hash: "c".repeat(64),
            recent_mfa_required: false,
            expires_at: (fixture.clock.now() + Duration::minutes(10)).unix_timestamp(),
            event_id: Uuid::new_v4(),
            protected_event: serde_json::json!({"ciphertext":"approval"}),
            correlation_id: "native-test".to_owned(),
            expected_owner_principal_kind: "user".to_owned(),
            expected_owner_subject: "run-user".to_owned(),
            expected_scope_kind: "tenant".to_owned(),
            expected_scope_id: "tenant-run".to_owned(),
            expected_tenant_id: Some("tenant-run".to_owned()),
            parked_provider_wait: None,
        }
    }

    #[tokio::test]
    async fn native_candidate_cannot_bypass_settlement_or_provider_parking() {
        let fixture = fixture().await;
        seed_queued(&fixture).await;
        let lease = fixture
            .service
            .claim_next("native-worker")
            .await
            .unwrap()
            .unwrap();
        let lease = fixture.service.start(&lease).await.unwrap();
        let budget = native_budget(&fixture, &lease).await;
        let prepared = fixture
            .service
            .prepare_native_approval(
                &lease,
                native_call(budget, "consequential"),
                native_candidate(),
            )
            .await
            .unwrap();
        let id = prepared.tool_call_id();
        assert!(matches!(
            fixture
                .service
                .request_approval(prepared.lease(), native_approval_request(&fixture, id))
                .await,
            Err(AiError::Conflict)
        ));
        assert!(matches!(
            fixture
                .service
                .request_prepared_native_approval(
                    prepared.lease(),
                    native_approval_request(&fixture, id),
                    id
                )
                .await,
            Err(AiError::Conflict)
        ));
        let mut unproven = native_approval_request(&fixture, id);
        unproven.parked_provider_wait = Some(PreparedApprovalProviderWait {
            source_checkpoint_id: Uuid::new_v4(),
            source_checkpoint_fingerprint: "1".repeat(64),
            parked_checkpoint_id: Uuid::new_v4(),
            parked_checkpoint_fingerprint: "2".repeat(64),
            protected_parked_checkpoint: serde_json::json!({"ciphertext":"unproven"}),
            provider_kind: "openai".to_owned(),
            provider_model: "reviewed-model".to_owned(),
            provider_response_id: Some("native-response".to_owned()),
            budget_reservation_id: budget,
        });
        let rejected_approval_id = unproven.id;
        assert!(matches!(
            fixture
                .service
                .request_prepared_native_approval(prepared.lease(), unproven, id)
                .await,
            Err(AiError::Conflict)
        ));
        assert!(
            AiApprovalRecord::find_by_id(&fixture.database, &rejected_approval_id)
                .await
                .unwrap()
                .is_none()
        );
        let stored = fixture
            .service
            .load_prepared_native_approval(prepared.lease(), id)
            .await
            .unwrap();
        assert_eq!(stored.state, "prepared");
        assert!(stored.final_approval_id.is_none());
        assert_eq!(
            run_record(&fixture, lease.run_id()).await.row_version,
            prepared.lease().row_version
        );
    }
    #[tokio::test]
    async fn native_pending_action_blocks_success_and_abandons_on_failed_owner() {
        let fixture = fixture().await;
        seed_queued(&fixture).await;
        let claimed = fixture
            .service
            .claim_next("native-worker")
            .await
            .unwrap()
            .unwrap();
        let lease = fixture.service.start(&claimed).await.unwrap();
        let budget = native_budget(&fixture, &lease).await;
        let prepared = fixture
            .service
            .prepare_native_approval(
                &lease,
                native_call(budget, "consequential"),
                native_candidate(),
            )
            .await
            .unwrap();
        let success = AiRunCompletion::new(AiRunState::Completed, "done", None, None).unwrap();
        assert!(matches!(
            fixture.service.finish(prepared.lease(), success).await,
            Err(AiError::Conflict)
        ));
        assert_eq!(run_record(&fixture, lease.run_id()).await.state, "running");
        let failure = AiRunCompletion::new(
            AiRunState::RecoveryRequired,
            "native_provider_unsettled",
            Some("native_provider_unsettled".to_owned()),
            None,
        )
        .unwrap();
        fixture
            .service
            .finish(prepared.lease(), failure)
            .await
            .unwrap();
        let candidate = AiNativeApprovalCandidateRecord::find_by_id(
            &fixture.database,
            &prepared.tool_call_id().0,
        )
        .await
        .unwrap()
        .unwrap();
        let call = AiToolCallRecord::find_by_id(&fixture.database, &prepared.tool_call_id().0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(candidate.state, "abandoned");
        assert!(candidate.final_approval_id.is_none());
        assert_eq!(call.state, "cancelled");
        assert_eq!(
            call.authorization_code.as_deref(),
            Some("native_approval_abandoned")
        );
        assert!(call.approval_id.is_none());
        assert!(call.protected_result.is_none());
        assert!(
            fixture
                .service
                .load_prepared_native_approval(prepared.lease(), prepared.tool_call_id())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn inline_fenced_event_writes_refuse_a_closed_epoch() {
        let fixture = fixture().await;
        seed_queued(&fixture).await;
        let lease = fixture
            .service
            .claim_next("event-worker")
            .await
            .expect("claim should succeed")
            .expect("queued run should lease");
        let lease = fixture
            .service
            .start(&lease)
            .await
            .expect("run should start");
        let live = || PreparedLiveDeltaEvent {
            id: Uuid::new_v4(),
            event_type: "provider_live_delta".to_owned(),
            protected_payload: serde_json::json!({}),
            correlation_id: "live-correlation".to_owned(),
            provider_kind: "local_harness".to_owned(),
            provider_model: "event-model".to_owned(),
            budget_reservation_id: Uuid::new_v4(),
            expected_owner_principal_kind: "user".to_owned(),
            expected_owner_subject: "run-user".to_owned(),
            expected_scope_kind: "tenant".to_owned(),
            expected_scope_id: "tenant-run".to_owned(),
            expected_tenant_id: None,
        };
        let activity = || PreparedProviderActivityEvent {
            id: Uuid::new_v4(),
            inbox_event_id: Uuid::new_v4(),
            protected_payload: serde_json::json!({}),
            protected_inbox_payload: serde_json::json!({}),
            correlation_id: "activity-correlation".to_owned(),
            provider_kind: "local_harness".to_owned(),
            provider_model: "event-model".to_owned(),
            budget_reservation_id: Uuid::new_v4(),
            expected_owner_principal_kind: "user".to_owned(),
            expected_owner_subject: "run-user".to_owned(),
            expected_scope_kind: "tenant".to_owned(),
            expected_scope_id: "tenant-run".to_owned(),
            expected_tenant_id: None,
        };
        let ui = || PreparedUiIntentEvent {
            id: Uuid::new_v4(),
            inbox_event_id: Uuid::new_v4(),
            protected_payload: serde_json::json!({}),
            protected_inbox_payload: serde_json::json!({}),
            correlation_id: "ui-correlation".to_owned(),
            provider_kind: "local_harness".to_owned(),
            provider_model: "event-model".to_owned(),
            provider_response_id: None,
            budget_reservation_id: Uuid::new_v4(),
            usage: AiBudgetAmounts::default(),
            cached_input_tokens: 0,
            expected_owner_principal_kind: "user".to_owned(),
            expected_owner_subject: "run-user".to_owned(),
            expected_scope_kind: "tenant".to_owned(),
            expected_scope_id: "tenant-run".to_owned(),
            expected_tenant_id: None,
        };

        // While the epoch is live these writes get past the epoch check and
        // fail on their unrelated, deliberately invalid event bindings.
        let open = [
            fixture
                .service
                .append_live_delta_event(&lease, live())
                .await,
            fixture
                .service
                .append_provider_activity_event(&lease, activity())
                .await,
            fixture
                .service
                .append_ui_intent_event(&lease, ui())
                .await
                .map(|_| ()),
        ];
        for result in open {
            assert!(result.is_err());
            assert!(!matches!(result, Err(AiError::RuntimeNotReady)));
        }

        crate::orm_runtime_recovery::close_test_epoch(&fixture.database).await;
        assert!(matches!(
            fixture
                .service
                .append_live_delta_event(&lease, live())
                .await,
            Err(AiError::RuntimeNotReady)
        ));
        assert!(matches!(
            fixture
                .service
                .append_provider_activity_event(&lease, activity())
                .await,
            Err(AiError::RuntimeNotReady)
        ));
        assert!(matches!(
            fixture.service.append_ui_intent_event(&lease, ui()).await,
            Err(AiError::RuntimeNotReady)
        ));
    }

    #[tokio::test]
    async fn legacy_delegated_queued_runs_fail_closed_at_claim() {
        let mut fixture = fixture().await;
        let ordinary = fixture.principal_reference.clone();
        let mut legacy = Vec::new();
        for (index, (kind, reference)) in
            crate::principal_projection::test_delegated_references(&ordinary)
                .into_iter()
                .enumerate()
        {
            // Legacy rows, inserted directly, as a pre-0.130.0 release
            // persisted them. One is a scheduled retry.
            fixture.principal_reference = reference;
            let run_id = seed_queued(&fixture).await;
            if index == 1 {
                let run = run_record(&fixture, run_id).await;
                AiRunRecord::update_by_id(
                    &fixture.database,
                    &run.id,
                    UpdateAiRunRecordInput {
                        state: Some(AiRunState::RetryScheduled.as_str().to_owned()),
                        retry_count: Some(1),
                        ..Default::default()
                    },
                )
                .await
                .expect("legacy retry should update");
            }
            legacy.push((kind, run_id));
            fixture.clock.advance_seconds(1);
        }
        fixture.principal_reference = ordinary.clone();
        let current = seed_queued(&fixture).await;

        // Rows created in the same second are ordered by ID, so claim until
        // empty: exactly one lease is ever issued, for the ordinary run.
        let mut leases = Vec::new();
        for _ in 0..8 {
            match fixture
                .service
                .claim_next("claim-worker")
                .await
                .expect("claim should succeed")
            {
                Some(lease) => leases.push(lease),
                None => break,
            }
        }
        assert_eq!(leases.len(), 1);
        let lease = leases.remove(0);
        assert_eq!(lease.run_id(), current);
        assert_eq!(lease.principal_reference(), &ordinary);

        let outcomes = attempt_outcomes(&fixture).await;
        let events = session_events(&fixture).await;
        let inbox = inbox_events(&fixture).await;
        let audits = fixture
            .database
            .transaction(TransactionMode::Default, |tx| {
                Box::pin(async move {
                    tx.query::<AiAuditEventRecord>()
                        .limit(32)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await
            .expect("audits should query");
        let code = crate::principal_projection::DELEGATED_REFERENCE_CODE;
        for (kind, run_id) in legacy {
            let run = run_record(&fixture, run_id).await;
            assert_eq!(run.state, AiRunState::Failed.as_str(), "{kind}");
            assert_eq!(run.error_code.as_deref(), Some(code), "{kind}");
            assert!(run.lease_owner.is_none() && run.lease_expires_at.is_none());
            assert!(run.next_attempt_at.is_none(), "{kind} is not rescheduled");
            assert!(
                !provider_dispatch_possible(&fixture, run_id).await,
                "{kind} reserved no provider budget"
            );
            let outcome = outcomes
                .iter()
                .find(|outcome| outcome.run_id == run_id.0)
                .expect("the refused attempt has one outcome");
            assert_eq!(outcome.final_state, AiRunState::Failed.as_str());
            assert_eq!(outcome.outcome_code, code);
            assert!(outcome.provider_response_id.is_none());
            let terminal = events
                .iter()
                .find(|event| event.run_id == Some(run_id.0))
                .expect("a canonical terminal event is written");
            assert_eq!(terminal.event_type, "run_failed", "{kind}");
            let metadata =
                open_terminal_event_metadata(&terminal.event_type, &terminal.protected_payload)
                    .expect("terminal metadata should open")
                    .expect("terminal metadata is content-free");
            assert_eq!(metadata["failure"]["code"], code);
            assert_eq!(metadata["failure"]["retryable"], false);
            assert_eq!(metadata["failure"]["admission"], "refused_uncertain");
            assert_eq!(
                inbox
                    .iter()
                    .filter(|event| event.event_type == "run_failed"
                        && event.session_id == Some(run.session_id))
                    .count(),
                1
            );
            assert!(audits.iter().any(|audit| {
                audit.action == "ai.run.principal_refused"
                    && audit.resource_reference == run_id.0.to_string()
                    && audit.reason_code == code
            }));
            let (tool_calls, approvals) = fixture
                .database
                .transaction(TransactionMode::Default, move |tx| {
                    Box::pin(async move {
                        let calls = tx
                            .query::<AiToolCallRecord>()
                            .filter(AiToolCallRecordWhereInput {
                                run_id: Some(UuidFilter {
                                    eq: Some(run_id.0),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .limit(1)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        let approvals = tx
                            .query::<AiApprovalRecord>()
                            .limit(1)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        Ok((calls, approvals))
                    })
                })
                .await
                .expect("tool and approval rows should query");
            assert!(tool_calls.is_empty() && approvals.is_empty(), "{kind}");
        }
        fixture
            .service
            .heartbeat(&lease)
            .await
            .expect("the ordinary lease remains current");
    }
}

/// Returns whether one run already produced a durable assistant message.
///
/// This is the only evidence allowed to decide "already answered". It reads
/// committed rows in the caller's transaction and never consults worker state,
/// elapsed time, or a provider report.
pub(crate) async fn run_produced_assistant_output(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    session_id: Uuid,
    run_id: Uuid,
) -> Result<bool, OrmPublicError> {
    let rows = tx
        .query::<AiMessageRecord>()
        .filter(AiMessageRecordWhereInput {
            // Scoped by session first so the read stays inside one
            // conversation's indexed keyset rather than scanning globally.
            session_id: Some(UuidFilter {
                eq: Some(session_id),
                ..Default::default()
            }),
            run_id: Some(UuidFilter {
                eq: Some(run_id),
                ..Default::default()
            }),
            message_role: Some(StringFilter {
                eq: Some("assistant".to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        })
        .internal_limit(1)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    Ok(!rows.is_empty())
}

/// Returns whether committed budget rows leave any provider dispatch possible.
///
/// Absence is proof-bearing only when no reservation exists or every exact-run
/// reservation is durably `released`/`expired`. Any other value, including a
/// malformed future state, remains fail-closed as possible dispatch.
pub(crate) async fn run_provider_dispatch_possible(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    run_id: Uuid,
) -> Result<bool, OrmPublicError> {
    let rows = tx
        .query::<AiBudgetReservationRecord>()
        .filter(AiBudgetReservationRecordWhereInput {
            run_id: Some(UuidFilter {
                eq: Some(run_id),
                ..Default::default()
            }),
            state: Some(StringFilter {
                not_in: Some(vec!["released".to_owned(), "expired".to_owned()]),
                ..Default::default()
            }),
            ..Default::default()
        })
        .internal_limit(1)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    Ok(!rows.is_empty())
}

/// Returns whether committed tool-call rows leave a consequential
/// (non-read-only) application tool effect possible for this run.
///
/// Absence is proof-bearing only for calls that never reached execution: a
/// call still awaiting native preparation or approval, a control-blocked call,
/// or a call cancelled while waiting for an approval that was never consumed.
/// Cancellation records an executing or source-retained call as `cancelled`
/// without approval consumption evidence, so such a call stays possible, as
/// does any unknown state or an unbounded call set.
pub(crate) async fn run_consequential_tool_dispatch_possible(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    run_id: Uuid,
) -> Result<bool, OrmPublicError> {
    const MAXIMUM_CALLS: usize = 4_096;
    let calls = tx
        .query::<AiToolCallRecord>()
        .filter(AiToolCallRecordWhereInput {
            run_id: Some(UuidFilter {
                eq: Some(run_id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .internal_limit(MAXIMUM_CALLS as i64 + 1)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    if calls.len() > MAXIMUM_CALLS {
        return Ok(true);
    }
    for call in calls.iter().filter(|call| call.risk != "read_only") {
        let never_dispatched = match call.state.as_str() {
            "control_blocked" | "approval_prepared" | "waiting_approval" => true,
            "cancelled" => match call.approval_id {
                Some(approval_id) => tx
                    .find_by_id::<AiApprovalRecord>(&approval_id)
                    .await
                    .map_err(OrmPublicError::from)?
                    .is_some_and(|approval| {
                        approval.tool_call_id == call.id
                            && approval.consumed_uses == 0
                            && approval.consumed_at.is_none()
                            && approval.state != "consumed"
                    }),
                None => false,
            },
            _ => false,
        };
        if !never_dispatched {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(all(test, feature = "sqlite"))]
#[path = "orm_run_heartbeat_tests.rs"]
mod heartbeat_contention_tests;
