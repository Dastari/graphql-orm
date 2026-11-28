use std::time::Duration;

use super::{
    AiRestorePlanTelemetry, AiRestoreReadinessTelemetry, AiRetentionTelemetry,
    AiRunRecoveryTelemetry, AiRunTransitionTelemetry, AiTelemetryOperationId,
};
use crate::{
    AiRestorePlan, AiRestoredRunDisposition, AiRunState, AiRuntimeReadinessReport,
    AiSessionRetentionReport,
};

impl AiRunTransitionTelemetry {
    /// Records an authoritative durable transition without resource identity.
    ///
    /// Restore/recovery transitions may intentionally differ from ordinary
    /// worker state-machine edges, so this observation does not authorize or
    /// validate the transition.
    pub const fn new(
        operation_id: AiTelemetryOperationId,
        from: AiRunState,
        to: AiRunState,
    ) -> Self {
        Self {
            operation_id,
            from,
            to,
        }
    }

    /// Telemetry-only operation correlation.
    pub const fn operation_id(self) -> AiTelemetryOperationId {
        self.operation_id
    }

    /// Prior durable state.
    pub const fn from(self) -> AiRunState {
        self.from
    }

    /// Committed durable state.
    pub const fn to(self) -> AiRunState {
        self.to
    }
}

impl AiRunRecoveryTelemetry {
    /// Builds an aggregate from one completed bounded recovery pass.
    pub fn from_report(
        operation_id: AiTelemetryOperationId,
        duration: Duration,
        report: &crate::AiRunRecoveryReport,
    ) -> Self {
        Self {
            operation_id,
            duration,
            requeued: u64::from(report.requeued),
            checkpoint_requeued: u64::from(report.checkpoint_requeued),
            recovery_required: u64::from(report.recovery_required),
            failed: u64::from(report.failed),
            completed: u64::from(report.completed),
            quarantined: u64::from(report.quarantined),
            skipped: u64::from(report.skipped),
            budget_reservations_settled: u64::from(report.budget_reservations_settled),
        }
    }

    /// Telemetry-only operation correlation.
    pub const fn operation_id(self) -> AiTelemetryOperationId {
        self.operation_id
    }

    /// Completed pass duration.
    pub const fn duration(self) -> Duration {
        self.duration
    }

    /// Safely requeued pre-provider claims.
    pub const fn requeued(self) -> u64 {
        self.requeued
    }

    /// Safely requeued complete checkpoint claims.
    pub const fn checkpoint_requeued(self) -> u64 {
        self.checkpoint_requeued
    }

    /// Claims moved to manual recovery.
    pub const fn recovery_required(self) -> u64 {
        self.recovery_required
    }

    /// Claims failed after exhausting safe retry.
    pub const fn failed(self) -> u64 {
        self.failed
    }

    /// Claims finalized from exact durable output evidence.
    pub const fn completed(self) -> u64 {
        self.completed
    }

    /// Expired claims with malformed durable state, closed for manual recovery.
    pub const fn quarantined(self) -> u64 {
        self.quarantined
    }

    /// Rows or reservations left unchanged for a later pass after a failure.
    pub const fn skipped(self) -> u64 {
        self.skipped
    }

    /// Closed-attempt budget reservations settled at their reserved estimate.
    pub const fn budget_reservations_settled(self) -> u64 {
        self.budget_reservations_settled
    }
}

impl AiRetentionTelemetry {
    /// Projects one retention report without copying its opaque cursor.
    pub fn from_report(
        operation_id: AiTelemetryOperationId,
        duration: Duration,
        report: &AiSessionRetentionReport,
    ) -> Self {
        let rows_deleted = [
            report.live_delta_events_deleted,
            report.deleting_session_events_deleted,
            report.deleting_session_context_checkpoints_deleted,
            report.context_checkpoints_invalidated,
            report.expired_run_checkpoints_deleted,
            report.deleting_session_run_checkpoints_deleted,
            report.message_blocks_deleted,
            report.deleting_session_attachments_deleted,
            report.deleting_session_attachment_artifacts_deleted,
        ]
        .into_iter()
        .map(u64::from)
        .sum();
        let payloads_tombstoned = [
            report.deleting_session_inbox_payloads_purged,
            report.deleting_session_proposal_payloads_purged,
            report.deleting_session_tool_payloads_purged,
            report.deleting_session_approval_payloads_purged,
            report.expired_tool_payloads_purged,
            report.expired_approval_payloads_purged,
        ]
        .into_iter()
        .map(u64::from)
        .sum();
        let cleanups_requested = [
            report.deleting_session_attachment_cleanups_requested,
            report.deleting_session_attachment_artifact_cleanups_requested,
            report.deleting_session_run_checkpoint_references_cleared,
        ]
        .into_iter()
        .map(u64::from)
        .sum();
        let blocked = [
            report.sessions_not_ready,
            report.messages_blocked,
            report.attachment_cleanups_blocked,
            report.proposal_payload_purges_blocked,
            report.tool_payload_purges_blocked,
            report.raw_payload_purges_blocked,
            report.raw_checkpoint_purges_blocked,
            report.run_checkpoint_purges_blocked,
        ]
        .into_iter()
        .map(u64::from)
        .sum();
        Self {
            operation_id,
            duration,
            sessions_scanned: u64::from(report.sessions_scanned),
            sessions_changed: u64::from(report.sessions_changed),
            sessions_finalized: u64::from(report.deleting_sessions_finalized),
            rows_deleted,
            payloads_tombstoned,
            messages_scrubbed: u64::from(report.message_contents_purged),
            cleanups_requested,
            blocked,
            conflicted: u64::from(report.sessions_conflicted),
            failed: u64::from(report.sessions_failed),
            has_more: report.next_session_cursor.is_some(),
        }
    }

