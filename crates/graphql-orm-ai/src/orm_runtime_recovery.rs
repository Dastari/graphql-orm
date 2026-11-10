//! Crate-owned runtime recovery epochs and restore closure.
//!
//! The epoch ledger decides whether this deployment may open the runtime,
//! claim runs, or consume approvals. Its liveness marker is excluded from
//! portable backups, so a restored store is closed until this service records
//! a new epoch.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use std::collections::BTreeMap;
use std::sync::Arc;

use agql_auth::Clock;
use graphql_orm::db::Database;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::filters::StringFilter;
use graphql_orm::graphql::orm::{
    ConditionalUpdateOutcome, DefaultWriteBackend, MutationContext, OrderDirection,
    PaginationConfig, TransactionError, TransactionMode,
};
use uuid::Uuid;

use crate::orm_restore::classify_restored_run;
use crate::orm_runs::{
    append_attempt_outcome, append_terminal_run_event, canonical_second, exact_state,
    lease_from_record,
};
use crate::persistence::{
    AiApprovalRecord, AiApprovalRecordWhereInput, AiAuditEventRecord, AiEgressConsentRecord,
    AiEgressConsentRecordOrderByInput, AiRunRecord, AiRuntimeRecoveryRecord,
    AiRuntimeRecoveryRecordOrderByInput, CreateAiAuditEventRecordInput,
    CreateAiRuntimeRecoveryRecordInput, UpdateAiApprovalRecordInput,
    UpdateAiEgressConsentRecordInput, UpdateAiRunRecordInput,
};
use crate::restore::restored_run_disposition;
use crate::runtime::ai_schema_module_fingerprint;
use crate::{
    AI_SCHEMA_MODULE_VERSION, AiCollectedRestorePlan, AiError, AiRestoreCollectorLimits,
    AiRestoreIssueSeverity, AiRestoreReconciler, AiRestoredRunDisposition, AiRunState,
    AiRuntimeReadinessProof, AiRuntimeStartGate, OrmAiRestoreFactCollector,
};

/// Public message carried by the closed-epoch refusal inside a transaction.
pub(crate) const RUNTIME_EPOCH_NOT_LIVE: &str = "AI runtime recovery epoch is not live";

const EPOCH_STATE_LIVE: &str = "live";
const EPOCH_STATE_BLOCKED: &str = "blocked";
const RECOVERY_ACTOR_KIND: &str = "ai_runtime_recovery";
const RECOVERY_ACTOR_SUBJECT: &str = "runtime-recovery";
const MAXIMUM_OPERATOR_SUBJECT_BYTES: usize = 256;
const MAXIMUM_TRANSACTION_ATTEMPTS: usize = 8;

/// Approval states that still carry reusable authority.
const REUSABLE_APPROVAL_STATES: [&str; 3] = ["pending", "approved", "resume_claimed"];

/// Run states whose lease is held by a worker.
const LEASED_RUN_STATES: [AiRunState; 4] = [
    AiRunState::Leased,
    AiRunState::Running,
    AiRunState::WaitingTool,
    AiRunState::WaitingReauth,
];

/// Non-terminal run states a restore closes. `RecoveryRequired` is already
/// closed and is left unchanged.
const RESTORE_CLOSED_RUN_STATES: [AiRunState; 9] = [
    AiRunState::Queued,
    AiRunState::Leased,
    AiRunState::Running,
    AiRunState::RetryScheduled,
    AiRunState::WaitingApproval,
    AiRunState::WaitingTool,
    AiRunState::WaitingReauth,
    AiRunState::WaitingProvider,
    AiRunState::WaitingSubscription,
];

/// Redacted counts of the closure repairs one epoch transition applied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct AiRuntimeClosureCounts {
    approvals_expired: u64,
    egress_consents_revoked: u64,
    leases_fenced: u64,
    runs_requeued: u64,
    runs_recovery_required: u64,
}

impl AiRuntimeClosureCounts {
    /// Pending, approved, or resume-claimed approvals moved to `expired`.
    pub const fn approvals_expired(self) -> u64 {
        self.approvals_expired
    }

    /// Unrevoked egress consents revoked by a restore.
    pub const fn egress_consents_revoked(self) -> u64 {
        self.egress_consents_revoked
    }

    /// Active leases expired so ordinary lease recovery reclassifies them.
    pub const fn leases_fenced(self) -> u64 {
        self.leases_fenced
    }

    /// Unstarted leased runs returned to the queue for a new attempt.
    pub const fn runs_requeued(self) -> u64 {
        self.runs_requeued
    }

    /// Non-terminal runs moved to `RecoveryRequired` with a new generation.
    pub const fn runs_recovery_required(self) -> u64 {
        self.runs_recovery_required
    }
}

