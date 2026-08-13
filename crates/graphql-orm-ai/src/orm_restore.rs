//! Bounded database-derived restore fact collection.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use std::collections::{BTreeMap, BTreeSet};

use graphql_orm::db::Database;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::orm::{
    DefaultWriteBackend, OrderDirection, PaginationConfig, TransactionError, TransactionMode,
};
use graphql_orm_storage::validate_blob_key;
use sha2::{Digest, Sha256};

use crate::orm_provider_session::AiProviderSessionBindingRecord;
use crate::persistence::{
    AiApprovalRecord, AiApprovalRecordOrderByInput, AiAttachmentArtifactRecord,
    AiAttachmentArtifactRecordOrderByInput, AiAttachmentRecord, AiAttachmentRecordOrderByInput,
    AiAuditEventRecord, AiAuditEventRecordOrderByInput, AiBudgetPolicyRecord,
    AiBudgetPolicyRecordOrderByInput, AiEgressConsentRecord, AiEgressConsentRecordOrderByInput,
    AiMessageRecord, AiPricingPolicyRecord, AiPricingPolicyRecordOrderByInput, AiRunRecord,
    AiRunRecordOrderByInput, AiSessionRecord, AiSubscriptionWaitAdoptionRecord,
    AiSubscriptionWaiterRecord,
};
use crate::{
    AiAttachmentServiceLimits, AiBudgetAmounts, AiBudgetPolicyManagementLimits,
    AiCollectedRestoreFacts, AiError, AiExternalEffectState, AiPricingCatalogManagementLimits,
    AiRestoreAuditKind, AiRestoreAuditStatus, AiRestoreSnapshotFacts,
    AiRestoredCoordinatorCheckpoint, AiRestoredRun, AiRunId, AiRunState, AiScope,
    ProtectedContentEnvelope, valid_mime, valid_safe_reference, valid_sha256,
};

const MAXIMUM_COLLECTION_BOUND: usize = 1_000_000;
const ORM_BACKUP_REDACTED_VALUE: &str = "[graphql-orm:redacted]";
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

/// Host-attested deployment ceilings and row bounds for policy restore audits.
///
/// These inputs are immutable for one collection pass and are included in the
/// collected-facts digest. Omitting them leaves budget and pricing audits
/// explicitly `not_implemented`; the collector never invents permissive
/// ceilings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRestorePolicyAuditLimits {
    budget_management: AiBudgetPolicyManagementLimits,
    pricing_management: AiPricingCatalogManagementLimits,
    maximum_budget_policies: usize,
    maximum_pricing_policies: usize,
    maximum_audit_events: usize,
}

impl AiRestorePolicyAuditLimits {
    /// Creates validated policy-audit inputs.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] when any row bound is zero
    /// or exceeds the compiled one-million-row ceiling.
    pub fn new(
        budget_management: AiBudgetPolicyManagementLimits,
        pricing_management: AiPricingCatalogManagementLimits,
        maximum_budget_policies: usize,
        maximum_pricing_policies: usize,
        maximum_audit_events: usize,
    ) -> Result<Self, AiError> {
        if [
            maximum_budget_policies,
            maximum_pricing_policies,
            maximum_audit_events,
        ]
        .into_iter()
        .any(|bound| !(1..=MAXIMUM_COLLECTION_BOUND).contains(&bound))
        {
            return Err(AiError::InvalidConfiguration(
                "invalid restore policy-audit limits".to_owned(),
            ));
        }
        Ok(Self {
            budget_management,
            pricing_management,
            maximum_budget_policies,
            maximum_pricing_policies,
            maximum_audit_events,
        })
    }

    /// Current deployment budget-policy management bounds.
    pub const fn budget_management(self) -> AiBudgetPolicyManagementLimits {
        self.budget_management
    }

    /// Current deployment immutable pricing-catalog bounds.
    pub const fn pricing_management(self) -> AiPricingCatalogManagementLimits {
        self.pricing_management
    }

    /// Maximum budget-policy rows read by one pass.
    pub const fn maximum_budget_policies(self) -> usize {
        self.maximum_budget_policies
    }

    /// Maximum pricing-policy rows read by one pass.
    pub const fn maximum_pricing_policies(self) -> usize {
        self.maximum_pricing_policies
    }

    /// Maximum audit-event rows scanned for pricing-creation linkage.
    pub const fn maximum_audit_events(self) -> usize {
        self.maximum_audit_events
    }
}

/// Host-attested attachment intake ceilings and row bounds for the
/// database-only attachment metadata restore audit.
///
/// These inputs cannot prove that referenced objects exist or that their
/// bytes match the restored rows. Object verification is represented by the
/// separate fatal [`AiRestoreAuditKind::AttachmentObjectBytes`] category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRestoreAttachmentMetadataAuditLimits {
    service: AiAttachmentServiceLimits,
    maximum_attachments: usize,
    maximum_artifacts: usize,
}

impl AiRestoreAttachmentMetadataAuditLimits {
    /// Creates validated attachment metadata-audit inputs.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] when either row bound is zero
    /// or exceeds the compiled one-million-row ceiling.
    pub fn new(
        service: AiAttachmentServiceLimits,
        maximum_attachments: usize,
        maximum_artifacts: usize,
    ) -> Result<Self, AiError> {
        if [maximum_attachments, maximum_artifacts]
            .into_iter()
            .any(|bound| !(1..=MAXIMUM_COLLECTION_BOUND).contains(&bound))
        {
            return Err(AiError::InvalidConfiguration(
                "invalid restore attachment metadata-audit limits".to_owned(),
            ));
        }
        Ok(Self {
            service,
            maximum_attachments,
            maximum_artifacts,
        })
    }

    /// Host-attested attachment intake limits used for restored-row checks.
    pub const fn service(self) -> AiAttachmentServiceLimits {
        self.service
    }

    /// Maximum attachment rows read by one pass.
    pub const fn maximum_attachments(self) -> usize {
        self.maximum_attachments
    }

    /// Maximum derived-artifact rows read by one pass.
    pub const fn maximum_artifacts(self) -> usize {
        self.maximum_artifacts
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
    policy_audit_limits: Option<AiRestorePolicyAuditLimits>,
    attachment_metadata_audit_limits: Option<AiRestoreAttachmentMetadataAuditLimits>,
}

impl OrmAiRestoreFactCollector {
    /// Creates a collector with conservative default bounds.
    pub fn new(database: Database<DefaultWriteBackend>) -> Self {
        Self {
            database,
            limits: AiRestoreCollectorLimits::default(),
            policy_audit_limits: None,
            attachment_metadata_audit_limits: None,
        }
    }

