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
        let images = extract_references(
            descriptor,
            output,
            self.clock.now().unix_timestamp_nanos() / 1_000_000,
        )?;
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
        };
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
            )
            .await?;
        Ok(RetainedDisclosure {
            protected_result,
            protected_source,
        })
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
            || origin.lease_generation() != call.lease_generation
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
        let images = self.reopen_tool_images(lease, &context).await?;
        if images.is_empty() {
            return Ok(images);
        }
        let mut manifest = tool_manifest.clone();
        manifest.capability = AiEgressCapability::ImageAnalysis;
        manifest.attachment_count =
            u32::try_from(images.len()).map_err(|_| AiError::EgressDenied)?;
        manifest.sources = images
            .iter()
            .map(|image| {
                image
                    .reference()
                    .egress_source(&context, image.byte_count())
            })
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
                std::slice::from_ref(&context),
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
        let resolver = self.runtime.tool_media_resolver(policy.resolver_id())?;
        let operation = async {
            let mut images = Vec::with_capacity(context.images().len());
            let mut bytes = 0_u64;
            for reference in context.images() {
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
                images.push(image);
            }
            Ok(images)
        };
        tokio::time::timeout(
            std::time::Duration::from_millis(policy.limits().maximum_resolution_ms()),
            operation,
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
    Ok(images)
}
