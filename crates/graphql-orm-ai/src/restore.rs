//! Side-effect-safe restore fact and reconciliation planning contracts.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{AiRunId, AiRunState};

const REQUIRED_RESTORE_AUDITS: [AiRestoreAuditKind; 18] = [
    AiRestoreAuditKind::RunRecoveryClassification,
    AiRestoreAuditKind::ApprovalRevalidationCandidates,
    AiRestoreAuditKind::EgressConsentRevalidationCandidates,
    AiRestoreAuditKind::EncryptionKeys,
    AiRestoreAuditKind::AttachmentMetadataGraph,
    AiRestoreAuditKind::AttachmentObjectBytes,
    AiRestoreAuditKind::UsageFacts,
    AiRestoreAuditKind::BudgetPolicies,
    AiRestoreAuditKind::PricingPolicies,
    AiRestoreAuditKind::SkillCatalog,
    AiRestoreAuditKind::RulePolicies,
    AiRestoreAuditKind::CoordinatorCheckpoints,
    AiRestoreAuditKind::ContextCheckpoints,
    AiRestoreAuditKind::ProviderWebhookReceipts,
    AiRestoreAuditKind::ProviderBackgroundSubmissions,
    AiRestoreAuditKind::UiIntentEvents,
    AiRestoreAuditKind::SessionRetention,
    AiRestoreAuditKind::StreamContinuity,
];

/// External side-effect certainty captured for an interrupted run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiExternalEffectState {
    /// No external call/tool could have occurred.
    None,
    /// Interrupted work is proven idempotent under a stable key.
    ProvenIdempotent,
    /// A non-idempotent or unknown external effect may have occurred.
    Uncertain,
    /// External effect is confirmed and must not be repeated automatically.
    Confirmed,
}

/// Coordinator-checkpoint evidence captured by a trusted snapshot adapter.
///
/// This classification is not itself replay authority. The restored runtime
/// must still reopen the exact protected checkpoint under current authority
/// and consume it through the new run fence before provider transport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiRestoredCoordinatorCheckpoint {
    /// No complete adoptable tool-batch checkpoint is linked.
    #[default]
    None,
    /// A validated completed read-only tool batch is linked.
    ReadOnlyToolBatch,
    /// A validated approval-bound completed supervised tool batch is linked.
    SupervisedToolBatch,
}

/// Restored run facts needed for reconciliation; payloads are intentionally
/// absent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiRestoredRun {
    /// Run ID.
    pub run_id: AiRunId,
    /// State captured in the backup.
    pub state: AiRunState,
    /// External-effect certainty.
    pub external_effect: AiExternalEffectState,
    /// Trusted classification of the exact linked coordinator checkpoint.
    #[serde(default)]
    pub coordinator_checkpoint: AiRestoredCoordinatorCheckpoint,
    /// Whether a provider continuation reference exists.
    pub has_provider_continuation: bool,
    /// Whether a provider file reference exists.
    pub has_provider_file: bool,
}

