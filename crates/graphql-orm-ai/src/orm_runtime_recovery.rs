//! Crate-owned runtime recovery epochs and restore closure.
//!
//! The epoch ledger decides whether this deployment may open the runtime,
//! grant or use run leases, or consume approvals. Its deployment-local marker
//! is excluded from portable backups, so a store restored from a portable
//! backup is closed until this service records a new epoch.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

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
use crate::runtime::ai_schema_module_fingerprint;
use crate::{
    AI_SCHEMA_MODULE_VERSION, AiCollectedRestorePlan, AiError, AiRestoreCollectorLimits,
    AiRestoreIssueSeverity, AiRestoreReconciler, AiRunLease, AiRunState, AiRuntimeReadinessProof,
    AiRuntimeStartGate, OrmAiRestoreFactCollector,
};

/// Public message carried by the closed-epoch refusal inside a transaction.
pub(crate) const RUNTIME_EPOCH_NOT_LIVE: &str = "AI runtime recovery epoch is not live";
const BOUND_REACHED: &str =
    "runtime recovery bound reached; nothing was changed; raise it with with_limits";

const EPOCH_STATE_LIVE: &str = "live";
const EPOCH_STATE_RESTORE_CLOSED: &str = "restore_closed";
const RECOVERY_ACTOR_KIND: &str = "ai_runtime_recovery";
const RECOVERY_ACTOR_SUBJECT: &str = "runtime-recovery";
const MAXIMUM_OPERATOR_SUBJECT_BYTES: usize = 256;
const MAXIMUM_ACKNOWLEDGEMENT_REASON_BYTES: usize = 1_024;
const MAXIMUM_TRANSACTION_ATTEMPTS: usize = 8;

/// Approval states that still carry reusable authority.
const REUSABLE_APPROVAL_STATES: [&str; 3] = ["pending", "approved", "resume_claimed"];

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

    /// Non-terminal runs moved to `RecoveryRequired` with a new generation.
    pub const fn runs_recovery_required(self) -> u64 {
        self.runs_recovery_required
    }

    const fn is_empty(self) -> bool {
        self.approvals_expired == 0
            && self.egress_consents_revoked == 0
            && self.runs_recovery_required == 0
    }
}

/// Result of applying runtime closure to a restored store.
///
/// The recorded restore-closure epoch is never live. Review the plan and the
/// closed runs, then call
/// [`OrmAiRuntimeRecoveryService::acknowledge_reviewed_restore`] with
/// [`Self::epoch_id`] to record a live epoch.
#[derive(Debug)]
pub struct AiAppliedRestore {
    epoch_id: Uuid,
    plan: AiCollectedRestorePlan,
    closure: AiRuntimeClosureCounts,
    repeated: bool,
}

impl AiAppliedRestore {
    /// Restore-closure epoch to acknowledge.
    pub const fn epoch_id(&self) -> Uuid {
        self.epoch_id
    }

    /// Collected plan for operator review. Its fatal issues include audits
    /// this release does not implement.
    pub fn plan(&self) -> &AiCollectedRestorePlan {
        &self.plan
    }

    /// Closure repairs applied by this call.
    pub const fn closure(&self) -> AiRuntimeClosureCounts {
        self.closure
    }

    /// Whether this call found the store already closed by an earlier
    /// reconcile on this deployment, changed nothing, and returned that
    /// epoch instead of recording another.
    pub const fn is_repeat(&self) -> bool {
        self.repeated
    }
}

