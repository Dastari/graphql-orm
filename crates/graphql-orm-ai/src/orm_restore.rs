//! Bounded database-derived restore fact collection.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use std::collections::BTreeMap;

use graphql_orm::db::Database;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::orm::{
    DefaultWriteBackend, OrderDirection, PaginationConfig, TransactionError, TransactionMode,
};
use sha2::{Digest, Sha256};

use crate::persistence::{
    AiApprovalRecord, AiApprovalRecordOrderByInput, AiEgressConsentRecord,
    AiEgressConsentRecordOrderByInput, AiRunRecord, AiRunRecordOrderByInput,
};
use crate::{
    AiCollectedRestoreFacts, AiError, AiExternalEffectState, AiRestoreAuditKind,
    AiRestoreAuditStatus, AiRestoreSnapshotFacts, AiRestoredCoordinatorCheckpoint, AiRestoredRun,
    AiRunId, AiRunState,
};

const MAXIMUM_COLLECTION_BOUND: usize = 1_000_000;
/// Deployment-owned hard bounds for one restore fact collection pass.
///
/// A reached bound is reported as a fatal incomplete audit; rows are never
/// silently omitted from a readiness-eligible fact set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRestoreCollectorLimits {
    maximum_runs: usize,
    maximum_approvals: usize,
    maximum_egress_consents: usize,
}

impl AiRestoreCollectorLimits {
    /// Creates validated collection bounds.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] when any bound is zero or
    /// exceeds the compiled one-million-row ceiling.
    pub fn new(
        maximum_runs: usize,
        maximum_approvals: usize,
        maximum_egress_consents: usize,
    ) -> Result<Self, AiError> {
        if [maximum_runs, maximum_approvals, maximum_egress_consents]
            .into_iter()
            .any(|bound| !(1..=MAXIMUM_COLLECTION_BOUND).contains(&bound))
        {
            return Err(AiError::InvalidConfiguration(
                "invalid restore collector limits".to_owned(),
            ));
        }
        Ok(Self {
            maximum_runs,
            maximum_approvals,
            maximum_egress_consents,
        })
    }

    /// Maximum run rows read by one pass.
    pub const fn maximum_runs(self) -> usize {
        self.maximum_runs
    }

    /// Maximum approval rows read by one pass.
    pub const fn maximum_approvals(self) -> usize {
        self.maximum_approvals
    }

    /// Maximum egress-consent rows read by one pass.
    pub const fn maximum_egress_consents(self) -> usize {
        self.maximum_egress_consents
    }
}

impl Default for AiRestoreCollectorLimits {
    fn default() -> Self {
        Self {
            maximum_runs: 10_000,
            maximum_approvals: 10_000,
            maximum_egress_consents: 10_000,
        }
    }
}

/// Generated-ORM restore fact collector.
///
/// Collection runs in one read transaction, performs no provider, tool, blob,
/// or application I/O, and marks every audit not implemented by this version
/// as incomplete. The resulting opaque facts can create only a dry-run plan.
#[derive(Clone)]
pub struct OrmAiRestoreFactCollector {
    database: Database<DefaultWriteBackend>,
    limits: AiRestoreCollectorLimits,
}

impl OrmAiRestoreFactCollector {
    /// Creates a collector with conservative default bounds.
    pub fn new(database: Database<DefaultWriteBackend>) -> Self {
        Self {
            database,
            limits: AiRestoreCollectorLimits::default(),
        }
    }