    /// Overrides deployment-owned collection bounds.
    #[must_use]
    pub fn with_limits(mut self, limits: AiRestoreCollectorLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Enables complete budget- and pricing-policy database audits.
    ///
    /// The supplied host-attested ceilings are bound into the collected facts.
    /// This method does not prove they match live service configuration and
    /// grants no configuration or runtime-start authority.
    #[must_use]
    pub fn with_policy_audits(mut self, limits: AiRestorePolicyAuditLimits) -> Self {
        self.policy_audit_limits = Some(limits);
        self
    }

    /// Enables the complete database-only attachment metadata graph audit.
    ///
    /// The supplied service ceilings are host-attested and bound into the
    /// collected facts. This method performs no blob/provider I/O and cannot
    /// complete [`AiRestoreAuditKind::AttachmentObjectBytes`].
    #[must_use]
    pub fn with_attachment_metadata_audit(
        mut self,
        limits: AiRestoreAttachmentMetadataAuditLimits,
    ) -> Self {
        self.attachment_metadata_audit_limits = Some(limits);
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
    /// Collection always covers conservative run classification and
    /// approval/consent revalidation-candidate counts. When configured through
    /// [`Self::with_policy_audits`], it also covers budget- and pricing-policy
    /// integrity relative to the supplied host-attested ceilings. When
    /// configured through [`Self::with_attachment_metadata_audit`], it covers
    /// attachment/artifact lifecycle, parent, and object-reference metadata,
    /// but not external object bytes. It does not claim that candidates passed
    /// the later repair graph or that supplied ceilings match live service
    /// configuration. Every other audit category remains explicitly
    /// `not_implemented`, so the collected plan stays fatal and cannot be
    /// mistaken for production readiness.
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
        let policy_audit_limits = self.policy_audit_limits;
        let attachment_metadata_audit_limits = self.attachment_metadata_audit_limits;
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
                    // Portable backups redact opaque provider-session cursors.
                    // Any binding therefore blocks restore readiness in this
                    // release; reading one row is sufficient to prove the
                    // required empty-set contract failed.
                    let provider_sessions = tx
                        .query::<AiProviderSessionBindingRecord>()
                        .default_order()
                        .limit(1)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    // Portable backup redacts live wait plans/cursors and
                    // unconsumed adopted outcomes. Read the bounded graph so
                    // terminal/consumed tombstones do not block readiness.
                    let subscription_waiters = tx
                        .query::<AiSubscriptionWaiterRecord>()
                        .default_order()
                        .limit(query_limit(limits.maximum_runs))
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    let subscription_wait_adoptions = tx
                        .query::<AiSubscriptionWaitAdoptionRecord>()
                        .default_order()
                        .limit(query_limit(limits.maximum_runs))
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    let policies = if let Some(policy_limits) = policy_audit_limits {
                        let budget_policies = tx
                            .query::<AiBudgetPolicyRecord>()
                            .order_by(AiBudgetPolicyRecordOrderByInput {
                                updated_at: Some(OrderDirection::Asc),
                            })
                            .limit(query_limit(policy_limits.maximum_budget_policies))
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        let pricing_policies = tx
                            .query::<AiPricingPolicyRecord>()
                            .order_by(AiPricingPolicyRecordOrderByInput {
                                created_at: Some(OrderDirection::Asc),
                            })
                            .limit(query_limit(policy_limits.maximum_pricing_policies))
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        let pricing_audits = tx
                            .query::<AiAuditEventRecord>()
                            .order_by(AiAuditEventRecordOrderByInput {
                                created_at: Some(OrderDirection::Asc),
                            })
                            .limit(query_limit(policy_limits.maximum_audit_events))
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        Some(CollectedPolicyRows {
                            limits: policy_limits,
                            budget_policies,
                            pricing_policies,
                            pricing_audits,
                        })
                    } else {
                        None
                    };
                    let attachments =
                        if let Some(attachment_limits) = attachment_metadata_audit_limits {
                            let attachment_rows = tx
                                .query::<AiAttachmentRecord>()
                                .order_by(AiAttachmentRecordOrderByInput {
                                    created_at: Some(OrderDirection::Asc),
                                })
                                .limit(query_limit(attachment_limits.maximum_attachments))
                                .fetch_all()
                                .await
                                .map_err(OrmPublicError::from)?;
                            let artifact_rows = tx
                                .query::<AiAttachmentArtifactRecord>()
                                .order_by(AiAttachmentArtifactRecordOrderByInput {
                                    created_at: Some(OrderDirection::Asc),
                                })
                                .limit(query_limit(attachment_limits.maximum_artifacts))
                                .fetch_all()
                                .await
                                .map_err(OrmPublicError::from)?;
                            let mut sessions = BTreeMap::new();
                            let mut messages = BTreeMap::new();
                            if attachment_rows.len() <= attachment_limits.maximum_attachments
                                && artifact_rows.len() <= attachment_limits.maximum_artifacts
                            {
                                for attachment in &attachment_rows {
                                    if let std::collections::btree_map::Entry::Vacant(entry) =
                                        sessions.entry(attachment.session_id)
                                    {
                                        let session = tx
                                            .find_by_id::<AiSessionRecord>(&attachment.session_id)
                                            .await
                                            .map_err(OrmPublicError::from)?;
                                        entry.insert(session);
                                    }
                                    if let Some(message_id) = attachment.message_id
                                        && let std::collections::btree_map::Entry::Vacant(entry) =
                                            messages.entry(message_id)
                                    {
                                        let message = tx
                                            .find_by_id::<AiMessageRecord>(&message_id)
                                            .await
                                            .map_err(OrmPublicError::from)?;
                                        entry.insert(message);
                                    }
                                }
                            }
                            Some(CollectedAttachmentMetadataRows {
                                limits: attachment_limits,
                                attachments: attachment_rows,
                                artifacts: artifact_rows,
                                sessions,
                                messages,
                            })
                        } else {
                            None
                        };
                    Ok(CollectedRows {
                        runs,
                        approvals,
                        egress_consents,
                        provider_sessions,
                        subscription_waiters,
                        subscription_wait_adoptions,
                        policies,
                        attachments,
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
    provider_sessions: Vec<AiProviderSessionBindingRecord>,
    subscription_waiters: Vec<AiSubscriptionWaiterRecord>,
    subscription_wait_adoptions: Vec<AiSubscriptionWaitAdoptionRecord>,
    policies: Option<CollectedPolicyRows>,
    attachments: Option<CollectedAttachmentMetadataRows>,
}

struct CollectedPolicyRows {
    limits: AiRestorePolicyAuditLimits,
    budget_policies: Vec<AiBudgetPolicyRecord>,
    pricing_policies: Vec<AiPricingPolicyRecord>,
    pricing_audits: Vec<AiAuditEventRecord>,
}

struct CollectedAttachmentMetadataRows {
    limits: AiRestoreAttachmentMetadataAuditLimits,
    attachments: Vec<AiAttachmentRecord>,
    artifacts: Vec<AiAttachmentArtifactRecord>,
    sessions: BTreeMap<graphql_orm::uuid::Uuid, Option<AiSessionRecord>>,
    messages: BTreeMap<graphql_orm::uuid::Uuid, Option<AiMessageRecord>>,
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
                | AiRunState::WaitingSubscription
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

        let invalid_provider_session_binding_count =
            u64::try_from(self.provider_sessions.len()).map_err(|_| AiError::PersistenceFailed)?;
        let provider_session_evidence = self
            .provider_sessions
            .iter()
            .map(|binding| (binding.id, binding.row_version, binding.state.clone()))
            .collect::<Vec<_>>();
        statuses.insert(
            AiRestoreAuditKind::ProviderSessionBindings,
            if invalid_provider_session_binding_count == 0 {
                AiRestoreAuditStatus::Complete
            } else {
                AiRestoreAuditStatus::Invalid {
                    count: invalid_provider_session_binding_count,
                }
            },
        );
        let waits_truncated = self.subscription_waiters.len() > limits.maximum_runs;
        let adoptions_truncated = self.subscription_wait_adoptions.len() > limits.maximum_runs;
        let adoption_states = self
            .subscription_wait_adoptions
            .iter()
            .map(|adoption| (adoption.waiter_id, adoption.state.as_str()))
            .collect::<BTreeMap<_, _>>();
        let waiter_ids = self
            .subscription_waiters
            .iter()
            .map(|waiter| waiter.id)
            .collect::<BTreeSet<_>>();
        let invalid_waiters = self
            .subscription_waiters
            .iter()
            .filter(|waiter| match waiter.state.as_str() {
                "waiting" | "claimed" => true,
                "adopted" => adoption_states
                    .get(&waiter.id)
                    .is_none_or(|state| *state != "consumed"),
                "cancelled" | "failed" | "recovery_required" => false,
                _ => true,
            })
            .count();
        let invalid_adoptions = self
            .subscription_wait_adoptions
            .iter()
            .filter(|adoption| {
                !waiter_ids.contains(&adoption.waiter_id)
                    || !matches!(
                        adoption.state.as_str(),
                        "queued" | "claimed" | "consumed" | "cancelled" | "recovery_required"
                    )
            })
            .count();
        let invalid_subscription_wait_count =
            u64::try_from(invalid_waiters.saturating_add(invalid_adoptions))
                .map_err(|_| AiError::PersistenceFailed)?
                .saturating_add(u64::from(waits_truncated || adoptions_truncated));
        let subscription_wait_evidence = self
            .subscription_waiters
            .iter()
            .map(|waiter| (waiter.id, waiter.row_version, waiter.state.clone()))
            .collect::<Vec<_>>();
        let subscription_adoption_evidence = self
            .subscription_wait_adoptions
            .iter()
            .map(|adoption| (adoption.id, adoption.row_version, adoption.state.clone()))
            .collect::<Vec<_>>();
        if invalid_subscription_wait_count > 0 {
            statuses.insert(
                AiRestoreAuditKind::CoordinatorCheckpoints,
                AiRestoreAuditStatus::Invalid {
                    count: invalid_subscription_wait_count,
                },
            );
        }

        let mut invalid_budget_policy_count = 0_u64;
        let mut invalid_pricing_policy_count = 0_u64;
        let policy_evidence = if let Some(policies) = self.policies {
            let outcome = policies.audit()?;
            invalid_budget_policy_count = outcome.invalid_budget_policy_count;
            invalid_pricing_policy_count = outcome.invalid_pricing_policy_count;
            statuses.insert(AiRestoreAuditKind::BudgetPolicies, outcome.budget_status);
            statuses.insert(AiRestoreAuditKind::PricingPolicies, outcome.pricing_status);
            Some(outcome.evidence)
        } else {
            None
        };

        let mut invalid_attachment_metadata_count = 0_u64;
        let attachment_metadata_evidence = if let Some(attachments) = self.attachments {
            let outcome = attachments.audit()?;
            invalid_attachment_metadata_count = outcome.invalid_count;
            statuses.insert(AiRestoreAuditKind::AttachmentMetadataGraph, outcome.status);
            Some(outcome.evidence)
        } else {
            None
        };

        let source_rows = serde_json::to_vec(&(
            run_evidence,
            approval_evidence,
            consent_evidence,
            provider_session_evidence,
            subscription_wait_evidence,
            subscription_adoption_evidence,
            policy_evidence,
            attachment_metadata_evidence,
        ))
        .map_err(|_| AiError::PersistenceFailed)?;
        AiCollectedRestoreFacts::new(
            AiRestoreSnapshotFacts {
                module_fingerprint,
                missing_key_versions: Vec::new(),
                runs,
                pending_approval_count,
                pending_egress_consent_count,
                invalid_attachment_metadata_count,
                invalid_attachment_object_count: 0,
                invalid_usage_fact_count: 0,
                invalid_budget_policy_count,
                invalid_pricing_policy_count,
                invalid_skill_catalog_count: 0,
                invalid_rule_policy_count: 0,
                invalid_coordinator_checkpoint_count: invalid_subscription_wait_count,
                invalid_context_checkpoint_count: 0,
                invalid_provider_webhook_receipt_count: 0,
                invalid_provider_background_submission_count: 0,
                invalid_provider_session_binding_count,
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

#[derive(serde::Serialize)]
struct AttachmentMetadataAuditEvidence {
    limits: AttachmentMetadataLimitEvidence,
    attachment_rows_digest: Option<String>,
    artifact_rows_digest: Option<String>,
    session_parent_rows_digest: Option<String>,
    message_parent_rows_digest: Option<String>,
    expected_object_rows_digest: Option<String>,
}

#[derive(serde::Serialize)]
struct AttachmentMetadataLimitEvidence {
    maximum_attachment_bytes: u64,
    maximum_filename_bytes: usize,
    upload_ticket_ttl_seconds: i64,
    upload_processing_ttl_seconds: i64,
    maximum_attachments: usize,
    maximum_artifacts: usize,
}

struct AttachmentMetadataAuditOutcome {
    status: AiRestoreAuditStatus,
    invalid_count: u64,
    evidence: AttachmentMetadataAuditEvidence,
}

impl CollectedAttachmentMetadataRows {
    fn audit(mut self) -> Result<AttachmentMetadataAuditOutcome, AiError> {
        let truncated = self.attachments.len() > self.limits.maximum_attachments
            || self.artifacts.len() > self.limits.maximum_artifacts;
        if truncated {
            self.attachments.clear();
            self.artifacts.clear();
            self.sessions.clear();
            self.messages.clear();
        }
        self.attachments
            .sort_by_key(|record| (record.created_at, record.id));
        self.artifacts
            .sort_by_key(|record| (record.created_at, record.id));

        let (
            attachment_rows_digest,
            artifact_rows_digest,
            session_parent_rows_digest,
            message_parent_rows_digest,
        ) = if truncated {
            (None, None, None, None)
        } else {
            (
                Some(serialized_digest(&self.attachments)?),
                Some(serialized_digest(&self.artifacts)?),
                Some(serialized_digest(&self.sessions)?),
                Some(serialized_digest(&self.messages)?),
            )
        };

        let mut invalid = BTreeSet::new();
        let mut local_references = BTreeMap::new();
        let mut provider_references = BTreeMap::new();
        let attachment_ids = self
            .attachments
            .iter()
            .map(|record| record.id)
            .collect::<BTreeSet<_>>();
        let non_deleted_attachment_ids = self
            .attachments
            .iter()
            .filter(|record| record.deleted_at.is_none())
            .map(|record| record.id)
            .collect::<BTreeSet<_>>();
        for record in &self.attachments {
            if !restored_attachment_is_valid(
                record,
                &self.sessions,
                &self.messages,
                self.limits.service,
            ) {
                invalid.insert((false, record.id));
            }
            for reference in [
                record.blob_reference.as_deref(),
                record.quarantine_blob_reference.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                record_unique_reference(
                    &mut local_references,
                    reference,
                    (false, record.id),
                    &mut invalid,
                );
            }
        }
        for record in &self.artifacts {
            if !attachment_ids.contains(&record.attachment_id)
                || !non_deleted_attachment_ids.contains(&record.attachment_id)
                || invalid.contains(&(false, record.attachment_id))
                || !restored_attachment_artifact_is_valid(record)
            {
                invalid.insert((true, record.id));
            }
            if let Some(reference) = record.blob_reference.as_deref() {
                record_unique_reference(
                    &mut local_references,
                    reference,
                    (true, record.id),
                    &mut invalid,
                );
            }
            if let (Some(kind), Some(profile), Some(reference)) = (
                record.provider_kind.as_deref(),
                record.provider_profile_id.as_deref(),
                record.provider_reference.as_deref(),
            ) {
                let exact_reference = (kind.to_owned(), profile.to_owned(), reference.to_owned());
                if let Some(existing) = provider_references.insert(exact_reference, record.id) {
                    invalid.insert((true, existing));
                    invalid.insert((true, record.id));
                }
            }
        }

        let expected_objects = self
            .attachments
            .iter()
            .flat_map(|record| {
                [
                    record.blob_reference.as_ref().map(|reference| {
                        (
                            "attachment",
                            record.id,
                            reference,
                            record.byte_count,
                            record.sha256.as_deref(),
                            record.detected_mime.as_deref(),
                        )
                    }),
                    record.quarantine_blob_reference.as_ref().map(|reference| {
                        (
                            "attachment_quarantine",
                            record.id,
                            reference,
                            record.byte_count,
                            record.sha256.as_deref(),
                            record.detected_mime.as_deref(),
                        )
                    }),
                ]
                .into_iter()
                .flatten()
            })
            .chain(self.artifacts.iter().filter_map(|record| {
                record.blob_reference.as_ref().map(|reference| {
                    (
                        "attachment_artifact",
                        record.id,
                        reference,
                        Some(record.byte_count),
                        record.sha256.as_deref(),
                        record.detected_mime.as_deref(),
                    )
                })
            }))
            .collect::<Vec<_>>();
        let expected_object_rows_digest = if truncated {
            None
        } else {
            Some(serialized_digest(&expected_objects)?)
        };
        let invalid_count = invalid.len() as u64;

        Ok(AttachmentMetadataAuditOutcome {
            status: audit_status(truncated, invalid_count),
            invalid_count,
            evidence: AttachmentMetadataAuditEvidence {
                limits: AttachmentMetadataLimitEvidence::from(self.limits),
                attachment_rows_digest,
                artifact_rows_digest,
                session_parent_rows_digest,
                message_parent_rows_digest,
                expected_object_rows_digest,
            },
        })
    }
}

impl From<AiRestoreAttachmentMetadataAuditLimits> for AttachmentMetadataLimitEvidence {
    fn from(value: AiRestoreAttachmentMetadataAuditLimits) -> Self {
        Self {
            maximum_attachment_bytes: value.service.maximum_attachment_bytes(),
            maximum_filename_bytes: value.service.maximum_filename_bytes(),
            upload_ticket_ttl_seconds: value.service.upload_ticket_ttl().whole_seconds(),
            upload_processing_ttl_seconds: value.service.upload_processing_ttl().whole_seconds(),
            maximum_attachments: value.maximum_attachments,
            maximum_artifacts: value.maximum_artifacts,
        }
    }
}

fn restored_attachment_is_valid(
    record: &AiAttachmentRecord,
    sessions: &BTreeMap<graphql_orm::uuid::Uuid, Option<AiSessionRecord>>,
    messages: &BTreeMap<graphql_orm::uuid::Uuid, Option<AiMessageRecord>>,
    limits: AiAttachmentServiceLimits,
) -> bool {
    let valid_parent = sessions
        .get(&record.session_id)
        .and_then(Option::as_ref)
        .is_some_and(|session| {
            !session.id.is_nil()
                && session.row_version >= 0
                && matches!(session.state.as_str(), "active" | "archived")
                && session.deleted_at.is_none()
                && session.owner_principal_kind == record.owner_principal_kind
                && session.owner_subject == record.owner_subject
        });
    let valid_message = record.message_id.is_none_or(|message_id| {
        !message_id.is_nil()
            && messages
                .get(&message_id)
                .and_then(Option::as_ref)
                .is_some_and(|message| {
                    message.id == message_id
                        && message.session_id == record.session_id
                        && message.row_version >= 0
                        && message.message_role == "user"
                        && message.author_principal_kind.as_deref()
                            == Some(record.owner_principal_kind.as_str())
                        && message.author_subject.as_deref() == Some(record.owner_subject.as_str())
                        && message.client_message_id.is_some_and(|id| !id.is_nil())
                        && message.completion_state == "complete"
                        && message.finalized_at.is_some()
                        && (record.deleted_at.is_some() || message.content_purged_at.is_none())
                })
    });
    let expected_bytes = record
        .expected_byte_count
        .and_then(|value| u64::try_from(value).ok());
    let observed_bytes = record
        .byte_count
        .and_then(|value| u64::try_from(value).ok());
    let base_valid = !record.id.is_nil()
        && !record.session_id.is_nil()
        && record.row_version >= 0
        && record.created_at > 0
        && valid_parent
        && valid_message
        && valid_principal_kind(&record.owner_principal_kind)
        && valid_safe_subject(&record.owner_subject)
        && valid_restored_filename(&record.safe_filename, limits.maximum_filename_bytes())
        && record.declared_mime.as_deref().is_none_or(valid_mime)
        && record.detected_mime.as_deref().is_none_or(valid_mime)
        && expected_bytes
            .is_none_or(|value| value > 0 && value <= limits.maximum_attachment_bytes())
        && observed_bytes.is_none_or(|value| {
            value > 0
                && value <= limits.maximum_attachment_bytes()
                && expected_bytes.is_none_or(|expected| value == expected)
        })
        && record.sha256.as_deref().is_none_or(valid_sha256)
        && record.upload_token_hash.as_deref().is_none_or(valid_sha256)
        && record
            .blob_reference
            .as_deref()
            .is_none_or(valid_local_object_reference)
        && record
            .quarantine_blob_reference
            .as_deref()
            .is_none_or(valid_local_object_reference)
        && record
            .scanner_version
            .as_deref()
            .is_none_or(|value| valid_safe_reference(value, 128))
        && record
            .acceptance_policy_version
            .as_deref()
            .is_none_or(|value| valid_safe_reference(value, 128))
        && record
            .rejection_code
            .as_deref()
            .is_none_or(|value| valid_safe_reference(value, 128))
        && record
            .upload_expires_at
            .is_none_or(|value| value >= record.created_at)
        && record
            .processing_expires_at
            .is_none_or(|value| value >= record.created_at)
        && record
            .cleanup_lease_expires_at
            .is_none_or(|value| value >= record.created_at)
        && record
            .cleanup_next_attempt_at
            .is_none_or(|value| value >= record.created_at)
        && record
            .finalized_at
            .is_none_or(|value| value >= record.created_at)
        && record
            .deleted_at
            .is_none_or(|value| value >= record.created_at)
        && record.cleanup_generation.is_none_or(|value| value > 0)
        && record.cleanup_retry_count.is_none_or(|value| value >= 0)
        && valid_attachment_cleanup_shape(record);
    if !base_valid {
        return false;
    }

    let has_verifiable_object_metadata = record.blob_reference.is_some()
        && record.quarantine_blob_reference.is_none()
        && record.detected_mime.is_some()
        && observed_bytes.is_some()
        && record.sha256.is_some()
        && record.rejection_code.is_none();
    // Only stable post-restore states can complete this graph. Live upload,
    // scanner, deletion, and cleanup claims require repair first even if an
    // unredacted live database still contains their external references.
    match (
        record.quarantine_state.as_str(),
        record.scan_state.as_str(),
        record.processing_state.as_str(),
    ) {
        ("ready", "clean", "ready") => {
            record.message_id.is_none()
                && has_verifiable_object_metadata
                && expected_bytes.is_some()
                && record.scanner_version.is_some()
                && record.acceptance_policy_version.is_some()
                && record.upload_token_hash.is_none()
                && record.upload_expires_at.is_some()
                && record.finalized_at.is_none()
                && record.deleted_at.is_none()
        }
        ("released", "clean", "complete") => {
            // Schema 0.14 deliberately left upload expiry and the new scanner
            // provenance columns nullable for pre-intake finalized rows. Their
            // clean/released state, object digest, owner, and parents remain
            // independently verifiable without fabricating migration data.
            has_verifiable_object_metadata
                && record.upload_token_hash.is_none()
                && record.finalized_at.is_some()
                && record.deleted_at.is_none()
        }
        ("rejected", "rejected", "complete") | ("failed", "failed", "complete") => {
            record.message_id.is_none()
                && record.upload_token_hash.is_none()
                && record.rejection_code.is_some()
                && record.finalized_at.is_none()
                && record.deleted_at.is_none()
                && record.blob_reference.is_none()
                && record.quarantine_blob_reference.is_none()
        }
        ("expired", "failed", "complete") => {
            record.message_id.is_none()
                && record.blob_reference.is_none()
                && record.quarantine_blob_reference.is_none()
                && record.upload_token_hash.is_none()
                && record.rejection_code.is_some()
                && record.finalized_at.is_none()
                && record.cleanup_generation.is_some_and(|value| value > 0)
                && record.deleted_at.is_some()
        }
        ("deleted", "pending" | "clean" | "rejected" | "failed", "complete") => {
            record.blob_reference.is_none()
                && record.quarantine_blob_reference.is_none()
                && record.upload_token_hash.is_none()
                && record.deleted_at.is_some()
        }
        _ => false,
    }
}

fn valid_attachment_cleanup_shape(record: &AiAttachmentRecord) -> bool {
    match record.processing_state.as_str() {
        "pending" | "scanning" | "ready" => {
            record.processing_expires_at.is_some() == (record.processing_state == "scanning")
                && record.cleanup_generation.is_none()
                && record.cleanup_lease_expires_at.is_none()
                && record.cleanup_retry_count.is_none()
                && record.cleanup_next_attempt_at.is_none()
        }
        "deleting" => {
            record.processing_expires_at.is_some()
                && record.cleanup_generation.is_none()
                && record.cleanup_lease_expires_at.is_none()
                && record.cleanup_next_attempt_at.is_none()
        }
        "cleanup_required" | "retention_cleanup_required" => {
            record.processing_expires_at.is_none()
                && record.cleanup_generation.is_none()
                && record.cleanup_lease_expires_at.is_none()
                && record.cleanup_retry_count.is_none()
                && record.cleanup_next_attempt_at.is_none()
        }
        "cleanup_in_progress" => {
            record.processing_expires_at.is_none()
                && record.cleanup_generation.is_some()
                && record.cleanup_lease_expires_at.is_some()
                && record.cleanup_next_attempt_at.is_none()
        }
        "cleanup_backoff" => {
            record.processing_expires_at.is_none()
                && record.cleanup_generation.is_some()
                && record.cleanup_lease_expires_at.is_none()
                && record.cleanup_retry_count.is_some_and(|value| value > 0)
                && record.cleanup_next_attempt_at.is_some()
        }
        "complete" => {
            record.processing_expires_at.is_none()
                && record.cleanup_lease_expires_at.is_none()
                && record.cleanup_next_attempt_at.is_none()
        }
        _ => false,
    }
}

fn restored_attachment_artifact_is_valid(record: &AiAttachmentArtifactRecord) -> bool {
    let has_content = record.blob_reference.is_some()
        || record.protected_content.is_some()
        || record.provider_reference.is_some();
    !record.id.is_nil()
        && !record.attachment_id.is_nil()
        && record.row_version >= 0
        && record.created_at > 0
        && record
            .deleted_at
            .is_none_or(|value| value >= record.created_at)
        && record.detected_mime.as_deref().is_none_or(valid_mime)
        && record.sha256.as_deref().is_none_or(valid_sha256)
        && record
            .provider_reference
            .as_deref()
            .is_none_or(|value| value != ORM_BACKUP_REDACTED_VALUE)
        && record
            .blob_reference
            .as_deref()
            .is_none_or(valid_local_object_reference)
        && record.protected_content.as_ref().is_none_or(|value| {
            serde_json::from_value::<ProtectedContentEnvelope>(value.clone()).is_ok()
        })
        && crate::orm_session_retention::validate_attachment_artifact(record, record.attachment_id)
            .is_ok()
        && match record.cleanup_state.as_deref() {
            None => {
                has_content
                    && record.deleted_at.is_none()
                    && record.cleanup_generation.is_none()
                    && record.cleanup_lease_expires_at.is_none()
                    && record.cleanup_retry_count.is_none()
                    && record.cleanup_next_attempt_at.is_none()
                    && (record.blob_reference.is_none() || record.sha256.is_some())
            }
            Some("complete") => !has_content && record.deleted_at.is_some(),
            Some(_) => false,
        }
}

fn valid_restored_filename(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value != "."
        && value != ".."
        && value.trim_matches([' ', '.']) == value
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':'))
}

fn valid_safe_subject(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

fn valid_local_object_reference(value: &str) -> bool {
    value != ORM_BACKUP_REDACTED_VALUE
        && valid_safe_reference(value, 4_096)
        && validate_blob_key(value).is_ok()
}

fn record_unique_reference(
    references: &mut BTreeMap<String, (bool, uuid::Uuid)>,
    reference: &str,
    row: (bool, uuid::Uuid),
    invalid: &mut BTreeSet<(bool, uuid::Uuid)>,
) {
    if let Some(existing) = references.insert(reference.to_owned(), row) {
        invalid.insert(existing);
        invalid.insert(row);
    }
}

#[derive(serde::Serialize)]
struct PolicyAuditEvidence {
    limits: PolicyLimitEvidence,
    budget_rows_digest: Option<String>,
    pricing_rows_digest: Option<String>,
    pricing_audit_rows_digest: Option<String>,
}

#[derive(serde::Serialize)]
struct PolicyLimitEvidence {
    budget_ceiling: AiBudgetAmounts,
    maximum_budget_policies_per_scope: usize,
    maximum_budget_policies: usize,
    maximum_fixed_call_microunits: u64,
    maximum_token_rate_microunits_per_million: u64,
    maximum_builtin_tool_microunits_per_call: u64,
    maximum_pricing_versions_per_route: usize,
    maximum_pricing_policies: usize,
    maximum_audit_events: usize,
}

struct PolicyAuditOutcome {
    budget_status: AiRestoreAuditStatus,
    pricing_status: AiRestoreAuditStatus,
    invalid_budget_policy_count: u64,
    invalid_pricing_policy_count: u64,
    evidence: PolicyAuditEvidence,
}

impl CollectedPolicyRows {
    fn audit(mut self) -> Result<PolicyAuditOutcome, AiError> {
        // Timestamp ties at the SQL sentinel boundary cannot enter accepted
        // evidence: an overflow clears the whole dependent category, while a
        // complete scan contains every row and is canonically sorted by ID.
        let budget_truncated = self.budget_policies.len() > self.limits.maximum_budget_policies;
        if budget_truncated {
            self.budget_policies.clear();
        }
        self.budget_policies
            .sort_by_key(|record| (record.updated_at, record.id));
        let budget_rows_digest = if budget_truncated {
            None
        } else {
            Some(serialized_digest(&self.budget_policies)?)
        };
        let invalid_budget_policy_count = if budget_truncated {
            0
        } else {
            invalid_budget_policy_count(&self.budget_policies, self.limits.budget_management)
        };

        let pricing_truncated = self.pricing_policies.len() > self.limits.maximum_pricing_policies
            || self.pricing_audits.len() > self.limits.maximum_audit_events;
        if pricing_truncated {
            self.pricing_policies.clear();
            self.pricing_audits.clear();
        } else {
            self.pricing_audits.retain(|record| {
                record.action == "ai.pricing_policy.create"
                    || record.resource_kind == "pricing_policy"
            });
        }
        self.pricing_policies
            .sort_by_key(|record| (record.created_at, record.id));
        self.pricing_audits
            .sort_by_key(|record| (record.created_at, record.id));
        let (pricing_rows_digest, pricing_audit_rows_digest) = if pricing_truncated {
            (None, None)
        } else {
            (
                Some(serialized_digest(&self.pricing_policies)?),
                Some(serialized_digest(&self.pricing_audits)?),
            )
        };
        let invalid_pricing_policy_count = if pricing_truncated {
            0
        } else {
            invalid_pricing_policy_count(
                &self.pricing_policies,
                &self.pricing_audits,
                self.limits.pricing_management,
            )
        };

        Ok(PolicyAuditOutcome {
            budget_status: audit_status(budget_truncated, invalid_budget_policy_count),
            pricing_status: audit_status(pricing_truncated, invalid_pricing_policy_count),
            invalid_budget_policy_count,
            invalid_pricing_policy_count,
            evidence: PolicyAuditEvidence {
                limits: PolicyLimitEvidence::from(self.limits),
                budget_rows_digest,
                pricing_rows_digest,
                pricing_audit_rows_digest,
            },
        })
    }
}

impl From<AiRestorePolicyAuditLimits> for PolicyLimitEvidence {
    fn from(value: AiRestorePolicyAuditLimits) -> Self {
        let budget = value.budget_management;
        let pricing = value.pricing_management;
        Self {
            budget_ceiling: budget.maximum_ceiling(),
            maximum_budget_policies_per_scope: budget.maximum_policies_per_scope(),
            maximum_budget_policies: value.maximum_budget_policies,
            maximum_fixed_call_microunits: pricing.maximum_fixed_call_microunits(),
            maximum_token_rate_microunits_per_million: pricing
                .maximum_token_rate_microunits_per_million(),
            maximum_builtin_tool_microunits_per_call: pricing
                .maximum_builtin_tool_microunits_per_call(),
            maximum_pricing_versions_per_route: pricing.maximum_versions_per_route(),
            maximum_pricing_policies: value.maximum_pricing_policies,
            maximum_audit_events: value.maximum_audit_events,
        }
    }
}

fn invalid_budget_policy_count(
    records: &[AiBudgetPolicyRecord],
    limits: AiBudgetPolicyManagementLimits,
) -> u64 {
    let mut invalid = BTreeSet::new();
    let mut scopes = BTreeMap::<&str, Vec<uuid::Uuid>>::new();
    for record in records {
        scopes
            .entry(record.scope_key.as_str())
            .or_default()
            .push(record.id);
        if !restored_budget_policy_is_valid(record, limits.maximum_ceiling()) {
            invalid.insert(record.id);
        }
    }
    for ids in scopes
        .values()
        .filter(|ids| ids.len() > limits.maximum_policies_per_scope())
    {
        invalid.extend(ids.iter().copied());
    }
    invalid.len() as u64
}

fn restored_budget_policy_is_valid(
    record: &AiBudgetPolicyRecord,
    maximum: AiBudgetAmounts,
) -> bool {
    let scope = AiScope {
        kind: record.scope_kind.clone(),
        id: record.scope_id.clone(),
        tenant_id: record.tenant_id.clone(),
    };
    let principal_is_valid = match (
        record.principal_kind.as_deref(),
        record.principal_subject.as_deref(),
    ) {
        (None, None) => true,
        (Some(kind), Some(subject)) => {
            valid_principal_kind(kind)
                && !subject.trim().is_empty()
                && subject.len() <= 512
                && !subject.chars().any(char::is_control)
        }
        _ => false,
    };
    let ceilings = [
        (record.maximum_input_tokens, maximum.input_tokens),
        (record.maximum_output_tokens, maximum.output_tokens),
        (record.maximum_tool_units, maximum.tool_units),
        (record.maximum_image_units, maximum.image_units),
        (record.maximum_cost_microunits, maximum.cost_microunits),
        (record.maximum_runs, maximum.runs),
    ];
    record.id != uuid::Uuid::nil()
        && record.row_version >= 0
        && record.updated_at > 0
        && valid_scope(&scope)
        && record.scope_key == crate::ai_scope_key(&scope)
        && principal_is_valid
        && matches!(
            record.interval_kind.as_str(),
            "minute" | "hour" | "day" | "month" | "lifetime"
        )
        && ceilings.iter().any(|(value, _)| value.is_some())
        && ceilings.into_iter().all(|(value, maximum)| {
            value.is_none_or(|value| u64::try_from(value).is_ok_and(|value| value <= maximum))
        })
}

fn valid_scope(scope: &AiScope) -> bool {
    !scope.kind.trim().is_empty()
        && scope.kind.len() <= 128
        && !scope.kind.chars().any(char::is_control)
        && !scope.id.trim().is_empty()
        && scope.id.len() <= 512
        && !scope.id.chars().any(char::is_control)
        && scope.tenant_id.as_ref().is_none_or(|tenant| {
            !tenant.trim().is_empty()
                && tenant.len() <= 512
                && !tenant.chars().any(char::is_control)
        })
}

fn valid_principal_kind(kind: &str) -> bool {
    kind == "user"
        || kind.strip_prefix("api_token:").is_some_and(|value| {
            !value.is_empty()
                && value.len() <= 64
                && value.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b'-')
                })
        })
}

fn invalid_pricing_policy_count(
    policies: &[AiPricingPolicyRecord],
    audits: &[AiAuditEventRecord],
    limits: AiPricingCatalogManagementLimits,
) -> u64 {
    let mut invalid = BTreeSet::new();
    let mut routes = BTreeMap::<(&str, &str, &str), Vec<uuid::Uuid>>::new();
    let mut policy_by_reference = BTreeMap::new();
    for policy in policies {
        policy_by_reference.insert(policy.version_reference.as_str(), policy);
        routes
            .entry((
                policy.scope_key.as_str(),
                policy.provider_kind.as_str(),
                policy.provider_model.as_str(),
            ))
            .or_default()
            .push(policy.id);
        if crate::orm_pricing::validate_restored_pricing_record(policy, limits).is_err() {
            invalid.insert(policy.id);
        }
    }
    for ids in routes
        .values()
        .filter(|ids| ids.len() > limits.maximum_versions_per_route())
    {
        invalid.extend(ids.iter().copied());
    }

    let mut audits_by_reference = BTreeMap::<&str, Vec<&AiAuditEventRecord>>::new();
    let mut orphan_or_malformed_audits = 0_u64;
    for audit in audits {
        audits_by_reference
            .entry(audit.resource_reference.as_str())
            .or_default()
            .push(audit);
        if !policy_by_reference.contains_key(audit.resource_reference.as_str()) {
            orphan_or_malformed_audits += 1;
        }
    }
    for policy in policies {
        let linked = audits_by_reference
            .get(policy.version_reference.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default();
        if linked.len() != 1 || !valid_pricing_creation_audit(linked[0], policy) {
            invalid.insert(policy.id);
        }
    }
    (invalid.len() as u64).saturating_add(orphan_or_malformed_audits)
}

fn valid_pricing_creation_audit(
    audit: &AiAuditEventRecord,
    policy: &AiPricingPolicyRecord,
) -> bool {
    audit.id != uuid::Uuid::nil()
        && audit.actor_principal_kind == policy.created_by_principal_kind
        && audit.actor_subject == policy.created_by_subject
        && audit.action == "ai.pricing_policy.create"
        && audit.resource_kind == "pricing_policy"
        && audit.resource_reference == policy.version_reference
        && audit.outcome == "allowed"
        && audit.reason_code == "immutable_pricing_version_created"
        && uuid::Uuid::parse_str(&audit.correlation_id).is_ok_and(|id| !id.is_nil())
        && audit.causation_id.is_none()
        && audit.policy_version.is_none()
        && audit.created_at >= policy.created_at
}

fn serialized_digest(value: &impl serde::Serialize) -> Result<String, AiError> {
    let encoded = serde_json::to_vec(value).map_err(|_| AiError::PersistenceFailed)?;
    Ok(hex::encode(Sha256::digest(encoded)))
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
                cancellation_request_id: None,
                cancellation_requested_at: None,
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

    fn policy_audit_limits(
        maximum_budget_policies: usize,
        maximum_pricing_policies: usize,
        maximum_audit_events: usize,
    ) -> AiRestorePolicyAuditLimits {
        let budget = AiBudgetPolicyManagementLimits::new(
            AiBudgetAmounts {
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
                tool_units: 1_000_000,
                image_units: 1_000_000,
                cost_microunits: 1_000_000,
                runs: 1_000_000,
            },
            10,
        )
        .expect("test budget limits should validate");
        let pricing = AiPricingCatalogManagementLimits::new(1_000_000, 1_000_000, 10)
            .expect("test pricing limits should validate")
            .with_maximum_builtin_tool_microunits_per_call(1_000_000);
        AiRestorePolicyAuditLimits::new(
            budget,
            pricing,
            maximum_budget_policies,
            maximum_pricing_policies,
            maximum_audit_events,
        )
        .expect("test policy-audit limits should validate")
    }

    async fn seed_budget_policy(
        database: &Database<SqliteBackend>,
        scope_key_override: Option<&str>,
        maximum_input_tokens: i64,
    ) {
        let scope = AiScope::new("project", "restore-project").with_tenant_id("tenant-1");
        AiBudgetPolicyRecord::insert(
            database,
            crate::persistence::CreateAiBudgetPolicyRecordInput {
                scope_key: scope_key_override
                    .map(str::to_owned)
                    .unwrap_or_else(|| crate::ai_scope_key(&scope)),
                scope_kind: scope.kind,
                scope_id: scope.id,
                tenant_id: scope.tenant_id,
                principal_kind: Some("user".to_owned()),
                principal_subject: Some("restore-user".to_owned()),
                interval_kind: "day".to_owned(),
                maximum_input_tokens: Some(maximum_input_tokens),
                maximum_output_tokens: Some(10_000),
                maximum_tool_units: None,
                maximum_image_units: None,
                maximum_cost_microunits: Some(100_000),
                maximum_runs: Some(100),
                enabled: true,
            },
        )
        .await
        .expect("budget policy should insert");
    }

    async fn seed_pricing_policy(
        database: &Database<SqliteBackend>,
        fixed_call_microunits: i64,
        creator_kind: &str,
        with_audit: bool,
    ) -> String {
        let id = Uuid::new_v4();
        let version_reference = format!("pricing:{id}");
        let scope = AiScope::new("project", "restore-project").with_tenant_id("tenant-1");
        AiPricingPolicyRecord::insert(
            database,
            crate::persistence::CreateAiPricingPolicyRecordInput {
                id,
                version_reference: version_reference.clone(),
                scope_key: crate::ai_scope_key(&scope),
                scope_kind: scope.kind,
                scope_id: scope.id,
                tenant_id: scope.tenant_id,
                provider_kind: "openai".to_owned(),
                provider_model: "gpt-restore".to_owned(),
                fixed_call_microunits,
                input_microunits_per_million: 100_000,
                cached_input_microunits_per_million: 50_000,
                output_microunits_per_million: 200_000,
                web_search_microunits_per_call: 10_000,
                file_search_microunits_per_call: 20_000,
                created_by_principal_kind: creator_kind.to_owned(),
                created_by_subject: "restore-admin".to_owned(),
            },
        )
        .await
        .expect("pricing policy should insert");
        if with_audit {
            seed_pricing_audit(database, &version_reference, creator_kind).await;
        }
        version_reference
    }

    async fn seed_pricing_audit(
        database: &Database<SqliteBackend>,
        version_reference: &str,
        creator_kind: &str,
    ) {
        AiAuditEventRecord::insert(
            database,
            crate::persistence::CreateAiAuditEventRecordInput {
                actor_principal_kind: creator_kind.to_owned(),
                actor_subject: "restore-admin".to_owned(),
                action: "ai.pricing_policy.create".to_owned(),
                resource_kind: "pricing_policy".to_owned(),
                resource_reference: version_reference.to_owned(),
                outcome: "allowed".to_owned(),
                reason_code: "immutable_pricing_version_created".to_owned(),
                correlation_id: Uuid::new_v4().to_string(),
                causation_id: None,
                policy_version: None,
            },
        )
        .await
        .expect("pricing creation audit should insert");
    }

    fn attachment_metadata_audit_limits(
        maximum_attachments: usize,
        maximum_artifacts: usize,
    ) -> AiRestoreAttachmentMetadataAuditLimits {
        AiRestoreAttachmentMetadataAuditLimits::new(
            AiAttachmentServiceLimits::default(),
            maximum_attachments,
            maximum_artifacts,
        )
        .expect("test attachment metadata-audit limits should validate")
    }

    async fn seed_attachment_session(database: &Database<SqliteBackend>) -> (Uuid, Uuid) {
        let session_id = Uuid::new_v4();
        let message_id = Uuid::new_v4();
        AiSessionRecord::insert(
            database,
            crate::persistence::CreateAiSessionRecordInput {
                id: session_id,
                owner_principal_kind: "user".to_owned(),
                owner_subject: "restore-attachment-user".to_owned(),
                tenant_id: Some("tenant-1".to_owned()),
                scope_kind: "project".to_owned(),
                scope_id: "restore-attachments".to_owned(),
                title: "Restore attachment test".to_owned(),
                title_revision: 0,
                title_source: "default".to_owned(),
                state: "active".to_owned(),
                stream_head: 0,
                message_head: 1,
                last_activity_at: 1_900_000_000,
                archived_at: None,
                deleted_at: None,
            },
        )
        .await
        .expect("attachment parent session should insert");
        AiMessageRecord::insert(
            database,
            crate::persistence::CreateAiMessageRecordInput {
                id: message_id,
                session_id,
                sequence: 1,
                message_role: "user".to_owned(),
                author_principal_kind: Some("user".to_owned()),
                author_subject: Some("restore-attachment-user".to_owned()),
                client_message_id: Some(Uuid::new_v4()),
                content_hash: Some("restore-attachment-content".to_owned()),
                run_id: None,
                provider_kind: None,
                provider_model: None,
                protected_preview: None,
                block_count: 0,
                completion_state: "complete".to_owned(),
                finalized_at: Some(1_900_000_000),
                content_purged_at: None,
            },
        )
        .await
        .expect("attachment parent message should insert");
        (session_id, message_id)
    }

    async fn seed_released_attachment(
        database: &Database<SqliteBackend>,
        session_id: Uuid,
        message_id: Option<Uuid>,
        blob_reference: &str,
    ) -> Uuid {
        let id = Uuid::new_v4();
        AiAttachmentRecord::insert(
            database,
            crate::persistence::CreateAiAttachmentRecordInput {
                id,
                owner_principal_kind: "user".to_owned(),
                owner_subject: "restore-attachment-user".to_owned(),
                session_id,
                message_id,
                blob_reference: Some(blob_reference.to_owned()),
                quarantine_blob_reference: None,
                safe_filename: "restored.txt".to_owned(),
                declared_mime: Some("text/plain".to_owned()),
                detected_mime: Some("text/plain".to_owned()),
                expected_byte_count: Some(7),
                byte_count: Some(7),
                sha256: Some("0".repeat(64)),
                upload_token_hash: None,
                upload_expires_at: Some(2_000_000_000),
                quarantine_state: "released".to_owned(),
                scan_state: "clean".to_owned(),
                processing_state: "complete".to_owned(),
                processing_expires_at: None,
                cleanup_generation: None,
                cleanup_lease_expires_at: None,
                cleanup_retry_count: None,
                cleanup_next_attempt_at: None,
                scanner_version: Some("restore-scanner-v1".to_owned()),
                acceptance_policy_version: Some("restore-policy-v1".to_owned()),
                rejection_code: None,
                finalized_at: Some(1_900_000_000),
                deleted_at: None,
            },
        )
        .await
        .expect("released attachment should insert");
        id
    }

    async fn seed_active_artifact(
        database: &Database<SqliteBackend>,
        attachment_id: Uuid,
        blob_reference: &str,
    ) -> Uuid {
        let id = Uuid::new_v4();
        AiAttachmentArtifactRecord::insert(
            database,
            crate::persistence::CreateAiAttachmentArtifactRecordInput {
                id,
                attachment_id,
                artifact_kind: "extracted_text".to_owned(),
                blob_reference: Some(blob_reference.to_owned()),
                protected_content: None,
                detected_mime: Some("text/plain".to_owned()),
                byte_count: 7,
                sha256: Some("1".repeat(64)),
                provider_kind: None,
                provider_profile_id: None,
                provider_reference: None,
                provider_expires_at: None,
                cleanup_state: None,
                cleanup_generation: None,
                cleanup_lease_expires_at: None,
                cleanup_retry_count: None,
                cleanup_next_attempt_at: None,
                deleted_at: None,
            },
        )
        .await
        .expect("active attachment artifact should insert");
        id
    }

    #[tokio::test]
    async fn attachment_metadata_graph_completes_without_claiming_object_bytes() {
        let database = database().await;
        let (session_id, message_id) = seed_attachment_session(&database).await;
        let attachment_id = seed_released_attachment(
            &database,
            session_id,
            Some(message_id),
            "ai-attachments/objects/test/attachment",
        )
        .await;
        seed_active_artifact(
            &database,
            attachment_id,
            "ai-attachments/objects/test/artifact",
        )
        .await;
        let limits = attachment_metadata_audit_limits(10, 10);
        let first = OrmAiRestoreFactCollector::new(database.clone())
            .with_attachment_metadata_audit(limits)
            .collect("module-fingerprint")
            .await
            .expect("valid attachment metadata graph should collect");
        let repeated = OrmAiRestoreFactCollector::new(database.clone())
            .with_attachment_metadata_audit(limits)
            .collect("module-fingerprint")
            .await
            .expect("attachment metadata collection should be deterministic");

        assert_eq!(first.digest(), repeated.digest());
        assert_eq!(first.facts().invalid_attachment_metadata_count, 0);
        assert_eq!(first.facts().invalid_attachment_object_count, 0);
        assert_eq!(
            first
                .audit_statuses()
                .get(&AiRestoreAuditKind::AttachmentMetadataGraph),
            Some(&AiRestoreAuditStatus::Complete)
        );
        assert_eq!(
            first
                .audit_statuses()
                .get(&AiRestoreAuditKind::AttachmentObjectBytes),
            Some(&AiRestoreAuditStatus::NotImplemented)
        );
        let plan = crate::AiRestoreReconciler::new("module-fingerprint")
            .plan_collected(&first)
            .expect("metadata-complete plan should hash");
        assert!(plan.plan().issues.iter().any(|issue| {
            issue.code == "AI_RESTORE_AUDIT_INCOMPLETE"
                && issue.resource_ref.as_deref() == Some("attachment_object_bytes")
        }));

        let narrower_service = AiAttachmentServiceLimits::new(6, 255, time::Duration::minutes(10))
            .expect("narrow attachment service limits should validate");
        let changed_limits = AiRestoreAttachmentMetadataAuditLimits::new(narrower_service, 10, 10)
            .expect("changed attachment metadata limits should validate");
        let changed = OrmAiRestoreFactCollector::new(database)
            .with_attachment_metadata_audit(changed_limits)
            .collect("module-fingerprint")
            .await
            .expect("changed attachment limits should collect");
        assert_ne!(first.digest(), changed.digest());
        assert_eq!(
            changed
                .audit_statuses()
                .get(&AiRestoreAuditKind::AttachmentMetadataGraph),
            Some(&AiRestoreAuditStatus::Invalid { count: 2 })
        );
    }

    #[tokio::test]
    async fn attachment_parent_corruption_and_orphan_artifacts_are_invalid() {
        let database = database().await;
        let (session_id, _) = seed_attachment_session(&database).await;
        let attachment_id = seed_released_attachment(
            &database,
            session_id,
            Some(Uuid::new_v4()),
            "ai-attachments/objects/test/orphan-message",
        )
        .await;
        seed_active_artifact(
            &database,
            Uuid::new_v4(),
            "ai-attachments/objects/test/orphan-artifact",
        )
        .await;
        seed_active_artifact(
            &database,
            attachment_id,
            "ai-attachments/objects/test/valid-artifact",
        )
        .await;

        let collected = OrmAiRestoreFactCollector::new(database.clone())
            .with_attachment_metadata_audit(attachment_metadata_audit_limits(10, 10))
            .collect("module-fingerprint")
            .await
            .expect("corrupt attachment graph should remain inspectable");

        assert_eq!(collected.facts().invalid_attachment_metadata_count, 3);
        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::AttachmentMetadataGraph),
            Some(&AiRestoreAuditStatus::Invalid { count: 3 })
        );
    }

    #[tokio::test]
    async fn deleting_sessions_and_tombstoned_artifact_parents_require_repair() {
        let deleting_database = database().await;
        let (session_id, message_id) = seed_attachment_session(&deleting_database).await;
        let attachment_id = seed_released_attachment(
            &deleting_database,
            session_id,
            Some(message_id),
            "ai-attachments/objects/test/deleting-session-parent",
        )
        .await;
        seed_active_artifact(
            &deleting_database,
            attachment_id,
            "ai-attachments/objects/test/deleting-session-artifact",
        )
        .await;
        AiSessionRecord::update_by_id(
            &deleting_database,
            &session_id,
            crate::persistence::UpdateAiSessionRecordInput {
                state: Some("deleting".to_owned()),
                deleted_at: Some(Some(1_900_000_001)),
                ..Default::default()
            },
        )
        .await
        .expect("deleting session should update")
        .expect("deleting session should remain present");
        let deleting = OrmAiRestoreFactCollector::new(deleting_database)
            .with_attachment_metadata_audit(attachment_metadata_audit_limits(10, 10))
            .collect("module-fingerprint")
            .await
            .expect("deleting-session graph should remain inspectable");
        assert_eq!(deleting.facts().invalid_attachment_metadata_count, 2);

        let tombstone_database = database().await;
        let (session_id, message_id) = seed_attachment_session(&tombstone_database).await;
        let attachment_id = seed_released_attachment(
            &tombstone_database,
            session_id,
            Some(message_id),
            "ai-attachments/objects/test/tombstoned-parent",
        )
        .await;
        seed_active_artifact(
            &tombstone_database,
            attachment_id,
            "ai-attachments/objects/test/orphaned-by-tombstone",
        )
        .await;
        AiAttachmentRecord::update_by_id(
            &tombstone_database,
            &attachment_id,
            crate::persistence::UpdateAiAttachmentRecordInput {
                blob_reference: Some(None),
                quarantine_blob_reference: Some(None),
                quarantine_state: Some("deleted".to_owned()),
                processing_state: Some("complete".to_owned()),
                deleted_at: Some(Some(1_900_000_001)),
                ..Default::default()
            },
        )
        .await
        .expect("attachment tombstone should update")
        .expect("attachment tombstone should remain present");
        let tombstoned = OrmAiRestoreFactCollector::new(tombstone_database)
            .with_attachment_metadata_audit(attachment_metadata_audit_limits(10, 10))
            .collect("module-fingerprint")
            .await
            .expect("tombstoned-parent graph should remain inspectable");
        assert_eq!(tombstoned.facts().invalid_attachment_metadata_count, 1);
        assert_eq!(
            tombstoned
                .audit_statuses()
                .get(&AiRestoreAuditKind::AttachmentMetadataGraph),
            Some(&AiRestoreAuditStatus::Invalid { count: 1 })
        );
    }

    #[tokio::test]
    async fn legacy_finalized_attachment_may_omit_original_expected_size() {
        let database = database().await;
        let (session_id, message_id) = seed_attachment_session(&database).await;
        let attachment_id = seed_released_attachment(
            &database,
            session_id,
            Some(message_id),
            "ai-attachments/objects/test/legacy-finalized",
        )
        .await;
        AiAttachmentRecord::update_by_id(
            &database,
            &attachment_id,
            crate::persistence::UpdateAiAttachmentRecordInput {
                expected_byte_count: Some(None),
                upload_expires_at: Some(None),
                scanner_version: Some(None),
                acceptance_policy_version: Some(None),
                ..Default::default()
            },
        )
        .await
        .expect("legacy expected-size update should succeed")
        .expect("legacy attachment should remain present");

        let collected = OrmAiRestoreFactCollector::new(database.clone())
            .with_attachment_metadata_audit(attachment_metadata_audit_limits(10, 10))
            .collect("module-fingerprint")
            .await
            .expect("legacy finalized attachment should collect");
        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::AttachmentMetadataGraph),
            Some(&AiRestoreAuditStatus::Complete)
        );

        AiAttachmentRecord::update_by_id(
            &database,
            &attachment_id,
            crate::persistence::UpdateAiAttachmentRecordInput {
                message_id: Some(None),
                quarantine_state: Some("ready".to_owned()),
                processing_state: Some("ready".to_owned()),
                finalized_at: Some(None),
                scanner_version: Some(Some("restore-scanner-v1".to_owned())),
                acceptance_policy_version: Some(Some("restore-policy-v1".to_owned())),
                ..Default::default()
            },
        )
        .await
        .expect("pre-release legacy shape should update")
        .expect("pre-release attachment should remain present");
        let ready = OrmAiRestoreFactCollector::new(database)
            .with_attachment_metadata_audit(attachment_metadata_audit_limits(10, 10))
            .collect("module-fingerprint")
            .await
            .expect("pre-release legacy shape should remain inspectable");
        assert_eq!(ready.facts().invalid_attachment_metadata_count, 1);
    }

