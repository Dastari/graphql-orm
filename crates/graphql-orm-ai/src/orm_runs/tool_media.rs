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

    pub(crate) async fn retain_tool_result_source(
        &self,
        lease: &AiRunLease,
        call_id: AiToolCallId,
        protected_result: serde_json::Value,
        protected_source: serde_json::Value,
        origin: crate::AiToolExecutionProvenance,
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
                    let expected = serde_json::to_value(&origin)
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
                        || origin.attempt_id() != lease.attempt_id
                        || origin.lease_generation() != lease.lease_generation
                        || origin.argument_hash() != call.argument_hash
                        || origin.tool_fingerprint() != call.tool_fingerprint
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
                                state: Some("source_retained".to_owned()),
                                ..Default::default()
                            },
                        )
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