    /// Overrides deployment-owned collection bounds.
    #[must_use]
    pub fn with_limits(mut self, limits: AiRestoreCollectorLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Returns the ORM database handle used by the collector.
    pub fn database(&self) -> &Database<DefaultWriteBackend> {
        &self.database
    }

    /// Collects redacted restore facts from the restored database.
    ///
    /// `snapshot_module_fingerprint` must come from the already verified
    /// backup manifest. It is recorded for the pure reconciler's exact module
    /// comparison but is not trusted as applied-restore evidence.
    ///
    /// This initial collector completely covers conservative run
    /// classification and approval/consent revalidation-candidate counts. It
    /// does not claim that the candidate rows have passed the later complete
    /// repair and validation graph. All remaining audit categories are
    /// explicitly `not_implemented`, so the collected plan remains fatal and
    /// cannot be mistaken for production readiness.
    ///
    /// The runtime and all database writers must remain closed for the whole
    /// collection pass. The single transaction bounds the operation but does
    /// not replace that restore-quiescence requirement.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for an empty or oversized module
    /// fingerprint and [`AiError::PersistenceFailed`] when the bounded ORM
    /// snapshot cannot be read or deterministically hashed.
    pub async fn collect(
        &self,
        snapshot_module_fingerprint: impl Into<String>,
    ) -> Result<AiCollectedRestoreFacts, AiError> {
        let module_fingerprint = snapshot_module_fingerprint.into();
        if module_fingerprint.is_empty() || module_fingerprint.len() > 256 {
            return Err(AiError::InvalidInput(
                "invalid restore module fingerprint".to_owned(),
            ));
        }

        let limits = self.limits;
        let database = self
            .database
            .clone()
            .with_pagination_config(PaginationConfig::unbounded());
        let collected = database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let runs = tx
                        .query::<AiRunRecord>()
                        .order_by(AiRunRecordOrderByInput {
                            created_at: Some(OrderDirection::Asc),
                        })
                        .limit(query_limit(limits.maximum_runs))
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    let approvals = tx
                        .query::<AiApprovalRecord>()
                        .order_by(AiApprovalRecordOrderByInput {
                            created_at: Some(OrderDirection::Asc),
                        })
                        .limit(query_limit(limits.maximum_approvals))
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    let egress_consents = tx
                        .query::<AiEgressConsentRecord>()
                        .order_by(AiEgressConsentRecordOrderByInput {
                            granted_at: Some(OrderDirection::Asc),
                        })
                        .limit(query_limit(limits.maximum_egress_consents))
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    Ok(CollectedRows {
                        runs,
                        approvals,
                        egress_consents,
                    })
                })
            })
            .await
            .map_err(map_transaction)?;

        collected.into_facts(module_fingerprint, limits)
    }
}

struct CollectedRows {
    runs: Vec<AiRunRecord>,
    approvals: Vec<AiApprovalRecord>,
    egress_consents: Vec<AiEgressConsentRecord>,
}