/// Preflight facts for one restored snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiRestoreSnapshotFacts {
    /// Backup module fingerprint.
    pub module_fingerprint: String,
    /// Required encryption key versions missing from the deployment.
    pub missing_key_versions: Vec<String>,
    /// Runs requiring reconciliation.
    pub runs: Vec<AiRestoredRun>,
    /// Number of pending, approved, or resume-claimed unconsumed approvals to
    /// expire/revalidate.
    pub pending_approval_count: u64,
    /// Number of pending egress consents to expire/revalidate.
    pub pending_egress_consent_count: u64,
    /// Attachment, artifact, parent, or lifecycle metadata rows that fail the
    /// database-only graph audit.
    #[serde(default, alias = "invalid_attachment_count")]
    pub invalid_attachment_metadata_count: u64,
    /// Local attachment/artifact objects that fail verified-manifest and
    /// restored-target byte-count/SHA-256 validation.
    #[serde(default)]
    pub invalid_attachment_object_count: u64,
    /// Usage facts that fail reservation, scope, principal, provider, or
    /// non-negative/cached-subset integrity validation.
    pub invalid_usage_fact_count: u64,
    /// Budget policies with invalid scope keys, principal pairs, intervals, or
    /// ceilings.
    pub invalid_budget_policy_count: u64,
    /// Immutable pricing versions with invalid unique references, scope/route
    /// bindings, rates, or creator audit linkage.
    pub invalid_pricing_policy_count: u64,
    /// Skill identities/current versions with invalid scope, publication,
    /// protected content, strict policy format, provenance, or checksum.
    pub invalid_skill_catalog_count: u64,
    /// Hierarchical rule layers with invalid deterministic scope identity,
    /// strict format, checksum, deployment ceiling, or lineage semantics.
    pub invalid_rule_policy_count: u64,
    /// Protected coordinator checkpoints with invalid v2 rule fingerprint,
    /// cumulative usage, scope, fence, or current-lineage binding.
    pub invalid_coordinator_checkpoint_count: u64,
    /// Context-summary checkpoints with invalid exact prefix coverage, source
    /// hash/provenance, parent lineage, protection envelope, provider/budget
    /// evidence, or retention invalidation state.
    #[serde(default)]
    pub invalid_context_checkpoint_count: u64,
    /// Provider webhook receipts with invalid deterministic identity,
    /// provider/profile/event/response binding, signature fact, lifecycle
    /// state, exact submission/run/attempt terminal linkage, or creation and
    /// reconciliation audit linkage.
    #[serde(default)]
    pub invalid_provider_webhook_receipt_count: u64,
    /// Provider background submissions with invalid deterministic identity,
    /// run/attempt/fence/profile/request/budget/egress/response binding,
    /// lifecycle state, terminal outcome/usage/output/checkpoint/event/receipt
    /// graph, or preparation/acceptance/reconciliation audit linkage.
    #[serde(default)]
    pub invalid_provider_background_submission_count: u64,
    /// UI-intent session/inbox event pairs with invalid protected payloads,
    /// source/binding evidence, owner/scope linkage, or committed budget proof.
    pub invalid_ui_intent_event_count: u64,
    /// Message-content tombstones, retained block rows, or expected
    /// retention-gap classifications that fail the current purge contract.
    pub invalid_session_retention_count: u64,
    /// Duplicate durable stream sequence count.
    pub duplicate_stream_sequence_count: u64,
    /// Retention/known stream gap count.
    pub stream_gap_count: u64,
}

/// Durable audit categories required before an applied restore may open the
/// runtime.
///
/// The database collector reports every category explicitly. A category that
/// is absent, truncated, or not yet implemented is never interpreted as a
/// successful zero-count audit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AiRestoreAuditKind {
    /// Conservative durable run recovery candidate classification.
    RunRecoveryClassification,
    /// Pending or reusable approval revalidation candidate discovery.
    ApprovalRevalidationCandidates,
    /// Non-revoked egress-consent revalidation candidate discovery.
    EgressConsentRevalidationCandidates,
    /// Deployment encryption-key availability.
    EncryptionKeys,
    /// Local attachment/derived-artifact metadata and parent-graph integrity.
    AttachmentMetadataGraph,
    /// Verified-manifest and restored-target attachment object-byte integrity.
    AttachmentObjectBytes,
    /// Usage-ledger integrity.
    UsageFacts,
    /// Budget-policy integrity.
    BudgetPolicies,
    /// Immutable pricing-catalog integrity.
    PricingPolicies,
    /// Skill identity, version, and protected-content integrity.
    SkillCatalog,
    /// Hierarchical rule-policy integrity.
    RulePolicies,
    /// Coordinator-checkpoint integrity.
    CoordinatorCheckpoints,
    /// Context-compaction checkpoint integrity.
    ContextCheckpoints,
    /// Provider webhook receipt integrity.
    ProviderWebhookReceipts,
    /// Provider background-submission integrity.
    ProviderBackgroundSubmissions,
    /// UI-intent event integrity.
    UiIntentEvents,
    /// Session-retention and tombstone integrity.
    SessionRetention,
    /// Stream sequence uniqueness and represented-gap integrity.
    StreamContinuity,
}

impl AiRestoreAuditKind {
    /// Every audit category required by the current restore contract.
    pub const fn required() -> &'static [Self] {
        &REQUIRED_RESTORE_AUDITS
    }

    /// Stable content-free audit identifier used in restore issues.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RunRecoveryClassification => "run_recovery_classification",
            Self::ApprovalRevalidationCandidates => "approval_revalidation_candidates",
            Self::EgressConsentRevalidationCandidates => "egress_consent_revalidation_candidates",
            Self::EncryptionKeys => "encryption_keys",
            Self::AttachmentMetadataGraph => "attachment_metadata_graph",
            Self::AttachmentObjectBytes => "attachment_object_bytes",
            Self::UsageFacts => "usage_facts",
            Self::BudgetPolicies => "budget_policies",
            Self::PricingPolicies => "pricing_policies",
            Self::SkillCatalog => "skill_catalog",
            Self::RulePolicies => "rule_policies",
            Self::CoordinatorCheckpoints => "coordinator_checkpoints",
            Self::ContextCheckpoints => "context_checkpoints",
            Self::ProviderWebhookReceipts => "provider_webhook_receipts",
            Self::ProviderBackgroundSubmissions => "provider_background_submissions",
            Self::UiIntentEvents => "ui_intent_events",
            Self::SessionRetention => "session_retention",
            Self::StreamContinuity => "stream_continuity",
        }
    }
}