    /// Telemetry-only operation correlation.
    pub const fn operation_id(self) -> AiTelemetryOperationId {
        self.operation_id
    }

    /// Completed pass duration.
    pub const fn duration(self) -> Duration {
        self.duration
    }

    /// Sessions whose pruning failed and is retried by a later pass.
    pub const fn failed(self) -> u64 {
        self.failed
    }

    /// Sessions scanned.
    pub const fn sessions_scanned(self) -> u64 {
        self.sessions_scanned
    }

    /// Sessions changed.
    pub const fn sessions_changed(self) -> u64 {
        self.sessions_changed
    }

    /// Sessions finalized as dependency-proved deleted shells.
    pub const fn sessions_finalized(self) -> u64 {
        self.sessions_finalized
    }

    /// Protected/metadata rows physically deleted or invalidated.
    pub const fn rows_deleted(self) -> u64 {
        self.rows_deleted
    }

    /// Protected payload fields tombstoned in retained rows.
    pub const fn payloads_tombstoned(self) -> u64 {
        self.payloads_tombstoned
    }

    /// Message previews scrubbed.
    pub const fn messages_scrubbed(self) -> u64 {
        self.messages_scrubbed
    }

    /// External cleanup or checkpoint-reference cleanup work requested.
    pub const fn cleanups_requested(self) -> u64 {
        self.cleanups_requested
    }

    /// Safe blockers observed across proof categories.
    pub const fn blocked(self) -> u64 {
        self.blocked
    }

    /// Sessions whose CAS transaction conflicted.
    pub const fn conflicted(self) -> u64 {
        self.conflicted
    }

    /// Whether the bounded scan returned an opaque next-page cursor.
    pub const fn has_more(self) -> bool {
        self.has_more
    }
}

impl AiRestorePlanTelemetry {
    /// Projects a restore plan without fingerprints, issue text, or run IDs.
    pub fn from_plan(
        operation_id: AiTelemetryOperationId,
        duration: Duration,
        plan: &AiRestorePlan,
    ) -> Self {
        let mut preserved_terminal = 0_u64;
        let mut requeued = 0_u64;
        let mut recovery_required = 0_u64;
        for action in &plan.run_actions {
            match action.disposition {
                AiRestoredRunDisposition::PreserveTerminal => preserved_terminal += 1,
                AiRestoredRunDisposition::RequeueWithNewAttempt => requeued += 1,
                AiRestoredRunDisposition::RecoveryRequired => recovery_required += 1,
            }
        }
        let fatal_issues = plan.fatal_issue_count();
        Self {
            operation_id,
            duration,
            preserved_terminal,
            requeued,
            recovery_required,
            approvals_to_revalidate: plan.approvals_to_revalidate,
            consents_to_revalidate: plan.consents_to_revalidate,
            fatal_issues,
            warning_issues: u64::try_from(plan.issues.len())
                .unwrap_or(u64::MAX)
                .saturating_sub(fatal_issues),
        }
    }

    /// Telemetry-only operation correlation.
    pub const fn operation_id(self) -> AiTelemetryOperationId {
        self.operation_id
    }

    /// Completed planning duration.
    pub const fn duration(self) -> Duration {
        self.duration
    }

    /// Terminal runs preserved.
    pub const fn preserved_terminal(self) -> u64 {
        self.preserved_terminal
    }

    /// Runs safe to requeue under a new fence.
    pub const fn requeued(self) -> u64 {
        self.requeued
    }

    /// Runs requiring manual recovery.
    pub const fn recovery_required(self) -> u64 {
        self.recovery_required
    }

    /// Approval rows requiring current-policy revalidation.
    pub const fn approvals_to_revalidate(self) -> u64 {
        self.approvals_to_revalidate
    }

    /// Egress-consent rows requiring current-policy revalidation.
    pub const fn consents_to_revalidate(self) -> u64 {
        self.consents_to_revalidate
    }

    /// Fatal restore issues.
    pub const fn fatal_issues(self) -> u64 {
        self.fatal_issues
    }

    /// Nonfatal restore warnings.
    pub const fn warning_issues(self) -> u64 {
        self.warning_issues
    }
}

impl AiRestoreReadinessTelemetry {
    /// Projects readiness without copying the module fingerprint.
    pub fn from_report(
        operation_id: AiTelemetryOperationId,
        report: &AiRuntimeReadinessReport,
    ) -> Self {
        Self {
            operation_id,
            executor_bound: report.executor_bound,
            restore_reconciled: report.restore_reconciled,
            fatal_issue_count: report.fatal_issue_count,
            non_fingerprint_requirements_ready: report.executor_bound
                && report.restore_reconciled
                && report.fatal_issue_count == 0,
        }
    }

    /// Telemetry-only operation correlation.
    pub const fn operation_id(self) -> AiTelemetryOperationId {
        self.operation_id
    }

    /// Whether the finished host GraphQL executor was bound.
    pub const fn executor_bound(self) -> bool {
        self.executor_bound
    }

    /// Whether restore reconciliation completed.
    pub const fn restore_reconciled(self) -> bool {
        self.restore_reconciled
    }

    /// Fatal issue count.
    pub const fn fatal_issue_count(self) -> u64 {
        self.fatal_issue_count
    }

    /// Content-free readiness evaluation, excluding the fingerprint comparison.
    ///
    /// The authoritative start gate separately compares the exact private
    /// module fingerprint; this convenience value is not startup authority.
    pub const fn non_fingerprint_requirements_ready(self) -> bool {
        self.non_fingerprint_requirements_ready
    }
}
