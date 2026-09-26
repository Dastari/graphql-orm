//! Closed native approval checkpoint proofs. Prior callback effects are never replay work.

use super::*;
use crate::orm_native_approvals::{StoredNativeControlReceipt, native_receipt_hash};
use crate::orm_tools::NativeApprovalPreparation;
use crate::{AiNativeToolControlKind, AiPreparedNativeApproval};

pub(super) const SOURCE_KIND: &str = "native_approval_provider_turn_persisted";
pub(super) const OUTCOME_KIND: &str = "native_approved_outcome_persisted";

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct NativeApprovalSource {
    format_version: u8,
    checkpoint_kind: String,
    provider_turns: u32,
    total_tool_calls: u32,
    scope: AiScope,
    rule_fingerprint: String,
    rule_usage: AiRuleRunUsage,
    correlation_id: String,
    result_egress_route: serde_json::Value,
    provider_result: serde_json::Value,
    outcomes: Vec<NativeOutcomeSnapshot>,
    candidate_id: Uuid,
    pending_call_index: usize,
}

impl std::fmt::Debug for NativeApprovalSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeApprovalSource")
            .field("candidate_id", &self.candidate_id)
            .field("pending_call_index", &self.pending_call_index)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeOutcomeSnapshot {
    format_version: u8,
    kind: NativeOutcomeKind,
    call_index: usize,
    provider_call_id: String,
    tool_call_id: Uuid,
    model_input: ModelInputBlock,
    egress_manifest: AiEgressManifest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    control_kind: Option<AiNativeToolControlKind>,
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum NativeOutcomeKind {
    Application,
    FrameworkControl,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredPausedControlReceipt {
    format_version: u8,
    kind: String,
    pending_tool_call_id: Uuid,
    model_input: ModelInputBlock,
    egress_manifest: AiEgressManifest,
    egress_decision_id: Uuid,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeParkedWait {
    format_version: u8,
    kind: String,
    approval_id: Uuid,
    tool_call_id: Uuid,
    source_checkpoint_id: Uuid,
    source_checkpoint_fingerprint: String,
    provider_session_binding_id: Uuid,
    provider_session_park_generation: i64,
    provider_session_continuation_fingerprint: String,
}

struct ValidatedNativeSource {
    provider: ProviderResultSnapshot,
    candidate: AiNativeApprovalCandidateRecord,
    preparation: NativeApprovalPreparation,
    ordinary_tools: Vec<PreparedCoordinatorCheckpointTool>,
}

impl OrmAiCoordinatorCheckpointService {
    /// Protects a completed native turn containing one prepared approval and
    /// every ordered application result or no-effect control receipt.
    ///
    /// Provider and callback usage must already be settled and counted once.
    /// This checkpoint grants no approval and never replays earlier effects.
    ///
    /// # Errors
    ///
    /// Rejects missing/duplicate/reordered outcomes, changed protected evidence,
    /// uncommitted usage, stale authorization, or an inexact pending candidate.
    #[allow(clippy::too_many_arguments)]
    pub async fn persist_native_approval_provider_turn(
        &self,
        lease: &AiRunLease,
        result: &AiProviderCallResult,
        candidate: &AiPreparedNativeApproval,
        scope: &AiScope,
        correlation_id: &str,
        route: &AiToolResultEgressRoute,
        rules: &AiResolvedRuleSet,
        rule_usage: AiRuleRunUsage,
        provider_turns: u32,
        total_tool_calls: u32,
    ) -> Result<AiRunLease, AiError> {
        if lease.state() != crate::AiRunState::Running
            || result.run_id() != lease.run_id()
            || result.session_id() != lease.session_id()
            || result.attempt_id() != lease.attempt_id()
            || result.lease_generation() != lease.lease_generation()
            || candidate.lease().run_id() != lease.run_id()
            || candidate.lease().attempt_id() != lease.attempt_id()
            || candidate.lease().lease_generation() != lease.lease_generation()
        {
            return Err(AiError::Conflict);
        }
        let outcomes = result
            .native_outcome_checkpoint_values()?
            .into_iter()
            .map(|value| serde_json::from_value(value).map_err(|_| AiError::Conflict))
            .collect::<Result<Vec<NativeOutcomeSnapshot>, _>>()?;
        let pending_call_index = outcomes
            .iter()
            .position(|outcome| outcome.tool_call_id == candidate.tool_call_id().0)
            .ok_or(AiError::Conflict)?;
        let source = NativeApprovalSource {
            format_version: 1,
            checkpoint_kind: SOURCE_KIND.to_owned(),
            provider_turns,
            total_tool_calls,
            scope: scope.clone(),
            rule_fingerprint: rules.fingerprint().to_owned(),
            rule_usage,
            correlation_id: correlation_id.to_owned(),
            result_egress_route: route.checkpoint_value(),
            provider_result: result.checkpoint_value(),
            outcomes,
            candidate_id: candidate.tool_call_id().0,
            pending_call_index,
        };
        let (principal, policy) = self.current_policy(lease, scope).await?;
        let validated = self
            .validate_native_source(lease, &source, &policy, "prepared", None, false)
            .await?;
        let checkpoint_id = Uuid::new_v4();
        let clear = serde_json::to_value(&source).map_err(|_| AiError::PersistenceFailed)?;
        enforce_size(&clear, self.limits.maximum_state_bytes)?;
        let protected_state = self
            .protect(&policy, native_context(checkpoint_id, scope), clear)
            .await?;
        enforce_size(&protected_state, self.limits.maximum_state_bytes)?;
        let (current, current_policy) = self.current_policy(lease, scope).await?;
        let current_rules = self.rule_resolver.resolve_rules(lease, scope).await?;
        if current.reference() != principal.reference()
            || current_policy != policy
            || current_rules.rules().fingerprint() != rules.fingerprint()
            || rule_usage.validate(&current_rules).is_err()
        {
            return Err(AiError::ReauthorizationFailed);
        }
        let provider = validated.provider;
        let checkpoint_hash = coordinator_checkpoint_hash(
            lease.run_id(),
            lease.attempt_id(),
            lease.lease_generation(),
            checkpoint_id,
            SOURCE_KIND,
            provider.provider_kind.as_str(),
            &provider.provider_model,
            provider.provider_response_id.as_deref(),
            provider.budget_reservation_id,
            &protected_state,
        )?;
        self.run_service
            .append_coordinator_checkpoint(
                lease,
                PreparedCoordinatorCheckpoint {
                    native_binding: Some(
                        crate::orm_runs::PreparedNativeCheckpointBinding::Source {
                            candidate_id: source.candidate_id,
                            pending_call_index: source.pending_call_index,
                            callback_count: source.outcomes.len(),
                        },
                    ),
                    id: checkpoint_id,
                    checkpoint_kind: SOURCE_KIND.to_owned(),
                    provider_kind: provider.provider_kind.as_str().to_owned(),
                    provider_model: provider.provider_model,
                    provider_response_id: provider.provider_response_id,
                    budget_reservation_id: provider.budget_reservation_id,
                    protected_state,
                    checkpoint_hash,
                    completed_tools: validated.ordinary_tools,
                },
            )
            .await
    }

    async fn validate_native_source(
        &self,
        lease: &AiRunLease,
        source: &NativeApprovalSource,
        policy: &AiContentProtectionPolicy,
        candidate_state: &str,
        source_checkpoint: Option<Uuid>,
        completed: bool,
    ) -> Result<ValidatedNativeSource, AiError> {
        let provider: ProviderResultSnapshot =
            serde_json::from_value(source.provider_result.clone())
                .map_err(|_| AiError::Conflict)?;
        if source.format_version != 1
            || source.checkpoint_kind != SOURCE_KIND
            || source.provider_turns == 0
            || !valid_reference(&source.correlation_id)
            || source.rule_fingerprint.len() != 64
            || source.outcomes.is_empty()
            || source.outcomes.len() > 256
            || source.outcomes.len() != provider.tool_calls.len()
            || source.pending_call_index >= source.outcomes.len()
            || source.total_tool_calls
                < u32::try_from(source.outcomes.len()).map_err(|_| AiError::Conflict)?
            || source.rule_usage.provider_calls() != u64::from(source.provider_turns)
            || source.rule_usage.steps()
                != u64::from(source.provider_turns) + u64::from(source.total_tool_calls)
            || provider.session_id != lease.session_id().0
            || provider.run_id != lease.run_id().0
            || provider.lease_generation <= 0
            || provider.lease_generation > lease.lease_generation()
            || (candidate_state == "prepared"
                && (provider.attempt_id != lease.attempt_id()
                    || provider.lease_generation != lease.lease_generation()))
            || !(1..=2).contains(&provider.format_version)
            || provider
                .provider_response_id
                .as_deref()
                .is_none_or(|id| !valid_reference(id))
        {
            return Err(AiError::Conflict);
        }
        validate_native_order(
            source.pending_call_index,
            source.candidate_id,
            &provider.tool_calls,
            source.outcomes.iter().map(|outcome| {
                (
                    outcome.call_index,
                    outcome.tool_call_id,
                    outcome.provider_call_id.as_str(),
                    outcome.kind,
                    outcome.control_kind,
                )
            }),
        )?;
        let session =
            AiSessionRecord::find_by_id(self.run_service.database(), &lease.session_id().0)
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
                .ok_or(AiError::NotFound)?;
        validate_session_binding(&session, lease, &source.scope)?;
        let budget = AiBudgetReservationRecord::find_by_id(
            self.run_service.database(),
            &provider.budget_reservation_id,
        )
        .await
        .map_err(|error| map_orm(OrmPublicError::from(error)))?
        .ok_or(AiError::Conflict)?;
        if !checkpoint_budget_matches(
            &budget,
            lease,
            provider.attempt_id,
            provider.lease_generation,
            &source.scope,
            &principal_reference_kind(lease.principal_reference()),
            provider.provider_kind.as_str(),
            &provider.provider_model,
        ) || budget.reasoning_effort != provider.reasoning_effort.as_str()
        {
            return Err(AiError::Conflict);
        }
        let candidate = AiNativeApprovalCandidateRecord::find_by_id(
            self.run_service.database(),
            &source.candidate_id,
        )
        .await
        .map_err(|error| map_orm(OrmPublicError::from(error)))?
        .ok_or(AiError::Conflict)?;
        if candidate.state != candidate_state
            || candidate.run_id != lease.run_id().0
            || candidate.attempt_id != provider.attempt_id
            || candidate.lease_generation != provider.lease_generation
            || candidate.budget_reservation_id != provider.budget_reservation_id
            || candidate.payload_purged_at.is_some()
            || candidate.control_receipt_hash.is_none()
            || candidate.control_egress_decision_id.is_none()
            || candidate.control_egress_manifest_hash.is_none()
            || (candidate_state == "prepared"
                && (candidate.final_approval_id.is_some()
                    || candidate.settled_checkpoint_id.is_some()))
            || (candidate_state == "finalized"
                && (candidate.final_approval_id.is_none()
                    || candidate.settled_checkpoint_id != source_checkpoint))
        {
            return Err(AiError::Conflict);
        }
        let opened = self
            .open(
                policy,
                ContentProtectionContext {
                    entity: "graphql_orm_ai_native_approval_candidates".to_owned(),
                    row_id: candidate.id.to_string(),
                    field: "protected_preparation".to_owned(),
                    scope: source.scope.clone(),
                },
                candidate
                    .protected_preparation
                    .as_ref()
                    .ok_or(AiError::Conflict)?,
            )
            .await?;
        let preparation: NativeApprovalPreparation =
            serde_json::from_value(opened).map_err(|_| AiError::Conflict)?;
        preparation.binding.validate(&preparation.preview)?;
        if preparation.context.scope != source.scope
            || preparation.context.tool_call_index != source.pending_call_index
            || preparation.context.provider_turn_index != source.provider_turns - 1
            || preparation.context.correlation_id != source.correlation_id
            || preparation.binding.tool_call_id.0 != candidate.id
            || preparation.binding.session_id != lease.session_id()
            || preparation.binding.stable_hash() != candidate.binding_hash
            || preparation.binding.preview_hash != candidate.preview_hash
            || !matches!(
                preparation.tool_maturity,
                ToolMaturity::AutonomousWrite | ToolMaturity::SupervisedWrite
            )
        {
            return Err(AiError::Conflict);
        }
        let rules = self
            .rule_resolver
            .resolve_rules(lease, &source.scope)
            .await?;
        if rules.rules().fingerprint() != source.rule_fingerprint
            || source.rule_usage.validate(&rules).is_err()
            || rules.rules().constrain_tool(
                &preparation.binding.tool_fingerprint,
                preparation.tool_maturity,
                AiApprovalRule::OneShot,
            ) != Some(AiApprovalRule::OneShot)
        {
            return Err(AiError::ReauthorizationFailed);
        }
        let route =
            AiToolResultEgressRoute::from_checkpoint_value(source.result_egress_route.clone())?;
        let mut ids = BTreeSet::new();
        let mut provider_ids = BTreeSet::new();
        let mut ordinary_tools = Vec::new();
        for (index, (outcome, requested)) in
            source.outcomes.iter().zip(&provider.tool_calls).enumerate()
        {
            if outcome.format_version != 1
                || outcome.call_index != index
                || outcome.provider_call_id != requested.call_id
                || !ids.insert(outcome.tool_call_id)
                || !provider_ids.insert(&outcome.provider_call_id)
                || !valid_reference(&requested.call_id)
                || requested.tool_fingerprint.len() != 64
            {
                return Err(AiError::Conflict);
            }
            let ModelInputBlock::ToolResult {
                call_id,
                tool_id,
                output,
            } = &outcome.model_input
            else {
                return Err(AiError::Conflict);
            };
            if call_id != &requested.call_id || tool_id != &requested.tool_id {
                return Err(AiError::Conflict);
            }
            let call =
                AiToolCallRecord::find_by_id(self.run_service.database(), &outcome.tool_call_id)
                    .await
                    .map_err(|error| map_orm(OrmPublicError::from(error)))?
                    .ok_or(AiError::Conflict)?;
            let step = AiRunStepRecord::find_by_id(self.run_service.database(), &call.id)
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
                .ok_or(AiError::Conflict)?;
            if call.run_id != provider.run_id
                || call.payload_purged_at.is_some()
                || step.step_kind != "application_tool"
                || (index != source.pending_call_index
                    && call.lease_generation != provider.lease_generation)
                || (index == source.pending_call_index
                    && (call.lease_generation < provider.lease_generation
                        || call.lease_generation > lease.lease_generation()
                        || (!completed && call.lease_generation != lease.lease_generation())))
                || call.provider_kind.as_deref() != Some(provider.provider_kind.as_str())
                || call.provider_model.as_deref() != Some(provider.provider_model.as_str())
                || call.provider_response_id != provider.provider_response_id
                || call.budget_reservation_id != Some(provider.budget_reservation_id)
                || call.provider_call_id != requested.call_id
                || call.tool_id != requested.tool_id
                || call.tool_fingerprint != requested.tool_fingerprint
                || call.tool_call_index != i64::try_from(index).map_err(|_| AiError::Conflict)?
                || call.provider_turn_index != i64::from(source.provider_turns - 1)
                || call.argument_hash != canonical_json_hash(&requested.arguments)?
                || step.run_id != call.run_id
                || step.lease_generation != call.lease_generation
                || step.state
                    != if candidate_state == "finalized"
                        && index == source.pending_call_index
                        && !completed
                    {
                        "running"
                    } else {
                        call.state.as_str()
                    }
            {
                return Err(AiError::Conflict);
            }
            let arguments = self
                .open(
                    policy,
                    ContentProtectionContext {
                        entity: "graphql_orm_ai_tool_calls".to_owned(),
                        row_id: call.id.to_string(),
                        field: "protected_arguments".to_owned(),
                        scope: source.scope.clone(),
                    },
                    call.protected_arguments.as_ref().ok_or(AiError::Conflict)?,
                )
                .await?;
            if arguments != requested.arguments {
                return Err(AiError::Conflict);
            }
            let manifest = &outcome.egress_manifest;
            if !route.matches_manifest(
                manifest,
                lease,
                &source.scope,
                provider.provider_kind.as_str(),
                &provider.provider_model,
            ) || manifest.sources.len() != 1
                || manifest.capability != crate::AiEgressCapability::ToolResult
            {
                return Err(AiError::EgressDenied);
            }
            let decision_id = match outcome.kind {
                NativeOutcomeKind::Application => {
                    if outcome.control_kind.is_some()
                        || index == source.pending_call_index
                        || !matches!(call.state.as_str(), "completed" | "execution_failed")
                        || call.completed_at.is_none()
                        || step.finished_at.is_none()
                        || call.approval_id.is_some()
                        || !matches!(
                            call.risk.as_str(),
                            "read_only" | "low_risk_write" | "non_idempotent_write"
                        )
                        || (index > source.pending_call_index && call.risk != "read_only")
                        || manifest.sources[0].kind != "application_tool_result"
                        || manifest.sources[0].reference != call.id.to_string()
                        || call.result_egress_manifest_hash.as_deref()
                            != Some(manifest.stable_hash().as_str())
                    {
                        return Err(AiError::Conflict);
                    }
                    let retained = self
                        .open(
                            policy,
                            ContentProtectionContext {
                                entity: "graphql_orm_ai_tool_calls".to_owned(),
                                row_id: call.id.to_string(),
                                field: "protected_result".to_owned(),
                                scope: source.scope.clone(),
                            },
                            call.protected_result.as_ref().ok_or(AiError::Conflict)?,
                        )
                        .await?;
                    if retained != *output {
                        return Err(AiError::Conflict);
                    }
                    let maturity = if call.risk == "read_only" {
                        ToolMaturity::ReadOnly
                    } else {
                        ToolMaturity::AutonomousWrite
                    };
                    if rules.rules().constrain_tool(
                        &call.tool_fingerprint,
                        maturity,
                        AiApprovalRule::None,
                    ) != Some(AiApprovalRule::None)
                    {
                        return Err(AiError::ReauthorizationFailed);
                    }
                    ordinary_tools.push(PreparedCoordinatorCheckpointTool {
                        id: call.id,
                        provider_call_id: call.provider_call_id,
                        tool_id: call.tool_id,
                        result_egress_manifest_hash: manifest.stable_hash(),
                    });
                    call.result_egress_decision_id.ok_or(AiError::Conflict)?
                }
                NativeOutcomeKind::FrameworkControl => {
                    let kind = outcome.control_kind.ok_or(AiError::Conflict)?;
                    if *output != kind.model_value()
                        || manifest.sources[0].kind != "native_tool_control_receipt"
                        || manifest.sources[0].classification != crate::DataClassification::Internal
                        || manifest.sources[0].trust != crate::AiSourceTrust::TrustedRuntime
                    {
                        return Err(AiError::Conflict);
                    }
                    let receipt_binding = match kind {
                        AiNativeToolControlKind::ApprovalPending => {
                            json!({"providerCallId":call.provider_call_id,"toolId":call.tool_id,"output":output})
                        }
                        AiNativeToolControlKind::ConsequentialCallsPaused => {
                            serde_json::to_value(&outcome.model_input)
                                .map_err(|_| AiError::Conflict)?
                        }
                    };
                    let expected_source =
                        format!("v1:{}:{}", call.id, native_receipt_hash(&receipt_binding)?);
                    if manifest.sources[0].reference != expected_source
                        || manifest.estimated_bytes
                            != u64::try_from(
                                serde_json::to_vec(&receipt_binding)
                                    .map_err(|_| AiError::Conflict)?
                                    .len(),
                            )
                            .map_err(|_| AiError::Conflict)?
                    {
                        return Err(AiError::Conflict);
                    }
                    let (entity, field, protected, expected_hash, decision_id) = match kind {
                        AiNativeToolControlKind::ApprovalPending => {
                            if index != source.pending_call_index
                                || call.id != candidate.id
                                || (!completed
                                    && !matches!(
                                        (candidate_state, call.state.as_str()),
                                        ("prepared", "approval_prepared")
                                            | ("finalized", "waiting_approval")
                                    ))
                                || (completed
                                    && !matches!(
                                        call.state.as_str(),
                                        "completed" | "execution_failed"
                                    ))
                                || call.argument_hash != preparation.binding.argument_hash
                                || call.tool_fingerprint != preparation.binding.tool_fingerprint
                                || candidate.control_egress_manifest_hash.as_deref()
                                    != Some(manifest.stable_hash().as_str())
                            {
                                return Err(AiError::Conflict);
                            }
                            (
                                "graphql_orm_ai_native_approval_candidates",
                                "protected_control_receipt",
                                candidate.protected_control_receipt.as_ref(),
                                candidate.control_receipt_hash.as_deref(),
                                candidate.control_egress_decision_id,
                            )
                        }
                        AiNativeToolControlKind::ConsequentialCallsPaused => {
                            if index <= source.pending_call_index
                                || call.state != "control_blocked"
                                || call.completed_at.is_none()
                                || step.finished_at.is_none()
                                || call.approval_id.is_some()
                            {
                                return Err(AiError::Conflict);
                            }
                            (
                                "graphql_orm_ai_tool_calls",
                                "protected_result",
                                call.protected_result.as_ref(),
                                None,
                                call.result_egress_decision_id,
                            )
                        }
                    };
                    let stored_value = self
                        .open(
                            policy,
                            ContentProtectionContext {
                                entity: entity.to_owned(),
                                row_id: call.id.to_string(),
                                field: field.to_owned(),
                                scope: source.scope.clone(),
                            },
                            protected.ok_or(AiError::Conflict)?,
                        )
                        .await?;
                    if expected_hash.is_some_and(|hash| {
                        native_receipt_hash(&stored_value).map_or(true, |actual| actual != hash)
                    }) {
                        return Err(AiError::Conflict);
                    }
                    match kind {
                        AiNativeToolControlKind::ApprovalPending => {
                            let stored: StoredNativeControlReceipt =
                                serde_json::from_value(stored_value)
                                    .map_err(|_| AiError::Conflict)?;
                            if stored.format_version != 1
                                || stored.provider_call_id != call.provider_call_id
                                || stored.tool_id != call.tool_id
                                || stored.egress_manifest != *manifest
                                || Some(stored.egress_decision_id) != decision_id
                            {
                                return Err(AiError::Conflict);
                            }
                        }
                        AiNativeToolControlKind::ConsequentialCallsPaused => {
                            let stored: StoredPausedControlReceipt =
                                serde_json::from_value(stored_value)
                                    .map_err(|_| AiError::Conflict)?;
                            if stored.format_version != 1
                                || stored.kind != "ConsequentialCallsPaused"
                                || stored.pending_tool_call_id != candidate.id
                                || stored.model_input != outcome.model_input
                                || stored.egress_manifest != *manifest
                                || Some(stored.egress_decision_id) != decision_id
                            {
                                return Err(AiError::Conflict);
                            }
                        }
                    }
                    decision_id.ok_or(AiError::Conflict)?
                }
            };
            self.validate_native_egress(lease, &source.scope, manifest, decision_id)
                .await?;
        }
        Ok(ValidatedNativeSource {
            provider,
            candidate,
            preparation,
            ordinary_tools,
        })
    }

    async fn validate_native_egress(
        &self,
        lease: &AiRunLease,
        scope: &AiScope,
        manifest: &AiEgressManifest,
        decision_id: Uuid,
    ) -> Result<(), AiError> {
        let event = AiEgressEventRecord::find_by_id(self.run_service.database(), &decision_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::EgressDenied)?;
        if event.run_id != Some(lease.run_id().0)
            || event.principal_subject != lease.principal_reference().subject
            || event.scope_kind != scope.kind
            || event.scope_id != scope.id
            || event.manifest_hash != manifest.stable_hash()
            || event.destination != manifest.destination
            || event.capability != "tool_result"
            || event.outcome != "allow"
            || event.classification != classification_value(manifest.maximum_classification())
            || u64::try_from(event.estimated_bytes).ok() != Some(manifest.estimated_bytes)
            || u64::try_from(event.estimated_tokens).ok() != Some(manifest.estimated_tokens)
        {
            return Err(AiError::EgressDenied);
        }
        Ok(())
    }
}

fn native_context(id: Uuid, scope: &AiScope) -> ContentProtectionContext {
    ContentProtectionContext {
        entity: "graphql_orm_ai_run_checkpoints".to_owned(),
        row_id: id.to_string(),
        field: "protected_state".to_owned(),
        scope: scope.clone(),
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeApprovedOutcome {
    format_version: u8,
    checkpoint_kind: String,
    source_checkpoint_id: Uuid,
    source_checkpoint_hash: String,
    source: NativeApprovalSource,
    completed_tool: serde_json::Value,
    continuation: serde_json::Value,
    outcome_egress_decision_id: Uuid,
    approval_id: Uuid,
    approval_binding_hash: String,
    approval_preview_hash: String,
    approval_policy_version: String,
    approval_authorization_state_digest: String,
}

impl OrmAiCoordinatorCheckpointService {
    async fn open_native_checkpoint(
        &self,
        lease: &AiRunLease,
        id: Uuid,
        kind: &str,
        scope: &AiScope,
        policy: &AiContentProtectionPolicy,
    ) -> Result<(AiRunCheckpointRecord, serde_json::Value), AiError> {
        let row = AiRunCheckpointRecord::find_by_id(self.run_service.database(), &id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::Conflict)?;
        if row.run_id != lease.run_id().0
            || row.checkpoint_kind != kind
            || row.assistant_message_id.is_some()
            || row.lease_generation <= 0
            || row.lease_generation > lease.lease_generation()
        {
            return Err(AiError::Conflict);
        }
        let budget = AiBudgetReservationRecord::find_by_id(
            self.run_service.database(),
            &row.budget_reservation_id.ok_or(AiError::Conflict)?,
        )
        .await
        .map_err(|error| map_orm(OrmPublicError::from(error)))?
        .ok_or(AiError::Conflict)?;
        if (kind == SOURCE_KIND
            && (budget.attempt_id != row.attempt_id
                || budget.lease_generation != row.lease_generation))
            || budget.lease_generation > row.lease_generation
            || !checkpoint_budget_matches(
                &budget,
                lease,
                budget.attempt_id,
                budget.lease_generation,
                scope,
                &principal_reference_kind(lease.principal_reference()),
                &budget.provider_kind,
                &budget.provider_model,
            )
        {
            return Err(AiError::Conflict);
        }
        let protected = row.protected_state.as_ref().ok_or(AiError::Conflict)?;
        enforce_size(protected, self.limits.maximum_state_bytes)?;
        if row.checkpoint_hash
            != coordinator_checkpoint_hash(
                lease.run_id(),
                row.attempt_id,
                row.lease_generation,
                row.id,
                kind,
                &budget.provider_kind,
                &budget.provider_model,
                row.provider_response_id.as_deref(),
                budget.id,
                protected,
            )?
        {
            return Err(AiError::Conflict);
        }
        let opened = self
            .open(policy, native_context(id, scope), protected)
            .await?;
        enforce_size(&opened, self.limits.maximum_state_bytes)?;
        Ok((row, opened))
    }

    pub(super) async fn adopt_native_provider_turn(
        &self,
        claim: &crate::AiApprovedRunClaim,
    ) -> Result<AiAdoptedSupervisedProviderTurn, AiError> {
        let lease = claim.lease();
        if lease.state() != crate::AiRunState::WaitingTool {
            return Err(AiError::Conflict);
        }
        let session =
            AiSessionRecord::find_by_id(self.run_service.database(), &lease.session_id().0)
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
                .ok_or(AiError::Conflict)?;
        let scope = AiScope {
            kind: session.scope_kind,
            id: session.scope_id,
            tenant_id: session.tenant_id,
        };
        let (principal, policy) = self.current_policy(lease, &scope).await?;
        let parked_id = lease.latest_checkpoint_id().ok_or(AiError::Conflict)?;
        let (parked_row, parked_value) = self
            .open_native_checkpoint(lease, parked_id, "approval_wait_parked", &scope, &policy)
            .await?;
        let parked: NativeParkedWait =
            serde_json::from_value(parked_value).map_err(|_| AiError::Conflict)?;
        let id = parked.source_checkpoint_id;
        let (checkpoint, clear) = self
            .open_native_checkpoint(lease, id, SOURCE_KIND, &scope, &policy)
            .await?;
        if parked.format_version != 1
            || parked.kind != "approval_wait_parked"
            || parked.approval_id != claim.approval_id().0
            || parked.tool_call_id != claim.tool_call_id().0
            || parked.source_checkpoint_fingerprint != checkpoint.checkpoint_hash
            || parked_row.attempt_id != checkpoint.attempt_id
            || parked_row.lease_generation != checkpoint.lease_generation
            || parked_row.provider_response_id != checkpoint.provider_response_id
            || parked_row.budget_reservation_id != checkpoint.budget_reservation_id
            || checkpoint.lease_generation.checked_add(1) != Some(lease.lease_generation())
            || checkpoint.attempt_id == lease.attempt_id()
        {
            return Err(AiError::Conflict);
        }
        let binding = crate::orm_provider_session::AiProviderSessionBindingRecord::find_by_id(
            self.run_service.database(),
            &parked.provider_session_binding_id,
        )
        .await
        .map_err(|error| map_orm(OrmPublicError::from(error)))?
        .ok_or(AiError::Conflict)?;
        if binding.session_id != lease.session_id().0
            || binding.claimed_run_id != Some(lease.run_id().0)
            || binding.state != "parked_wait"
            || binding.parked_wait_kind.as_deref() != Some("approval")
            || binding.parked_wait_id != Some(claim.approval_id().0)
            || binding.park_generation != parked.provider_session_park_generation
            || binding.parked_source_checkpoint_id != Some(id)
            || binding.parked_source_checkpoint_fingerprint.as_deref()
                != Some(checkpoint.checkpoint_hash.as_str())
            || binding.parked_checkpoint_id != Some(parked_id)
            || binding.parked_checkpoint_fingerprint.as_deref()
                != Some(parked_row.checkpoint_hash.as_str())
            || binding.parked_continuation_fingerprint.as_deref()
                != Some(parked.provider_session_continuation_fingerprint.as_str())
            || binding.parked_confirmed_at.is_none()
            || binding
                .parked_expires_at
                .is_none_or(|expiry| expiry <= self.clock.now().unix_timestamp())
        {
            return Err(AiError::Conflict);
        }
        let source: NativeApprovalSource =
            serde_json::from_value(clear).map_err(|_| AiError::Conflict)?;
        if source.scope != scope || source.candidate_id != claim.tool_call_id().0 {
            return Err(AiError::Conflict);
        }
        let proof = self
            .validate_native_source(lease, &source, &policy, "finalized", Some(id), false)
            .await?;
        if proof.provider.attempt_id != checkpoint.attempt_id
            || proof.provider.lease_generation != checkpoint.lease_generation
            || proof.provider.budget_reservation_id
                != checkpoint.budget_reservation_id.ok_or(AiError::Conflict)?
            || proof.provider.provider_response_id != checkpoint.provider_response_id
            || proof.candidate.final_approval_id != Some(claim.approval_id().0)
        {
            return Err(AiError::Conflict);
        }
        let call = AiToolCallRecord::find_by_id(self.run_service.database(), &source.candidate_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::Conflict)?;
        let approval =
            AiApprovalRecord::find_by_id(self.run_service.database(), &claim.approval_id().0)
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
                .ok_or(AiError::Conflict)?;
        validate_native_approval(lease, &proof.preparation, &call, &approval)?;
        if approval.state != "resume_claimed"
            || approval.consumed_uses != 0
            || approval.consumed_at.is_some()
            || approval.expires_at <= self.clock.now().unix_timestamp()
            || call.completed_at.is_some()
            || call.protected_result.is_some()
        {
            return Err(AiError::Conflict);
        }
        let (current, current_policy) = self.current_policy(lease, &scope).await?;
        if current.reference() != principal.reference() || current_policy != policy {
            return Err(AiError::ReauthorizationFailed);
        }
        self.reauthorize_native_rules(lease, &source).await?;
        let response_id = proof
            .provider
            .provider_response_id
            .clone()
            .ok_or(AiError::Conflict)?;
        Ok(AiAdoptedSupervisedProviderTurn {
            checkpoint_id: parked_id,
            approval_id: claim.approval_id(),
            tool_call_id: claim.tool_call_id(),
            provider_turns: source.provider_turns,
            total_tool_calls: source.total_tool_calls,
            scope,
            correlation_id: source.correlation_id.clone(),
            result_egress_route: AiToolResultEgressRoute::from_checkpoint_value(
                source.result_egress_route.clone(),
            )?,
            provider_result: proof.provider,
            provider_result_value: source.provider_result.clone(),
            pending_continuation: crate::ModelContinuation::ProviderResponse { response_id },
            rule_fingerprint: source.rule_fingerprint.clone(),
            rule_usage: source.rule_usage,
            native: Some(Box::new(source)),
            native_source_checkpoint_id: Some(id),
        })
    }

    pub(crate) async fn persist_native_approved_outcome(
        &self,
        adopted: AiAdoptedSupervisedProviderTurn,
        completed: &AiPersistedApplicationToolCall,
        continuation: AiAgentContinuation,
        outcome_egress_decision_id: Uuid,
    ) -> Result<AiProtectedSupervisedToolBatch, AiError> {
        let lease = completed.lease();
        if lease.state() != crate::AiRunState::Running
            || lease.latest_checkpoint_id() != Some(adopted.checkpoint_id)
            || completed.id() != adopted.tool_call_id
        {
            return Err(AiError::Conflict);
        }
        let source = adopted.native.ok_or(AiError::Conflict)?;
        let (_, policy) = self.current_policy(lease, &source.scope).await?;
        let (source_row, source_value) = self
            .open_native_checkpoint(
                lease,
                adopted
                    .native_source_checkpoint_id
                    .ok_or(AiError::Conflict)?,
                SOURCE_KIND,
                &source.scope,
                &policy,
            )
            .await?;
        if source_value
            != serde_json::to_value(source.as_ref()).map_err(|_| AiError::PersistenceFailed)?
        {
            return Err(AiError::Conflict);
        }
        let proof = self
            .validate_native_source(
                lease,
                &source,
                &policy,
                "finalized",
                Some(source_row.id),
                true,
            )
            .await?;
        let approval_id = proof.candidate.final_approval_id.ok_or(AiError::Conflict)?;
        let approval = AiApprovalRecord::find_by_id(self.run_service.database(), &approval_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::Conflict)?;
        let payload = NativeApprovedOutcome {
            format_version: 1,
            checkpoint_kind: OUTCOME_KIND.to_owned(),
            source_checkpoint_id: source_row.id,
            source_checkpoint_hash: source_row.checkpoint_hash,
            source: *source,
            completed_tool: completed.checkpoint_value().ok_or(AiError::EgressDenied)?,
            continuation: continuation.checkpoint_value(),
            outcome_egress_decision_id,
            approval_id,
            approval_binding_hash: approval.binding_hash,
            approval_preview_hash: approval.action_preview_hash,
            approval_policy_version: approval.policy_version,
            approval_authorization_state_digest: approval.authorization_state_digest,
        };
        let (tool, _) = self
            .validate_native_outcome(lease, &payload, &policy)
            .await?;
        let call = AiToolCallRecord::find_by_id(self.run_service.database(), &tool.id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::Conflict)?;
        if call.lease_generation != lease.lease_generation() {
            return Err(AiError::Conflict);
        }
        let id = Uuid::new_v4();
        let value = serde_json::to_value(&payload).map_err(|_| AiError::PersistenceFailed)?;
        enforce_size(&value, self.limits.maximum_state_bytes)?;
        let protected_state = self
            .protect(&policy, native_context(id, &payload.source.scope), value)
            .await?;
        enforce_size(&protected_state, self.limits.maximum_state_bytes)?;
        let (_, current_policy) = self.current_policy(lease, &payload.source.scope).await?;
        if current_policy != policy {
            return Err(AiError::ReauthorizationFailed);
        }
        self.reauthorize_native_rules(lease, &payload.source)
            .await?;
        let requested_tool_id = proof.provider.tool_calls[payload.source.pending_call_index]
            .tool_id
            .clone();
        let provider = proof.provider;
        let checkpoint_hash = coordinator_checkpoint_hash(
            lease.run_id(),
            lease.attempt_id(),
            lease.lease_generation(),
            id,
            OUTCOME_KIND,
            provider.provider_kind.as_str(),
            &provider.provider_model,
            provider.provider_response_id.as_deref(),
            provider.budget_reservation_id,
            &protected_state,
        )?;
        let renewed = self
            .run_service
            .append_coordinator_checkpoint(
                lease,
                PreparedCoordinatorCheckpoint {
                    native_binding: Some(
                        crate::orm_runs::PreparedNativeCheckpointBinding::Outcome {
                            candidate_id: payload.source.candidate_id,
                            source_checkpoint_id: payload.source_checkpoint_id,
                            pending_call_index: payload.source.pending_call_index,
                            callback_count: payload.source.outcomes.len(),
                        },
                    ),
                    id,
                    checkpoint_kind: OUTCOME_KIND.to_owned(),
                    provider_kind: provider.provider_kind.as_str().to_owned(),
                    provider_model: provider.provider_model,
                    provider_response_id: provider.provider_response_id,
                    budget_reservation_id: provider.budget_reservation_id,
                    protected_state,
                    checkpoint_hash,
                    completed_tools: vec![PreparedCoordinatorCheckpointTool {
                        id: tool.id,
                        provider_call_id: tool.provider_call_id,
                        tool_id: requested_tool_id,
                        result_egress_manifest_hash: tool.egress_manifest.stable_hash(),
                    }],
                },
            )
            .await?;
        Ok(AiProtectedSupervisedToolBatch {
            checkpoint_id: id,
            tool_call_id: completed.id(),
            lease: renewed,
            provider_turns: payload.source.provider_turns,
            total_tool_calls: payload.source.total_tool_calls,
            scope: payload.source.scope,
            rule_fingerprint: payload.source.rule_fingerprint,
            rule_usage: payload.source.rule_usage,
        })
    }

    async fn validate_native_outcome(
        &self,
        lease: &AiRunLease,
        payload: &NativeApprovedOutcome,
        policy: &AiContentProtectionPolicy,
    ) -> Result<(ToolResultSnapshot, AiAgentContinuation), AiError> {
        if payload.format_version != 1 || payload.checkpoint_kind != OUTCOME_KIND {
            return Err(AiError::Conflict);
        }
        let (source_row, source_value) = self
            .open_native_checkpoint(
                lease,
                payload.source_checkpoint_id,
                SOURCE_KIND,
                &payload.source.scope,
                policy,
            )
            .await?;
        if source_row.checkpoint_hash != payload.source_checkpoint_hash
            || source_value
                != serde_json::to_value(&payload.source).map_err(|_| AiError::PersistenceFailed)?
        {
            return Err(AiError::Conflict);
        }
        let proof = self
            .validate_native_source(
                lease,
                &payload.source,
                policy,
                "finalized",
                Some(source_row.id),
                true,
            )
            .await?;
        if proof.provider.attempt_id != source_row.attempt_id
            || proof.provider.lease_generation != source_row.lease_generation
            || Some(proof.provider.budget_reservation_id) != source_row.budget_reservation_id
            || proof.provider.provider_response_id != source_row.provider_response_id
            || proof.candidate.final_approval_id != Some(payload.approval_id)
        {
            return Err(AiError::Conflict);
        }
        let tool: ToolResultSnapshot = serde_json::from_value(payload.completed_tool.clone())
            .map_err(|_| AiError::Conflict)?;
        let call =
            AiToolCallRecord::find_by_id(self.run_service.database(), &payload.source.candidate_id)
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
                .ok_or(AiError::Conflict)?;
        let approval =
            AiApprovalRecord::find_by_id(self.run_service.database(), &payload.approval_id)
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
                .ok_or(AiError::Conflict)?;
        validate_native_approval(lease, &proof.preparation, &call, &approval)?;
        if tool.id != call.id
            || tool.provider_call_id != call.provider_call_id
            || tool.state != call.state
            || !matches!(call.state.as_str(), "completed" | "execution_failed")
            || call.completed_at.is_none()
            || approval.state != "consumed"
            || approval.consumed_uses != 1
            || approval.consumed_at.is_none()
            || approval.binding_hash != payload.approval_binding_hash
            || approval.action_preview_hash != payload.approval_preview_hash
            || approval.policy_version != payload.approval_policy_version
            || approval.authorization_state_digest != payload.approval_authorization_state_digest
            || call.authorization_policy_version.as_deref()
                != Some(approval.policy_version.as_str())
            || call.authorization_state_digest.as_deref()
                != Some(approval.authorization_state_digest.as_str())
            || call.result_egress_manifest_hash.as_deref()
                != Some(tool.egress_manifest.stable_hash().as_str())
        {
            return Err(AiError::Conflict);
        }
        let ModelInputBlock::ToolResult {
            call_id,
            tool_id,
            output,
        } = &tool.model_input
        else {
            return Err(AiError::Conflict);
        };
        if call_id != &call.provider_call_id || tool_id != &call.tool_id {
            return Err(AiError::Conflict);
        }
        let stored = self
            .open(
                policy,
                ContentProtectionContext {
                    entity: "graphql_orm_ai_tool_calls".to_owned(),
                    row_id: call.id.to_string(),
                    field: "protected_result".to_owned(),
                    scope: payload.source.scope.clone(),
                },
                call.protected_result.as_ref().ok_or(AiError::Conflict)?,
            )
            .await?;
        if stored != *output {
            return Err(AiError::Conflict);
        }
        let route = AiToolResultEgressRoute::from_checkpoint_value(
            payload.source.result_egress_route.clone(),
        )?;
        let original = &tool.egress_manifest;
        if !route.matches_manifest(
            original,
            lease,
            &payload.source.scope,
            proof.provider.provider_kind.as_str(),
            &proof.provider.provider_model,
        ) || original.sources.len() != 1
            || original.sources[0].kind != "application_tool_result"
            || original.sources[0].reference != call.id.to_string()
        {
            return Err(AiError::EgressDenied);
        }
        self.validate_native_egress(
            lease,
            &payload.source.scope,
            original,
            call.result_egress_decision_id.ok_or(AiError::Conflict)?,
        )
        .await?;
        let continuation =
            AiAgentContinuation::from_checkpoint_value(payload.continuation.clone())?;
        let expected = json!({"formatVersion":1,"kind":"FrameworkApprovedToolOutcome","toolCallId":call.id,"providerCallId":call.provider_call_id,"toolId":call.tool_id,"state":call.state,"output":output});
        if payload.continuation.get("formatVersion") != Some(&json!(4))
            || continuation.input()
                != [ModelInputBlock::Json {
                    value: expected.clone(),
                }]
            || continuation.provider_response_id() != proof.provider.provider_response_id.as_deref()
            || payload.continuation.get("reasoningEffort")
                != Some(&json!(proof.provider.reasoning_effort))
            || continuation.transfers().len() != 1
            || !continuation.replay_transfers().is_empty()
        {
            return Err(AiError::Conflict);
        }
        let mut expected_manifest = original.clone();
        expected_manifest.sources.push(crate::AiDataSourceRef {
            kind: "native_approved_outcome".to_owned(),
            reference: native_receipt_hash(&expected)?,
            classification: crate::DataClassification::Internal,
            trust: crate::AiSourceTrust::TrustedRuntime,
        });
        expected_manifest.estimated_bytes = u64::try_from(
            serde_json::to_vec(&ModelInputBlock::Json { value: expected })
                .map_err(|_| AiError::PersistenceFailed)?
                .len(),
        )
        .map_err(|_| AiError::Conflict)?;
        expected_manifest.estimated_tokens = expected_manifest.estimated_bytes;
        if continuation.transfers()[0] != expected_manifest {
            return Err(AiError::EgressDenied);
        }
        self.validate_native_egress(
            lease,
            &payload.source.scope,
            &expected_manifest,
            payload.outcome_egress_decision_id,
        )
        .await?;
        Ok((tool, continuation))
    }

    async fn reauthorize_native_rules(
        &self,
        lease: &AiRunLease,
        source: &NativeApprovalSource,
    ) -> Result<(), AiError> {
        let rules = self
            .rule_resolver
            .resolve_rules(lease, &source.scope)
            .await?;
        if rules.rules().fingerprint() != source.rule_fingerprint
            || source.rule_usage.validate(&rules).is_err()
        {
            return Err(AiError::ReauthorizationFailed);
        }
        Ok(())
    }

    pub(super) async fn adopt_native_outcome(
        &self,
        lease: &AiRunLease,
        id: Uuid,
    ) -> Result<AiAdoptedSupervisedToolBatch, AiError> {
        if lease.state() != crate::AiRunState::Running || lease.latest_checkpoint_id() != Some(id) {
            return Err(AiError::Conflict);
        }
        let session =
            AiSessionRecord::find_by_id(self.run_service.database(), &lease.session_id().0)
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
                .ok_or(AiError::Conflict)?;
        let scope = AiScope {
            kind: session.scope_kind.clone(),
            id: session.scope_id.clone(),
            tenant_id: session.tenant_id.clone(),
        };
        validate_session_binding(&session, lease, &scope)?;
        let (principal, policy) = self.current_policy(lease, &scope).await?;
        let (row, value) = self
            .open_native_checkpoint(lease, id, OUTCOME_KIND, &scope, &policy)
            .await?;
        let payload: NativeApprovedOutcome =
            serde_json::from_value(value).map_err(|_| AiError::Conflict)?;
        let provider: ProviderResultSnapshot =
            serde_json::from_value(payload.source.provider_result.clone())
                .map_err(|_| AiError::Conflict)?;
        if payload.source.scope != scope
            || provider.lease_generation > row.lease_generation
            || provider.budget_reservation_id
                != row.budget_reservation_id.ok_or(AiError::Conflict)?
            || provider.provider_response_id != row.provider_response_id
        {
            return Err(AiError::Conflict);
        }
        let (tool, continuation) = self
            .validate_native_outcome(lease, &payload, &policy)
            .await?;
        let call = AiToolCallRecord::find_by_id(self.run_service.database(), &tool.id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::Conflict)?;
        if call.lease_generation != row.lease_generation {
            return Err(AiError::Conflict);
        }
        let (current, current_policy) = self.current_policy(lease, &scope).await?;
        if current.reference() != principal.reference() || current_policy != policy {
            return Err(AiError::ReauthorizationFailed);
        }
        self.reauthorize_native_rules(lease, &payload.source)
            .await?;
        Ok(AiAdoptedSupervisedToolBatch {
            checkpoint_id: id,
            approval_id: crate::AiApprovalId(payload.approval_id),
            tool_call_id: crate::AiToolCallId(tool.id),
            provider_turns: payload.source.provider_turns,
            total_tool_calls: payload.source.total_tool_calls,
            scope,
            continuation,
            rule_fingerprint: payload.source.rule_fingerprint,
            rule_usage: payload.source.rule_usage,
            native: true,
        })
    }
}

fn validate_native_approval(
    lease: &AiRunLease,
    preparation: &NativeApprovalPreparation,
    call: &AiToolCallRecord,
    approval: &AiApprovalRecord,
) -> Result<(), AiError> {
    if approval.tool_call_id != call.id
        || approval.session_id != lease.session_id().0
        || call.approval_id != Some(approval.id)
        || approval.argument_hash != call.argument_hash
        || approval.tool_fingerprint != call.tool_fingerprint
        || approval.binding_hash != preparation.binding.stable_hash()
        || approval.action_preview_hash != preparation.binding.preview_hash
        || approval.policy_version != preparation.binding.policy_version
        || approval.authorization_state_digest != preparation.binding.authorization_state_digest
        || approval.maximum_uses != 1
        || approval.decided_at.is_none()
        || approval
            .approver_subject
            .as_deref()
            .is_none_or(str::is_empty)
        || approval.principal_reference_fingerprint
            != crate::AiApprovalBinding::principal_fingerprint(lease.principal_reference())
    {
        return Err(AiError::Conflict);
    }
    Ok(())
}

fn validate_native_order<'a>(
    pending: usize,
    candidate: Uuid,
    requested: &[ProviderToolSnapshot],
    outcomes: impl IntoIterator<
        Item = (
            usize,
            Uuid,
            &'a str,
            NativeOutcomeKind,
            Option<AiNativeToolControlKind>,
        ),
    >,
) -> Result<(), AiError> {
    if requested.is_empty()
        || requested.len() > 256
        || pending >= requested.len()
        || candidate.is_nil()
    {
        return Err(AiError::Conflict);
    }
    let mut ids = BTreeSet::new();
    let mut callbacks = BTreeSet::new();
    let mut count = 0;
    for (position, (index, id, callback, kind, control)) in outcomes.into_iter().enumerate() {
        let Some(expected) = requested.get(position) else {
            return Err(AiError::Conflict);
        };
        if index != position
            || id.is_nil()
            || !ids.insert(id)
            || !callbacks.insert(callback)
            || callback != expected.call_id
            || (id == candidate) != (position == pending)
        {
            return Err(AiError::Conflict);
        }
        match (kind, control) {
            (NativeOutcomeKind::Application, None) if position != pending => (),
            (
                NativeOutcomeKind::FrameworkControl,
                Some(AiNativeToolControlKind::ApprovalPending),
            ) if position == pending => (),
            (
                NativeOutcomeKind::FrameworkControl,
                Some(AiNativeToolControlKind::ConsequentialCallsPaused),
            ) if position > pending => (),
            _ => return Err(AiError::Conflict),
        }
        count += 1;
    }
    if count != requested.len() {
        return Err(AiError::Conflict);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_order_requires_complete_unique_ordered_callbacks_and_exact_pending_identity() {
        let requests = (0..4)
            .map(|index| ProviderToolSnapshot {
                call_id: format!("call-{index}"),
                tool_id: "registered-tool".to_owned(),
                provider_name: None,
                tool_fingerprint: "a".repeat(64),
                arguments: json!({}),
            })
            .collect::<Vec<_>>();
        let ids = (0..4).map(|_| Uuid::new_v4()).collect::<Vec<_>>();
        let candidate = ids[1];
        let good = vec![
            (0, ids[0], "call-0", NativeOutcomeKind::Application, None),
            (
                1,
                ids[1],
                "call-1",
                NativeOutcomeKind::FrameworkControl,
                Some(AiNativeToolControlKind::ApprovalPending),
            ),
            (
                2,
                ids[2],
                "call-2",
                NativeOutcomeKind::FrameworkControl,
                Some(AiNativeToolControlKind::ConsequentialCallsPaused),
            ),
            (3, ids[3], "call-3", NativeOutcomeKind::Application, None),
        ];
        assert!(validate_native_order(1, candidate, &requests, good.clone()).is_ok());
        assert!(validate_native_order(1, candidate, &requests, good[..3].iter().copied()).is_err());
        let mut bad = good.clone();
        bad.push(good[3]);
        assert!(validate_native_order(1, candidate, &requests, bad).is_err());
        let mut bad = good.clone();
        bad.swap(0, 3);
        assert!(validate_native_order(1, candidate, &requests, bad).is_err());
        let mut bad = good.clone();
        bad[2].1 = candidate;
        assert!(validate_native_order(1, candidate, &requests, bad).is_err());
        let mut bad = good.clone();
        bad[2].2 = "call-1";
        assert!(validate_native_order(1, candidate, &requests, bad).is_err());
        let mut bad = good.clone();
        bad[0].4 = Some(AiNativeToolControlKind::ApprovalPending);
        assert!(validate_native_order(1, candidate, &requests, bad).is_err());
        let mut bad = good.clone();
        bad[2].4 = Some(AiNativeToolControlKind::ApprovalPending);
        assert!(validate_native_order(1, candidate, &requests, bad).is_err());
        let mut bad = good.clone();
        bad[0].3 = NativeOutcomeKind::FrameworkControl;
        bad[0].4 = Some(AiNativeToolControlKind::ConsequentialCallsPaused);
        assert!(validate_native_order(1, candidate, &requests, bad).is_err());
        assert!(validate_native_order(1, Uuid::new_v4(), &requests, good.clone()).is_err());
        assert!(validate_native_order(0, candidate, &requests, good.clone()).is_err());
        assert!(validate_native_order(4, candidate, &requests, good).is_err());
    }
}