/// Completeness of one database-derived restore audit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
#[non_exhaustive]
pub enum AiRestoreAuditStatus {
    /// The category's declared bounded collection/audit scope completed over
    /// the entire required row set.
    Complete,
    /// This collector version does not yet implement the audit.
    NotImplemented,
    /// The configured bound was reached before the audit could complete.
    LimitExceeded,
    /// Rows were read, but one or more failed structural validation.
    Invalid {
        /// Number of structurally invalid rows observed within the bound.
        count: u64,
    },
}

impl AiRestoreAuditStatus {
    /// Returns whether the category was completely audited.
    pub const fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }
}

/// Database-derived restore facts and explicit audit-completeness evidence.
///
/// Fields are private so a host cannot silently replace an unimplemented or
/// truncated audit with a successful zero count. This value is still dry-run
/// planning input; it does not prove that repairs were applied and cannot open
/// runtime readiness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiCollectedRestoreFacts {
    facts: AiRestoreSnapshotFacts,
    audit_statuses: BTreeMap<AiRestoreAuditKind, AiRestoreAuditStatus>,
    source_rows_digest: String,
    digest: String,
}

impl AiCollectedRestoreFacts {
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn new(
        facts: AiRestoreSnapshotFacts,
        audit_statuses: BTreeMap<AiRestoreAuditKind, AiRestoreAuditStatus>,
        source_rows_digest: String,
    ) -> Result<Self, crate::AiError> {
        let encoded = serde_json::to_vec(&(&facts, &audit_statuses, &source_rows_digest))
            .map_err(|_| crate::AiError::PersistenceFailed)?;
        Ok(Self {
            facts,
            audit_statuses,
            source_rows_digest,
            digest: hex::encode(Sha256::digest(encoded)),
        })
    }

    /// Returns the redacted facts collected from the database.
    pub(crate) fn facts(&self) -> &AiRestoreSnapshotFacts {
        &self.facts
    }

    /// Number of run recovery candidates collected from the database.
    pub fn run_count(&self) -> usize {
        self.facts.runs.len()
    }

    /// Number of approval rows requiring restore-time revalidation.
    pub const fn pending_approval_count(&self) -> u64 {
        self.facts.pending_approval_count
    }

    /// Number of egress-consent rows requiring restore-time revalidation.
    pub const fn pending_egress_consent_count(&self) -> u64 {
        self.facts.pending_egress_consent_count
    }

    /// Returns explicit status for every required audit category.
    pub fn audit_statuses(&self) -> &BTreeMap<AiRestoreAuditKind, AiRestoreAuditStatus> {
        &self.audit_statuses
    }

    /// Stable content-free digest of the accepted row identities, CAS
    /// versions, and classification evidence.
    ///
    /// A category that exceeded its bound contributes no partial evidence;
    /// its explicit [`AiRestoreAuditStatus::LimitExceeded`] remains fatal.
    pub fn source_rows_digest(&self) -> &str {
        &self.source_rows_digest
    }

    /// Stable SHA-256 digest of the collected facts and audit statuses.
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// Planned recovery disposition for one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiRestoredRunDisposition {
    /// Preserve a terminal state.
    PreserveTerminal,
    /// Requeue using a new attempt and fencing generation.
    RequeueWithNewAttempt,
    /// Require manual recovery review and never replay automatically.
    RecoveryRequired,
}

/// Redacted planned run repair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiRestoredRunAction {
    /// Run ID.
    pub run_id: AiRunId,
    /// Recovery disposition.
    pub disposition: AiRestoredRunDisposition,
    /// Lease owner/attempt/expiry/heartbeat must be cleared.
    pub clear_lease: bool,
    /// Provider continuation must be reverified before use.
    pub reverify_provider_continuation: bool,
    /// Provider file must be reverified before use.
    pub reverify_provider_file: bool,
}

