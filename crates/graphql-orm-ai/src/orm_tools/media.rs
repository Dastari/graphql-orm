//! Current protected source loading. These paths never execute application effects.

use super::*;
use crate::tool_media::RetainedToolResultSource;
use crate::{
    AiResolvedToolImage, AiToolExecutionProvenance, AiToolImageReference,
    AiToolResultDisclosureContext, AiToolResultDisclosurePurpose,
};

pub(super) struct RetainedDisclosure {
    pub(super) protected_result: serde_json::Value,
    pub(super) protected_source: serde_json::Value,
}

impl OrmAiApplicationToolCallService {
    pub(crate) async fn record_current_source_decision(
        &self,
        manifest: &AiEgressManifest,
        decision: &crate::AiEgressDecision,
    ) -> Result<(), AiError> {
        self.egress_audit.record(manifest, decision).await
    }

    pub(crate) async fn validate_delivery_fence(&self, lease: &AiRunLease) -> Result<(), AiError> {
        self.run_service.validate_tool_disclosure_fence(lease).await
    }
    pub(super) async fn bind_original_provider_cursor(
        &self,
        lease: &AiRunLease,
        provenance: Option<AiToolExecutionProvenance>,
    ) -> Result<Option<AiToolExecutionProvenance>, AiError> {
        let Some(provenance) = provenance else {
            return Ok(None);
        };
        self.run_service
            .validate_tool_disclosure_fence(lease)
            .await?;
        let fingerprint = if let Some(binding_id) = provenance.provider_session_binding_id() {
            let binding = crate::orm_provider_session::AiProviderSessionBindingRecord::find_by_id(
                self.run_service.database(),
                &binding_id,
            )
            .await
            .map_err(|_| AiError::PersistenceFailed)?
            .ok_or(AiError::Conflict)?;
            if binding.session_id != lease.session_id().0
                || binding.state != "claimed"
                || binding.claimed_run_id != Some(lease.run_id().0)
                || binding.claimed_attempt_id != Some(lease.attempt_id())
                || binding.claimed_run_lease_generation != Some(lease.lease_generation())
                || binding
                    .claim_expires_at
                    .is_none_or(|expiry| expiry <= self.clock.now().unix_timestamp())
                || binding.provider_profile_id != provenance.provider_profile_id()
                || binding.registration_fingerprint
                    != provenance
                        .provider_registration_fingerprint()
                        .ok_or(AiError::Conflict)?
            {
                return Err(AiError::Conflict);
            }
            Some(binding.cursor_fingerprint)
        } else {
            None
        };
        match fingerprint {
            Some(fingerprint) => provenance
                .with_provider_cursor_fingerprint(fingerprint)
                .map(Some),
            None => Ok(Some(provenance)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn retain_disclosure_source(
        &self,
        lease: &AiRunLease,
        descriptor: &AiToolDescriptor,
        provenance: &AiToolExecutionProvenance,
        call_context: &AiApplicationToolCallContext,
        policy: &crate::AiContentProtectionPolicy,
        output: &serde_json::Value,
        disclosure_fingerprint: &str,
        classification: DataClassification,
        trust: AiSourceTrust,
    ) -> Result<RetainedDisclosure, AiError> {
        self.current_access(lease, &call_context.scope).await?;
        let protected_result = self
            .protect(
                policy,
                protection_context(
                    "graphql_orm_ai_tool_calls",
                    provenance.tool_call_id().0,
                    "protected_result",
                    &call_context.scope,
                ),
                output.clone(),
            )
            .await?;
        self.run_service
            .retain_tool_effect_output(lease, provenance, protected_result.clone())
            .await?;
        let images = extract_references(
            descriptor,
            output,
            self.clock.now().unix_timestamp_nanos() / 1_000_000,
        )?;
        let provider_cursor_fingerprint =
            provenance.provider_cursor_fingerprint().map(str::to_owned);
        if provenance.provider_session_binding_id().is_some()
            != provider_cursor_fingerprint.is_some()
        {
            return Err(AiError::Conflict);
        }
        let source = RetainedToolResultSource {
            format_version: 1,
            execution: provenance.clone(),
            principal: lease.principal_reference().clone(),
            scope: call_context.scope.clone(),
            result_sha256: canonical_json_hash(output)?,
            disclosure_fingerprint: disclosure_fingerprint.to_owned(),
            classification,
            trust,
            delegation_reference: call_context.delegation_reference.clone(),
            images,
            provider_cursor_fingerprint: provider_cursor_fingerprint.clone(),
            effect_attempt_id: lease.attempt_id(),
            effect_lease_generation: lease.lease_generation(),
        };
        let source_context = AiToolResultDisclosureContext::from_retained(
            source.clone(),
            lease,
            AiToolResultDisclosurePurpose::ToolResult,
        )?;
        let image_count =
            i64::try_from(source_context.images().len()).map_err(|_| AiError::PersistenceFailed)?;
        let protected_source = self
            .protect(
                policy,
                protection_context(
                    "graphql_orm_ai_tool_calls",
                    provenance.tool_call_id().0,
                    "protected_result_source",
                    &call_context.scope,
                ),
                serde_json::to_value(source).map_err(|_| AiError::PersistenceFailed)?,
            )
            .await?;
        self.run_service
            .retain_tool_result_source(
                lease,
                provenance.tool_call_id(),
                protected_result.clone(),
                protected_source.clone(),
                provenance.clone(),
                source_context.source_reference(),
                classification_value(classification).to_owned(),
                image_count,
                provider_cursor_fingerprint,
            )
            .await?;
        Ok(RetainedDisclosure {
            protected_result,
            protected_source,
        })
    }

    pub(crate) async fn retained_session_sources(
        &self,
        lease: &AiRunLease,
        inference: &AiEgressManifest,
        binding_id: Uuid,
        cursor_fingerprint: &str,
    ) -> Result<Vec<AiToolResultDisclosureContext>, AiError> {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            Box::pin(self.retained_session_sources_inner(
                lease,
                inference,
                binding_id,
                cursor_fingerprint,
            )),
        )
        .await
        .map_err(|_| AiError::ToolExecutionFailed)?
    }

    async fn retained_session_sources_inner(
        &self,
        lease: &AiRunLease,
        inference: &AiEgressManifest,
        binding_id: Uuid,
        cursor_fingerprint: &str,
    ) -> Result<Vec<AiToolResultDisclosureContext>, AiError> {
        use graphql_orm::graphql::filters::{StringFilter, UuidFilter};
        self.current_access(lease, &inference.scope).await?;
        self.run_service
            .validate_tool_disclosure_fence(lease)
            .await?;
        // Query only small source-address rows. Never scan entire protected result/argument bodies
        // to discover history. An incomplete bounded source list cannot admit a retained cursor.
        let rows = AiToolResultSourceRecord::query(self.run_service.database().pool())
            .filter(AiToolResultSourceRecordWhereInput {
                session_id: Some(UuidFilter {
                    eq: Some(lease.session_id().0),
                    ..Default::default()
                }),
                provider_kind: Some(StringFilter {
                    eq: Some(inference.provider_kind.clone()),
                    ..Default::default()
                }),
                provider_profile_id: Some(StringFilter {
                    eq: Some(inference.provider_profile_id.clone()),
                    ..Default::default()
                }),
                provider_model: Some(StringFilter {
                    eq: Some(inference.model.clone()),
                    ..Default::default()
                }),
                provider_session_binding_id: Some(UuidFilter {
                    eq: Some(binding_id),
                    ..Default::default()
                }),
                provider_cursor_fingerprint: Some(StringFilter {
                    eq: Some(cursor_fingerprint.to_owned()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .limit(65)
            .fetch_all()
            .await
            .map_err(|_| AiError::PersistenceFailed)?;
        if rows.len() > 64 {
            return Err(AiError::ResultBudgetExceeded);
        }
        let operation = async {
            let mut sources = Vec::with_capacity(rows.len());
            for row in rows {
                let source = self
                    .reopen_tool_result_source(
                        lease,
                        AiToolCallId(row.id),
                        AiToolResultDisclosurePurpose::History,
                    )
                    .await?;
                if source.source_reference() != row.source_reference
                    || source.execution_provenance().run_id().0 != row.run_id
                    || source.execution_provenance().provider_kind().as_str() != row.provider_kind
                    || source.execution_provenance().provider_profile_id()
                        != row.provider_profile_id
                    || source.execution_provenance().provider_model() != row.provider_model
                    || classification_value(source.classification()) != row.classification
                    || i64::try_from(source.images().len()).ok() != Some(row.image_count)
                {
                    return Err(AiError::Conflict);
                }
                sources.push(source);
            }
            self.run_service
                .validate_tool_disclosure_fence(lease)
                .await?;
            Ok(sources)
        };
        tokio::time::timeout(std::time::Duration::from_secs(30), Box::pin(operation))
            .await
            .map_err(|_| AiError::ToolExecutionFailed)?
    }

    pub(crate) async fn current_user_text(
        &self,
        lease: &AiRunLease,
        scope: &AiScope,
    ) -> Result<String, AiError> {
        use graphql_orm::graphql::filters::UuidFilter;
        self.run_service
            .validate_tool_disclosure_fence(lease)
            .await?;
        let principal = self.current_access(lease, scope).await?;
        let policy = self
            .runtime
            .content_protection_policy_resolver()
            .resolve(principal.principal(), scope)
            .await?;
        let message =
            AiMessageRecord::find_by_id(self.run_service.database(), &lease.input_message_id())
                .await
                .map_err(|_| AiError::PersistenceFailed)?
                .ok_or(AiError::NotFound)?;
        if message.session_id != lease.session_id().0
            || message.run_id != Some(lease.run_id().0)
            || message.message_role != "user"
            || message.content_purged_at.is_some()
            || message.completion_state != "complete"
            || message.block_count != 1
            || message.author_subject.as_deref()
                != Some(lease.principal_reference().subject.as_str())
            || !policy.ready
            || policy.scope != *scope
        {
            return Err(AiError::Conflict);
        }
        let blocks = AiMessageBlockRecord::query(self.run_service.database().pool())
            .filter(AiMessageBlockRecordWhereInput {
                message_id: Some(UuidFilter {
                    eq: Some(message.id),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .limit(2)
            .fetch_all()
            .await
            .map_err(|_| AiError::PersistenceFailed)?;
        if blocks.len() != 1 {
            return Err(AiError::Conflict);
        }
        let block = &blocks[0];
        if block.block_index != 0
            || block.block_kind != "text"
            || block.byte_count < 0
            || usize::try_from(block.byte_count)
                .map_or(true, |bytes| bytes > self.limits.maximum_model_output_bytes)
        {
            return Err(AiError::Conflict);
        }
        let value = self
            .open(
                &policy,
                protection_context(
                    "graphql_orm_ai_message_blocks",
                    block.id,
                    "protected_content",
                    scope,
                ),
                &block.protected_content,
            )
            .await?;
        let text = value
            .get("text")
            .and_then(serde_json::Value::as_str)
            .ok_or(AiError::Conflict)?;
        if i64::try_from(text.len()).ok() != Some(block.byte_count) {
            return Err(AiError::Conflict);
        }
        self.run_service
            .validate_tool_disclosure_fence(lease)
            .await?;
        Ok(text.to_owned())
    }

    /// Reopens one original protected tool-result source under the current disclosure lease.
    /// This derives identity from retained owner records and independently checks arguments,
    /// result bytes, immutable profile and selected references. It never repeats the tool.
    /// Legacy results without retained source metadata gain no source-specific allowance.
    ///
    /// # Errors
    /// Returns a safe error for a cancelled/stale fence, inaccessible scope, missing/purged result,
    /// changed descriptor, mismatched origin/content/resource or invalid protection envelope.
    pub async fn reopen_tool_result_source(
        &self,
        lease: &AiRunLease,
        id: AiToolCallId,
        purpose: AiToolResultDisclosurePurpose,
    ) -> Result<AiToolResultDisclosureContext, AiError> {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            Box::pin(self.reopen_tool_result_source_inner(lease, id, purpose)),
        )
        .await
        .map_err(|_| AiError::ToolExecutionFailed)?
    }

    async fn reopen_tool_result_source_inner(
        &self,
        lease: &AiRunLease,
        id: AiToolCallId,
        purpose: AiToolResultDisclosurePurpose,
    ) -> Result<AiToolResultDisclosureContext, AiError> {
        self.run_service
            .validate_tool_disclosure_fence(lease)
            .await?;
        let call = AiToolCallRecord::find_by_id(self.run_service.database(), &id.0)
            .await
            .map_err(|_| AiError::PersistenceFailed)?
            .ok_or(AiError::NotFound)?;
        let step = AiRunStepRecord::find_by_id(self.run_service.database(), &id.0)
            .await
            .map_err(|_| AiError::PersistenceFailed)?
            .ok_or(AiError::NotFound)?;
        let original_run = AiRunRecord::find_by_id(self.run_service.database(), &call.run_id)
            .await
            .map_err(|_| AiError::PersistenceFailed)?
            .ok_or(AiError::NotFound)?;
        let session =
            AiSessionRecord::find_by_id(self.run_service.database(), &lease.session_id().0)
                .await
                .map_err(|_| AiError::PersistenceFailed)?
                .ok_or(AiError::NotFound)?;
        let scope = AiScope {
            kind: session.scope_kind.clone(),
            id: session.scope_id.clone(),
            tenant_id: session.tenant_id.clone(),
        };
        validate_session_binding(&session, lease, &scope)?;
        let principal = self.current_access(lease, &scope).await?;
        let policy = self
            .runtime
            .content_protection_policy_resolver()
            .resolve(principal.principal(), &scope)
            .await?;
        if !policy.ready
            || policy.scope != scope
            || call.payload_purged_at.is_some()
            || original_run.session_id != lease.session_id().0
            || step.run_id != call.run_id
            || step.lease_generation != call.lease_generation
            || step.step_kind != "application_tool"
            || !(matches!(
                call.state.as_str(),
                "completed" | "execution_failed" | "egress_denied" | "egress_audit_failed"
            ) && step.state == call.state
                && call.completed_at.is_some()
                && step.finished_at.is_some()
                || call.state == "source_retained" && step.state == "running")
        {
            return Err(AiError::Conflict);
        }
        let source_value = self
            .open(
                &policy,
                protection_context(
                    "graphql_orm_ai_tool_calls",
                    id.0,
                    "protected_result_source",
                    &scope,
                ),
                call.protected_result_source
                    .as_ref()
                    .ok_or(AiError::Forbidden)?,
            )
            .await?;
        let source: RetainedToolResultSource =
            serde_json::from_value(source_value).map_err(|_| AiError::PersistenceFailed)?;
        let descriptor = self
            .runtime
            .tool_catalog()
            .descriptor(&AiToolId::parse(call.tool_id.clone())?)
            .cloned()
            .ok_or(AiError::Forbidden)?;
        let origin = &source.execution;
        let original_principal: agql_auth::PrincipalReference =
            serde_json::from_value(original_run.principal_reference)
                .map_err(|_| AiError::PersistenceFailed)?;
        if source.scope != scope
            || source.principal != original_principal
            || origin.run_id().0 != call.run_id
            || origin.session_id() != lease.session_id()
            || origin.tool_call_id() != id
            || source.effect_lease_generation != call.lease_generation
            || source.provider_cursor_fingerprint.as_deref() != origin.provider_cursor_fingerprint()
            || origin.approval_id().map(|id| id.0) != call.approval_id
            || (call.state != "source_retained"
                && (call.result_classification.as_deref()
                    != Some(classification_value(source.classification))
                    || call.disclosure_schema_fingerprint.as_deref()
                        != Some(source.disclosure_fingerprint.as_str())))
            || origin.provider_call_id() != call.provider_call_id
            || origin.tool_fingerprint() != call.tool_fingerprint
            || descriptor.fingerprint != call.tool_fingerprint
            || origin.argument_hash() != call.argument_hash
            || origin.provider_kind().as_str()
                != call.provider_kind.as_deref().ok_or(AiError::Conflict)?
            || origin.provider_model() != call.provider_model.as_deref().ok_or(AiError::Conflict)?
            || origin.provider_response_id() != call.provider_response_id.as_deref()
            || Some(origin.budget_reservation_id().0) != call.budget_reservation_id
            || source.delegation_reference != call.delegation_reference
            || call.execution_provenance.as_ref()
                != Some(&serde_json::to_value(origin).map_err(|_| AiError::PersistenceFailed)?)
        {
            return Err(AiError::Conflict);
        }
        let arguments = self
            .open(
                &policy,
                protection_context(
                    "graphql_orm_ai_tool_calls",
                    id.0,
                    "protected_arguments",
                    &scope,
                ),
                call.protected_arguments.as_ref().ok_or(AiError::Conflict)?,
            )
            .await?;
        let output = self
            .open(
                &policy,
                protection_context(
                    "graphql_orm_ai_tool_calls",
                    id.0,
                    "protected_result",
                    &scope,
                ),
                call.protected_result.as_ref().ok_or(AiError::Conflict)?,
            )
            .await?;
        if canonical_json_hash(&arguments)? != call.argument_hash
            || canonical_json_hash(&output)? != source.result_sha256
            || source.images != extract_references_at_capture(&descriptor, &output)?
            || descriptor.graphql_contract.as_ref().is_none_or(|contract| {
                contract.disclosure_schema_fingerprint != source.disclosure_fingerprint
            })
            || !matches!(
                source.trust,
                AiSourceTrust::ResolverResult | AiSourceTrust::TrustedRuntime
            )
        {
            return Err(AiError::Conflict);
        }
        self.run_service
            .validate_tool_disclosure_fence(lease)
            .await?;
        AiToolResultDisclosureContext::from_retained(source, lease, purpose)
    }

    pub(crate) async fn authorize_tool_images(
        &self,
        lease: &AiRunLease,
        id: AiToolCallId,
        tool_manifest: &AiEgressManifest,
    ) -> Result<Vec<AiResolvedToolImage>, AiError> {
        let context = self
            .reopen_tool_result_source(lease, id, AiToolResultDisclosurePurpose::ToolResult)
            .await?;
        if tool_manifest.capability != AiEgressCapability::ToolResult
            || tool_manifest.sources != vec![context.data_source()]
            || tool_manifest.scope != *context.scope()
            || tool_manifest.session_id != Some(lease.session_id())
            || tool_manifest.run_id != Some(lease.run_id())
        {
            return Err(AiError::Conflict);
        }
        // Revalidate text/tree disclosure too: authorization may have changed after effect retention.
        let text_decision = self
            .runtime
            .authorize_tool_source_egress(
                lease.principal_reference(),
                tool_manifest,
                std::slice::from_ref(&context),
            )
            .await?;
        self.egress_audit
            .record(tool_manifest, &text_decision)
            .await?;
        text_decision.authorize(tool_manifest)?;
        self.authorize_retained_image_analysis(lease, &context, tool_manifest)
            .await
    }

    pub(crate) async fn authorize_retained_image_analysis(
        &self,
        lease: &AiRunLease,
        context: &AiToolResultDisclosureContext,
        source_manifest: &AiEgressManifest,
    ) -> Result<Vec<AiResolvedToolImage>, AiError> {
        let images = self.reopen_tool_images(lease, context).await?;
        if images.is_empty() {
            return Ok(images);
        }
        let mut manifest = source_manifest.clone();
        manifest.capability = AiEgressCapability::ImageAnalysis;
        manifest.attachment_count =
            u32::try_from(images.len()).map_err(|_| AiError::EgressDenied)?;
        manifest.sources = images
            .iter()
            .map(|image| image.reference().egress_source(context, image.byte_count()))
            .collect();
        // Inline base64 expands exact bytes by 4/3; include MIME/data URI and typed frame overhead.
        manifest.estimated_bytes = images
            .iter()
            .try_fold(0_u64, |sum, image| {
                sum.checked_add(image.byte_count().div_ceil(3).checked_mul(4)?)?
                    .checked_add(image.reference().mime().len() as u64 + 128)
            })
            .ok_or(AiError::EgressDenied)?;
        let decision = self
            .runtime
            .authorize_tool_source_egress(
                lease.principal_reference(),
                &manifest,
                std::slice::from_ref(context),
            )
            .await?;
        self.egress_audit.record(&manifest, &decision).await?;
        decision.authorize(&manifest)?;
        self.current_access(lease, context.scope()).await?;
        self.run_service
            .validate_tool_disclosure_fence(lease)
            .await?;
        Ok(images)
    }

    pub(crate) async fn reopen_tool_images(
        &self,
        lease: &AiRunLease,
        context: &AiToolResultDisclosureContext,
    ) -> Result<Vec<AiResolvedToolImage>, AiError> {
        self.run_service
            .validate_tool_disclosure_fence(lease)
            .await?;
        let descriptor = self
            .runtime
            .tool_catalog()
            .descriptors()
            .find(|descriptor| {
                descriptor.fingerprint == context.execution_provenance().tool_fingerprint()
            })
            .ok_or(AiError::Forbidden)?;
        if context.images().is_empty() {
            return Ok(Vec::new());
        }
        let policy = descriptor.result_media.as_ref().ok_or(AiError::Forbidden)?;
        crate::tool_media::validate_image_reference_batch(context.images(), policy.limits())?;
        let resolver = self.runtime.tool_media_resolver(policy.resolver_id())?;
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_millis(policy.limits().maximum_resolution_ms());
        let operation = async {
            let mut images = Vec::with_capacity(context.images().len());
            let mut bytes = 0_u64;
            for reference in context.images() {
                if std::time::Instant::now() >= deadline {
                    return Err(AiError::ToolExecutionFailed);
                }
                self.run_service
                    .validate_tool_disclosure_fence(lease)
                    .await?;
                let principal = self.current_access(lease, context.scope()).await?;
                let now = i64::try_from(self.clock.now().unix_timestamp_nanos() / 1_000_000)
                    .map_err(|_| AiError::Conflict)?;
                reference.validate_at(policy.limits(), now)?;
                let image = resolver
                    .resolve_for_provider(&principal, descriptor, context, reference)
                    .await?;
                if std::time::Instant::now() >= deadline {
                    return Err(AiError::ToolExecutionFailed);
                }
                if image.reference() != reference {
                    return Err(AiError::Conflict);
                }
                let now = i64::try_from(self.clock.now().unix_timestamp_nanos() / 1_000_000)
                    .map_err(|_| AiError::Conflict)?;
                image.validate_policy(policy.resolver_id(), policy.limits(), now)?;
                bytes = bytes
                    .checked_add(image.byte_count())
                    .ok_or(AiError::Forbidden)?;
                if bytes > policy.limits().maximum_raw_bytes() {
                    return Err(AiError::Forbidden);
                }
                self.current_access(lease, context.scope()).await?;
                self.run_service
                    .validate_tool_disclosure_fence(lease)
                    .await?;
                if std::time::Instant::now() >= deadline {
                    return Err(AiError::ToolExecutionFailed);
                }
                images.push(image);
            }
            Ok(images)
        };
        tokio::time::timeout(
            std::time::Duration::from_millis(policy.limits().maximum_resolution_ms()),
            Box::pin(operation),
        )
        .await
        .map_err(|_| AiError::ToolExecutionFailed)?
    }
}

fn extract_references(
    descriptor: &AiToolDescriptor,
    output: &serde_json::Value,
    now_ms: i128,
) -> Result<Vec<AiToolImageReference>, AiError> {
    extract_references_inner(
        descriptor,
        output,
        Some(i64::try_from(now_ms).map_err(|_| AiError::Forbidden)?),
    )
}
fn extract_references_at_capture(
    descriptor: &AiToolDescriptor,
    output: &serde_json::Value,
) -> Result<Vec<AiToolImageReference>, AiError> {
    extract_references_inner(descriptor, output, None)
}
fn extract_references_inner(
    descriptor: &AiToolDescriptor,
    output: &serde_json::Value,
    now_ms: Option<i64>,
) -> Result<Vec<AiToolImageReference>, AiError> {
    let Some(policy) = &descriptor.result_media else {
        return Ok(Vec::new());
    };
    let mut images = Vec::new();
    for path in policy.object_paths() {
        let mut value = output.get("data").ok_or(AiError::Forbidden)?;
        for field in path {
            value = value.get(field).ok_or(AiError::Forbidden)?;
        }
        if value.is_null() {
            continue;
        }
        let at = now_ms
            .or_else(|| {
                value
                    .get(policy.fields().captured_at_ms())
                    .and_then(serde_json::Value::as_i64)
            })
            .unwrap_or(0);
        if let Some(image) =
            AiToolImageReference::from_output(value, policy.fields(), policy.limits(), at)?
        {
            if images
                .iter()
                .any(|existing: &AiToolImageReference| existing.reference() == image.reference())
            {
                return Err(AiError::Forbidden);
            }
            images.push(image);
        }
    }
    crate::tool_media::validate_image_reference_batch(&images, policy.limits())?;
    Ok(images)
}
