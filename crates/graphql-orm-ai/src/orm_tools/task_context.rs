//! Owning read-only original task projection. It never creates a message or repeats an effect.
use super::*;

impl OrmAiApplicationToolCallService {
    /// Reopens the original task and projects only actual completed invocation statuses.
    ///
    /// This read-only value keeps original message/event identities opaque and contains no
    /// prior result/tree/image content. The host prices its conservative bytes/tokens, then
    /// consumes the same value through [`crate::AiAgentContinuation::with_prepared_task_context`]
    /// before constructing the complete final plan. User text remains untrusted context.
    ///
    /// # Errors
    /// Rejects cancelled/obsolete leases, current access denial, missing or purged original
    /// source, non-terminal invocation rows, or more than 64 retained status records.
    pub async fn prepare_task_continuation_context(
        &self,
        lease: &AiRunLease,
        scope: &AiScope,
    ) -> Result<crate::AiPreparedTaskContinuationContext, AiError> {
        use crate::persistence::{AiSessionEventRecord, AiSessionEventRecordWhereInput};
        use graphql_orm::graphql::filters::{StringFilter, UuidFilter};
        let text = self.current_user_text(lease, scope).await?;
        let message =
            AiMessageRecord::find_by_id(self.run_service.database(), &lease.input_message_id())
                .await
                .map_err(|_| AiError::PersistenceFailed)?
                .ok_or(AiError::Conflict)?;
        let event = AiSessionEventRecord::query(self.run_service.database().pool())
            .filter(AiSessionEventRecordWhereInput {
                session_id: Some(UuidFilter {
                    eq: Some(lease.session_id().0),
                    ..Default::default()
                }),
                event_type: Some(StringFilter {
                    eq: Some("message_queued".into()),
                    ..Default::default()
                }),
                run_id: Some(UuidFilter {
                    eq: Some(lease.run_id().0),
                    ..Default::default()
                }),
                correlation_id: Some(StringFilter {
                    eq: Some(
                        message
                            .client_message_id
                            .ok_or(AiError::Conflict)?
                            .to_string(),
                    ),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .limit(2)
            .fetch_all()
            .await
            .map_err(|_| AiError::PersistenceFailed)?;
        let [event] = event.as_slice() else {
            return Err(AiError::Conflict);
        };
        if event.run_id != Some(lease.run_id().0)
            || message
                .client_message_id
                .map(|id| id.to_string())
                .as_deref()
                != Some(event.correlation_id.as_str())
        {
            return Err(AiError::Conflict);
        }
        let mut calls = AiToolCallRecord::query(self.run_service.database().pool())
            .filter(AiToolCallRecordWhereInput {
                run_id: Some(UuidFilter {
                    eq: Some(lease.run_id().0),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .limit(65)
            .fetch_all()
            .await
            .map_err(|_| AiError::PersistenceFailed)?;
        if calls.len() > 64
            || calls.iter().any(|call| {
                !matches!(call.state.as_str(), "completed" | "execution_failed")
                    || call.completed_at.is_none()
                    || call.payload_purged_at.is_some()
            })
        {
            return Err(AiError::Conflict);
        }
        calls.sort_by_key(|call| call.id);
        let mut statuses = calls.iter().map(|call| json!({
            "toolCallId":call.id,"toolId":call.tool_id,"state":call.state,
            "completedAt":call.completed_at,"invocationAlreadyCompleted":true,"replayAllowed":false
        })).collect::<Vec<_>>();
        statuses.sort_by_key(|status| status["toolCallId"].as_str().unwrap_or_default().to_owned());
        let source_fingerprint = canonical_json_hash(&json!({
            "message":message,"eventId":event.id,"eventSequence":event.sequence,
            "eventSource":event.protected_payload,"goalSha256":canonical_json_hash(&json!(text))?,
            "calls":calls.iter().map(|call| (call.id,call.row_version,call.lease_generation,call.state.as_str())).collect::<Vec<_>>()
        }))?;
        let value = json!({
            "formatVersion":1,"kind":"FrameworkTaskContinuationContext","contextReset":true,
            "originalMessageId":message.id,"originalEventId":event.id,
            "originalClientMessageId":message.client_message_id,
            "goalContext":{"trust":"untrusted_user_content","text":text},
            "savedInvocationStatuses":statuses,"effectsMustNotBeReplayed":true,
            "priorProtectedContentIncluded":false,"newHumanEventCreated":false
        });
        let frame = ModelInputBlock::Text {
            text: serde_json::to_string(&value).map_err(|_| AiError::PersistenceFailed)?,
        };
        let bytes = u64::try_from(
            serde_json::to_vec(&frame)
                .map_err(|_| AiError::PersistenceFailed)?
                .len(),
        )
        .map_err(|_| AiError::Conflict)?;
        if bytes > 128 * 1024 {
            return Err(AiError::Conflict);
        }
        self.run_service
            .validate_tool_disclosure_fence(lease)
            .await?;
        Ok(crate::AiPreparedTaskContinuationContext {
            session_id: lease.session_id(),
            run_id: lease.run_id(),
            attempt_id: lease.attempt_id(),
            lease_generation: lease.lease_generation(),
            frame,
            bytes,
            sources: vec![
                AiDataSourceRef {
                    kind: "original_task_context".into(),
                    reference: source_fingerprint.clone(),
                    classification: DataClassification::Internal,
                    trust: AiSourceTrust::UserProvided,
                },
                AiDataSourceRef {
                    kind: "saved_invocation_statuses".into(),
                    reference: source_fingerprint.clone(),
                    classification: DataClassification::Internal,
                    trust: AiSourceTrust::TrustedRuntime,
                },
            ],
            source_fingerprint,
        })
    }

    pub(crate) async fn validate_task_continuation_context(
        &self,
        lease: &AiRunLease,
        scope: &AiScope,
        prepared: &crate::AiPreparedTaskContinuationContext,
    ) -> Result<(), AiError> {
        if prepared.session_id != lease.session_id()
            || prepared.run_id != lease.run_id()
            || prepared.attempt_id != lease.attempt_id()
            || prepared.lease_generation != lease.lease_generation()
        {
            return Err(AiError::Conflict);
        }
        let current = self.prepare_task_continuation_context(lease, scope).await?;
        if current.source_fingerprint != prepared.source_fingerprint
            || current.frame != prepared.frame
            || current.sources != prepared.sources
            || current.bytes != prepared.bytes
        {
            return Err(AiError::Conflict);
        }
        Ok(())
    }
}
