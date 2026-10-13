//! Fenced retention of actual effect output before source disclosure.

use super::*;

impl OrmAiRunService {
    pub(crate) async fn validate_tool_disclosure_fence(
        &self,
        lease: &AiRunLease,
    ) -> Result<(), AiError> {
        let lease = lease.clone();
        let now = canonical_second(self.clock.now());
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let current = load_and_validate_active_lease(tx, &lease, now).await?;
                    if persisted_state(&current)? != AiRunState::Running {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    Ok(())
                })
            })
            .await
            .map_err(map_transaction)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn retain_tool_result_source(
        &self,
        lease: &AiRunLease,
        call_id: AiToolCallId,
        protected_result: serde_json::Value,
        protected_source: serde_json::Value,
        origin: crate::AiToolExecutionProvenance,
        source_reference: String,
        classification: String,
        image_count: i64,
        provider_cursor_fingerprint: Option<String>,
    ) -> Result<(), AiError> {
        let lease = lease.clone();
        let now = canonical_second(self.clock.now());
        self.database
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let current = load_and_validate_active_lease(tx, &lease, now).await?;
                    if persisted_state(&current)? != AiRunState::Running {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let call = tx
                        .find_by_id::<AiToolCallRecord>(&call_id.0)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    let step = tx
                        .find_by_id::<AiRunStepRecord>(&call_id.0)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    let expected = serde_json::to_value(origin.original_invocation())
                        .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))?;
                    if call.run_id != lease.run_id.0
                        || step.run_id != call.run_id
                        || call.lease_generation != lease.lease_generation
                        || step.lease_generation != call.lease_generation
                        || step.state != "running"
                        || call.state != "executing"
                        || call.protected_result.is_some()
                        || call.protected_result_source.is_some()
                        || call.execution_provenance.as_ref() != Some(&expected)
                        || origin.tool_call_id() != call_id
                        || origin.run_id() != lease.run_id
                        || origin.session_id() != lease.session_id
                        || origin.argument_hash() != call.argument_hash
                        || origin.tool_fingerprint() != call.tool_fingerprint
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    if let Some(approval_id) = origin.approval_id() {
                        let approval = tx
                            .find_by_id::<AiApprovalRecord>(&approval_id.0)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        if call.approval_id != Some(approval.id)
                            || approval.tool_call_id != call.id
                            || approval.state != "consumed"
                            || approval.consumed_uses != 1
                            || approval.consumed_at.is_none()
                            || origin.approval_binding_hash()
                                != Some(approval.binding_hash.as_str())
                            || origin.policy_version() != Some(approval.policy_version.as_str())
                            || origin.authorization_state_digest()
                                != Some(approval.authorization_state_digest.as_str())
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                    } else if call.approval_id.is_some()
                        || origin.attempt_id() != lease.attempt_id
                        || origin.lease_generation() != lease.lease_generation
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let outcome = tx
                        .compare_and_swap::<AiToolCallRecord>(
                            &call.id,
                            call.row_version,
                            AiToolCallRecordWhereInput::default(),
                            UpdateAiToolCallRecordInput {
                                protected_result: Some(Some(protected_result)),
                                protected_result_source: Some(Some(protected_source)),
                                execution_provenance: Some(Some(
                                    serde_json::to_value(&origin).map_err(|_| {
                                        OrmPublicError::new(OrmErrorCode::InternalError)
                                    })?,
                                )),
                                state: Some("source_retained".to_owned()),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?;
                    tx.insert::<AiToolResultSourceRecord>(CreateAiToolResultSourceRecordInput {
                        id: call_id.0,
                        session_id: lease.session_id.0,
                        run_id: lease.run_id.0,
                        provider_kind: origin.provider_kind().as_str().into(),
                        provider_profile_id: origin.provider_profile_id().into(),
                        provider_model: origin.provider_model().into(),
                        provider_session_binding_id: origin.provider_session_binding_id(),
                        provider_cursor_fingerprint,
                        source_reference,
                        classification,
                        image_count,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    match outcome {
                        ConditionalUpdateOutcome::Updated(_) => Ok(()),
                        _ => Err(OrmPublicError::new(OrmErrorCode::Conflict)),
                    }
                })
            })
            .await
            .map_err(map_transaction)
    }
}