impl CollectedRows {
    fn into_facts(
        mut self,
        module_fingerprint: String,
        limits: AiRestoreCollectorLimits,
    ) -> Result<AiCollectedRestoreFacts, AiError> {
        let mut statuses = AiRestoreAuditKind::required()
            .iter()
            .copied()
            .map(|audit| (audit, AiRestoreAuditStatus::NotImplemented))
            .collect::<BTreeMap<_, _>>();

        // The query may cut through equal timestamp values, but an overflowed
        // category is discarded in full. Only complete result sets reach the
        // canonical in-memory `(timestamp, id)` ordering and evidence digest.
        let runs_truncated = self.runs.len() > limits.maximum_runs;
        if runs_truncated {
            self.runs.clear();
        }
        self.runs.sort_by_key(|run| (run.created_at, run.id));
        let run_evidence = self
            .runs
            .iter()
            .map(|run| (run.id, run.row_version, run.state.clone()))
            .collect::<Vec<_>>();
        let mut invalid_runs = 0_u64;
        let mut runs = Vec::with_capacity(self.runs.len());
        for run in self.runs {
            let Some(state) = AiRunState::from_persisted(&run.state) else {
                invalid_runs += 1;
                continue;
            };
            if run.retry_count < 0 || run.lease_generation < 0 {
                invalid_runs += 1;
                continue;
            }
            let external_effect = match state {
                AiRunState::Queued
                    if run.attempt_id.is_none()
                        && run.lease_owner.is_none()
                        && run.lease_generation == 0
                        && run.lease_expires_at.is_none()
                        && run.lease_heartbeat_at.is_none()
                        && run.retry_count == 0
                        && run.latest_checkpoint_id.is_none() =>
                {
                    AiExternalEffectState::None
                }
                AiRunState::Leased
                    if run.attempt_id.is_some()
                        && run.lease_owner.is_some()
                        && run.lease_generation == 1
                        && run.lease_expires_at.is_some()
                        && run.lease_heartbeat_at.is_some()
                        && run.retry_count == 0
                        && run.latest_checkpoint_id.is_none() =>
                {
                    AiExternalEffectState::None
                }
                AiRunState::Completed | AiRunState::Failed | AiRunState::Cancelled => {
                    AiExternalEffectState::Confirmed
                }
                AiRunState::Queued
                | AiRunState::Leased
                | AiRunState::RetryScheduled
                | AiRunState::Running
                | AiRunState::WaitingApproval
                | AiRunState::WaitingTool
                | AiRunState::WaitingReauth
                | AiRunState::WaitingProvider
                | AiRunState::RecoveryRequired => AiExternalEffectState::Uncertain,
            };
            runs.push(AiRestoredRun {
                run_id: AiRunId(run.id),
                state,
                external_effect,
                coordinator_checkpoint: AiRestoredCoordinatorCheckpoint::None,
                has_provider_continuation: false,
                has_provider_file: false,
            });
        }
        statuses.insert(
            AiRestoreAuditKind::RunRecoveryClassification,
            audit_status(runs_truncated, invalid_runs),
        );

        let approvals_truncated = self.approvals.len() > limits.maximum_approvals;
        if approvals_truncated {
            self.approvals.clear();
        }
        self.approvals
            .sort_by_key(|approval| (approval.created_at, approval.id));
        let approval_evidence = self
            .approvals
            .iter()
            .map(|approval| (approval.id, approval.row_version, approval.state.clone()))
            .collect::<Vec<_>>();
        let mut invalid_approvals = 0_u64;
        let mut pending_approval_count = 0_u64;
        for approval in self.approvals {
            if !matches!(
                approval.state.as_str(),
                "pending"
                    | "approved"
                    | "resume_claimed"
                    | "denied"
                    | "expired"
                    | "revoked"
                    | "consumed"
            ) || approval.maximum_uses < 1
                || approval.consumed_uses < 0
                || approval.consumed_uses > approval.maximum_uses
            {
                invalid_approvals += 1;
                continue;
            }
            if matches!(
                approval.state.as_str(),
                "pending" | "approved" | "resume_claimed"
            ) && approval.consumed_uses < approval.maximum_uses
            {
                pending_approval_count += 1;
            }
        }
        statuses.insert(
            AiRestoreAuditKind::ApprovalRevalidationCandidates,
            audit_status(approvals_truncated, invalid_approvals),
        );

        let consents_truncated = self.egress_consents.len() > limits.maximum_egress_consents;
        if consents_truncated {
            self.egress_consents.clear();
        }
        self.egress_consents
            .sort_by_key(|consent| (consent.granted_at, consent.id));
        let consent_evidence = self
            .egress_consents
            .iter()
            .map(|consent| (consent.id, consent.row_version, consent.revoked_at))
            .collect::<Vec<_>>();
        let mut invalid_consents = 0_u64;
        let mut pending_egress_consent_count = 0_u64;
        for consent in self.egress_consents {
            if consent.principal_subject.is_empty()
                || consent.scope_kind.is_empty()
                || consent.scope_id.is_empty()
                || consent.destination.is_empty()
                || consent.capability.is_empty()
                || consent.purpose.is_empty()
                || consent.purpose_grant_reference.is_empty()
                || consent.manifest_constraints_hash.is_empty()
                || consent.assurance.is_empty()
                || consent.expires_at < consent.granted_at
                || consent
                    .revoked_at
                    .is_some_and(|revoked_at| revoked_at < consent.granted_at)
            {
                invalid_consents += 1;
                continue;
            }
            if consent.revoked_at.is_none() {
                pending_egress_consent_count += 1;
            }
        }
        statuses.insert(
            AiRestoreAuditKind::EgressConsentRevalidationCandidates,
            audit_status(consents_truncated, invalid_consents),
        );

        let source_rows = serde_json::to_vec(&(run_evidence, approval_evidence, consent_evidence))
            .map_err(|_| AiError::PersistenceFailed)?;
        AiCollectedRestoreFacts::new(
            AiRestoreSnapshotFacts {
                module_fingerprint,
                missing_key_versions: Vec::new(),
                runs,
                pending_approval_count,
                pending_egress_consent_count,
                invalid_attachment_count: 0,
                invalid_usage_fact_count: 0,
                invalid_budget_policy_count: 0,
                invalid_pricing_policy_count: 0,
                invalid_skill_catalog_count: 0,
                invalid_rule_policy_count: 0,
                invalid_coordinator_checkpoint_count: 0,
                invalid_context_checkpoint_count: 0,
                invalid_provider_webhook_receipt_count: 0,
                invalid_provider_background_submission_count: 0,
                invalid_ui_intent_event_count: 0,
                invalid_session_retention_count: 0,
                duplicate_stream_sequence_count: 0,
                stream_gap_count: 0,
            },
            statuses,
            hex::encode(Sha256::digest(source_rows)),
        )
    }
}