/// Result of applying runtime closure to a restored store.
///
/// The recorded epoch is live only when the collected plan had no fatal
/// issue. While any required audit is incomplete, the epoch is recorded as
/// blocked and no readiness proof is returned.
#[derive(Debug)]
pub struct AiAppliedRestore {
    epoch_id: Uuid,
    plan: AiCollectedRestorePlan,
    closure: AiRuntimeClosureCounts,
    readiness: Option<AiRuntimeReadinessProof>,
}

impl AiAppliedRestore {
    /// Recovery epoch recorded for this restore.
    pub const fn epoch_id(&self) -> Uuid {
        self.epoch_id
    }

    /// Collected plan the closure was applied for.
    pub fn plan(&self) -> &AiCollectedRestorePlan {
        &self.plan
    }

    /// Closure repairs applied in the epoch transaction.
    pub const fn closure(&self) -> AiRuntimeClosureCounts {
        self.closure
    }

    /// Whether the recorded epoch is live and may open the runtime.
    pub const fn is_live(&self) -> bool {
        self.readiness.is_some()
    }

    /// Readiness proof for a live epoch.
    pub fn readiness_proof(&self) -> Option<&AiRuntimeReadinessProof> {
        self.readiness.as_ref()
    }

    /// Consumes the result and returns the readiness proof for a live epoch.
    pub fn into_readiness_proof(self) -> Option<AiRuntimeReadinessProof> {
        self.readiness
    }
}

/// Generated-ORM recovery epoch ledger and restore-closure applier.
///
/// This is the only producer of [`AiRuntimeReadinessProof`]. Every epoch
/// transition runs in one state-machine transaction.
#[derive(Clone)]
pub struct OrmAiRuntimeRecoveryService {
    database: Database<DefaultWriteBackend>,
    clock: Arc<dyn Clock>,
    limits: AiRestoreCollectorLimits,
}

impl OrmAiRuntimeRecoveryService {
    /// Creates a recovery service with conservative default bounds.
    pub fn new(database: Database<DefaultWriteBackend>, clock: Arc<dyn Clock>) -> Self {
        Self {
            database: database.with_pagination_config(PaginationConfig::unbounded()),
            clock,
            limits: AiRestoreCollectorLimits::default(),
        }
    }

    /// Overrides the deployment-owned bounds on rows one closure pass may
    /// touch. Exceeding a bound aborts the transaction and changes nothing.
    #[must_use]
    pub fn with_limits(mut self, limits: AiRestoreCollectorLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Mints the proof that opens `gate`, recording an epoch when needed.
    ///
    /// - A live current epoch for the compiled module returns a proof
    ///   without writing, so ordinary restarts keep their runs and approvals.
    /// - A live epoch for an earlier module fingerprint (an in-place upgrade)
    ///   records a live successor epoch without repairs.
    /// - An empty ledger (a new store, or the first open after upgrading to a
    ///   release with this ledger) records the first live epoch and first
    ///   expires every pending, approved, or resume-claimed approval and
    ///   fences every active lease, because an empty ledger can also come from
    ///   a backup taken before any epoch existed.
    /// - A current epoch that is not live (restored from a backup, or blocked
    ///   by a restore with fatal issues) refuses. Run
    ///   [`Self::reconcile_restore`].
    ///
    /// # Errors
    ///
    /// Returns [`AiError::RuntimeNotReady`] when the current epoch is not
    /// live, [`AiError::InvalidConfiguration`] when `gate` was built for a
    /// different module, and [`AiError::PersistenceFailed`] or
    /// [`AiError::Conflict`] for persistence failure or a bound reached by
    /// the first-epoch closure.
    pub async fn readiness_proof(
        &self,
        gate: &AiRuntimeStartGate,
    ) -> Result<AiRuntimeReadinessProof, AiError> {
        let module_fingerprint = ai_schema_module_fingerprint()?;
        if gate.expected_module_fingerprint() != module_fingerprint {
            return Err(AiError::InvalidConfiguration(
                "runtime start gate belongs to a different AI schema module".to_owned(),
            ));
        }
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.readiness_once(module_fingerprint.clone()).await {
                Ok(result) => return result,
                Err(error) if attempt < MAXIMUM_TRANSACTION_ATTEMPTS && retryable(&error) => {
                    tokio::task::yield_now().await;
                }
                Err(error) => return Err(map_transaction(error)),
            }
        }
    }

