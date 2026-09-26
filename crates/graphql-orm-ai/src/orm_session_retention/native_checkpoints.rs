//! Metadata-only proof for atomic expiry of a closed native checkpoint graph.

use super::*;
use graphql_orm::graphql::orm::RetentionContext;

pub(super) const SOURCE: &str = "native_approval_provider_turn_persisted";
pub(super) const OUTCOME: &str = "native_approved_outcome_persisted";

pub(super) fn is_native(kind: &str) -> bool {
    matches!(kind, SOURCE | OUTCOME)
}

pub(super) async fn group(
    tx: &mut RetentionContext<'_, DefaultWriteBackend>,
    checkpoint: &AiRunCheckpointRetentionProjection,
) -> Result<Vec<AiRunCheckpointRetentionProjection>, OrmPublicError> {
    let Some(budget) = checkpoint.budget_reservation_id else {
        return Ok(Vec::new());
    };
    let rows = tx
        .project::<AiRunCheckpointRetentionProjection>()
        .filter(AiRunCheckpointRecordWhereInput {
            run_id: Some(UuidFilter {
                eq: Some(checkpoint.run_id),
                ..Default::default()
            }),
            checkpoint_kind: Some(StringFilter {
                in_list: Some(vec![SOURCE.to_owned(), OUTCOME.to_owned()]),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(8_193)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    // Conservatively bound the metadata scan. Each prepared turn has one source
    // and at most one outcome; an overflow is never accepted as complete proof.
    if rows.len() >= 8_193 {
        return Ok(Vec::new());
    }
    let rows = rows
        .into_iter()
        .filter(|row| row.budget_reservation_id == Some(budget))
        .collect::<Vec<_>>();
    if !(1..=2).contains(&rows.len())
        || rows
            .iter()
            .filter(|row| row.checkpoint_kind == SOURCE)
            .count()
            != 1
        || rows
            .iter()
            .filter(|row| row.checkpoint_kind == OUTCOME)
            .count()
            > 1
        || !rows.iter().any(|row| row.id == checkpoint.id)
    {
        return Ok(Vec::new());
    }
    Ok(rows)
}

fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

async fn attempt_outcome(
    tx: &mut RetentionContext<'_, DefaultWriteBackend>,
    checkpoint: &AiRunCheckpointRetentionProjection,
) -> Result<Option<AiRunAttemptOutcomeRecord>, OrmPublicError> {
    let attempts = tx
        .query::<AiRunAttemptRecord>()
        .filter(AiRunAttemptRecordWhereInput {
            id: Some(UuidFilter {
                eq: Some(checkpoint.attempt_id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(2)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    let outcomes = tx
        .query::<AiRunAttemptOutcomeRecord>()
        .filter(AiRunAttemptOutcomeRecordWhereInput {
            attempt_id: Some(UuidFilter {
                eq: Some(checkpoint.attempt_id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(2)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    if attempts.len() != 1 || outcomes.len() != 1 {
        return Ok(None);
    }
    let attempt = &attempts[0];
    let outcome = &outcomes[0];
    if attempt.id != checkpoint.attempt_id
        || attempt.run_id != checkpoint.run_id
        || attempt.lease_generation != checkpoint.lease_generation
        || attempt.worker_id.trim().is_empty()
        || attempt.worker_id.len() > 256
        || attempt.worker_id.chars().any(char::is_control)
        || attempt.claimed_at > checkpoint.created_at
        || outcome.attempt_id != attempt.id
        || outcome.run_id != attempt.run_id
        || outcome.lease_generation != attempt.lease_generation
        || outcome.worker_id != attempt.worker_id
        || outcome.finished_at < checkpoint.created_at
        || outcome.finished_at < attempt.claimed_at
        || outcome.outcome_code.trim().is_empty()
        || outcome.outcome_code.len() > 200
        || outcome.outcome_code.chars().any(char::is_control)
    {
        return Ok(None);
    }
    Ok(outcomes.into_iter().next())
}

/// Never opens raw source/results. The immutable checkpoint metadata and already
/// tombstoned call/candidate/approval graph are the retention authority.
#[allow(clippy::too_many_arguments)]
pub(super) async fn validate(
    tx: &mut RetentionContext<'_, DefaultWriteBackend>,
    checkpoint: &AiRunCheckpointRetentionProjection,
    run: &AiRunRecord,
    session_id: Uuid,
    raw_cutoff: i64,
    calls: &[AiToolCallRecord],
    approvals: &HashMap<Uuid, &AiApprovalRecord>,
) -> Result<bool, OrmPublicError> {
    if run.state == "recovery_required"
        || !run_state_is_retention_closed(
            AiRunState::from_persisted(&run.state)
                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?,
        )
    {
        return Ok(false);
    }
    let rows = group(tx, checkpoint).await?;
    let Some(source) = rows.iter().find(|row| row.checkpoint_kind == SOURCE) else {
        return Ok(false);
    };
    let outcome = rows.iter().find(|row| row.checkpoint_kind == OUTCOME);
    if rows.iter().any(|row| {
        row.id.is_nil()
            || row.attempt_id.is_nil()
            || row.run_id != run.id
            || row.lease_generation <= 0
            || row.lease_generation > run.lease_generation
            || row.assistant_message_id.is_some()
            || !hash(&row.checkpoint_hash)
            || row.created_at > raw_cutoff
            || run.latest_checkpoint_id == Some(row.id)
            || row.provider_response_id != source.provider_response_id
    }) || source.provider_response_id.as_ref().is_none_or(|id| {
        id.trim().is_empty() || id.len() > 1_024 || id.chars().any(char::is_control)
    }) {
        return Ok(false);
    }
    let Some(budget_id) = source.budget_reservation_id else {
        return Ok(false);
    };
    let budgets = tx
        .query::<AiBudgetReservationRecord>()
        .filter(AiBudgetReservationRecordWhereInput {
            id: Some(UuidFilter {
                eq: Some(budget_id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(2)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    if budgets.len() != 1 {
        return Ok(false);
    }
    let budget = &budgets[0];
    if budget.session_id != session_id
        || budget.run_id != run.id
        || budget.attempt_id != source.attempt_id
        || budget.lease_generation != source.lease_generation
        || budget.state != "committed"
        || budget.actual_runs != Some(1)
        || budget.reconciled_at.is_none_or(|at| at > source.created_at)
        || budget.created_at > source.created_at
        || budget.provider_kind.trim().is_empty()
        || budget.provider_model.trim().is_empty()
    {
        return Ok(false);
    }
    let cohort_ids = calls
        .iter()
        .filter(|call| call.run_id == run.id && call.budget_reservation_id == Some(budget_id))
        .map(|call| call.id)
        .collect::<Vec<_>>();
    if cohort_ids.is_empty() || cohort_ids.len() > 256 {
        return Ok(false);
    }
    let candidates = tx
        .query::<AiNativeApprovalCandidateRecord>()
        .filter(AiNativeApprovalCandidateRecordWhereInput {
            run_id: Some(UuidFilter {
                eq: Some(run.id),
                ..Default::default()
            }),
            id: Some(UuidFilter {
                in_list: Some(cohort_ids),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(2)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    if candidates.len() != 1 {
        return Ok(false);
    }
    let candidate = &candidates[0];
    if candidate.id.is_nil()
        || candidate.run_id != run.id
        || candidate.budget_reservation_id != budget_id
        || candidate.attempt_id != source.attempt_id
        || candidate.lease_generation != source.lease_generation
        || !hash(&candidate.binding_hash)
        || !hash(&candidate.preview_hash)
        || candidate
            .control_receipt_hash
            .as_deref()
            .is_none_or(|value| !hash(value))
        || candidate
            .control_egress_manifest_hash
            .as_deref()
            .is_none_or(|value| !hash(value))
        || candidate
            .control_egress_decision_id
            .is_none_or(|id| id.is_nil())
        || candidate.payload_purged_at.is_none()
        || candidate.protected_preparation.is_some()
        || candidate.protected_control_receipt.is_some()
    {
        return Ok(false);
    }
    let Some(source_attempt) = attempt_outcome(tx, source).await? else {
        return Ok(false);
    };
    let approval = candidate
        .final_approval_id
        .and_then(|id| approvals.get(&id).copied());
    let pending = calls.iter().find(|call| call.id == candidate.id);
    let Some(pending) = pending else {
        return Ok(false);
    };
    match candidate.state.as_str() {
        "abandoned" => {
            if candidate.final_approval_id.is_some()
                || candidate.settled_checkpoint_id.is_some()
                || candidate.finalized_at.is_some()
                || pending.approval_id.is_some()
                || pending.state != "cancelled"
                || pending.lease_generation != source.lease_generation
                || outcome.is_some()
                || !matches!(source_attempt.final_state.as_str(), "failed" | "cancelled")
            {
                return Ok(false);
            }
        }
        "finalized" => {
            let Some(approval) = approval else {
                return Ok(false);
            };
            if candidate.settled_checkpoint_id != Some(source.id)
                || candidate.finalized_at.is_none()
                || source_attempt.final_state != "waiting_approval"
                || source_attempt.outcome_code != "approval_wait_parked"
                || source_attempt.provider_response_id != source.provider_response_id
                || approval.tool_call_id != candidate.id
                || approval.session_id != session_id
                || approval.binding_hash != candidate.binding_hash
                || approval.action_preview_hash != candidate.preview_hash
                || pending.approval_id != Some(approval.id)
                || approval.argument_hash != pending.argument_hash
                || approval.tool_fingerprint != pending.tool_fingerprint
            {
                return Ok(false);
            }
            if approval.state == "consumed" {
                let Some(outcome) = outcome else {
                    return Ok(false);
                };
                if approval.maximum_uses != 1
                    || approval.consumed_uses != 1
                    || approval.consumed_at.is_none()
                    || approval.decided_at.is_none()
                    || approval.approver_subject.is_none()
                    || source.lease_generation.checked_add(1) != Some(outcome.lease_generation)
                    || outcome.attempt_id == source.attempt_id
                    || outcome.created_at < source.created_at
                    || pending.lease_generation != outcome.lease_generation
                    || pending.authorization_policy_version.as_deref()
                        != Some(approval.policy_version.as_str())
                    || pending.authorization_state_digest.as_deref()
                        != Some(approval.authorization_state_digest.as_str())
                {
                    return Ok(false);
                }
                let Some(terminal) = attempt_outcome(tx, outcome).await? else {
                    return Ok(false);
                };
                if !(matches!(
                    terminal.final_state.as_str(),
                    "completed" | "failed" | "cancelled"
                ) || (terminal.final_state == "retry_scheduled"
                    && terminal.outcome_code == "checkpoint_adoption_ready"))
                {
                    return Ok(false);
                }
            } else if !matches!(approval.state.as_str(), "denied" | "expired" | "revoked")
                || approval.consumed_uses != 0
                || approval.consumed_at.is_some()
                || outcome.is_some()
                || (pending.lease_generation != source.lease_generation
                    && source.lease_generation.checked_add(1) != Some(pending.lease_generation))
            {
                return Ok(false);
            }
        }
        _ => return Ok(false),
    }
    let mut cohort = calls
        .iter()
        .filter(|call| call.run_id == run.id && call.budget_reservation_id == Some(budget_id))
        .collect::<Vec<_>>();
    cohort.sort_by_key(|call| call.tool_call_index);
    if cohort.is_empty() || cohort.len() > 256 {
        return Ok(false);
    }
    let Some(pending_index) = cohort.iter().position(|call| call.id == candidate.id) else {
        return Ok(false);
    };
    let mut provider_ids = HashSet::new();
    let mut ids = HashSet::new();
    for (index, call) in cohort.into_iter().enumerate() {
        if call.tool_call_index
            != i64::try_from(index).map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?
            || call.provider_response_id != source.provider_response_id
            || call.provider_kind.as_deref() != Some(budget.provider_kind.as_str())
            || call.provider_model.as_deref() != Some(budget.provider_model.as_str())
            || !provider_ids.insert(&call.provider_call_id)
            || !ids.insert(call.id)
            || (call.id != candidate.id && call.lease_generation != source.lease_generation)
            || !tool_call_state_is_terminal(&call.state)
            || call.completed_at.is_none_or(|at| at > raw_cutoff)
            || call.payload_purged_at.is_none()
            || call.protected_arguments.is_some()
            || call.protected_result.is_some()
            || (call.id == candidate.id && call.risk == "read_only")
            || (call.id != candidate.id
                && (call.approval_id.is_some()
                    || !matches!(
                        call.state.as_str(),
                        "completed" | "execution_failed" | "control_blocked"
                    )
                    || (call.state == "control_blocked"
                        && (index <= pending_index || call.risk == "read_only"))))
            || (index > pending_index
                && call.risk != "read_only"
                && call.state != "control_blocked")
        {
            return Ok(false);
        }
        let steps = tx
            .query::<AiRunStepRecord>()
            .filter(AiRunStepRecordWhereInput {
                id: Some(UuidFilter {
                    eq: Some(call.id),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .limit(2)
            .fetch_all()
            .await
            .map_err(OrmPublicError::from)?;
        if steps.len() != 1 || steps[0].step_kind != "application_tool" {
            return Ok(false);
        }
        validate_tool_step(&steps[0], call)?;
        let grant = call.approval_id.and_then(|id| approvals.get(&id).copied());
        if call.approval_id.is_some() != grant.is_some()
            || !tool_approval_states_match(call, grant)
            || grant.is_some_and(|grant| {
                grant.tool_call_id != call.id
                    || !approval_state_is_terminal(&grant.state)
                    || grant.payload_purged_at.is_none()
                    || grant.protected_resource_bindings.is_some()
                    || grant.protected_action_preview.is_some()
            })
        {
            return Ok(false);
        }
        if let Some(grant) = grant {
            validate_approval(grant, session_id)?;
        }
    }
    for approval in approvals.values() {
        if ids.contains(&approval.tool_call_id)
            && calls
                .iter()
                .find(|call| call.id == approval.tool_call_id)
                .is_none_or(|call| call.approval_id != Some(approval.id))
        {
            return Ok(false);
        }
    }
    let bindings = tx
        .query::<AiProviderSessionBindingRecord>()
        .filter(AiProviderSessionBindingRecordWhereInput {
            session_id: Some(UuidFilter {
                eq: Some(session_id),
                ..Default::default()
            }),
            ..Default::default()
        })
        .limit(2)
        .fetch_all()
        .await
        .map_err(OrmPublicError::from)?;
    if bindings.len() > 1 {
        return Ok(false);
    }
    for binding in bindings {
        if binding.state == "deleted" {
            continue;
        }
        if binding.claimed_run_id == Some(run.id) {
            return Ok(false);
        }
        let exact_wait = candidate.final_approval_id.is_some()
            && binding.parked_wait_id == candidate.final_approval_id;
        let references_group = rows.iter().any(|row| {
            binding.parked_source_checkpoint_id == Some(row.id)
                || binding.parked_checkpoint_id == Some(row.id)
        });
        if exact_wait || references_group {
            // commit_turn preserves the reclaimed wait as audit metadata.
            // Only an idle, successfully committed binding makes it historical.
            if !exact_wait
                || binding.parked_source_checkpoint_id != Some(source.id)
                || binding.parked_source_checkpoint_fingerprint.as_deref()
                    != Some(source.checkpoint_hash.as_str())
                || binding.state != "active"
                || binding.parked_reclaimed_at.is_none()
                || binding.parked_wait_kind.as_deref() != Some("approval")
                || binding.claimed_run_id.is_some()
                || binding.claimed_attempt_id.is_some()
                || binding.claimed_run_lease_generation.is_some()
                || binding.claim_owner.is_some()
                || binding.claim_expires_at.is_some()
                || binding.last_run_id != Some(run.id)
                || binding.last_assistant_message_id.is_none()
                || binding.cleanup_owner.is_some()
                || binding.cleanup_lease_expires_at.is_some()
                || binding.cleanup_next_attempt_at.is_some()
                || binding.cleanup_reason_code.is_some()
                || run.state != "completed"
                || approval.is_none_or(|approval| approval.state != "consumed")
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
