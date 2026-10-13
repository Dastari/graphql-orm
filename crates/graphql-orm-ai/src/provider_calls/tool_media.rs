//! Current admission of retained native history and truthful withholding without effect replay.
use super::*;

impl AiProviderCallExecutor {
    pub(super) async fn prepare_retained_tool_history(
        &self,
        lease: &AiRunLease,
        plan: &mut AiProviderCallPlan,
        claim: &crate::AiProviderSessionClaim,
    ) -> Result<u64, AiError> {
        let Some(service) = &self.tool_result_source_service else {
            if self.runtime.tool_catalog().descriptors().any(|descriptor| {
                descriptor.result_media.is_some()
                    || descriptor.maximum_classification >= crate::DataClassification::Restricted
            }) {
                return Err(AiError::InvalidConfiguration(
                    "current protected tool source loader is required".into(),
                ));
            }
            return Ok(0);
        };
        let binding = crate::orm_provider_session::AiProviderSessionBindingRecord::find_by_id(
            service.run_service().database(),
            &claim.binding_id(),
        )
        .await
        .map_err(|_| AiError::PersistenceFailed)?
        .ok_or(AiError::Conflict)?;
        if binding.session_id != lease.session_id().0
            || binding.state != "claimed"
            || binding.claimed_run_id != Some(lease.run_id().0)
            || binding.claimed_attempt_id != Some(lease.attempt_id())
            || binding.claimed_run_lease_generation != Some(lease.lease_generation())
            || binding.registration_fingerprint != claim.descriptor().registration_fingerprint()
            || binding.provider_profile_id != claim.descriptor().provider_profile_id()
            || binding.claim_generation != claim.binding_claim_generation()
        {
            return Err(AiError::Conflict);
        }
        let inference = plan
            .transfers
            .iter_mut()
            .find(|manifest| manifest.capability == AiEgressCapability::ModelInference)
            .ok_or(AiError::EgressDenied)?;
        let contexts = service
            .retained_session_sources(
                lease,
                inference,
                claim.binding_id(),
                &binding.cursor_fingerprint,
            )
            .await?;
        if contexts.is_empty() {
            return Ok(0);
        }
        let mut images = 0_u64;
        let mut labels = Vec::new();
        for source in &contexts {
            let descriptor = self
                .runtime
                .tool_catalog()
                .descriptors()
                .find(|descriptor| {
                    descriptor.fingerprint == source.execution_provenance().tool_fingerprint()
                })
                .ok_or(AiError::EgressDenied)?;
            for reference in source.images() {
                let media = descriptor
                    .result_media
                    .as_ref()
                    .ok_or(AiError::EgressDenied)?;
                let now_ms = i64::try_from(self.clock.now().unix_timestamp_nanos() / 1_000_000)
                    .map_err(|_| AiError::EgressDenied)?;
                reference
                    .validate_at(media.limits(), now_ms)
                    .map_err(|_| AiError::EgressDenied)?;
                images = images.checked_add(1).ok_or(AiError::EgressDenied)?;
                labels.push(serde_json::json!({"reference":reference.reference(),"capturedAtMs":reference.captured_at_ms(),"actionExpiresAtMs":reference.action_expires_at_ms(),"retainedUntilMs":reference.retained_until_ms(),"historicalObservation":true,"actionEvidenceRenewed":false,"currentFieldDescribesOriginalCaptureOnly":true}));
            }
            let data_source = source.data_source();
            if !inference.sources.contains(&data_source) {
                inference.sources.push(data_source);
            }
        }
        if images > plan.budget.estimate.image_units {
            return Err(AiError::EgressDenied);
        }
        if !labels.is_empty() {
            plan.request.input.push(ModelInputBlock::Text {
                text: serde_json::json!({"retainedToolImageCaptureLabels":labels}).to_string(),
            });
            inference.estimated_bytes = plan
                .request
                .conservative_provider_egress_bytes(&plan.provider_kind)
                .map_err(|_| AiError::EgressDenied)?;
        }
        // This is a preflight only. Actual post-reservation proof admission repeats owning
        // principal/resource/media checks immediately before provider dispatch.
        self.runtime
            .authorize_tool_source_egress(lease.principal_reference(), inference, &contexts)
            .await?
            .authorize(inference)?;
        Ok(images)
    }

    pub(super) async fn withheld_tool_history_plan(
        &self,
        lease: &AiRunLease,
        mut plan: AiProviderCallPlan,
    ) -> Result<AiProviderCallPlan, AiError> {
        let service = self
            .tool_result_source_service
            .as_ref()
            .ok_or(AiError::EgressDenied)?;
        let text = service.current_user_text(lease, &plan.budget.scope).await?;
        // Retain only the current independently reopened human input and ordinary owned
        // attachments. Prior model text/JSON can contain derived Restricted content too.
        plan.request
            .input
            .retain(|input| matches!(input, ModelInputBlock::Attachment { .. }));
        plan.request.input.insert(0, ModelInputBlock::Text { text });
        let notice = serde_json::json!({"savedToolResultsRetained":true,"historicalToolContentWithheld":true,
            "executionRepeated":false,"providerContextReset":true});
        plan.request.input.push(ModelInputBlock::Text {
            text: notice.to_string(),
        });
        plan.request.continuation = None;
        plan.request.continuation_mode = ModelContinuationMode::ProviderRetained;
        plan.request.tools.clear();
        plan.request.builtin_tools.clear();
        plan.request.maximum_builtin_tool_calls = None;
        plan.tool_rule_bindings.clear();
        plan.classified_native = false;
        plan.transfers.retain(|manifest| {
            manifest.capability != AiEgressCapability::ToolResult
                && manifest.capability != AiEgressCapability::ImageAnalysis
        });
        let inference = plan
            .transfers
            .iter_mut()
            .find(|manifest| manifest.capability == AiEgressCapability::ModelInference)
            .ok_or(AiError::EgressDenied)?;
        inference.sources.retain(|source| {
            source.kind != "application_tool_result" && source.kind != "tool_result_image"
        });
        inference.sources.push(crate::AiDataSourceRef {
            kind: "saved_tool_content_withheld".into(),
            reference: format!(
                "{}:{}:{}",
                lease.run_id().0,
                lease.attempt_id(),
                lease.lease_generation()
            ),
            classification: crate::DataClassification::Public,
            trust: crate::AiSourceTrust::TrustedRuntime,
        });
        inference.estimated_bytes = plan
            .request
            .conservative_provider_egress_bytes(&plan.provider_kind)
            .map_err(|_| AiError::EgressDenied)?;
        let idempotency = serde_json::json!({"original":plan.budget.idempotency_key,"run":lease.run_id().0,
            "attempt":lease.attempt_id(),"generation":lease.lease_generation(),"purpose":"saved_tool_content_withheld"});
        plan.budget.idempotency_key = format!(
            "withheld:{}",
            crate::tools::canonical_json_digest(&idempotency)
        );
        // Original estimate/counters are preserved as an upper bound; no budget or general
        // source boundary is widened by removing unavailable content or resetting context.
        Ok(plan)
    }
}