    /// Applies runtime closure to a restored store and records a new epoch.
    ///
    /// The collector's audit configuration is re-bound to this service's
    /// database and collected here, so the plan always describes this store.
    /// One transaction then:
    ///
    /// 1. expires every pending, approved, or resume-claimed approval;
    /// 2. revokes every unrevoked egress consent;
    /// 3. returns an unstarted first lease to the queue when both the plan
    ///    and the current row classify it as effect-free, and moves every
    ///    other non-terminal run to `RecoveryRequired` with its lease cleared
    ///    and its lease generation incremented;
    /// 4. verifies that no reusable approval, unrevoked consent, or held
    ///    lease remains; and
    /// 5. records the epoch, live only when the plan has no fatal issue.
    ///
    /// Run it with every worker and writer stopped, after a portable restore
    /// or after any file-level or physical database restore. It never calls a
    /// provider or application tool.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for an invalid fingerprint or
    /// operator subject, [`AiError::Conflict`] when a bound is reached or a
    /// row changed concurrently, and [`AiError::PersistenceFailed`] for
    /// malformed durable state or persistence failure. On error nothing is
    /// committed and the store stays closed.
    pub async fn reconcile_restore(
        &self,
        collector: &OrmAiRestoreFactCollector,
        snapshot_module_fingerprint: impl Into<String>,
        operator_subject: Option<String>,
    ) -> Result<AiAppliedRestore, AiError> {
        if operator_subject.as_deref().is_some_and(|subject| {
            subject.trim().is_empty()
                || subject.len() > MAXIMUM_OPERATOR_SUBJECT_BYTES
                || subject.chars().any(char::is_control)
        }) {
            return Err(AiError::InvalidInput(
                "invalid restore operator subject".to_owned(),
            ));
        }
        let module_fingerprint = ai_schema_module_fingerprint()?;
        let collected = collector
            .rebind(self.database.clone())
            .collect(snapshot_module_fingerprint)
            .await?;
        let plan =
            AiRestoreReconciler::new(module_fingerprint.clone()).plan_collected(&collected)?;
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self
                .reconcile_once(
                    module_fingerprint.clone(),
                    plan.clone(),
                    operator_subject.clone(),
                )
                .await
            {
                Ok(result) => return result,
                Err(error) if attempt < MAXIMUM_TRANSACTION_ATTEMPTS && retryable(&error) => {
                    tokio::task::yield_now().await;
                }
                Err(error) => return Err(map_transaction(error)),
            }
        }
    }

    async fn readiness_once(
        &self,
        module_fingerprint: String,
    ) -> Result<Result<AiRuntimeReadinessProof, AiError>, TransactionError> {
        let now = canonical_second(self.clock.now()).unix_timestamp();
        let limits = self.limits;
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let current = current_epoch(tx).await?;
                    let (sequence, action_counts) = match current {
                        Some(epoch) if !epoch_is_live(&epoch) => {
                            return Ok(Err(AiError::RuntimeNotReady));
                        }
                        Some(epoch) if epoch.module_fingerprint == module_fingerprint => {
                            return Ok(Ok(AiRuntimeReadinessProof::new(
                                module_fingerprint,
                                epoch.id,
                            )));
                        }
                        Some(epoch) => (
                            next_sequence(epoch.epoch_sequence)?,
                            serde_json::json!({
                                "transition": "module_upgrade",
                                "previousEpochId": epoch.id,
                            }),
                        ),
                        None => {
                            let mut counts = AiRuntimeClosureCounts::default();
                            expire_reusable_approvals(tx, limits.maximum_approvals(), &mut counts)
                                .await?;
                            fence_active_leases(tx, limits.maximum_runs(), now, &mut counts)
                                .await?;
                            (
                                1,
                                serde_json::json!({
                                    "transition": "first_epoch",
                                    "closure": counts,
                                }),
                            )
                        }
                    };
                    let epoch_id = Uuid::new_v4();
                    insert_epoch(
                        tx,
                        PreparedEpoch {
                            id: epoch_id,
                            sequence,
                            module_fingerprint: module_fingerprint.clone(),
                            live: true,
                            fatal_issue_count: 0,
                            warning_count: 0,
                            action_counts,
                            operator_subject: None,
                            now,
                        },
                    )
                    .await?;
                    Ok(Ok(AiRuntimeReadinessProof::new(
                        module_fingerprint,
                        epoch_id,
                    )))
                })
            })
            .await
    }

    async fn reconcile_once(
        &self,
        module_fingerprint: String,
        plan: AiCollectedRestorePlan,
        operator_subject: Option<String>,
    ) -> Result<Result<AiAppliedRestore, AiError>, TransactionError> {
        let now = canonical_second(self.clock.now()).unix_timestamp();
        let limits = self.limits;
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let planned = plan
                        .plan()
                        .run_actions
                        .iter()
                        .map(|action| (action.run_id.0, action.disposition))
                        .collect::<BTreeMap<_, _>>();
                    let mut counts = AiRuntimeClosureCounts::default();
                    expire_reusable_approvals(tx, limits.maximum_approvals(), &mut counts).await?;
                    revoke_egress_consents(tx, limits.maximum_egress_consents(), now, &mut counts)
                        .await?;
                    close_restored_runs(tx, limits.maximum_runs(), &planned, now, &mut counts)
                        .await?;
                    verify_closure(tx, limits).await?;

                    let fatal_issue_count = plan.plan().fatal_issue_count();
                    let warning_count = plan
                        .plan()
                        .issues
                        .iter()
                        .filter(|issue| issue.severity == AiRestoreIssueSeverity::Warning)
                        .count() as u64;
                    let live = fatal_issue_count == 0;
                    let sequence = match current_epoch(tx).await? {
                        Some(epoch) => next_sequence(epoch.epoch_sequence)?,
                        None => 1,
                    };
                    let epoch_id = Uuid::new_v4();
                    insert_epoch(
                        tx,
                        PreparedEpoch {
                            id: epoch_id,
                            sequence,
                            module_fingerprint: module_fingerprint.clone(),
                            live,
                            fatal_issue_count,
                            warning_count,
                            action_counts: serde_json::json!({
                                "transition": "restore_reconciled",
                                "factsDigest": plan.facts_digest(),
                                "planDigest": plan.plan_digest(),
                                "closure": counts,
                            }),
                            operator_subject: operator_subject.clone(),
                            now,
                        },
                    )
                    .await?;
                    let readiness =
                        live.then(|| AiRuntimeReadinessProof::new(module_fingerprint, epoch_id));
                    Ok(Ok(AiAppliedRestore {
                        epoch_id,
                        plan,
                        closure: counts,
                        readiness,
                    }))
                })
            })
            .await
    }
}