/// Stable restore issue severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiRestoreIssueSeverity {
    /// Prevents runtime startup.
    Fatal,
    /// Requires reset/review but can be represented safely.
    Warning,
}

/// Redacted restore issue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiRestoreIssue {
    /// Stable issue code.
    pub code: String,
    /// Severity.
    pub severity: AiRestoreIssueSeverity,
    /// Affected safe reference when useful.
    pub resource_ref: Option<String>,
}

/// Dry-run restore reconciliation plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiRestorePlan {
    /// Expected compiled module fingerprint.
    pub expected_module_fingerprint: String,
    /// Run repairs.
    pub run_actions: Vec<AiRestoredRunAction>,
    /// Pending approvals to expire/revalidate.
    pub approvals_to_revalidate: u64,
    /// Pending egress consents to expire/revalidate.
    pub consents_to_revalidate: u64,
    /// Redacted issues.
    pub issues: Vec<AiRestoreIssue>,
}

/// Restore plan bound to one exact database-collected fact set.
///
/// This remains a dry-run artifact. Neither this value nor its digests prove
/// that any mutation or post-apply validation occurred.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiCollectedRestorePlan {
    plan: AiRestorePlan,
    facts_digest: String,
    plan_digest: String,
}

impl AiCollectedRestorePlan {
    /// Returns the redacted dry-run plan.
    pub fn plan(&self) -> &AiRestorePlan {
        &self.plan
    }

    /// Digest of the exact collected facts used to build this plan.
    pub fn facts_digest(&self) -> &str {
        &self.facts_digest
    }

    /// Stable SHA-256 digest binding the facts digest and dry-run plan.
    pub fn plan_digest(&self) -> &str {
        &self.plan_digest
    }
}

impl AiRestorePlan {
    /// Returns fatal issue count.
    pub fn fatal_issue_count(&self) -> u64 {
        self.issues
            .iter()
            .filter(|issue| issue.severity == AiRestoreIssueSeverity::Fatal)
            .count() as u64
    }
}

/// Pure reconciler. It plans database repairs but performs no I/O and no
/// external calls.
#[derive(Clone, Debug)]
pub struct AiRestoreReconciler {
    expected_module_fingerprint: String,
}

impl AiRestoreReconciler {
    /// Creates a reconciler for the compiled AI schema module.
    pub fn new(expected_module_fingerprint: impl Into<String>) -> Self {
        Self {
            expected_module_fingerprint: expected_module_fingerprint.into(),
        }
    }