/// Generated-ORM recovery epoch ledger and restore-closure applier.
///
/// This is the only producer of [`AiRuntimeReadinessProof`]. Every epoch
/// transition runs in one state-machine transaction. Use one service per
/// store, and open each runtime only with a proof from the store its workers
/// use: leases carry the epoch they were granted under, and gate-guarded
/// services refuse a lease whose epoch is not the gate's.
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
    /// touch. Exceeding a bound aborts the transaction, changes nothing, and
    /// returns [`AiError::InvalidConfiguration`].
    #[must_use]
    pub fn with_limits(mut self, limits: AiRestoreCollectorLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Mints the proof that opens a runtime start gate for this store,
    /// recording an epoch when needed.
    ///
    /// - A live current epoch for the compiled module returns a proof
    ///   without writing, so ordinary restarts keep their runs and approvals.
    /// - A live epoch for an earlier module fingerprint (an in-place upgrade)
    ///   records a live successor epoch without repairs.
    /// - An empty ledger on a store with no runs, approvals or egress
    ///   consents (a new store) records the first live epoch.
    /// - An empty ledger on a store that already holds any of them refuses.
    ///   It may be a store upgraded in place from a release without this
    ///   ledger, or a backup taken before any epoch existed. The operator
    ///   decides: [`Self::acknowledge_in_place_upgrade`] for an in-place
    ///   upgrade, or [`Self::reconcile_restore`] and
    ///   [`Self::acknowledge_reviewed_restore`] for a restore.
    /// - A current epoch that is not live refuses. That includes every epoch
    ///   restored from a portable backup and every restore-closure epoch; run
    ///   [`Self::reconcile_restore`] and
    ///   [`Self::acknowledge_reviewed_restore`].
    ///
    /// # Errors
    ///
    /// Returns [`AiError::RuntimeNotReady`] when the current epoch is not
    /// live or an empty ledger sits on existing AI state, and
    /// [`AiError::PersistenceFailed`] or [`AiError::Conflict`] for
    /// persistence failure.
    pub async fn readiness_proof(&self) -> Result<AiRuntimeReadinessProof, AiError> {
        let module_fingerprint = ai_schema_module_fingerprint()?;
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

    /// Mints a proof for this store and opens `gate` with it.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::readiness_proof`] and
    /// [`AiRuntimeStartGate::open`].
    pub async fn open_start_gate(&self, gate: &AiRuntimeStartGate) -> Result<Uuid, AiError> {
        let proof = self.readiness_proof().await?;
        gate.open(&proof)?;
        Ok(proof.epoch_id())
    }

    /// Applies runtime closure to a restored store and records a
    /// restore-closure epoch, which is never live.
    ///
    /// The collector's audit configuration is re-bound to this service's
    /// database and collected here, so the plan always describes this store.
    /// One transaction then:
    ///
    /// 1. expires every pending, approved, or resume-claimed approval;
    /// 2. revokes every unrevoked egress consent;
    /// 3. moves every non-terminal run, including queued and unstarted ones,
    ///    to `RecoveryRequired` with its attempt and lease cleared and its
    ///    lease generation incremented, so operators decide what to retry
    ///    and work that ran after the backup is not repeated automatically;
    /// 4. verifies that no reusable approval, unrevoked consent, or held
    ///    lease remains; and
    /// 5. records the restore-closure epoch, unless the store's current epoch
    ///    is already a restore closure recorded on this deployment and the
    ///    pass changed nothing, in which case it returns that epoch.
    ///
    /// Run it with every worker and writer stopped, after a portable restore
    /// or after any file-level or physical database restore. It never calls a
    /// provider or application tool.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for an invalid fingerprint or
    /// operator subject, [`AiError::InvalidConfiguration`] when a bound is
    /// reached, [`AiError::Conflict`] when a row changed concurrently, and
    /// [`AiError::PersistenceFailed`] for malformed durable state or
    /// persistence failure. On error nothing is committed and the store
    /// stays closed.
    pub async fn reconcile_restore(
        &self,
        collector: &OrmAiRestoreFactCollector,
        snapshot_module_fingerprint: impl Into<String>,
        operator_subject: Option<String>,
    ) -> Result<AiAppliedRestore, AiError> {
        if operator_subject
            .as_deref()
            .is_some_and(|subject| !valid_text(subject, MAXIMUM_OPERATOR_SUBJECT_BYTES))
        {
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

    /// Records an operator's acknowledgement of a reviewed restore and a new
    /// live epoch, then returns its readiness proof.
    ///
    /// `reconciled_epoch_id` must be this store's current epoch, recorded by
    /// [`Self::reconcile_restore`] on this deployment. A restore-closure row
    /// that arrived with a portable backup is refused. The transaction
    /// re-verifies that no reusable approval or held lease exists, then
    /// records the operator subject and reason in the ledger and an
    /// `ai.runtime.restore_acknowledged` audit event.
    ///
    /// The acknowledgement accepts the plan's unresolved fatal issues,
    /// including audits this release does not implement. It does not retry
    /// any closed run: use the run-disposition service for that.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for an empty, oversized, or
    /// control-character subject or reason, [`AiError::Conflict`] when
    /// `reconciled_epoch_id` is not the store's current locally recorded
    /// restore closure for the compiled module, and
    /// [`AiError::PersistenceFailed`] when closure no longer holds or
    /// persistence fails.
    pub async fn acknowledge_reviewed_restore(
        &self,
        reconciled_epoch_id: Uuid,
        operator_subject: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<AiRuntimeReadinessProof, AiError> {
        let operator_subject = operator_subject.into();
        let reason = reason.into();
        if !valid_text(&operator_subject, MAXIMUM_OPERATOR_SUBJECT_BYTES)
            || !valid_text(&reason, MAXIMUM_ACKNOWLEDGEMENT_REASON_BYTES)
        {
            return Err(AiError::InvalidInput(
                "invalid restore acknowledgement".to_owned(),
            ));
        }
        let module_fingerprint = ai_schema_module_fingerprint()?;
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self
                .acknowledge_once(
                    module_fingerprint.clone(),
                    reconciled_epoch_id,
                    operator_subject.clone(),
                    reason.clone(),
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

    /// Records an operator's acknowledgement that this store was upgraded in
    /// place, not restored, and records its first live epoch.
    ///
    /// An existing store upgraded to this release starts with an empty
    /// ledger over existing runs, approvals or consents, which
    /// [`Self::readiness_proof`] cannot tell apart from a backup taken before
    /// any epoch existed. The operator tells the crate which it is. Use this
    /// only for a store that was never restored: choosing it for a restored
    /// store reopens approvals consumed after the backup. Use
    /// [`Self::reconcile_restore`] and [`Self::acknowledge_reviewed_restore`]
    /// for a restored store.
    ///
    /// One transaction requires an empty ledger, expires every lease that was
    /// granted before epochs existed so its worker re-claims through ordinary
    /// lease recovery, and records the first live epoch with the operator
    /// subject and reason, plus an `ai.runtime.in_place_upgrade_acknowledged`
    /// audit event. Runs, approvals and egress consents are otherwise left
    /// unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for an empty, oversized, or
    /// control-character subject or reason, [`AiError::Conflict`] when the
    /// ledger already has any row, [`AiError::InvalidConfiguration`] when the
    /// lease pass reaches its bound, and [`AiError::PersistenceFailed`] for
    /// persistence failure.
    pub async fn acknowledge_in_place_upgrade(
        &self,
        operator_subject: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<AiRuntimeReadinessProof, AiError> {
        let operator_subject = operator_subject.into();
        let reason = reason.into();
        if !valid_text(&operator_subject, MAXIMUM_OPERATOR_SUBJECT_BYTES)
            || !valid_text(&reason, MAXIMUM_ACKNOWLEDGEMENT_REASON_BYTES)
        {
            return Err(AiError::InvalidInput(
                "invalid in-place upgrade acknowledgement".to_owned(),
            ));
        }
        let module_fingerprint = ai_schema_module_fingerprint()?;
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self
                .acknowledge_upgrade_once(
                    module_fingerprint.clone(),
                    operator_subject.clone(),
                    reason.clone(),
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
                            // An empty ledger with existing AI state may be a
                            // backup taken before any epoch existed. Only a
                            // store with no runs, approvals or consents opens
                            // without reconcile and acknowledgement.
                            if holds_prior_ai_state(tx).await? {
                                return Ok(Err(AiError::RuntimeNotReady));
                            }
                            (1, serde_json::json!({"transition": "first_epoch"}))
                        }
                    };
                    let epoch_id = Uuid::new_v4();
                    insert_epoch(
                        tx,
                        PreparedEpoch {
                            id: epoch_id,
                            sequence,
                            module_fingerprint: module_fingerprint.clone(),
                            state: EPOCH_STATE_LIVE,
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
                    let mut counts = AiRuntimeClosureCounts::default();
                    expire_reusable_approvals(tx, limits.maximum_approvals(), &mut counts).await?;
                    revoke_egress_consents(tx, limits.maximum_egress_consents(), now, &mut counts)
                        .await?;
                    close_restored_runs(tx, limits.maximum_runs(), now, &mut counts).await?;
                    verify_closure(tx, limits, true).await?;

                    let current = current_epoch(tx).await?;
                    if let Some(epoch) = &current
                        && counts.is_empty()
                        && epoch_is_local_restore_closure(epoch)
                        && epoch.module_fingerprint == module_fingerprint
                    {
                        return Ok(Ok(AiAppliedRestore {
                            epoch_id: epoch.id,
                            plan,
                            closure: counts,
                            repeated: true,
                        }));
                    }

                    let fatal_issue_count = plan.plan().fatal_issue_count();
                    let warning_count = plan
                        .plan()
                        .issues
                        .iter()
                        .filter(|issue| issue.severity == AiRestoreIssueSeverity::Warning)
                        .count() as u64;
                    let sequence = match &current {
                        Some(epoch) => next_sequence(epoch.epoch_sequence)?,
                        None => 1,
                    };
                    let epoch_id = Uuid::new_v4();
                    insert_epoch(
                        tx,
                        PreparedEpoch {
                            id: epoch_id,
                            sequence,
                            module_fingerprint,
                            state: EPOCH_STATE_RESTORE_CLOSED,
                            fatal_issue_count,
                            warning_count,
                            action_counts: serde_json::json!({
                                "transition": "restore_reconciled",
                                "factsDigest": plan.facts_digest(),
                                "planDigest": plan.plan_digest(),
                                "closure": counts,
                            }),
                            operator_subject,
                            now,
                        },
                    )
                    .await?;
                    Ok(Ok(AiAppliedRestore {
                        epoch_id,
                        plan,
                        closure: counts,
                        repeated: false,
                    }))
                })
            })
            .await
    }

    async fn acknowledge_upgrade_once(
        &self,
        module_fingerprint: String,
        operator_subject: String,
        reason: String,
    ) -> Result<Result<AiRuntimeReadinessProof, AiError>, TransactionError> {
        let now = canonical_second(self.clock.now()).unix_timestamp();
        let limits = self.limits;
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    if current_epoch(tx).await?.is_some() {
                        return Ok(Err(AiError::Conflict));
                    }
                    let leases_fenced =
                        fence_epochless_leases(tx, limits.maximum_runs(), now).await?;
                    let epoch_id = Uuid::new_v4();
                    insert_epoch(
                        tx,
                        PreparedEpoch {
                            id: epoch_id,
                            sequence: 1,
                            module_fingerprint: module_fingerprint.clone(),
                            state: EPOCH_STATE_LIVE,
                            fatal_issue_count: 0,
                            warning_count: 0,
                            action_counts: serde_json::json!({
                                "transition": "in_place_upgrade_acknowledged",
                                "leasesFenced": leases_fenced,
                                "reason": reason,
                            }),
                            operator_subject: Some(operator_subject.clone()),
                            now,
                        },
                    )
                    .await?;
                    tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
                        actor_principal_kind: "operator".to_owned(),
                        actor_subject: operator_subject,
                        action: "ai.runtime.in_place_upgrade_acknowledged".to_owned(),
                        resource_kind: "ai_runtime_recovery".to_owned(),
                        resource_reference: epoch_id.to_string(),
                        outcome: "allowed".to_owned(),
                        reason_code: "in_place_upgrade_acknowledged".to_owned(),
                        correlation_id: epoch_id.to_string(),
                        causation_id: None,
                        policy_version: None,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    Ok(Ok(AiRuntimeReadinessProof::new(
                        module_fingerprint,
                        epoch_id,
                    )))
                })
            })
            .await
    }

    async fn acknowledge_once(
        &self,
        module_fingerprint: String,
        reconciled_epoch_id: Uuid,
        operator_subject: String,
        reason: String,
    ) -> Result<Result<AiRuntimeReadinessProof, AiError>, TransactionError> {
        let now = canonical_second(self.clock.now()).unix_timestamp();
        let limits = self.limits;
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let Some(current) = current_epoch(tx).await? else {
                        return Ok(Err(AiError::Conflict));
                    };
                    if current.id != reconciled_epoch_id
                        || !epoch_is_local_restore_closure(&current)
                        || current.module_fingerprint != module_fingerprint
                    {
                        return Ok(Err(AiError::Conflict));
                    }
                    verify_closure(tx, limits, false).await?;
                    let epoch_id = Uuid::new_v4();
                    insert_epoch(
                        tx,
                        PreparedEpoch {
                            id: epoch_id,
                            sequence: next_sequence(current.epoch_sequence)?,
                            module_fingerprint: module_fingerprint.clone(),
                            state: EPOCH_STATE_LIVE,
                            fatal_issue_count: 0,
                            warning_count: 0,
                            action_counts: serde_json::json!({
                                "transition": "restore_acknowledged",
                                "reconciledEpochId": current.id,
                                "reconciledFatalIssueCount": current.fatal_issue_count,
                                "reason": reason,
                            }),
                            operator_subject: Some(operator_subject.clone()),
                            now,
                        },
                    )
                    .await?;
                    tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
                        actor_principal_kind: "operator".to_owned(),
                        actor_subject: operator_subject,
                        action: "ai.runtime.restore_acknowledged".to_owned(),
                        resource_kind: "ai_runtime_recovery".to_owned(),
                        resource_reference: epoch_id.to_string(),
                        outcome: "allowed".to_owned(),
                        reason_code: "restore_acknowledged".to_owned(),
                        correlation_id: current.id.to_string(),
                        causation_id: Some(current.id.to_string()),
                        policy_version: None,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    Ok(Ok(AiRuntimeReadinessProof::new(
                        module_fingerprint,
                        epoch_id,
                    )))
                })
            })
            .await
    }
}

