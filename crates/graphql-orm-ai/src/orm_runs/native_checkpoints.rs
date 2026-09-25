//! Transaction-only native checkpoint graph checks. Protected proof is opened by its owner.

use super::*;

pub(crate) const SOURCE_KIND: &str = "native_approval_provider_turn_persisted";
pub(crate) const OUTCOME_KIND: &str = "native_approved_outcome_persisted";

#[derive(Clone, Copy)]
pub(crate) enum PendingPhase {
    Prepared,
    Waiting,
    Claimed { generation: i64 },
    Completed { generation: i64 },
}

pub(crate) struct NativeTurn<'a> {
    pub run_id: Uuid,
    pub attempt_id: Uuid,
    pub generation: i64,
    pub response_id: &'a str,
    pub budget: &'a AiBudgetReservationRecord,
}

pub(crate) async fn candidate(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    id: Uuid,
) -> Result<AiNativeApprovalCandidateRecord, OrmPublicError> {
    tx.find_by_id::<AiNativeApprovalCandidateRecord>(&id)
        .await
        .map_err(OrmPublicError::from)?
        .ok_or_else(OrmPublicError::not_found)
}

fn conflict() -> OrmPublicError {
    OrmPublicError::new(OrmErrorCode::Conflict)
}

/// Checks all callback rows, including no-effect control receipts, under one transaction.
/// The protected checkpoint service independently proves their ordered provider payloads.
pub(crate) async fn cohort(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    turn: NativeTurn<'_>,
    candidate: &AiNativeApprovalCandidateRecord,
    phase: PendingPhase,
    expected_shape: Option<(usize, usize)>,
) -> Result<BTreeSet<Uuid>, OrmPublicError> {
    let prepared = matches!(phase, PendingPhase::Prepared);
    if candidate.id.is_nil()
        || candidate.run_id != turn.run_id
        || candidate.attempt_id != turn.attempt_id
        || candidate.lease_generation != turn.generation
        || candidate.budget_reservation_id != turn.budget.id
        || candidate.state != if prepared { "prepared" } else { "finalized" }
        || candidate.protected_preparation.is_none()
        || candidate.protected_control_receipt.is_none()
        || candidate.payload_purged_at.is_some()
        || !valid_digest(&candidate.binding_hash)
        || !valid_digest(&candidate.preview_hash)
        || candidate
            .control_receipt_hash
            .as_deref()
            .is_none_or(|hash| !valid_digest(hash))
        || candidate
            .control_egress_manifest_hash
            .as_deref()
            .is_none_or(|hash| !valid_digest(hash))
        || candidate
            .control_egress_decision_id
            .is_none_or(|id| id.is_nil())
        || (prepared
            && (candidate.final_approval_id.is_some()
                || candidate.settled_checkpoint_id.is_some()
                || candidate.finalized_at.is_some()))
        || (!prepared
            && (candidate.final_approval_id.is_none()
                || candidate.settled_checkpoint_id.is_none()
                || candidate.finalized_at.is_none()))
        || turn.budget.run_id != turn.run_id
        || turn.budget.attempt_id != turn.attempt_id
        || turn.budget.lease_generation != turn.generation
        || turn.budget.state != "committed"
        || turn.budget.actual_runs != Some(1)
        || turn.budget.reconciled_at.is_none()
        || !valid_provider_reference(turn.response_id)
    {
        return Err(conflict());
    }
    let rows = tx
        .query::<AiToolCallRecord>()
        .filter(AiToolCallRecordWhereInput {
            run_id: Some(UuidFilter {
                eq: Some(turn.run_id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(4_097)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    if rows.len() >= 4_097 {
        return Err(conflict());
    }
    let mut rows = rows
        .into_iter()
        .filter(|call| call.budget_reservation_id == Some(turn.budget.id))
        .collect::<Vec<_>>();
    rows.sort_by_key(|call| call.tool_call_index);
    if rows.is_empty() || rows.len() > 256 {
        return Err(conflict());
    }
    let pending_index = rows
        .iter()
        .position(|call| call.id == candidate.id)
        .ok_or_else(conflict)?;
    if expected_shape.is_some_and(|(index, count)| index != pending_index || count != rows.len()) {
        return Err(conflict());
    }
    let mut ordinary = BTreeSet::new();
    let mut provider_ids = BTreeSet::new();
    for (index, call) in rows.iter().enumerate() {
        let pending = call.id == candidate.id;
        let generation = match phase {
            PendingPhase::Completed { generation } | PendingPhase::Claimed { generation }
                if pending =>
            {
                generation
            }
            _ => turn.generation,
        };
        let step = tx
            .find_by_id::<AiRunStepRecord>(&call.id)
            .await
            .map_err(OrmPublicError::from)?
            .ok_or_else(OrmPublicError::not_found)?;
        if call.run_id != turn.run_id
            || call.lease_generation != generation
            || call.provider_response_id.as_deref() != Some(turn.response_id)
            || call.provider_kind.as_deref() != Some(turn.budget.provider_kind.as_str())
            || call.provider_model.as_deref() != Some(turn.budget.provider_model.as_str())
            || call.tool_call_index != i64::try_from(index).map_err(|_| conflict())?
            || !provider_ids.insert(call.provider_call_id.as_str())
            || call.protected_arguments.is_none()
            || call.payload_purged_at.is_some()
            || step.run_id != turn.run_id
            || step.lease_generation != generation
            || step.step_kind != "application_tool"
        {
            return Err(conflict());
        }
        let (decision_id, manifest_hash) = if pending {
            if call.risk == "read_only" {
                return Err(conflict());
            }
            match phase {
                PendingPhase::Prepared => {
                    if call.state != "approval_prepared"
                        || step.state != "approval_prepared"
                        || call.approval_id.is_some()
                        || call.protected_result.is_some()
                        || call.completed_at.is_some()
                        || step.finished_at.is_some()
                    {
                        return Err(conflict());
                    }
                }
                PendingPhase::Waiting | PendingPhase::Claimed { .. } => {
                    if call.state != "waiting_approval"
                        || step.state != "running"
                        || call.approval_id != candidate.final_approval_id
                        || call.protected_result.is_some()
                        || call.completed_at.is_some()
                        || step.finished_at.is_some()
                    {
                        return Err(conflict());
                    }
                }
                PendingPhase::Completed { .. } => {
                    if !matches!(call.state.as_str(), "completed" | "execution_failed")
                        || step.state != call.state
                        || call.approval_id != candidate.final_approval_id
                        || call.protected_result.is_none()
                        || call.completed_at.is_none()
                        || step.finished_at.is_none()
                    {
                        return Err(conflict());
                    }
                    validate_egress(
                        tx,
                        turn.budget,
                        call.result_egress_decision_id,
                        call.result_egress_manifest_hash.as_deref(),
                    )
                    .await?;
                }
            }
            (
                candidate.control_egress_decision_id,
                candidate.control_egress_manifest_hash.as_deref(),
            )
        } else {
            if call.approval_id.is_some()
                || call.protected_result.is_none()
                || call.completed_at.is_none()
                || step.finished_at.is_none()
                || step.state != call.state
            {
                return Err(conflict());
            }
            if call.state == "control_blocked" {
                if index <= pending_index || call.risk == "read_only" {
                    return Err(conflict());
                }
            } else {
                if !matches!(call.state.as_str(), "completed" | "execution_failed")
                    || !matches!(
                        call.risk.as_str(),
                        "read_only" | "low_risk_write" | "non_idempotent_write"
                    )
                    || (index > pending_index && call.risk != "read_only")
                {
                    return Err(conflict());
                }
                ordinary.insert(call.id);
            }
            (
                call.result_egress_decision_id,
                call.result_egress_manifest_hash.as_deref(),
            )
        };
        validate_egress(tx, turn.budget, decision_id, manifest_hash).await?;
    }
    Ok(ordinary)
}

async fn validate_egress(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    budget: &AiBudgetReservationRecord,
    decision_id: Option<Uuid>,
    manifest_hash: Option<&str>,
) -> Result<(), OrmPublicError> {
    let id = decision_id.filter(|id| !id.is_nil()).ok_or_else(conflict)?;
    let hash = manifest_hash
        .filter(|hash| valid_digest(hash))
        .ok_or_else(conflict)?;
    let event = tx
        .query::<AiEgressEventRecord>()
        .filter(AiEgressEventRecordWhereInput {
            id: Some(UuidFilter {
                eq: Some(id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(1)
        .fetch_one()
        .await
        .map_err(OrmPublicError::from)?
        .ok_or_else(OrmPublicError::not_found)?;
    if event.run_id != Some(budget.run_id)
        || event.principal_subject != budget.principal_subject
        || event.scope_kind != budget.scope_kind
        || event.scope_id != budget.scope_id
        || event.capability != "tool_result"
        || event.outcome != "allow"
        || event.manifest_hash != hash
    {
        return Err(conflict());
    }
    Ok(())
}

pub(crate) async fn source(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    candidate: &AiNativeApprovalCandidateRecord,
    budget: &AiBudgetReservationRecord,
    response_id: Option<&str>,
) -> Result<AiRunCheckpointRecord, OrmPublicError> {
    let id = candidate.settled_checkpoint_id.ok_or_else(conflict)?;
    let source = tx
        .query::<AiRunCheckpointRecord>()
        .filter(AiRunCheckpointRecordWhereInput {
            id: Some(UuidFilter {
                eq: Some(id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(1)
        .fetch_one()
        .await
        .map_err(OrmPublicError::from)?
        .ok_or_else(OrmPublicError::not_found)?;
    let protected = source
        .protected_state
        .as_ref()
        .filter(|state| {
            serde_json::to_vec(state).is_ok_and(|bytes| bytes.len() <= 64 * 1024 * 1024)
        })
        .ok_or_else(conflict)?;
    if source.checkpoint_kind != SOURCE_KIND
        || source.run_id != candidate.run_id
        || source.attempt_id != candidate.attempt_id
        || source.lease_generation != candidate.lease_generation
        || source.budget_reservation_id != Some(candidate.budget_reservation_id)
        || source.budget_reservation_id != Some(budget.id)
        || source.assistant_message_id.is_some()
        || source.provider_response_id.as_deref() != response_id
        || budget.attempt_id != source.attempt_id
        || budget.lease_generation != source.lease_generation
        || coordinator_checkpoint_hash(
            AiRunId(source.run_id),
            source.attempt_id,
            source.lease_generation,
            source.id,
            SOURCE_KIND,
            &budget.provider_kind,
            &budget.provider_model,
            response_id,
            budget.id,
            protected,
        )
        .map_err(|_| conflict())?
            != source.checkpoint_hash
    {
        return Err(conflict());
    }
    Ok(source)
}

/// An outcome is new-fence evidence of the one approved effect, never a rebinding of old usage.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn outcome(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    run_id: Uuid,
    session_id: Uuid,
    generation: i64,
    budget: &AiBudgetReservationRecord,
    response_id: Option<&str>,
    candidate_id: Uuid,
    expected_source: Option<Uuid>,
    expected_shape: Option<(usize, usize)>,
) -> Result<(), OrmPublicError> {
    let candidate = candidate(tx, candidate_id).await?;
    let source = source(tx, &candidate, budget, response_id).await?;
    if source.run_id != run_id
        || expected_source.is_some_and(|id| id != source.id)
        || source.lease_generation.checked_add(1) != Some(generation)
    {
        return Err(conflict());
    }
    let approval_id = candidate.final_approval_id.ok_or_else(conflict)?;
    let approval = tx
        .find_by_id::<AiApprovalRecord>(&approval_id)
        .await
        .map_err(OrmPublicError::from)?
        .ok_or_else(OrmPublicError::not_found)?;
    let call = tx
        .find_by_id::<AiToolCallRecord>(&candidate_id)
        .await
        .map_err(OrmPublicError::from)?
        .ok_or_else(OrmPublicError::not_found)?;
    if approval.tool_call_id != candidate_id
        || approval.session_id != session_id
        || approval.binding_hash != candidate.binding_hash
        || approval.action_preview_hash != candidate.preview_hash
        || approval.state != "consumed"
        || approval.maximum_uses != 1
        || approval.consumed_uses != 1
        || approval.consumed_at.is_none()
        || approval.decided_at.is_none()
        || approval.approver_subject.is_none()
        || approval.argument_hash != call.argument_hash
        || approval.tool_fingerprint != call.tool_fingerprint
        || call.authorization_policy_version.as_deref() != Some(approval.policy_version.as_str())
        || call.authorization_state_digest.as_deref()
            != Some(approval.authorization_state_digest.as_str())
    {
        return Err(conflict());
    }
    let attempt = tx
        .query::<AiRunAttemptOutcomeRecord>()
        .filter(AiRunAttemptOutcomeRecordWhereInput {
            attempt_id: Some(UuidFilter {
                eq: Some(source.attempt_id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(1)
        .fetch_one()
        .await
        .map_err(OrmPublicError::from)?
        .ok_or_else(OrmPublicError::not_found)?;
    if attempt.run_id != run_id
        || attempt.lease_generation != source.lease_generation
        || attempt.final_state != "waiting_approval"
        || attempt.outcome_code != "approval_wait_parked"
        || attempt.provider_response_id.as_deref() != response_id
    {
        return Err(conflict());
    }
    cohort(
        tx,
        NativeTurn {
            run_id,
            attempt_id: source.attempt_id,
            generation: source.lease_generation,
            response_id: response_id.ok_or_else(conflict)?,
            budget,
        },
        &candidate,
        PendingPhase::Completed { generation },
        expected_shape,
    )
    .await?;
    Ok(())
}

/// Metadata-only snapshot proof for denial/expiry reconciliation; not an execution grant.
pub(crate) async fn waiting(
    tx: &mut MutationContext<'_, DefaultWriteBackend>,
    candidate: &AiNativeApprovalCandidateRecord,
    checkpoint: &AiRunCheckpointRecord,
    budget: &AiBudgetReservationRecord,
    approval: &AiApprovalRecord,
) -> Result<(), OrmPublicError> {
    let source = source(
        tx,
        candidate,
        budget,
        checkpoint.provider_response_id.as_deref(),
    )
    .await?;
    if source.id != checkpoint.id
        || candidate.final_approval_id != Some(approval.id)
        || candidate.binding_hash != approval.binding_hash
        || candidate.preview_hash != approval.action_preview_hash
        || candidate.id != approval.tool_call_id
        || budget.session_id != approval.session_id
    {
        return Err(conflict());
    }
    cohort(
        tx,
        NativeTurn {
            run_id: source.run_id,
            attempt_id: source.attempt_id,
            generation: source.lease_generation,
            response_id: source
                .provider_response_id
                .as_deref()
                .ok_or_else(conflict)?,
            budget,
        },
        candidate,
        PendingPhase::Waiting,
        None,
    )
    .await?;
    Ok(())
}

impl OrmAiRunService {
    /// Proves that only the pending native call moved to an approved fresh fence.
    /// Historical usage/source/callback rows must retain their original generation.
    pub(crate) async fn validate_native_approved_budget(
        &self,
        lease: &AiRunLease,
        approval_id: AiApprovalId,
        tool_call_id: AiToolCallId,
        budget_id: Uuid,
    ) -> Result<(), AiError> {
        let now = canonical_second(self.clock.now());
        let lease = lease.clone();
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let run = load_and_validate_active_lease(tx, &lease, now).await?;
                    if persisted_state(&run)? != AiRunState::WaitingTool {
                        return Err(conflict());
                    }
                    let candidate = candidate(tx, tool_call_id.0).await?;
                    let budget = tx
                        .find_by_id::<AiBudgetReservationRecord>(&budget_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    let approval = tx
                        .find_by_id::<AiApprovalRecord>(&approval_id.0)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    let call = tx
                        .find_by_id::<AiToolCallRecord>(&tool_call_id.0)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    let source = source(
                        tx,
                        &candidate,
                        &budget,
                        call.provider_response_id.as_deref(),
                    )
                    .await?;
                    if source.run_id != run.id
                        || source.lease_generation.checked_add(1) != Some(lease.lease_generation)
                        || source.attempt_id == lease.attempt_id
                        || budget.session_id != run.session_id
                        || candidate.final_approval_id != Some(approval.id)
                        || candidate.binding_hash != approval.binding_hash
                        || candidate.preview_hash != approval.action_preview_hash
                        || approval.tool_call_id != call.id
                        || approval.session_id != run.session_id
                        || approval.state != "resume_claimed"
                        || approval.maximum_uses != 1
                        || approval.consumed_uses != 0
                        || approval.consumed_at.is_some()
                        || approval.decided_at.is_none()
                        || approval.approver_subject.is_none()
                        || approval.expires_at <= now.unix_timestamp()
                        || approval.argument_hash != call.argument_hash
                        || approval.tool_fingerprint != call.tool_fingerprint
                        || call.budget_reservation_id != Some(budget_id)
                    {
                        return Err(conflict());
                    }
                    let parked = tx
                        .query::<AiRunCheckpointRecord>()
                        .filter(AiRunCheckpointRecordWhereInput {
                            id: Some(UuidFilter {
                                eq: Some(run.latest_checkpoint_id.ok_or_else(conflict)?),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .limit(1)
                        .fetch_one()
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    let protected = parked
                        .protected_state
                        .as_ref()
                        .filter(|value| {
                            serde_json::to_vec(value)
                                .is_ok_and(|bytes| bytes.len() <= 64 * 1024 * 1024)
                        })
                        .ok_or_else(conflict)?;
                    if parked.run_id != run.id
                        || parked.attempt_id != source.attempt_id
                        || parked.lease_generation != source.lease_generation
                        || parked.checkpoint_kind != "approval_wait_parked"
                        || parked.budget_reservation_id != Some(budget_id)
                        || parked.assistant_message_id.is_some()
                        || parked.provider_response_id != source.provider_response_id
                        || coordinator_checkpoint_hash(
                            lease.run_id,
                            parked.attempt_id,
                            parked.lease_generation,
                            parked.id,
                            &parked.checkpoint_kind,
                            &budget.provider_kind,
                            &budget.provider_model,
                            parked.provider_response_id.as_deref(),
                            budget_id,
                            protected,
                        )
                        .map_err(|_| conflict())?
                            != parked.checkpoint_hash
                    {
                        return Err(conflict());
                    }
                    let original = tx
                        .query::<AiRunAttemptOutcomeRecord>()
                        .filter(AiRunAttemptOutcomeRecordWhereInput {
                            attempt_id: Some(UuidFilter {
                                eq: Some(source.attempt_id),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .limit(1)
                        .fetch_one()
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if original.run_id != run.id
                        || original.lease_generation != source.lease_generation
                        || original.final_state != "waiting_approval"
                        || original.outcome_code != "approval_wait_parked"
                        || original.provider_response_id != source.provider_response_id
                    {
                        return Err(conflict());
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
                        .limit(2)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    if bindings.len() != 1 {
                        return Err(conflict());
                    }
                    let binding = &bindings[0];
                    if binding.state != "parked_wait"
                        || binding.parked_wait_kind.as_deref() != Some("approval")
                        || binding.parked_wait_id != Some(approval.id)
                        || binding.claimed_run_id != Some(run.id)
                        || binding.parked_confirmed_at.is_none()
                        || binding.parked_checkpoint_id != Some(parked.id)
                        || binding.parked_checkpoint_fingerprint.as_deref()
                            != Some(parked.checkpoint_hash.as_str())
                        || binding
                            .parked_expires_at
                            .is_none_or(|expiry| expiry <= now.unix_timestamp())
                    {
                        return Err(conflict());
                    }
                    cohort(
                        tx,
                        NativeTurn {
                            run_id: run.id,
                            attempt_id: source.attempt_id,
                            generation: source.lease_generation,
                            response_id: source
                                .provider_response_id
                                .as_deref()
                                .ok_or_else(conflict)?,
                            budget: &budget,
                        },
                        &candidate,
                        PendingPhase::Claimed {
                            generation: lease.lease_generation,
                        },
                        None,
                    )
                    .await?;
                    Ok(())
                })
            })
            .await
            .map_err(map_transaction)
    }
}
