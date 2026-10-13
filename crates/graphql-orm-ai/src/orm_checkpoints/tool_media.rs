//! Preserve genuine original source metadata in stored coordinator proof checks.
use super::*;

impl OrmAiCoordinatorCheckpointService {
    pub(super) async fn expected_application_source(
        &self,
        lease: &AiRunLease,
        scope: &AiScope,
        call: &AiToolCallRecord,
        output: &serde_json::Value,
    ) -> Result<String, AiError> {
        let Some(protected_source) = &call.protected_result_source else {
            return Ok(call.id.to_string());
        };
        let (_, policy) = self.current_policy(lease, scope).await?;
        let opened = self
            .open(
                &policy,
                ContentProtectionContext {
                    entity: "graphql_orm_ai_tool_calls".into(),
                    row_id: call.id.to_string(),
                    field: "protected_result_source".into(),
                    scope: scope.clone(),
                },
                protected_source,
            )
            .await?;
        let source: crate::tool_media::RetainedToolResultSource =
            serde_json::from_value(opened).map_err(|_| AiError::PersistenceFailed)?;
        if source.scope != *scope
            || source.execution.tool_call_id().0 != call.id
            || source.execution.run_id().0 != call.run_id
            || source.execution.session_id() != lease.session_id()
            || source.effect_lease_generation != call.lease_generation
            || source.execution.argument_hash() != call.argument_hash
            || source.execution.tool_fingerprint() != call.tool_fingerprint
            || source.execution.provider_call_id() != call.provider_call_id
            || source.disclosure_fingerprint
                != call
                    .disclosure_schema_fingerprint
                    .clone()
                    .ok_or(AiError::Conflict)?
            || source.result_sha256 != canonical_json_hash(output)?
            || call.result_classification.as_deref()
                != Some(classification_value(source.classification))
            || call.execution_provenance.as_ref()
                != Some(
                    &serde_json::to_value(&source.execution)
                        .map_err(|_| AiError::PersistenceFailed)?,
                )
        {
            return Err(AiError::Conflict);
        }
        Ok(crate::AiToolResultDisclosureContext::from_retained(
            source,
            lease,
            crate::AiToolResultDisclosurePurpose::Continuation,
        )?
        .source_reference())
    }
}