/// Refuses inside a transaction when the ledger records a current epoch that
/// is not live on this deployment.
///
/// An empty ledger is allowed so stores that have never opened the runtime
/// keep working; opening the runtime records the first epoch and applies the
/// first-epoch closure.
pub(crate) async fn require_live_epoch(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
) -> Result<(), OrmPublicError> {
    match current_epoch(tx).await? {
        Some(epoch) if !epoch_is_live(&epoch) => Err(epoch_not_live()),
        _ => Ok(()),
    }
}

/// Whether a public error is the closed-epoch refusal.
pub(crate) fn is_epoch_not_live(error: &OrmPublicError) -> bool {
    error.code == OrmErrorCode::ServiceUnavailable && error.message == RUNTIME_EPOCH_NOT_LIVE
}

fn epoch_not_live() -> OrmPublicError {
    OrmPublicError::with_message(OrmErrorCode::ServiceUnavailable, RUNTIME_EPOCH_NOT_LIVE)
}

fn epoch_is_live(epoch: &AiRuntimeRecoveryRecord) -> bool {
    epoch.state == EPOCH_STATE_LIVE && epoch.start_gate_open && epoch.live_at.is_some()
}

async fn current_epoch(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
) -> Result<Option<AiRuntimeRecoveryRecord>, OrmPublicError> {
    tx.query::<AiRuntimeRecoveryRecord>()
        .order_by(AiRuntimeRecoveryRecordOrderByInput {
            epoch_sequence: Some(OrderDirection::Desc),
            ..Default::default()
        })
        .limit(1)
        .fetch_one()
        .await
        .map_err(OrmPublicError::from)
}

fn next_sequence(current: i64) -> Result<i64, OrmPublicError> {
    if current < 1 {
        return Err(OrmPublicError::new(OrmErrorCode::InternalError));
    }
    current
        .checked_add(1)
        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))
}

struct PreparedEpoch {
    id: Uuid,
    sequence: i64,
    module_fingerprint: String,
    live: bool,
    fatal_issue_count: u64,
    warning_count: u64,
    action_counts: serde_json::Value,
    operator_subject: Option<String>,
    now: i64,
}

async fn insert_epoch(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    epoch: PreparedEpoch,
) -> Result<(), OrmPublicError> {
    let to_i64 = |value: u64| {
        i64::try_from(value).map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))
    };
    tx.insert::<AiRuntimeRecoveryRecord>(CreateAiRuntimeRecoveryRecordInput {
        id: epoch.id,
        epoch_sequence: epoch.sequence,
        module_version: AI_SCHEMA_MODULE_VERSION.to_owned(),
        module_fingerprint: epoch.module_fingerprint,
        state: if epoch.live {
            EPOCH_STATE_LIVE
        } else {
            EPOCH_STATE_BLOCKED
        }
        .to_owned(),
        start_gate_open: epoch.live,
        live_at: epoch.live.then_some(epoch.now),
        fatal_issue_count: to_i64(epoch.fatal_issue_count)?,
        warning_count: to_i64(epoch.warning_count)?,
        action_counts: epoch.action_counts,
        operator_subject: epoch.operator_subject,
        completed_at: Some(epoch.now),
    })
    .await
    .map_err(OrmPublicError::from)?;
    Ok(())
}