    /// Builds a dry-run plan. This method never resumes provider work or
    /// invokes application tools.
    pub fn plan(&self, facts: &AiRestoreSnapshotFacts) -> AiRestorePlan {
        let mut issues = Vec::new();
        if facts.module_fingerprint != self.expected_module_fingerprint {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_SCHEMA_FINGERPRINT_MISMATCH".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: None,
            });
        }
        for key_version in &facts.missing_key_versions {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_ENCRYPTION_KEY_MISSING".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: Some(key_version.clone()),
            });
        }
        if facts.invalid_attachment_metadata_count > 0 {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_ATTACHMENT_METADATA_INVALID".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: None,
            });
        }
        if facts.invalid_attachment_object_count > 0 {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_ATTACHMENT_OBJECT_INVALID".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: None,
            });
        }
        if facts.invalid_usage_fact_count > 0 {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_USAGE_FACT_INVALID".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: None,
            });
        }
        if facts.invalid_budget_policy_count > 0 {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_BUDGET_POLICY_INVALID".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: None,
            });
        }
        if facts.invalid_pricing_policy_count > 0 {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_PRICING_POLICY_INVALID".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: None,
            });
        }
        if facts.invalid_skill_catalog_count > 0 {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_SKILL_CATALOG_INVALID".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: None,
            });
        }
        if facts.invalid_rule_policy_count > 0 {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_RULE_POLICY_INVALID".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: None,
            });
        }
        if facts.invalid_coordinator_checkpoint_count > 0 {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_COORDINATOR_CHECKPOINT_INVALID".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: None,
            });
        }
        if facts.invalid_context_checkpoint_count > 0 {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_CONTEXT_CHECKPOINT_INVALID".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: None,
            });
        }
        if facts.invalid_provider_webhook_receipt_count > 0 {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_PROVIDER_WEBHOOK_RECEIPT_INVALID".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: None,
            });
        }
        if facts.invalid_provider_background_submission_count > 0 {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_PROVIDER_BACKGROUND_SUBMISSION_INVALID".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: None,
            });
        }
        if facts.invalid_ui_intent_event_count > 0 {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_UI_INTENT_EVENT_INVALID".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: None,
            });
        }
        if facts.invalid_session_retention_count > 0 {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_SESSION_RETENTION_INVALID".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: None,
            });
        }
        if facts.duplicate_stream_sequence_count > 0 {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_STREAM_SEQUENCE_DUPLICATE".to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: None,
            });
        }
        if facts.stream_gap_count > 0 {
            issues.push(AiRestoreIssue {
                code: "AI_RESTORE_STREAM_GAP_RESET_REQUIRED".to_owned(),
                severity: AiRestoreIssueSeverity::Warning,
                resource_ref: None,
            });
        }

        let run_actions = facts
            .runs
            .iter()
            .map(|run| AiRestoredRunAction {
                run_id: run.run_id,
                disposition: restored_run_disposition(run),
                clear_lease: true,
                reverify_provider_continuation: run.has_provider_continuation,
                reverify_provider_file: run.has_provider_file,
            })
            .collect();

        AiRestorePlan {
            expected_module_fingerprint: self.expected_module_fingerprint.clone(),
            run_actions,
            approvals_to_revalidate: facts.pending_approval_count,
            consents_to_revalidate: facts.pending_egress_consent_count,
            issues,
        }
    }

    /// Builds a fail-closed dry-run plan from database-collected facts.
    ///
    /// Every incomplete, truncated, or structurally invalid audit becomes a
    /// fatal issue. The returned digests are suitable for binding a future
    /// recovery epoch, but do not prove application or validation.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AiError::PersistenceFailed`] only if the redacted plan
    /// cannot be deterministically serialized for hashing.
    pub fn plan_collected(
        &self,
        collected: &AiCollectedRestoreFacts,
    ) -> Result<AiCollectedRestorePlan, crate::AiError> {
        let mut plan = self.plan(collected.facts());
        for audit in AiRestoreAuditKind::required() {
            let status = collected
                .audit_statuses()
                .get(audit)
                .copied()
                .unwrap_or(AiRestoreAuditStatus::NotImplemented);
            let code = match status {
                AiRestoreAuditStatus::Complete => continue,
                AiRestoreAuditStatus::NotImplemented => "AI_RESTORE_AUDIT_INCOMPLETE",
                AiRestoreAuditStatus::LimitExceeded => "AI_RESTORE_COLLECTION_LIMIT_EXCEEDED",
                AiRestoreAuditStatus::Invalid { .. } => "AI_RESTORE_AUDIT_INVALID",
            };
            plan.issues.push(AiRestoreIssue {
                code: code.to_owned(),
                severity: AiRestoreIssueSeverity::Fatal,
                resource_ref: Some(audit.as_str().to_owned()),
            });
        }
        let facts_digest = collected.digest().to_owned();
        let encoded = serde_json::to_vec(&(&facts_digest, &plan))
            .map_err(|_| crate::AiError::PersistenceFailed)?;
        Ok(AiCollectedRestorePlan {
            plan,
            facts_digest,
            plan_digest: hex::encode(Sha256::digest(encoded)),
        })
    }
}

fn restored_run_disposition(run: &AiRestoredRun) -> AiRestoredRunDisposition {
    if run.state.is_terminal() {
        return AiRestoredRunDisposition::PreserveTerminal;
    }
    if run.state == AiRunState::RecoveryRequired {
        return AiRestoredRunDisposition::RecoveryRequired;
    }
    if matches!(
        run.state,
        AiRunState::WaitingApproval | AiRunState::WaitingTool | AiRunState::WaitingProvider
    ) {
        return AiRestoredRunDisposition::RecoveryRequired;
    }
    match run.external_effect {
        AiExternalEffectState::None | AiExternalEffectState::ProvenIdempotent => {
            AiRestoredRunDisposition::RequeueWithNewAttempt
        }
        AiExternalEffectState::Confirmed
            if run.state == AiRunState::Running
                && run.coordinator_checkpoint
                    == AiRestoredCoordinatorCheckpoint::SupervisedToolBatch
                && run.has_provider_continuation =>
        {
            AiRestoredRunDisposition::RequeueWithNewAttempt
        }
        AiExternalEffectState::Uncertain | AiExternalEffectState::Confirmed => {
            AiRestoredRunDisposition::RecoveryRequired
        }
    }
}