    #[tokio::test]
    async fn unsafe_object_keys_and_wrong_message_authors_are_invalid() {
        let database = database().await;
        let (session_id, message_id) = seed_attachment_session(&database).await;
        AiMessageRecord::update_by_id(
            &database,
            &message_id,
            crate::persistence::UpdateAiMessageRecordInput {
                message_role: Some("assistant".to_owned()),
                author_subject: Some(Some("different-author".to_owned())),
                ..Default::default()
            },
        )
        .await
        .expect("message corruption should update")
        .expect("message should remain present");
        seed_released_attachment(
            &database,
            session_id,
            Some(message_id),
            "ai-attachments/objects/test/wrong-author",
        )
        .await;
        seed_released_attachment(&database, session_id, None, "../unsafe-object").await;

        let collected = OrmAiRestoreFactCollector::new(database)
            .with_attachment_metadata_audit(attachment_metadata_audit_limits(10, 10))
            .collect("module-fingerprint")
            .await
            .expect("unsafe attachment graph should remain inspectable");
        assert_eq!(collected.facts().invalid_attachment_metadata_count, 2);
        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::AttachmentMetadataGraph),
            Some(&AiRestoreAuditStatus::Invalid { count: 2 })
        );
    }

    #[tokio::test]
    async fn expired_attachment_requires_cleanup_generation_proof() {
        let database = database().await;
        let (session_id, _) = seed_attachment_session(&database).await;
        AiAttachmentRecord::insert(
            &database,
            crate::persistence::CreateAiAttachmentRecordInput {
                id: Uuid::new_v4(),
                owner_principal_kind: "user".to_owned(),
                owner_subject: "restore-attachment-user".to_owned(),
                session_id,
                message_id: None,
                blob_reference: None,
                quarantine_blob_reference: None,
                safe_filename: "expired.txt".to_owned(),
                declared_mime: Some("text/plain".to_owned()),
                detected_mime: None,
                expected_byte_count: Some(7),
                byte_count: None,
                sha256: None,
                upload_token_hash: None,
                upload_expires_at: Some(1_900_000_000),
                quarantine_state: "expired".to_owned(),
                scan_state: "failed".to_owned(),
                processing_state: "complete".to_owned(),
                processing_expires_at: None,
                cleanup_generation: None,
                cleanup_lease_expires_at: None,
                cleanup_retry_count: None,
                cleanup_next_attempt_at: None,
                scanner_version: None,
                acceptance_policy_version: None,
                rejection_code: Some("upload_ticket_expired".to_owned()),
                finalized_at: None,
                deleted_at: Some(1_900_000_001),
            },
        )
        .await
        .expect("corrupt expired attachment should insert");

        let collected = OrmAiRestoreFactCollector::new(database)
            .with_attachment_metadata_audit(attachment_metadata_audit_limits(10, 10))
            .collect("module-fingerprint")
            .await
            .expect("corrupt expired row should remain inspectable");
        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::AttachmentMetadataGraph),
            Some(&AiRestoreAuditStatus::Invalid { count: 1 })
        );
    }

    #[tokio::test]
    async fn redacted_transient_and_provider_references_fail_metadata_audit() {
        let database = database().await;
        let (session_id, message_id) = seed_attachment_session(&database).await;
        let attachment_id = seed_released_attachment(
            &database,
            session_id,
            Some(message_id),
            "ai-attachments/objects/test/provider-parent",
        )
        .await;
        AiAttachmentArtifactRecord::insert(
            &database,
            crate::persistence::CreateAiAttachmentArtifactRecordInput {
                id: Uuid::new_v4(),
                attachment_id,
                artifact_kind: "provider_file".to_owned(),
                blob_reference: None,
                protected_content: None,
                detected_mime: Some("text/plain".to_owned()),
                byte_count: 7,
                sha256: Some("1".repeat(64)),
                provider_kind: Some("openai".to_owned()),
                provider_profile_id: Some("restore-profile".to_owned()),
                provider_reference: Some(ORM_BACKUP_REDACTED_VALUE.to_owned()),
                provider_expires_at: None,
                cleanup_state: None,
                cleanup_generation: None,
                cleanup_lease_expires_at: None,
                cleanup_retry_count: None,
                cleanup_next_attempt_at: None,
                deleted_at: None,
            },
        )
        .await
        .expect("redacted provider artifact should insert");
        AiAttachmentRecord::insert(
            &database,
            crate::persistence::CreateAiAttachmentRecordInput {
                id: Uuid::new_v4(),
                owner_principal_kind: "user".to_owned(),
                owner_subject: "restore-attachment-user".to_owned(),
                session_id,
                message_id: None,
                blob_reference: None,
                quarantine_blob_reference: None,
                safe_filename: "pending.txt".to_owned(),
                declared_mime: Some("text/plain".to_owned()),
                detected_mime: None,
                expected_byte_count: Some(7),
                byte_count: None,
                sha256: None,
                upload_token_hash: None,
                upload_expires_at: Some(2_000_000_000),
                quarantine_state: "pending_upload".to_owned(),
                scan_state: "pending".to_owned(),
                processing_state: "pending".to_owned(),
                processing_expires_at: None,
                cleanup_generation: None,
                cleanup_lease_expires_at: None,
                cleanup_retry_count: None,
                cleanup_next_attempt_at: None,
                scanner_version: None,
                acceptance_policy_version: None,
                rejection_code: None,
                finalized_at: None,
                deleted_at: None,
            },
        )
        .await
        .expect("redacted pending attachment should insert");

        let collected = OrmAiRestoreFactCollector::new(database)
            .with_attachment_metadata_audit(attachment_metadata_audit_limits(10, 10))
            .collect("module-fingerprint")
            .await
            .expect("redacted graph should remain inspectable");
        assert_eq!(collected.facts().invalid_attachment_metadata_count, 2);
        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::AttachmentMetadataGraph),
            Some(&AiRestoreAuditStatus::Invalid { count: 2 })
        );
    }

    #[tokio::test]
    async fn duplicate_object_ownership_and_scan_bounds_fail_closed() {
        let database = database().await;
        let (session_id, message_id) = seed_attachment_session(&database).await;
        let mut attachment_ids = Vec::new();
        for _ in 0..2 {
            attachment_ids.push(
                seed_released_attachment(
                    &database,
                    session_id,
                    Some(message_id),
                    "ai-attachments/objects/test/shared",
                )
                .await,
            );
        }
        let duplicate = OrmAiRestoreFactCollector::new(database.clone())
            .with_attachment_metadata_audit(attachment_metadata_audit_limits(10, 10))
            .collect("module-fingerprint")
            .await
            .expect("duplicate object ownership should remain inspectable");
        assert_eq!(duplicate.facts().invalid_attachment_metadata_count, 2);
        assert_eq!(
            duplicate
                .audit_statuses()
                .get(&AiRestoreAuditKind::AttachmentMetadataGraph),
            Some(&AiRestoreAuditStatus::Invalid { count: 2 })
        );

        let bounded = OrmAiRestoreFactCollector::new(database.clone())
            .with_attachment_metadata_audit(attachment_metadata_audit_limits(1, 10))
            .collect("module-fingerprint")
            .await
            .expect("overbound attachment scan should remain inspectable");
        assert_eq!(bounded.facts().invalid_attachment_metadata_count, 0);
        assert_eq!(
            bounded
                .audit_statuses()
                .get(&AiRestoreAuditKind::AttachmentMetadataGraph),
            Some(&AiRestoreAuditStatus::LimitExceeded)
        );

        seed_active_artifact(
            &database,
            attachment_ids[0],
            "ai-attachments/objects/test/bounded-artifact-1",
        )
        .await;
        seed_active_artifact(
            &database,
            attachment_ids[0],
            "ai-attachments/objects/test/bounded-artifact-2",
        )
        .await;
        let artifact_bounded = OrmAiRestoreFactCollector::new(database)
            .with_attachment_metadata_audit(attachment_metadata_audit_limits(10, 1))
            .collect("module-fingerprint")
            .await
            .expect("overbound artifact scan should remain inspectable");
        assert_eq!(
            artifact_bounded.facts().invalid_attachment_metadata_count,
            0
        );
        assert_eq!(
            artifact_bounded
                .audit_statuses()
                .get(&AiRestoreAuditKind::AttachmentMetadataGraph),
            Some(&AiRestoreAuditStatus::LimitExceeded)
        );
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
            first
                .audit_statuses()
                .get(&AiRestoreAuditKind::AttachmentMetadataGraph),
            Some(&AiRestoreAuditStatus::NotImplemented)
        );

        let bound = crate::AiRestoreReconciler::new("module-fingerprint")
            .plan_collected(&first)
            .expect("collected plan should hash");
        assert_eq!(bound.facts_digest(), first.digest());
        assert_eq!(bound.plan().fatal_issue_count(), 15);
        assert!(bound.plan().issues.iter().any(|issue| {
            issue.code == "AI_RESTORE_AUDIT_INCOMPLETE"
                && issue.resource_ref.as_deref() == Some("attachment_metadata_graph")
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
        assert_eq!(missing_plan.plan().fatal_issue_count(), 19);
    }

    #[tokio::test]
    async fn configured_policy_audits_complete_and_bind_deployment_limits() {
        let database = database().await;
        seed_budget_policy(&database, None, 100_000).await;
        seed_pricing_policy(&database, 10_000, "user", true).await;
        let limits = policy_audit_limits(10, 10, 10);
        let collected = OrmAiRestoreFactCollector::new(database.clone())
            .with_policy_audits(limits)
            .collect("module-fingerprint")
            .await
            .expect("policy graphs should collect");

        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::BudgetPolicies),
            Some(&AiRestoreAuditStatus::Complete)
        );
        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::PricingPolicies),
            Some(&AiRestoreAuditStatus::Complete)
        );
        assert_eq!(collected.facts().invalid_budget_policy_count, 0);
        assert_eq!(collected.facts().invalid_pricing_policy_count, 0);
        let plan = crate::AiRestoreReconciler::new("module-fingerprint")
            .plan_collected(&collected)
            .expect("policy-complete plan should hash");
        assert_eq!(plan.plan().fatal_issue_count(), 13);
        let repeated = OrmAiRestoreFactCollector::new(database.clone())
            .with_policy_audits(limits)
            .collect("module-fingerprint")
            .await
            .expect("same-timestamp policy rows should collect deterministically");
        assert_eq!(collected.digest(), repeated.digest());

        let wider_pricing = AiPricingCatalogManagementLimits::new(2_000_000, 2_000_000, 10)
            .expect("wider test pricing limits should validate")
            .with_maximum_builtin_tool_microunits_per_call(2_000_000);
        let changed_limits =
            AiRestorePolicyAuditLimits::new(limits.budget_management(), wider_pricing, 10, 10, 10)
                .expect("changed policy limits should validate");
        let changed = OrmAiRestoreFactCollector::new(database)
            .with_policy_audits(changed_limits)
            .collect("module-fingerprint")
            .await
            .expect("changed deployment limits should collect");
        assert_ne!(collected.digest(), changed.digest());
    }

    #[tokio::test]
    async fn policy_corruption_and_missing_creation_audit_are_invalid() {
        let database = database().await;
        seed_budget_policy(&database, Some("wrong-scope-key"), 2_000_000).await;
        seed_pricing_policy(&database, 2_000_000, "user", false).await;
        let collected = OrmAiRestoreFactCollector::new(database)
            .with_policy_audits(policy_audit_limits(10, 10, 10))
            .collect("module-fingerprint")
            .await
            .expect("invalid policy rows should be represented safely");

        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::BudgetPolicies),
            Some(&AiRestoreAuditStatus::Invalid { count: 1 })
        );
        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::PricingPolicies),
            Some(&AiRestoreAuditStatus::Invalid { count: 1 })
        );
        assert_eq!(collected.facts().invalid_budget_policy_count, 1);
        assert_eq!(collected.facts().invalid_pricing_policy_count, 1);
    }

    #[tokio::test]
    async fn malformed_creator_duplicate_and_orphan_pricing_audits_are_invalid() {
        let database = database().await;
        let malformed = seed_pricing_policy(&database, 10_000, "bogus\0kind", true).await;
        seed_pricing_audit(&database, &malformed, "bogus\0kind").await;
        seed_pricing_audit(&database, &format!("pricing:{}", Uuid::new_v4()), "user").await;
        let collected = OrmAiRestoreFactCollector::new(database)
            .with_policy_audits(policy_audit_limits(10, 10, 10))
            .collect("module-fingerprint")
            .await
            .expect("corrupt pricing audit graph should remain inspectable");

        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::PricingPolicies),
            Some(&AiRestoreAuditStatus::Invalid { count: 2 })
        );
        assert_eq!(collected.facts().invalid_pricing_policy_count, 2);
    }

    #[tokio::test]
    async fn policy_scope_and_route_cardinality_are_integrity_failures() {
        let database = database().await;
        seed_budget_policy(&database, None, 100_000).await;
        seed_budget_policy(&database, None, 100_000).await;
        seed_pricing_policy(&database, 10_000, "user", true).await;
        seed_pricing_policy(&database, 10_000, "user", true).await;
        let budget = AiBudgetPolicyManagementLimits::new(
            AiBudgetAmounts {
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
                tool_units: 1_000_000,
                image_units: 1_000_000,
                cost_microunits: 1_000_000,
                runs: 1_000_000,
            },
            1,
        )
        .expect("cardinality budget limits should validate");
        let pricing = AiPricingCatalogManagementLimits::new(1_000_000, 1_000_000, 1)
            .expect("cardinality pricing limits should validate")
            .with_maximum_builtin_tool_microunits_per_call(1_000_000);
        let limits = AiRestorePolicyAuditLimits::new(budget, pricing, 10, 10, 10)
            .expect("cardinality audit limits should validate");
        let collected = OrmAiRestoreFactCollector::new(database)
            .with_policy_audits(limits)
            .collect("module-fingerprint")
            .await
            .expect("over-cardinality policy graphs should remain inspectable");

        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::BudgetPolicies),
            Some(&AiRestoreAuditStatus::Invalid { count: 2 })
        );
        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::PricingPolicies),
            Some(&AiRestoreAuditStatus::Invalid { count: 2 })
        );
    }

    #[tokio::test]
    async fn policy_audit_bounds_never_return_partial_success() {
        let database = database().await;
        seed_budget_policy(&database, None, 100_000).await;
        seed_budget_policy(&database, None, 100_000).await;
        seed_pricing_policy(&database, 10_000, "user", true).await;
        seed_pricing_policy(&database, 10_000, "user", true).await;
        let collected = OrmAiRestoreFactCollector::new(database)
            .with_policy_audits(policy_audit_limits(1, 1, 10))
            .collect("module-fingerprint")
            .await
            .expect("bounded policy collection should remain inspectable");

        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::BudgetPolicies),
            Some(&AiRestoreAuditStatus::LimitExceeded)
        );
        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::PricingPolicies),
            Some(&AiRestoreAuditStatus::LimitExceeded)
        );
        assert_eq!(collected.facts().invalid_budget_policy_count, 0);
        assert_eq!(collected.facts().invalid_pricing_policy_count, 0);
    }

    #[tokio::test]
    async fn pricing_audit_bound_applies_to_the_complete_audit_history_scan() {
        let database = database().await;
        seed_pricing_policy(&database, 10_000, "user", true).await;
        for index in 0..2 {
            AiAuditEventRecord::insert(
                &database,
                crate::persistence::CreateAiAuditEventRecordInput {
                    actor_principal_kind: "user".to_owned(),
                    actor_subject: "restore-admin".to_owned(),
                    action: "ai.unrelated.audit".to_owned(),
                    resource_kind: "unrelated".to_owned(),
                    resource_reference: format!("unrelated-{index}"),
                    outcome: "allowed".to_owned(),
                    reason_code: "unrelated".to_owned(),
                    correlation_id: Uuid::new_v4().to_string(),
                    causation_id: None,
                    policy_version: None,
                },
            )
            .await
            .expect("unrelated audit should insert");
        }
        let collected = OrmAiRestoreFactCollector::new(database)
            .with_policy_audits(policy_audit_limits(10, 10, 2))
            .collect("module-fingerprint")
            .await
            .expect("whole-history bound should remain inspectable");

        assert_eq!(
            collected
                .audit_statuses()
                .get(&AiRestoreAuditKind::PricingPolicies),
            Some(&AiRestoreAuditStatus::LimitExceeded)
        );
        assert_eq!(collected.facts().invalid_pricing_policy_count, 0);
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