async fn expire_reusable_approvals(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    maximum: usize,
    counts: &mut AiRuntimeClosureCounts,
) -> Result<(), OrmPublicError> {
    for state in REUSABLE_APPROVAL_STATES {
        let approvals = tx
            .query::<AiApprovalRecord>()
            .filter(approval_state(state))
            .limit(bounded_limit(maximum))
            .fetch_all()
            .await
            .map_err(OrmPublicError::from)?;
        if approvals.len() > maximum {
            return Err(bound_reached());
        }
        for approval in approvals {
            let outcome = tx
                .compare_and_swap::<AiApprovalRecord>(
                    &approval.id,
                    approval.row_version,
                    approval_state(state),
                    UpdateAiApprovalRecordInput {
                        state: Some("expired".to_owned()),
                        ..Default::default()
                    },
                )
                .await
                .map_err(OrmPublicError::from)?;
            if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
            }
            tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
                actor_principal_kind: RECOVERY_ACTOR_KIND.to_owned(),
                actor_subject: RECOVERY_ACTOR_SUBJECT.to_owned(),
                action: "ai.approval.expired".to_owned(),
                resource_kind: "ai_approval".to_owned(),
                resource_reference: approval.id.to_string(),
                outcome: "denied".to_owned(),
                reason_code: "approval_expired_by_runtime_recovery".to_owned(),
                correlation_id: approval.id.to_string(),
                causation_id: Some(approval.tool_call_id.to_string()),
                policy_version: Some(approval.policy_version),
            })
            .await
            .map_err(OrmPublicError::from)?;
            counts.approvals_expired = counts.approvals_expired.saturating_add(1);
        }
    }
    Ok(())
}

async fn revoke_egress_consents(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    maximum: usize,
    now: i64,
    counts: &mut AiRuntimeClosureCounts,
) -> Result<(), OrmPublicError> {
    let consents = tx
        .query::<AiEgressConsentRecord>()
        .order_by(AiEgressConsentRecordOrderByInput {
            granted_at: Some(OrderDirection::Asc),
        })
        .limit(bounded_limit(maximum))
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    if consents.len() > maximum {
        return Err(bound_reached());
    }
    for consent in consents
        .into_iter()
        .filter(|consent| consent.revoked_at.is_none())
    {
        let outcome = tx
            .compare_and_swap::<AiEgressConsentRecord>(
                &consent.id,
                consent.row_version,
                Default::default(),
                UpdateAiEgressConsentRecordInput {
                    revoked_at: Some(Some(now.max(consent.granted_at))),
                    ..Default::default()
                },
            )
            .await
            .map_err(OrmPublicError::from)?;
        if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
        tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
            actor_principal_kind: RECOVERY_ACTOR_KIND.to_owned(),
            actor_subject: RECOVERY_ACTOR_SUBJECT.to_owned(),
            action: "ai.egress_consent.revoked".to_owned(),
            resource_kind: "ai_egress_consent".to_owned(),
            resource_reference: consent.id.to_string(),
            outcome: "denied".to_owned(),
            reason_code: "egress_consent_revoked_by_runtime_recovery".to_owned(),
            correlation_id: consent.id.to_string(),
            causation_id: None,
            policy_version: None,
        })
        .await
        .map_err(OrmPublicError::from)?;
        counts.egress_consents_revoked = counts.egress_consents_revoked.saturating_add(1);
    }
    Ok(())
}

/// Expires every active lease at `now` so a stale in-memory fence fails its
/// next row-version check and ordinary lease recovery reclassifies the run.
async fn fence_active_leases(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    maximum: usize,
    now: i64,
    counts: &mut AiRuntimeClosureCounts,
) -> Result<(), OrmPublicError> {
    for state in LEASED_RUN_STATES {
        let runs = tx
            .query::<AiRunRecord>()
            .filter(exact_state(state.as_str()))
            .limit(bounded_limit(maximum))
            .fetch_all()
            .await
            .map_err(OrmPublicError::from)?;
        if runs.len() > maximum {
            return Err(bound_reached());
        }
        for run in runs {
            let Some(expires_at) = run.lease_expires_at else {
                return Err(OrmPublicError::new(OrmErrorCode::InternalError));
            };
            let outcome = tx
                .compare_and_swap::<AiRunRecord>(
                    &run.id,
                    run.row_version,
                    exact_state(&run.state),
                    UpdateAiRunRecordInput {
                        lease_expires_at: Some(Some(expires_at.min(now))),
                        ..Default::default()
                    },
                )
                .await
                .map_err(OrmPublicError::from)?;
            if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
                return Err(OrmPublicError::new(OrmErrorCode::Conflict));
            }
            counts.leases_fenced = counts.leases_fenced.saturating_add(1);
        }
    }
    Ok(())
}