/// Requires a live current epoch inside a transaction and returns its ID.
///
/// An empty ledger is not live: nothing may be claimed or consumed until
/// [`OrmAiRuntimeRecoveryService::readiness_proof`] records the first epoch.
pub(crate) async fn require_live_epoch(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
) -> Result<Uuid, OrmPublicError> {
    match current_epoch(tx).await? {
        Some(epoch) if epoch_is_live(&epoch) => Ok(epoch.id),
        _ => Err(epoch_not_live()),
    }
}

/// Whether `gate` is open for the epoch `lease` was granted under.
///
/// Combined with the in-transaction lease validation, which requires the
/// lease's epoch to be the store's current live epoch, this binds an open
/// gate to the store whose live epoch minted its proof.
pub(crate) fn gate_admits_lease(gate: &AiRuntimeStartGate, lease: &AiRunLease) -> bool {
    gate.is_ready()
        && lease.recovery_epoch_id().is_some()
        && gate.epoch_id() == lease.recovery_epoch_id()
}

/// Requires, inside a transaction, that `gate` is open for this store's
/// current live epoch. Used by gate-guarded store paths that hold no run
/// lease.
#[cfg_attr(not(feature = "provider-openai-core"), allow(dead_code))]
pub(crate) async fn require_gate_epoch(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    gate_epoch_id: Option<Uuid>,
) -> Result<(), OrmPublicError> {
    let live = require_live_epoch(tx).await?;
    if gate_epoch_id != Some(live) {
        return Err(epoch_not_live());
    }
    Ok(())
}