const fn query_limit(maximum: usize) -> i64 {
    maximum as i64 + 1
}

const fn audit_status(truncated: bool, invalid_count: u64) -> AiRestoreAuditStatus {
    if truncated {
        AiRestoreAuditStatus::LimitExceeded
    } else if invalid_count > 0 {
        AiRestoreAuditStatus::Invalid {
            count: invalid_count,
        }
    } else {
        AiRestoreAuditStatus::Complete
    }
}

fn map_transaction(error: TransactionError) -> AiError {
    match error.public_error().code {
        OrmErrorCode::InvalidInput
        | OrmErrorCode::CursorInvalid
        | OrmErrorCode::PageLimitExceeded => {
            AiError::InvalidInput("restore fact collection query was rejected".to_owned())
        }
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
    use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule};
    use graphql_orm::prelude::{Database, SqliteBackend};
    use uuid::Uuid;

    async fn database() -> Database<SqliteBackend> {
        let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .expect("in-memory SQLite should open");
        let module = crate::AiSchemaModule;
        let plan = database
            .schema()
            .plan_migration_to_entities(
                "ai-restore-collector-test-v1",
                "AI restore collector test",
                module.entities(),
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

    async fn seed_run(database: &Database<SqliteBackend>, state: &str, created_at: i64) -> Uuid {
        let id = Uuid::new_v4();
        AiRunRecord::insert(
            database,
            crate::persistence::CreateAiRunRecordInput {
                id,
                session_id: Uuid::new_v4(),
                input_message_id: Uuid::new_v4(),
                principal_reference: serde_json::json!({"subject": "restore-test"}),
                state: state.to_owned(),
                attempt_id: Some(Uuid::new_v4()),
                lease_owner: Some("restored-worker".to_owned()),
                lease_generation: 2,
                lease_expires_at: Some(created_at + 60),
                lease_heartbeat_at: Some(created_at),
                retry_count: 0,
                next_attempt_at: None,
                error_code: None,
                latest_checkpoint_id: None,
            },
        )
        .await
        .expect("run should insert");
        id
    }

    async fn seed_approval(database: &Database<SqliteBackend>) {
        AiApprovalRecord::insert(
            database,
            crate::persistence::CreateAiApprovalRecordInput {
                id: Uuid::new_v4(),
                tool_call_id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                principal_subject: "restore-test".to_owned(),
                principal_reference_fingerprint: "principal-fingerprint".to_owned(),
                delegated_actor_subject: None,
                delegation_reference: None,
                argument_hash: "argument-hash".to_owned(),
                tool_fingerprint: "tool-fingerprint".to_owned(),
                binding_hash: "binding-hash".to_owned(),
                execution_target_id: "local-application".to_owned(),
                target_schema_fingerprint: "schema-fingerprint".to_owned(),
                operation_name: "RestoreTest".to_owned(),
                operation_document_hash: "operation-hash".to_owned(),
                result_projection_fingerprint: "projection-fingerprint".to_owned(),
                disclosure_schema_fingerprint: "disclosure-fingerprint".to_owned(),
                policy_version: "policy-v1".to_owned(),
                authorization_state_digest: "authorization-digest".to_owned(),
                protected_resource_bindings: None,
                protected_action_preview: None,
                payload_purged_at: Some(1),
                action_preview_hash: "preview-hash".to_owned(),
                state: "approved".to_owned(),
                recent_mfa_required: false,
                approver_subject: Some("approver".to_owned()),
                expires_at: 2_000_000_000,
                decided_at: Some(1_900_000_000),
                maximum_uses: 1,
                consumed_uses: 0,
                consumed_at: None,
            },
        )
        .await
        .expect("approval should insert");
    }

    async fn seed_consent(database: &Database<SqliteBackend>) {
        AiEgressConsentRecord::insert(
            database,
            crate::persistence::CreateAiEgressConsentRecordInput {
                principal_subject: "restore-test".to_owned(),
                scope_kind: "project".to_owned(),
                scope_id: "project-1".to_owned(),
                tenant_id: Some("tenant-1".to_owned()),
                destination: "managed-provider".to_owned(),
                capability: "model-inference".to_owned(),
                purpose: "restore-test".to_owned(),
                purpose_grant_reference: "grant-1".to_owned(),
                manifest_constraints_hash: "manifest-hash".to_owned(),
                assurance: "standard".to_owned(),
                granted_at: 1_900_000_000,
                expires_at: 2_000_000_000,
                revoked_at: None,
            },
        )
        .await
        .expect("egress consent should insert");
    }

    #[tokio::test]
    async fn collector_derives_core_facts_and_fails_closed_for_remaining_audits() {
        let database = database().await;
        seed_run(&database, AiRunState::Running.as_str(), 1_900_000_000).await;
        seed_approval(&database).await;
        seed_consent(&database).await;

        let collector = OrmAiRestoreFactCollector::new(database);
        let first = collector
            .collect("module-fingerprint")
            .await
            .expect("collector should read restored rows");
        let second = collector
            .collect("module-fingerprint")
            .await
            .expect("repeat collection should succeed");

        assert_eq!(first.digest(), second.digest());
        assert_eq!(first.run_count(), 1);
        assert_eq!(
            first.facts().runs[0].external_effect,
            AiExternalEffectState::Uncertain
        );
        assert_eq!(first.pending_approval_count(), 1);
        assert_eq!(first.pending_egress_consent_count(), 1);
        assert_eq!(
            first
                .audit_statuses()
                .get(&AiRestoreAuditKind::RunRecoveryClassification),
            Some(&AiRestoreAuditStatus::Complete)
        );
        assert_eq!(
            first.audit_statuses().get(&AiRestoreAuditKind::Attachments),
            Some(&AiRestoreAuditStatus::NotImplemented)
        );

        let bound = crate::AiRestoreReconciler::new("module-fingerprint")
            .plan_collected(&first)
            .expect("collected plan should hash");
        assert_eq!(bound.facts_digest(), first.digest());
        assert_eq!(bound.plan().fatal_issue_count(), 14);
        assert!(bound.plan().issues.iter().any(|issue| {
            issue.code == "AI_RESTORE_AUDIT_INCOMPLETE"
                && issue.resource_ref.as_deref() == Some("attachments")
        }));
        assert_eq!(
            bound.plan().run_actions[0].disposition,
            crate::AiRestoredRunDisposition::RecoveryRequired
        );

        let missing_statuses = AiCollectedRestoreFacts::new(
            first.facts().clone(),
            BTreeMap::new(),
            first.source_rows_digest().to_owned(),
        )
        .expect("internal incomplete test facts should hash");
        let missing_plan = crate::AiRestoreReconciler::new("module-fingerprint")
            .plan_collected(&missing_statuses)
            .expect("missing-status plan should hash");
        assert_eq!(missing_plan.plan().fatal_issue_count(), 17);
    }

    #[tokio::test]
    async fn reached_collection_bound_is_fatal_and_never_silent() {
        let database = database().await;
        seed_run(&database, AiRunState::Queued.as_str(), 1_900_000_000).await;
        seed_run(&database, AiRunState::Queued.as_str(), 1_900_000_001).await;
        let limits =
            AiRestoreCollectorLimits::new(1, 1, 1).expect("test collection limits should validate");
        let collected = OrmAiRestoreFactCollector::new(database)
            .with_limits(limits)
            .collect("module-fingerprint")
            .await
            .expect("bounded collector should return explicit incompleteness");

        assert_eq!(collected.run_count(), 0);
        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::RunRecoveryClassification),
            Some(&AiRestoreAuditStatus::LimitExceeded)
        );
        let plan = crate::AiRestoreReconciler::new("module-fingerprint")
            .plan_collected(&collected)
            .expect("bounded plan should hash");
        assert!(plan.plan().issues.iter().any(|issue| {
            issue.code == "AI_RESTORE_COLLECTION_LIMIT_EXCEEDED"
                && issue.resource_ref.as_deref() == Some("run_recovery_classification")
        }));
    }

    #[tokio::test]
    async fn malformed_durable_run_state_is_an_invalid_audit() {
        let database = database().await;
        seed_run(&database, "unknown_restore_state", 1_900_000_000).await;
        let collected = OrmAiRestoreFactCollector::new(database)
            .collect("module-fingerprint")
            .await
            .expect("collector should report malformed rows without panicking");

        assert_eq!(collected.run_count(), 0);
        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::RunRecoveryClassification),
            Some(&AiRestoreAuditStatus::Invalid { count: 1 })
        );
    }

    #[tokio::test]
    async fn retry_scheduled_run_is_never_assumed_pre_effect() {
        let database = database().await;
        seed_run(
            &database,
            AiRunState::RetryScheduled.as_str(),
            1_900_000_000,
        )
        .await;
        let collected = OrmAiRestoreFactCollector::new(database)
            .collect("module-fingerprint")
            .await
            .expect("retry-scheduled row should collect conservatively");

        assert_eq!(
            collected.facts().runs[0].external_effect,
            AiExternalEffectState::Uncertain
        );
        let plan = crate::AiRestoreReconciler::new("module-fingerprint")
            .plan_collected(&collected)
            .expect("conservative plan should hash");
        assert_eq!(
            plan.plan().run_actions[0].disposition,
            crate::AiRestoredRunDisposition::RecoveryRequired
        );
    }

    #[tokio::test]
    async fn host_pagination_cap_cannot_silently_truncate_internal_collection() {
        let database = database()
            .await
            .with_pagination_config(PaginationConfig::explicit_only(1));
        seed_run(&database, AiRunState::Running.as_str(), 1_900_000_000).await;
        seed_run(&database, AiRunState::Running.as_str(), 1_900_000_001).await;
        let limits = AiRestoreCollectorLimits::new(10, 10, 10)
            .expect("test collection limits should validate");
        let collected = OrmAiRestoreFactCollector::new(database)
            .with_limits(limits)
            .collect("module-fingerprint")
            .await
            .expect("trusted internal scan should use its own hard bound");

        assert_eq!(collected.run_count(), 2);
        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::RunRecoveryClassification),
            Some(&AiRestoreAuditStatus::Complete)
        );
    }

    #[tokio::test]
    async fn tied_boundary_timestamps_have_a_total_deterministic_order() {
        let database = database().await;
        let created_at = 1_900_000_000;
        let first_id = seed_run(&database, AiRunState::Running.as_str(), created_at).await;
        let second_id = seed_run(&database, AiRunState::Running.as_str(), created_at).await;
        let limits =
            AiRestoreCollectorLimits::new(2, 2, 2).expect("test collection limits should validate");
        let collector = OrmAiRestoreFactCollector::new(database).with_limits(limits);

        let first = collector
            .collect("module-fingerprint")
            .await
            .expect("tied rows should collect");
        let second = collector
            .collect("module-fingerprint")
            .await
            .expect("repeat tied-row collection should succeed");

        let mut expected = vec![first_id, second_id];
        expected.sort_unstable();
        assert_eq!(
            first
                .facts()
                .runs
                .iter()
                .map(|run| run.run_id.0)
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(first.digest(), second.digest());
    }
}