async fn close_restored_runs(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    maximum: usize,
    planned: &BTreeMap<Uuid, AiRestoredRunDisposition>,
    now: i64,
    counts: &mut AiRuntimeClosureCounts,
) -> Result<(), OrmPublicError> {
    let now_time = time::OffsetDateTime::from_unix_timestamp(now)
        .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
    // Read every candidate before changing any row, so a run moved to
    // `RetryScheduled` here is not visited a second time.
    let mut candidates = Vec::new();
    for state in RESTORE_CLOSED_RUN_STATES {
        let mut runs = tx
            .query::<AiRunRecord>()
            .filter(exact_state(state.as_str()))
            .limit(bounded_limit(maximum))
            .fetch_all()
            .await
            .map_err(OrmPublicError::from)?;
        candidates.append(&mut runs);
        if candidates.len() > maximum {
            return Err(bound_reached());
        }
    }
    candidates.sort_by_key(|run| (run.created_at, run.id));
    for run in candidates {
        let restored = classify_restored_run(&run)
            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
        let requeue = planned.get(&run.id)
            == Some(&AiRestoredRunDisposition::RequeueWithNewAttempt)
            && restored_run_disposition(&restored)
                == AiRestoredRunDisposition::RequeueWithNewAttempt;
        if requeue && restored.state == AiRunState::Queued {
            // A pristine queued run holds no attempt or lease; its next
            // claim already creates a new attempt and generation.
            continue;
        }
        if requeue && restored.state == AiRunState::Leased {
            let lease = lease_from_record(&run)?;
            let retry_count = run
                .retry_count
                .checked_add(1)
                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
            let outcome = tx
                .compare_and_swap::<AiRunRecord>(
                    &run.id,
                    run.row_version,
                    exact_state(&run.state),
                    UpdateAiRunRecordInput {
                        state: Some(AiRunState::RetryScheduled.as_str().to_owned()),
                        attempt_id: Some(None),
                        lease_owner: Some(None),
                        lease_expires_at: Some(None),
                        lease_heartbeat_at: Some(None),
                        retry_count: Some(retry_count),
                        next_attempt_at: Some(Some(now)),
                        error_code: Some(Some("restore_requeued_before_start".to_owned())),
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
                "restore_requeued_before_start".to_owned(),
                None,
                now_time,
            )
            .await?;
            counts.runs_requeued = counts.runs_requeued.saturating_add(1);
            continue;
        }

        let active_lease = lease_from_record(&run).ok();
        let generation = run
            .lease_generation
            .checked_add(1)
            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
        let outcome = tx
            .compare_and_swap::<AiRunRecord>(
                &run.id,
                run.row_version,
                exact_state(&run.state),
                UpdateAiRunRecordInput {
                    state: Some(AiRunState::RecoveryRequired.as_str().to_owned()),
                    attempt_id: Some(None),
                    lease_owner: Some(None),
                    lease_generation: Some(generation),
                    lease_expires_at: Some(None),
                    lease_heartbeat_at: Some(None),
                    next_attempt_at: Some(None),
                    error_code: Some(Some("restore_recovery_required".to_owned())),
                    ..Default::default()
                },
            )
            .await
            .map_err(OrmPublicError::from)?;
        let ConditionalUpdateOutcome::Updated(updated) = outcome else {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        };
        if let Some(lease) = active_lease {
            append_attempt_outcome(
                tx,
                &lease,
                AiRunState::RecoveryRequired,
                "restore_recovery_required".to_owned(),
                None,
                now_time,
            )
            .await?;
        }
        append_terminal_run_event(
            tx,
            &updated,
            AiRunState::RecoveryRequired,
            Some("restore_recovery_required"),
            now_time,
        )
        .await?;
        counts.runs_recovery_required = counts.runs_recovery_required.saturating_add(1);
    }
    Ok(())
}

/// Post-apply validation inside the epoch transaction.
async fn verify_closure(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    limits: AiRestoreCollectorLimits,
) -> Result<(), OrmPublicError> {
    for state in REUSABLE_APPROVAL_STATES {
        let remaining = tx
            .query::<AiApprovalRecord>()
            .filter(approval_state(state))
            .limit(1)
            .fetch_all()
            .await
            .map_err(OrmPublicError::from)?;
        if !remaining.is_empty() {
            return Err(OrmPublicError::new(OrmErrorCode::InternalError));
        }
    }
    for state in RESTORE_CLOSED_RUN_STATES {
        let remaining = tx
            .query::<AiRunRecord>()
            .filter(exact_state(state.as_str()))
            .limit(bounded_limit(limits.maximum_runs()))
            .fetch_all()
            .await
            .map_err(OrmPublicError::from)?;
        if remaining.iter().any(|run| {
            run.lease_owner.is_some()
                || run.lease_expires_at.is_some()
                || run.lease_heartbeat_at.is_some()
                || run.attempt_id.is_some()
        }) {
            return Err(OrmPublicError::new(OrmErrorCode::InternalError));
        }
    }
    let consents = tx
        .query::<AiEgressConsentRecord>()
        .order_by(AiEgressConsentRecordOrderByInput {
            granted_at: Some(OrderDirection::Asc),
        })
        .limit(bounded_limit(limits.maximum_egress_consents()))
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    if consents.iter().any(|consent| consent.revoked_at.is_none()) {
        return Err(OrmPublicError::new(OrmErrorCode::InternalError));
    }
    Ok(())
}

fn approval_state(state: &str) -> AiApprovalRecordWhereInput {
    AiApprovalRecordWhereInput {
        state: Some(StringFilter {
            eq: Some(state.to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn bounded_limit(maximum: usize) -> i64 {
    i64::try_from(maximum.saturating_add(1)).unwrap_or(i64::MAX)
}

fn bound_reached() -> OrmPublicError {
    OrmPublicError::with_message(
        OrmErrorCode::Conflict,
        "runtime recovery bound reached; nothing was changed",
    )
}

fn retryable(error: &TransactionError) -> bool {
    matches!(error, TransactionError::Retryable(_))
        || matches!(
            error.public_error().code,
            OrmErrorCode::Conflict | OrmErrorCode::ConstraintViolation
        ) && error.public_error().message != bound_reached().message
}

fn map_transaction(error: TransactionError) -> AiError {
    let public = error.public_error();
    if is_epoch_not_live(public) {
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
    use agql_auth::FixedClock;
    use graphql_orm::graphql::orm::{ApplyOptions, ColumnBackupPolicy, OrmSchemaModule};
    use graphql_orm::prelude::SqliteBackend;
    use time::{Duration, OffsetDateTime};

    use crate::persistence::{CreateAiEgressConsentRecordInput, CreateAiRunRecordInput};
    use crate::{AiRunServiceLimits, OrmAiRunService};

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_900_000_000).expect("fixed timestamp")
    }

    async fn database() -> Database<SqliteBackend> {
        let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .expect("in-memory SQLite should open");
        let plan = database
            .schema()
            .plan_migration_to_entities(
                "ai-runtime-recovery-test-v1",
                "AI runtime recovery test",
                crate::AiSchemaModule.entities(),
            )
            .await
            .expect("AI schema should plan");
        database
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await
            .expect("AI schema should apply");
        database
    }

    fn services(
        database: &Database<SqliteBackend>,
    ) -> (
        OrmAiRuntimeRecoveryService,
        OrmAiRunService,
        AiRuntimeStartGate,
    ) {
        let clock = Arc::new(FixedClock::new(now()));
        let limits = AiRunServiceLimits::new(Duration::hours(1), Duration::hours(1), 16, 2, 8)
            .expect("run limits");
        (
            OrmAiRuntimeRecoveryService::new(database.clone(), clock.clone()),
            OrmAiRunService::new(database.clone(), clock, limits),
            AiRuntimeStartGate::new(ai_schema_module_fingerprint().expect("fingerprint")),
        )
    }

    async fn seed_queued_run(database: &Database<SqliteBackend>) -> Uuid {
        let id = Uuid::new_v4();
        AiRunRecord::insert(
            database,
            CreateAiRunRecordInput {
                execution_selection: None,
                id,
                session_id: Uuid::new_v4(),
                input_message_id: Uuid::new_v4(),
                principal_reference: serde_json::to_value(
                    agql_auth::AuthPrincipal::User(agql_auth::AuthUser {
                        user_id: "recovery-user".to_owned(),
                        session_id: Uuid::new_v4(),
                        roles: vec![],
                        scopes: vec![],
                        session: agql_auth::SessionContext::default(),
                        token_claims: agql_auth::AccessTokenMetadata::default(),
                    })
                    .reference(),
                )
                .expect("principal reference should serialize"),
                state: "queued".to_owned(),
                attempt_id: None,
                lease_owner: None,
                lease_generation: 0,
                lease_expires_at: None,
                lease_heartbeat_at: None,
                retry_count: 0,
                next_attempt_at: None,
                error_code: None,
                latest_checkpoint_id: None,
                cancellation_request_id: None,
                cancellation_requested_at: None,
            },
        )
        .await
        .expect("run should insert");
        id
    }

    async fn seed_consent(database: &Database<SqliteBackend>) -> Uuid {
        AiEgressConsentRecord::insert(
            database,
            CreateAiEgressConsentRecordInput {
                principal_subject: "recovery-test".to_owned(),
                scope_kind: "project".to_owned(),
                scope_id: "project-1".to_owned(),
                tenant_id: None,
                destination: "managed-provider".to_owned(),
                capability: "model-inference".to_owned(),
                purpose: "recovery-test".to_owned(),
                purpose_grant_reference: "grant-1".to_owned(),
                manifest_constraints_hash: "manifest-hash".to_owned(),
                assurance: "standard".to_owned(),
                granted_at: 1_800_000_000,
                expires_at: 2_000_000_000,
                revoked_at: None,
            },
        )
        .await
        .expect("egress consent should insert")
        .id
    }

    #[test]
    fn portable_backups_exclude_the_liveness_marker() {
        let entities = crate::AiSchemaModule.entities().to_vec();
        let descriptors = graphql_orm::graphql::orm::backup_descriptors_from_entities(&entities);
        let ledger = descriptors
            .iter()
            .find(|descriptor| descriptor.table_name == "graphql_orm_ai_runtime_recovery")
            .expect("ledger is part of the AI backup");
        let live_at = ledger
            .columns
            .iter()
            .find(|column| column.column_name == "live_at")
            .expect("liveness column");
        assert_eq!(live_at.backup_policy, ColumnBackupPolicy::Exclude);
    }

    #[tokio::test]
    async fn reconcile_restore_closes_a_live_store_and_requeues_only_unstarted_leases() {
        let database = database().await;
        let (recovery, runs, gate) = services(&database);
        let first = recovery.readiness_proof(&gate).await.expect("first epoch");
        let queued = seed_queued_run(&database).await;
        let leased = seed_queued_run(&database).await;
        let lease = runs
            .claim_next("recovery-worker")
            .await
            .expect("claim")
            .expect("one run should lease");
        let (queued, leased) = if lease.run_id().0 == leased {
            (queued, leased)
        } else {
            (leased, queued)
        };
        let consent = seed_consent(&database).await;

        // A file-level restore would carry the live marker; the operator runs
        // closure explicitly and the store stays closed while audits are
        // incomplete.
        let applied = recovery
            .reconcile_restore(
                &OrmAiRestoreFactCollector::new(database.clone()),
                ai_schema_module_fingerprint().expect("fingerprint"),
                None,
            )
            .await
            .expect("closure should apply");
        assert!(!applied.is_live());
        assert_ne!(applied.epoch_id(), first.epoch_id());
        assert_eq!(applied.closure().runs_requeued(), 1);
        assert_eq!(applied.closure().runs_recovery_required(), 0);
        assert_eq!(applied.closure().egress_consents_revoked(), 1);

        let requeued = AiRunRecord::find_by_id(&database, &leased)
            .await
            .expect("lookup")
            .expect("requeued run");
        assert_eq!(requeued.state, "retry_scheduled");
        assert!(requeued.attempt_id.is_none() && requeued.lease_owner.is_none());
        assert_eq!(
            requeued.error_code.as_deref(),
            Some("restore_requeued_before_start")
        );
        let untouched = AiRunRecord::find_by_id(&database, &queued)
            .await
            .expect("lookup")
            .expect("queued run");
        assert_eq!(untouched.state, "queued");
        assert_eq!(untouched.lease_generation, 0);
        let revoked = AiEgressConsentRecord::find_by_id(&database, &consent)
            .await
            .expect("lookup")
            .expect("consent");
        assert_eq!(revoked.revoked_at, Some(now().unix_timestamp()));

        assert!(matches!(
            recovery.readiness_proof(&gate).await,
            Err(AiError::RuntimeNotReady)
        ));
        assert!(matches!(
            runs.claim_next("recovery-worker").await,
            Err(AiError::RuntimeNotReady)
        ));
        assert!(matches!(
            runs.heartbeat(&lease).await,
            Err(AiError::Conflict)
        ));
    }

    #[tokio::test]
    async fn module_upgrade_records_a_live_successor_without_repairs() {
        let database = database().await;
        let (recovery, _, gate) = services(&database);
        let previous = Uuid::new_v4();
        database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    insert_epoch(
                        tx,
                        PreparedEpoch {
                            id: previous,
                            sequence: 1,
                            module_fingerprint: "previous-module".to_owned(),
                            live: true,
                            fatal_issue_count: 0,
                            warning_count: 0,
                            action_counts: serde_json::json!({}),
                            operator_subject: None,
                            now: 1_800_000_000,
                        },
                    )
                    .await
                })
            })
            .await
            .expect("previous epoch should insert");
        let consent = seed_consent(&database).await;

        let proof = recovery
            .readiness_proof(&gate)
            .await
            .expect("upgrade should record a live successor");
        assert_ne!(proof.epoch_id(), previous);
        let current = AiRuntimeRecoveryRecord::find_by_id(&database, &proof.epoch_id())
            .await
            .expect("lookup")
            .expect("successor epoch");
        assert_eq!(current.epoch_sequence, 2);
        assert!(epoch_is_live(&current));
        let consent = AiEgressConsentRecord::find_by_id(&database, &consent)
            .await
            .expect("lookup")
            .expect("consent");
        assert_eq!(consent.revoked_at, None);
        assert_eq!(
            recovery
                .readiness_proof(&gate)
                .await
                .expect("restart reopens the successor")
                .epoch_id(),
            proof.epoch_id()
        );
    }
}