/// Whether a public error is the closed-epoch refusal.
pub(crate) fn is_epoch_not_live(error: &OrmPublicError) -> bool {
    error.code == OrmErrorCode::ServiceUnavailable && error.message == RUNTIME_EPOCH_NOT_LIVE
}

fn epoch_not_live() -> OrmPublicError {
    OrmPublicError::with_message(OrmErrorCode::ServiceUnavailable, RUNTIME_EPOCH_NOT_LIVE)
}

/// Whether a ledger position can belong to a recorded epoch. Recorded epochs
/// start at 1, and `i64::MAX` could never be followed by a successor, so a
/// row outside that range (the column default 0, or a crafted value) is never
/// live or acknowledgeable.
const fn valid_epoch_sequence(sequence: i64) -> bool {
    sequence >= 1 && sequence < i64::MAX
}

fn epoch_is_live(epoch: &AiRuntimeRecoveryRecord) -> bool {
    valid_epoch_sequence(epoch.epoch_sequence)
        && epoch.state == EPOCH_STATE_LIVE
        && epoch.start_gate_open
        && epoch.recorded_locally_at.is_some()
}

fn epoch_is_local_restore_closure(epoch: &AiRuntimeRecoveryRecord) -> bool {
    valid_epoch_sequence(epoch.epoch_sequence)
        && epoch.state == EPOCH_STATE_RESTORE_CLOSED
        && !epoch.start_gate_open
        && epoch.recorded_locally_at.is_some()
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

/// Whether the store already holds runs, approvals or egress consents.
async fn holds_prior_ai_state(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
) -> Result<bool, OrmPublicError> {
    let runs = tx
        .query::<AiRunRecord>()
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
    let consents = tx
        .query::<AiEgressConsentRecord>()
        .limit(1)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    Ok(!runs.is_empty() || !approvals.is_empty() || !consents.is_empty())
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
    state: &'static str,
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
        state: epoch.state.to_owned(),
        start_gate_open: epoch.state == EPOCH_STATE_LIVE,
        recorded_locally_at: Some(epoch.now),
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

/// Expires every lease that carries no recovery epoch, so a stale in-memory
/// fence fails and ordinary lease recovery reclassifies the run. A lease row
/// with no expiry is fenced at `now` too. Returns the number fenced.
async fn fence_epochless_leases(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    maximum: usize,
    now: i64,
) -> Result<u64, OrmPublicError> {
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
    let mut fenced = 0_u64;
    for run in candidates {
        if run.recovery_epoch_id.is_some()
            || (run.lease_owner.is_none() && run.lease_expires_at.is_none())
        {
            continue;
        }
        let expires_at = run.lease_expires_at.map_or(now, |value| value.min(now));
        let outcome = tx
            .compare_and_swap::<AiRunRecord>(
                &run.id,
                run.row_version,
                exact_state(&run.state),
                UpdateAiRunRecordInput {
                    lease_expires_at: Some(Some(expires_at)),
                    ..Default::default()
                },
            )
            .await
            .map_err(OrmPublicError::from)?;
        if !matches!(outcome, ConditionalUpdateOutcome::Updated(_)) {
            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
        }
        fenced = fenced.saturating_add(1);
    }
    Ok(fenced)
}

/// Moves every non-terminal run to `RecoveryRequired` under a new lease
/// generation. Every candidate is read before any row changes.
async fn close_restored_runs(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    maximum: usize,
    now: i64,
    counts: &mut AiRuntimeClosureCounts,
) -> Result<(), OrmPublicError> {
    let now_time = time::OffsetDateTime::from_unix_timestamp(now)
        .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
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
        if run.lease_generation < 0 {
            return Err(OrmPublicError::new(OrmErrorCode::InternalError));
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
                    recovery_epoch_id: Some(None),
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

/// Post-apply validation inside the epoch transaction. Consents are checked
/// only right after closure; a consent granted after it is a fresh decision.
async fn verify_closure(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    limits: AiRestoreCollectorLimits,
    include_consents: bool,
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
    if include_consents {
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
    OrmPublicError::with_message(OrmErrorCode::Conflict, BOUND_REACHED)
}

fn retryable(error: &TransactionError) -> bool {
    let public = error.public_error();
    public.message != BOUND_REACHED
        && (matches!(error, TransactionError::Retryable(_))
            || matches!(
                public.code,
                OrmErrorCode::Conflict | OrmErrorCode::ConstraintViolation
            ))
}

fn map_transaction(error: TransactionError) -> AiError {
    let public = error.public_error();
    if is_epoch_not_live(public) {
        return AiError::RuntimeNotReady;
    }
    if public.message == BOUND_REACHED {
        return AiError::InvalidConfiguration(BOUND_REACHED.to_owned());
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

/// Records the first live epoch for a test store and returns its proof.
#[cfg(all(test, feature = "sqlite"))]
pub(crate) async fn open_test_epoch(
    database: &Database<DefaultWriteBackend>,
) -> AiRuntimeReadinessProof {
    OrmAiRuntimeRecoveryService::new(database.clone(), Arc::new(agql_auth::SystemClock))
        .readiness_proof()
        .await
        .expect("a test store should record its first live epoch")
}

/// Appends a non-live epoch to a test store without changing any other row.
#[cfg(all(test, feature = "sqlite"))]
pub(crate) async fn close_test_epoch(database: &Database<DefaultWriteBackend>) {
    database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let sequence = match current_epoch(tx).await? {
                    Some(epoch) => next_sequence(epoch.epoch_sequence)?,
                    None => 1,
                };
                insert_epoch(
                    tx,
                    PreparedEpoch {
                        id: Uuid::new_v4(),
                        sequence,
                        module_fingerprint: ai_schema_module_fingerprint()
                            .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?,
                        state: EPOCH_STATE_RESTORE_CLOSED,
                        fatal_issue_count: 0,
                        warning_count: 0,
                        action_counts: serde_json::json!({}),
                        operator_subject: None,
                        now: 1,
                    },
                )
                .await
            })
        })
        .await
        .expect("a closed test epoch should insert");
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;
    use agql_auth::FixedClock;
    use graphql_orm::graphql::orm::{ApplyOptions, ColumnBackupPolicy, Entity, OrmSchemaModule};
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

    fn principal_reference() -> agql_auth::PrincipalReference {
        agql_auth::AuthPrincipal::User(agql_auth::AuthUser {
            user_id: "recovery-user".to_owned(),
            session_id: Uuid::new_v4(),
            roles: vec![],
            scopes: vec![],
            session: agql_auth::SessionContext::default(),
            token_claims: agql_auth::AccessTokenMetadata::default(),
        })
        .reference()
    }

    async fn seed_session(
        database: &Database<SqliteBackend>,
        reference: &agql_auth::PrincipalReference,
    ) -> Uuid {
        let session_id = Uuid::new_v4();
        crate::persistence::AiSessionRecord::insert(
            database,
            crate::persistence::CreateAiSessionRecordInput {
                execution_selection: None,
                id: session_id,
                owner_principal_kind: "user".to_owned(),
                owner_subject: reference.subject.clone(),
                tenant_id: None,
                scope_kind: "project".to_owned(),
                scope_id: "project-1".to_owned(),
                title: "Recovery test".to_owned(),
                title_revision: 0,
                title_source: "default".to_owned(),
                state: "active".to_owned(),
                stream_head: 0,
                message_head: 0,
                last_activity_at: 1_800_000_000,
                archived_at: None,
                deleted_at: None,
            },
        )
        .await
        .expect("session should insert");
        session_id
    }

    async fn seed_run(database: &Database<SqliteBackend>, state: &str) -> Uuid {
        let reference = principal_reference();
        let session_id = seed_session(database, &reference).await;
        let id = Uuid::new_v4();
        let leased = state != "queued";
        AiRunRecord::insert(
            database,
            CreateAiRunRecordInput {
                recovery_epoch_id: None,
                execution_selection: None,
                id,
                session_id,
                input_message_id: Uuid::new_v4(),
                principal_reference: serde_json::to_value(&reference)
                    .expect("principal reference should serialize"),
                state: state.to_owned(),
                attempt_id: leased.then(Uuid::new_v4),
                lease_owner: leased.then(|| "legacy-worker".to_owned()),
                lease_generation: i64::from(leased),
                lease_expires_at: None,
                lease_heartbeat_at: leased.then_some(1_800_000_000),
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

    async fn run(database: &Database<SqliteBackend>, id: Uuid) -> AiRunRecord {
        AiRunRecord::find_by_id(database, &id)
            .await
            .expect("run lookup")
            .expect("run")
    }

    async fn reconcile(recovery: &OrmAiRuntimeRecoveryService) -> AiAppliedRestore {
        recovery
            .reconcile_restore(
                &OrmAiRestoreFactCollector::new(recovery.database.clone()),
                ai_schema_module_fingerprint().expect("fingerprint"),
                Some("restore-operator".to_owned()),
            )
            .await
            .expect("closure should apply")
    }

    /// The 0.127.0 shape of the run table, before `recovery_epoch_id`.
    #[derive(
        graphql_orm::prelude::GraphQLEntity,
        graphql_orm::prelude::GraphQLOperations,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
        PartialEq,
    )]
    #[graphql_entity(
        table = "graphql_orm_ai_runs",
        plural = "LegacyGraphqlOrmAiRuns",
        default_sort = "created_at ASC"
    )]
    struct LegacyAiRunRecord {
        pub execution_selection: Option<String>,
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "uuid")]
        pub id: graphql_orm::uuid::Uuid,
        #[filterable(type = "uuid")]
        pub session_id: graphql_orm::uuid::Uuid,
        #[filterable(type = "uuid")]
        pub input_message_id: graphql_orm::uuid::Uuid,
        #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
        pub principal_reference: serde_json::Value,
        #[filterable(type = "string")]
        pub state: String,
        pub attempt_id: Option<graphql_orm::uuid::Uuid>,
        pub lease_owner: Option<String>,
        pub lease_generation: i64,
        pub lease_expires_at: Option<i64>,
        pub lease_heartbeat_at: Option<i64>,
        pub retry_count: i64,
        pub next_attempt_at: Option<i64>,
        pub error_code: Option<String>,
        pub latest_checkpoint_id: Option<graphql_orm::uuid::Uuid>,
        pub cancellation_request_id: Option<graphql_orm::uuid::Uuid>,
        pub cancellation_requested_at: Option<i64>,
        #[sortable]
        pub created_at: i64,
        #[graphql_orm(version, default = "0")]
        pub row_version: i64,
    }

    /// The 0.127.0 shape of the recovery table, which that release never wrote.
    #[derive(
        graphql_orm::prelude::GraphQLEntity,
        graphql_orm::prelude::GraphQLOperations,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
        PartialEq,
    )]
    #[graphql_entity(
        table = "graphql_orm_ai_runtime_recovery",
        plural = "LegacyGraphqlOrmAiRuntimeRecovery",
        default_sort = "created_at DESC"
    )]
    struct LegacyAiRuntimeRecoveryRecord {
        #[primary_key]
        #[filterable(type = "uuid")]
        pub id: graphql_orm::uuid::Uuid,
        pub module_version: String,
        pub module_fingerprint: String,
        pub state: String,
        pub start_gate_open: bool,
        pub fatal_issue_count: i64,
        pub warning_count: i64,
        #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
        pub action_counts: serde_json::Value,
        pub operator_subject: Option<String>,
        #[sortable]
        pub created_at: i64,
        pub completed_at: Option<i64>,
        #[graphql_orm(version, default = "0")]
        pub row_version: i64,
    }

    #[tokio::test]
    async fn a_prior_schema_store_with_runs_upgrades_and_opens() {
        let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .expect("in-memory SQLite should open");
        let module = crate::AiSchemaModule;
        let mut prior_entities = module
            .entities()
            .iter()
            .copied()
            .filter(|entity| {
                !matches!(
                    entity.table_name,
                    "graphql_orm_ai_runs" | "graphql_orm_ai_runtime_recovery"
                )
            })
            .collect::<Vec<_>>();
        prior_entities.push(LegacyAiRunRecord::metadata());
        prior_entities.push(LegacyAiRuntimeRecoveryRecord::metadata());
        let prior_plan = database
            .schema()
            .plan_migration_to_entities("ai-recovery-prior-v1", "prior AI schema", &prior_entities)
            .await
            .expect("prior AI schema should plan");
        database
            .schema()
            .apply_migration(&prior_plan, ApplyOptions::default())
            .await
            .expect("prior AI schema should apply");
        let legacy_run = Uuid::new_v4();
        let legacy_reference = principal_reference();
        let legacy_session = seed_session(&database, &legacy_reference).await;
        LegacyAiRunRecord::insert(
            &database,
            CreateLegacyAiRunRecordInput {
                execution_selection: None,
                id: legacy_run,
                session_id: legacy_session,
                input_message_id: Uuid::new_v4(),
                principal_reference: serde_json::to_value(&legacy_reference)
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
        .expect("a prior-schema run should insert");

        let current_plan = database
            .schema()
            .plan_migration_to_entities(
                "ai-recovery-current-v1",
                "current AI schema",
                module.entities(),
            )
            .await
            .expect("current AI schema should plan");
        database
            .schema()
            .apply_migration(&current_plan, ApplyOptions::default())
            .await
            .expect("the recovery ledger and run epoch columns should migrate in place");
        let repeated = database
            .schema()
            .plan_migration_to_entities(
                "ai-recovery-current-v2",
                "current AI schema",
                module.entities(),
            )
            .await
            .expect("current AI schema should replan");
        assert!(repeated.steps.is_empty() && repeated.statements.is_empty());

        // The upgraded store already holds runs. The operator records that it
        // was upgraded in place, so its runs stay as they were.
        let (recovery, runs, gate) = services(&database);
        assert!(matches!(
            recovery.open_start_gate(&gate).await,
            Err(AiError::RuntimeNotReady)
        ));
        let proof = recovery
            .acknowledge_in_place_upgrade("upgrade-operator", "upgraded 0.127.0 in place")
            .await
            .expect("an in-place upgrade should open the store");
        gate.open(&proof).expect("the first epoch opens the gate");
        assert_eq!(run(&database, legacy_run).await.state, "queued");
        let lease = runs
            .claim_next("upgraded-worker")
            .await
            .expect("claim")
            .expect("the prior-schema run should lease");
        assert_eq!(lease.run_id().0, legacy_run);
        assert!(gate_admits_lease(&gate, &lease));
        let duplicate = database
            .transaction(TransactionMode::StateMachine, |tx| {
                Box::pin(async move {
                    let current = current_epoch(tx).await?.expect("current epoch");
                    insert_epoch(
                        tx,
                        PreparedEpoch {
                            id: Uuid::new_v4(),
                            sequence: current.epoch_sequence,
                            module_fingerprint: current.module_fingerprint,
                            state: EPOCH_STATE_LIVE,
                            fatal_issue_count: 0,
                            warning_count: 0,
                            action_counts: serde_json::json!({}),
                            operator_subject: None,
                            now: 1,
                        },
                    )
                    .await
                })
            })
            .await;
        assert!(duplicate.is_err(), "epoch sequences must stay unique");
    }

    #[test]
    fn portable_backups_exclude_the_deployment_local_marker() {
        let entities = crate::AiSchemaModule.entities().to_vec();
        let descriptors = graphql_orm::graphql::orm::backup_descriptors_from_entities(&entities);
        let ledger = descriptors
            .iter()
            .find(|descriptor| descriptor.table_name == "graphql_orm_ai_runtime_recovery")
            .expect("ledger is part of the AI backup");
        let marker = ledger
            .columns
            .iter()
            .find(|column| column.column_name == "recorded_locally_at")
            .expect("deployment-local marker column");
        assert_eq!(marker.backup_policy, ColumnBackupPolicy::Exclude);
    }

    #[tokio::test]
    async fn an_empty_ledger_refuses_claims_and_lease_recovery() {
        let database = database().await;
        let (recovery, runs, gate) = services(&database);
        assert!(matches!(
            runs.claim_next("recovery-worker").await,
            Err(AiError::RuntimeNotReady)
        ));
        assert!(matches!(
            runs.claim_next_approved("recovery-worker").await,
            Err(AiError::RuntimeNotReady)
        ));
        assert!(matches!(
            runs.recover_expired_leases().await,
            Err(AiError::RuntimeNotReady)
        ));
        recovery
            .open_start_gate(&gate)
            .await
            .expect("first epoch should open the gate");
        seed_run(&database, "queued").await;
        let lease = runs
            .claim_next("recovery-worker")
            .await
            .expect("claim")
            .expect("queued run should lease");
        assert_eq!(lease.recovery_epoch_id(), gate.epoch_id());
        assert!(gate_admits_lease(&gate, &lease));
    }

    async fn seed_approval(database: &Database<SqliteBackend>) -> Uuid {
        let id = Uuid::new_v4();
        AiApprovalRecord::insert(
            database,
            crate::persistence::CreateAiApprovalRecordInput {
                id,
                tool_call_id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                principal_subject: "recovery-user".to_owned(),
                principal_reference_fingerprint: "principal-fingerprint".to_owned(),
                delegated_actor_subject: None,
                delegation_reference: None,
                argument_hash: "argument-hash".to_owned(),
                tool_fingerprint: "tool-fingerprint".to_owned(),
                binding_hash: "binding-hash".to_owned(),
                execution_target_id: "local-application".to_owned(),
                target_schema_fingerprint: "schema-fingerprint".to_owned(),
                operation_name: "RecoveryTest".to_owned(),
                operation_document_hash: "operation-hash".to_owned(),
                result_projection_fingerprint: "projection-fingerprint".to_owned(),
                disclosure_schema_fingerprint: "disclosure-fingerprint".to_owned(),
                policy_version: "policy-v1".to_owned(),
                authorization_state_digest: "authorization-digest".to_owned(),
                protected_resource_bindings: None,
                protected_action_preview: None,
                payload_purged_at: None,
                action_preview_hash: "preview-hash".to_owned(),
                state: "approved".to_owned(),
                recent_mfa_required: false,
                approver_subject: Some("approver".to_owned()),
                expires_at: 2_000_000_000,
                decided_at: Some(1_850_000_000),
                maximum_uses: 1,
                consumed_uses: 0,
                consumed_at: None,
            },
        )
        .await
        .expect("approval should insert");
        id
    }

    #[tokio::test]
    async fn in_place_upgrade_opens_an_existing_store_without_closing_its_work() {
        let database = database().await;
        let (recovery, runs, gate) = services(&database);
        let queued = seed_run(&database, "queued").await;
        let running = seed_run(&database, "running").await;
        let approval = seed_approval(&database).await;
        let consent = seed_consent(&database).await;
        assert!(matches!(
            recovery.readiness_proof().await,
            Err(AiError::RuntimeNotReady)
        ));
        assert!(matches!(
            recovery
                .acknowledge_in_place_upgrade("upgrade-operator", "\u{7}")
                .await,
            Err(AiError::InvalidInput(_))
        ));

        let proof = recovery
            .acknowledge_in_place_upgrade("upgrade-operator", "upgraded 0.127.0 in place")
            .await
            .expect("an in-place upgrade should open the store");
        gate.open(&proof).expect("the first epoch opens the gate");

        // Runs, approvals and consents are untouched; only the lease granted
        // before epochs existed is fenced for ordinary recovery.
        let queued_row = run(&database, queued).await;
        assert_eq!(queued_row.state, "queued");
        assert_eq!(queued_row.lease_expires_at, None);
        let running_row = run(&database, running).await;
        assert_eq!(running_row.state, "running");
        assert_eq!(running_row.lease_expires_at, Some(now().unix_timestamp()));
        assert_eq!(
            AiApprovalRecord::find_by_id(&database, &approval)
                .await
                .expect("lookup")
                .expect("approval")
                .state,
            "approved"
        );
        assert_eq!(
            AiEgressConsentRecord::find_by_id(&database, &consent)
                .await
                .expect("lookup")
                .expect("consent")
                .revoked_at,
            None
        );
        let epoch = AiRuntimeRecoveryRecord::find_by_id(&database, &proof.epoch_id())
            .await
            .expect("lookup")
            .expect("first epoch");
        assert!(epoch_is_live(&epoch));
        assert_eq!(epoch.operator_subject.as_deref(), Some("upgrade-operator"));
        assert_eq!(epoch.action_counts["reason"], "upgraded 0.127.0 in place");
        assert_eq!(epoch.action_counts["leasesFenced"], 1);

        assert_eq!(
            recovery
                .readiness_proof()
                .await
                .expect("the acknowledged epoch is live")
                .epoch_id(),
            proof.epoch_id()
        );
        assert!(matches!(
            recovery
                .acknowledge_in_place_upgrade("upgrade-operator", "again")
                .await,
            Err(AiError::Conflict)
        ));
        let lease = runs
            .claim_next("upgraded-worker")
            .await
            .expect("claim")
            .expect("the queued run should lease");
        assert_eq!(lease.run_id().0, queued);
        assert!(gate_admits_lease(&gate, &lease));

        // The fenced pre-epoch lease recovers through the ordinary path.
        let report = runs
            .recover_expired_leases()
            .await
            .expect("fenced leases should recover through the ordinary path");
        assert_eq!(report.recovery_required, 1);
        let recovered = run(&database, running).await;
        assert_eq!(recovered.state, "recovery_required");
        assert!(recovered.lease_owner.is_none() && recovered.lease_expires_at.is_none());
    }

    #[tokio::test]
    async fn in_place_upgrade_is_refused_once_the_ledger_has_any_row() {
        let fresh = database().await;
        let (recovery, _, _) = services(&fresh);
        recovery.readiness_proof().await.expect("a new store opens");
        assert!(matches!(
            recovery
                .acknowledge_in_place_upgrade("upgrade-operator", "upgraded in place")
                .await,
            Err(AiError::Conflict)
        ));

        let restored = database().await;
        let (recovery, _, _) = services(&restored);
        seed_run(&restored, "queued").await;
        reconcile(&recovery).await;
        assert!(matches!(
            recovery
                .acknowledge_in_place_upgrade("upgrade-operator", "upgraded in place")
                .await,
            Err(AiError::Conflict)
        ));
        assert!(matches!(
            recovery.readiness_proof().await,
            Err(AiError::RuntimeNotReady)
        ));
    }

    #[tokio::test]
    async fn an_empty_ledger_on_existing_state_requires_reconcile_and_acknowledgement() {
        let database = database().await;
        let (recovery, runs, gate) = services(&database);
        // Leased rows without an expiry, as a stripped or pre-epoch snapshot
        // could carry them.
        let first = seed_run(&database, "running").await;
        let second = seed_run(&database, "leased").await;
        let consent = seed_consent(&database).await;
        assert!(matches!(
            recovery.readiness_proof().await,
            Err(AiError::RuntimeNotReady)
        ));
        assert!(matches!(
            runs.claim_next("recovery-worker").await,
            Err(AiError::RuntimeNotReady)
        ));

        let small = AiRestoreCollectorLimits::new(1, 1, 1).expect("small limits");
        assert!(matches!(
            recovery
                .clone()
                .with_limits(small)
                .reconcile_restore(
                    &OrmAiRestoreFactCollector::new(recovery.database.clone()),
                    ai_schema_module_fingerprint().expect("fingerprint"),
                    None,
                )
                .await,
            Err(AiError::InvalidConfiguration(_))
        ));
        assert_eq!(run(&database, first).await.state, "running");

        let applied = reconcile(&recovery).await;
        assert_eq!(applied.closure().runs_recovery_required(), 2);
        assert_eq!(applied.closure().egress_consents_revoked(), 1);
        for id in [first, second] {
            assert_eq!(run(&database, id).await.state, "recovery_required");
        }
        let revoked = AiEgressConsentRecord::find_by_id(&database, &consent)
            .await
            .expect("lookup")
            .expect("consent");
        assert!(revoked.revoked_at.is_some());
        let proof = recovery
            .acknowledge_reviewed_restore(applied.epoch_id(), "restore-operator", "reviewed")
            .await
            .expect("the reviewed closure should be acknowledged");
        gate.open(&proof)
            .expect("the acknowledged epoch opens the gate");
    }

    #[tokio::test]
    async fn reconcile_restore_closes_every_run_and_repeats_as_a_no_op() {
        let database = database().await;
        let (recovery, runs, gate) = services(&database);
        recovery.open_start_gate(&gate).await.expect("first epoch");
        let queued = seed_run(&database, "queued").await;
        let leased = seed_run(&database, "queued").await;
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
        let leased_generation = run(&database, leased).await.lease_generation;

        // A file-level restore would carry the deployment-local marker; the
        // operator runs closure explicitly and the store closes.
        let applied = reconcile(&recovery).await;
        assert!(!applied.is_repeat());
        assert_ne!(Some(applied.epoch_id()), gate.epoch_id());
        assert_eq!(applied.closure().runs_recovery_required(), 2);
        assert_eq!(applied.closure().egress_consents_revoked(), 1);

        for id in [queued, leased] {
            let closed = run(&database, id).await;
            assert_eq!(closed.state, "recovery_required");
            assert!(closed.attempt_id.is_none() && closed.lease_owner.is_none());
            assert!(closed.recovery_epoch_id.is_none());
        }
        assert_eq!(
            run(&database, leased).await.lease_generation,
            leased_generation + 1
        );
        let revoked = AiEgressConsentRecord::find_by_id(&database, &consent)
            .await
            .expect("lookup")
            .expect("consent");
        assert_eq!(revoked.revoked_at, Some(now().unix_timestamp()));

        assert!(matches!(
            recovery.readiness_proof().await,
            Err(AiError::RuntimeNotReady)
        ));
        assert!(matches!(
            runs.claim_next("recovery-worker").await,
            Err(AiError::RuntimeNotReady)
        ));
        assert!(matches!(
            runs.heartbeat(&lease).await,
            Err(AiError::RuntimeNotReady)
        ));

        let repeated = reconcile(&recovery).await;
        assert!(repeated.is_repeat());
        assert_eq!(repeated.epoch_id(), applied.epoch_id());
        assert_eq!(repeated.closure(), AiRuntimeClosureCounts::default());
        assert_eq!(run(&database, queued).await.state, "recovery_required");
    }

    #[tokio::test]
    async fn acknowledged_restore_reopens_only_after_local_closure() {
        let database = database().await;
        let (recovery, runs, gate) = services(&database);
        let first = recovery.readiness_proof().await.expect("first epoch");

        // Acknowledgement needs a current, locally recorded restore closure.
        assert!(matches!(
            recovery
                .acknowledge_reviewed_restore(first.epoch_id(), "operator", "reviewed")
                .await,
            Err(AiError::Conflict)
        ));
        let closed_run = seed_run(&database, "queued").await;
        let applied = reconcile(&recovery).await;
        assert!(matches!(
            recovery
                .acknowledge_reviewed_restore(Uuid::new_v4(), "operator", "reviewed")
                .await,
            Err(AiError::Conflict)
        ));
        assert!(matches!(
            recovery
                .acknowledge_reviewed_restore(applied.epoch_id(), "operator", " ")
                .await,
            Err(AiError::InvalidInput(_))
        ));

        let proof = recovery
            .acknowledge_reviewed_restore(
                applied.epoch_id(),
                "restore-operator",
                "reviewed closed runs after restore",
            )
            .await
            .expect("a reviewed local closure should be acknowledged");
        gate.open(&proof)
            .expect("the acknowledged epoch opens the gate");
        assert_eq!(
            recovery
                .readiness_proof()
                .await
                .expect("the acknowledged epoch is live")
                .epoch_id(),
            proof.epoch_id()
        );
        let acknowledged = AiRuntimeRecoveryRecord::find_by_id(&database, &proof.epoch_id())
            .await
            .expect("lookup")
            .expect("acknowledged epoch");
        assert_eq!(
            acknowledged.operator_subject.as_deref(),
            Some("restore-operator")
        );
        assert_eq!(
            acknowledged.action_counts["reason"],
            "reviewed closed runs after restore"
        );
        assert_eq!(
            acknowledged.action_counts["reconciledEpochId"],
            serde_json::json!(applied.epoch_id())
        );

        // Closed runs stay closed; only new work is claimable.
        assert_eq!(run(&database, closed_run).await.state, "recovery_required");
        let fresh = seed_run(&database, "queued").await;
        let lease = runs
            .claim_next("recovery-worker")
            .await
            .expect("claim")
            .expect("new work should be claimable");
        assert_eq!(lease.run_id().0, fresh);
        assert!(gate_admits_lease(&gate, &lease));
    }

    #[tokio::test]
    async fn a_restored_closure_row_cannot_be_acknowledged() {
        let database = database().await;
        let (recovery, _, _) = services(&database);
        let restored = Uuid::new_v4();
        database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    tx.insert::<AiRuntimeRecoveryRecord>(CreateAiRuntimeRecoveryRecordInput {
                        id: restored,
                        epoch_sequence: 1,
                        module_version: AI_SCHEMA_MODULE_VERSION.to_owned(),
                        module_fingerprint: ai_schema_module_fingerprint().expect("fingerprint"),
                        state: EPOCH_STATE_RESTORE_CLOSED.to_owned(),
                        start_gate_open: false,
                        recorded_locally_at: None,
                        fatal_issue_count: 1,
                        warning_count: 0,
                        action_counts: serde_json::json!({}),
                        operator_subject: None,
                        completed_at: Some(1_800_000_000),
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    Ok(())
                })
            })
            .await
            .expect("restored ledger row should insert");
        assert!(matches!(
            recovery
                .acknowledge_reviewed_restore(restored, "operator", "reviewed")
                .await,
            Err(AiError::Conflict)
        ));
        assert!(matches!(
            recovery.readiness_proof().await,
            Err(AiError::RuntimeNotReady)
        ));
    }

    #[tokio::test]
    async fn a_proof_from_another_store_does_not_admit_its_leases() {
        let store = database().await;
        let other = database().await;
        let (recovery, runs, gate) = services(&store);
        let (other_recovery, _, _) = services(&other);
        recovery.readiness_proof().await.expect("store epoch");
        other_recovery
            .open_start_gate(&gate)
            .await
            .expect("the other store's proof opens the gate");
        seed_run(&store, "queued").await;
        let lease = runs
            .claim_next("recovery-worker")
            .await
            .expect("claim")
            .expect("store run should lease");
        assert!(gate.is_ready());
        assert!(!gate_admits_lease(&gate, &lease));

        let gate_epoch = gate.epoch_id();
        let refused = store
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move { require_gate_epoch(tx, gate_epoch).await })
            })
            .await;
        assert!(matches!(refused, Err(error) if is_epoch_not_live(error.public_error())));
    }

    #[tokio::test]
    async fn module_upgrade_records_a_live_successor_without_repairs() {
        let database = database().await;
        let (recovery, _, _) = services(&database);
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
                            state: EPOCH_STATE_LIVE,
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
            .readiness_proof()
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
                .readiness_proof()
                .await
                .expect("restart reopens the successor")
                .epoch_id(),
            proof.epoch_id()
        );
    }
}
