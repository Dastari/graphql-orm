//! Fenced, protected execution of registered read-only application tools.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use std::collections::BTreeMap;
use std::sync::Arc;

use agql_auth::{Clock, PrincipalReferenceKind, ResolvedPrincipal};
use async_trait::async_trait;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use serde_json::json;
use sha2::{Digest, Sha256};
use time::Duration;
use uuid::Uuid;

use crate::orm_runs::{PreparedToolCallFinish, PreparedToolCallStart, PreparedToolLifecycleEvent};
use crate::persistence::*;
use crate::{
    AiApprovalBinding, AiApprovalId, AiApprovalRule, AiCanonicalActionPreview, AiDataSourceRef,
    AiDestinationTrust, AiEgressCapability, AiEgressDecisionAudit, AiEgressManifest, AiError,
    AiProviderCallResult, AiRunCompletion, AiRunLease, AiRunState, AiRuntime, AiScope,
    AiSessionAction, AiSessionId, AiSourceTrust, AiToolCallId, AiToolDescriptor, AiToolId,
    AiToolOperationDomain, AiToolOperationKind, AiToolRisk, ContentProtectionContext,
    DataClassification, GraphqlInvocationContext, ModelInputBlock, OrmAiApprovalService,
    OrmAiRunService, ToolGraphqlRequest, ToolMaturity,
};

/// Deployment-owned hard limits for one durable application-tool call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiApplicationToolCallLimits {
    maximum_argument_bytes: usize,
    maximum_model_output_bytes: usize,
    maximum_provider_turns: u32,
    maximum_calls_per_turn: usize,
    maximum_principal_age: Duration,
    maximum_execution_time: Duration,
}

impl AiApplicationToolCallLimits {
    /// Creates validated tool-call bounds.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] for zero/oversized content,
    /// turn limits outside `1..=1_024`, call limits outside `1..=64`, or a
    /// non-positive principal freshness/execution window.
    pub fn new(
        maximum_argument_bytes: usize,
        maximum_model_output_bytes: usize,
        maximum_provider_turns: u32,
        maximum_calls_per_turn: usize,
        maximum_principal_age: Duration,
        maximum_execution_time: Duration,
    ) -> Result<Self, AiError> {
        const MAXIMUM_BYTES: usize = 64 * 1024 * 1024;
        if maximum_argument_bytes == 0
            || maximum_argument_bytes > MAXIMUM_BYTES
            || maximum_model_output_bytes == 0
            || maximum_model_output_bytes > MAXIMUM_BYTES
            || !(1..=1_024).contains(&maximum_provider_turns)
            || !(1..=64).contains(&maximum_calls_per_turn)
            || !maximum_principal_age.is_positive()
            || !maximum_execution_time.is_positive()
        {
            return Err(AiError::InvalidConfiguration(
                "invalid application-tool call limits".to_owned(),
            ));
        }
        Ok(Self {
            maximum_argument_bytes,
            maximum_model_output_bytes,
            maximum_provider_turns,
            maximum_calls_per_turn,
            maximum_principal_age,
            maximum_execution_time,
        })
    }
}

/// Server-authored position and audit context for one provider tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiApplicationToolCallContext {
    provider_turn_index: u32,
    tool_call_index: usize,
    scope: AiScope,
    correlation_id: String,
    causation_id: String,
    delegation_reference: Option<String>,
}

impl AiApplicationToolCallContext {
    /// Creates a validated call context.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for an empty scope or malformed,
    /// empty, or oversized audit identifiers.
    pub fn new(
        provider_turn_index: u32,
        tool_call_index: usize,
        scope: AiScope,
        correlation_id: impl Into<String>,
        causation_id: impl Into<String>,
    ) -> Result<Self, AiError> {
        let correlation_id = correlation_id.into();
        let causation_id = causation_id.into();
        if scope.kind.trim().is_empty()
            || scope.kind.len() > 128
            || scope.id.trim().is_empty()
            || scope.id.len() > 1_024
            || !valid_audit_reference(&correlation_id)
            || !valid_audit_reference(&causation_id)
        {
            return Err(AiError::InvalidInput(
                "invalid application-tool call context".to_owned(),
            ));
        }
        Ok(Self {
            provider_turn_index,
            tool_call_index,
            scope,
            correlation_id,
            causation_id,
            delegation_reference: None,
        })
    }

    /// Adds a safe delegation/grant reference; never a bearer credential.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for an empty, control-containing, or
    /// oversized reference.
    pub fn with_delegation_reference(
        mut self,
        delegation_reference: impl Into<String>,
    ) -> Result<Self, AiError> {
        let delegation_reference = delegation_reference.into();
        if !valid_audit_reference(&delegation_reference) {
            return Err(AiError::InvalidInput(
                "invalid delegation reference".to_owned(),
            ));
        }
        self.delegation_reference = Some(delegation_reference);
        Ok(self)
    }
}

/// Deployment/scope-selected destination metadata for tool-result egress.
///
/// Provider kind and model are derived from the completed provider turn and
/// cannot be supplied here. The service also derives exact source,
/// classification, byte count, session, run, and scope bindings.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiToolResultEgressRoute {
    provider_profile_id: String,
    destination: String,
    destination_trust: AiDestinationTrust,
    purpose: String,
    retention: String,
    policy_version: String,
    residency: Option<String>,
    consent_reference: Option<String>,
}

impl AiToolResultEgressRoute {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn subscription_wait_manifest(
        &self,
        scope: AiScope,
        session_id: AiSessionId,
        run_id: crate::AiRunId,
        provider_kind: impl Into<String>,
        provider_model: impl Into<String>,
        source: AiDataSourceRef,
        estimated_bytes: u64,
    ) -> AiEgressManifest {
        AiEgressManifest {
            provider_profile_id: self.provider_profile_id.clone(),
            provider_kind: provider_kind.into(),
            model: provider_model.into(),
            destination: self.destination.clone(),
            destination_trust: self.destination_trust,
            capability: AiEgressCapability::ToolResult,
            scope,
            session_id: Some(session_id),
            run_id: Some(run_id),
            sources: vec![source],
            estimated_bytes,
            estimated_tokens: 0,
            attachment_count: 0,
            purpose: self.purpose.clone(),
            retention: self.retention.clone(),
            residency: self.residency.clone(),
            policy_version: self.policy_version.clone(),
            consent_reference: self.consent_reference.clone(),
        }
    }

    pub(crate) fn from_checkpoint_value(value: serde_json::Value) -> Result<Self, AiError> {
        let route: Self = serde_json::from_value(value).map_err(|_| AiError::PersistenceFailed)?;
        route.validate()?;
        Ok(route)
    }
    pub(crate) fn checkpoint_value(&self) -> serde_json::Value {
        serde_json::json!({
            "providerProfileId": self.provider_profile_id,
            "destination": self.destination,
            "destinationTrust": self.destination_trust,
            "purpose": self.purpose,
            "retention": self.retention,
            "policyVersion": self.policy_version,
            "residency": self.residency,
            "consentReference": self.consent_reference,
        })
    }

    pub(crate) fn matches_manifest(
        &self,
        manifest: &AiEgressManifest,
        lease: &AiRunLease,
        scope: &AiScope,
        provider_kind: &str,
        provider_model: &str,
    ) -> bool {
        manifest.provider_profile_id == self.provider_profile_id
            && manifest.provider_kind == provider_kind
            && manifest.model == provider_model
            && manifest.destination == self.destination
            && manifest.destination_trust == self.destination_trust
            && manifest.capability == AiEgressCapability::ToolResult
            && manifest.scope == *scope
            && manifest.session_id == Some(lease.session_id())
            && manifest.run_id == Some(lease.run_id())
            && manifest.purpose == self.purpose
            && manifest.retention == self.retention
            && manifest.residency == self.residency
            && manifest.policy_version == self.policy_version
            && manifest.consent_reference == self.consent_reference
    }

    /// Creates required server-owned destination metadata.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] when a required bounded value is
    /// empty or contains control characters.
    pub fn new(
        provider_profile_id: impl Into<String>,
        destination: impl Into<String>,
        destination_trust: AiDestinationTrust,
        purpose: impl Into<String>,
        retention: impl Into<String>,
        policy_version: impl Into<String>,
    ) -> Result<Self, AiError> {
        let route = Self {
            provider_profile_id: provider_profile_id.into(),
            destination: destination.into(),
            destination_trust,
            purpose: purpose.into(),
            retention: retention.into(),
            policy_version: policy_version.into(),
            residency: None,
            consent_reference: None,
        };
        route.validate()?;
        Ok(route)
    }

    /// Adds a bounded processing residency/region class.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for a malformed value.
    pub fn with_residency(mut self, residency: impl Into<String>) -> Result<Self, AiError> {
        self.residency = Some(residency.into());
        self.validate()?;
        Ok(self)
    }

    /// Adds a purpose-bound consent/grant reference.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for a malformed value.
    pub fn with_consent_reference(
        mut self,
        consent_reference: impl Into<String>,
    ) -> Result<Self, AiError> {
        self.consent_reference = Some(consent_reference.into());
        self.validate()?;
        Ok(self)
    }

    pub(crate) fn validate(&self) -> Result<(), AiError> {
        let required = [
            self.provider_profile_id.as_str(),
            self.destination.as_str(),
            self.purpose.as_str(),
            self.retention.as_str(),
            self.policy_version.as_str(),
        ];
        if required.iter().any(|value| !valid_audit_reference(value))
            || self
                .residency
                .as_deref()
                .is_some_and(|value| !valid_audit_reference(value))
            || self
                .consent_reference
                .as_deref()
                .is_some_and(|value| !valid_audit_reference(value))
        {
            return Err(AiError::InvalidInput(
                "invalid tool-result egress route".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Durable outcome of one model-requested application query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiApplicationToolCallState {
    /// Resolver output passed static disclosure and exact result egress.
    Completed,
    /// Resolver/policy execution failed and a safe error was authorized for the model.
    ExecutionFailed,
    /// Exact result disclosure was denied and no output may reach the model.
    EgressDenied,
    /// Egress audit persistence failed, closing transport despite an allow decision.
    EgressAuditFailed,
}

impl AiApplicationToolCallState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::ExecutionFailed => "execution_failed",
            Self::EgressDenied => "egress_denied",
            Self::EgressAuditFailed => "egress_audit_failed",
        }
    }
}

/// Protected and fenced application-tool outcome.
#[derive(Clone, Debug)]
pub struct AiPersistedApplicationToolCall {
    id: AiToolCallId,
    provider_call_id: String,
    state: AiApplicationToolCallState,
    model_input: Option<ModelInputBlock>,
    egress_manifest: Option<AiEgressManifest>,
    lease: AiRunLease,
}

/// Server-owned canonical preview builder for one exact consequential
/// resolver request.
///
/// Implementations may use a host dry-run/projection service, but must never
/// accept model prose as the authoritative preview. Every returned target must
/// include a current optimistic-concurrency version. This contract does not
/// grant approval or resolver authority.
#[async_trait]
pub trait AiCanonicalActionPreviewBuilder: Send + Sync {
    /// Builds a bounded canonical preview from current application state.
    ///
    /// # Errors
    ///
    /// Returns a safe error when current resource state cannot be loaded,
    /// preview generation is denied, or a complete bounded preview cannot be
    /// produced.
    async fn build_preview(
        &self,
        principal: &ResolvedPrincipal,
        descriptor: &AiToolDescriptor,
        request: &ToolGraphqlRequest,
    ) -> Result<AiCanonicalActionPreview, AiError>;
}

/// Durable result of staging one exact supervised mutation for human review.
#[derive(Clone, Debug)]
pub struct AiRequestedConsequentialToolCall {
    tool_call_id: AiToolCallId,
    approval_id: AiApprovalId,
    lease: AiRunLease,
}

impl AiRequestedConsequentialToolCall {
    /// Durable tool-call identifier.
    pub const fn tool_call_id(&self) -> AiToolCallId {
        self.tool_call_id
    }

    /// Pending one-shot approval identifier.
    pub const fn approval_id(&self) -> AiApprovalId {
        self.approval_id
    }

    /// Renewed lease in `WaitingApproval`.
    pub fn lease(&self) -> &AiRunLease {
        &self.lease
    }

    /// Consumes the staged result and returns its waiting lease.
    pub fn into_lease(self) -> AiRunLease {
        self.lease
    }
}

/// Consequential execution outcome after approval consumption.
#[derive(Clone, Debug)]
pub enum AiConsequentialToolCallOutcome {
    /// Resolver result was protected and durably closed; model disclosure may
    /// still be absent when exact egress was denied or its audit failed.
    Persisted(Box<AiPersistedApplicationToolCall>),
    /// Resolver execution or its post-side-effect handoff was ambiguous. The
    /// run is terminally closed for privileged reconciliation and must not be
    /// retried automatically.
    RecoveryRequired {
        /// Durable call left for privileged reconciliation.
        tool_call_id: AiToolCallId,
    },
}

impl AiConsequentialToolCallOutcome {
    /// Returns the durable tool-call identifier.
    pub const fn tool_call_id(&self) -> AiToolCallId {
        match self {
            Self::Persisted(call) => call.id(),
            Self::RecoveryRequired { tool_call_id } => *tool_call_id,
        }
    }

    /// Returns the persisted result when the call closed unambiguously.
    pub fn persisted(&self) -> Option<&AiPersistedApplicationToolCall> {
        match self {
            Self::Persisted(call) => Some(call.as_ref()),
            Self::RecoveryRequired { .. } => None,
        }
    }
}

impl AiPersistedApplicationToolCall {
    pub(crate) fn checkpoint_value(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "id": self.id.0,
            "providerCallId": self.provider_call_id,
            "state": self.state.as_str(),
            "modelInput": self.model_input.as_ref()?,
            "egressManifest": self.egress_manifest.as_ref()?,
        }))
    }

    /// Durable local tool-call identifier.
    pub const fn id(&self) -> AiToolCallId {
        self.id
    }

    /// Opaque provider call identifier matched by continuation input.
    pub fn provider_call_id(&self) -> &str {
        &self.provider_call_id
    }

    /// Durable safe lifecycle result.
    pub const fn state(&self) -> AiApplicationToolCallState {
        self.state
    }

    /// Separately egress-authorized provider continuation block, when allowed.
    pub fn model_input(&self) -> Option<&ModelInputBlock> {
        self.model_input.as_ref()
    }

    pub(crate) fn egress_manifest(&self) -> Option<&AiEgressManifest> {
        self.egress_manifest.as_ref()
    }

    /// Renewed lease proof required for the next call/turn/fenced write.
    pub fn lease(&self) -> &AiRunLease {
        &self.lease
    }

    /// Consumes the outcome and returns its renewed lease proof.
    pub fn into_lease(self) -> AiRunLease {
        self.lease
    }

    #[cfg(test)]
    pub(crate) fn test_completed(
        lease: AiRunLease,
        provider_call_id: &str,
        tool_id: &str,
        output: Option<serde_json::Value>,
        egress_manifest: Option<AiEgressManifest>,
    ) -> Self {
        Self {
            id: AiToolCallId::new(),
            provider_call_id: provider_call_id.to_owned(),
            state: AiApplicationToolCallState::Completed,
            model_input: output.map(|output| ModelInputBlock::ToolResult {
                call_id: provider_call_id.to_owned(),
                tool_id: tool_id.to_owned(),
                output,
            }),
            egress_manifest,
            lease,
        }
    }
}

/// Executes exact registered application queries through current auth and the
/// ordinary GraphQL resolver path, then protects and fences their results.
#[derive(Clone)]
pub struct OrmAiApplicationToolCallService {
    run_service: OrmAiRunService,
    runtime: Arc<AiRuntime>,
    egress_audit: Arc<dyn AiEgressDecisionAudit>,
    clock: Arc<dyn Clock>,
    limits: AiApplicationToolCallLimits,
    #[cfg(test)]
    automatic_mutation_fault: Option<AutomaticMutationTestFault>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutomaticMutationTestFault {
    AfterResolver,
    SerializeResult,
    BoundResult,
    ReauthorizeResult,
    ProtectResult,
    FinishResult,
}

#[derive(Clone, Copy)]
enum UnapprovedToolMode {
    ReadOnly,
    AutomaticMutation,
}

impl OrmAiApplicationToolCallService {
    /// Creates a protected ORM-backed application-tool service.
    pub fn new(
        run_service: OrmAiRunService,
        runtime: Arc<AiRuntime>,
        egress_audit: Arc<dyn AiEgressDecisionAudit>,
        clock: Arc<dyn Clock>,
        limits: AiApplicationToolCallLimits,
    ) -> Self {
        Self {
            run_service,
            runtime,
            egress_audit,
            clock,
            limits,
            #[cfg(test)]
            automatic_mutation_fault: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_automatic_mutation_fault(
        mut self,
        fault: AutomaticMutationTestFault,
    ) -> Self {
        self.automatic_mutation_fault = Some(fault);
        self
    }

    #[cfg(test)]
    fn inject_automatic_mutation_fault(
        &self,
        mode: UnapprovedToolMode,
        fault: AutomaticMutationTestFault,
    ) -> Result<(), AiError> {
        if matches!(mode, UnapprovedToolMode::AutomaticMutation)
            && self.automatic_mutation_fault == Some(fault)
        {
            return Err(AiError::PersistenceFailed);
        }
        Ok(())
    }

    /// Executes one exact provider-requested, explicitly enabled read query.
    ///
    /// The provider call must originate from a successful exactly bound turn.
    /// Arguments are protected and persisted before execution. The service
    /// rehydrates and checks access, invokes [`AiRuntime::execute_tool`] (which
    /// rehydrates again and uses ordinary resolver authorization), rechecks
    /// access, obtains and audits an exact tool-result egress decision, and
    /// atomically protects/fences the final row, step, event, and renewed run.
    /// Consequential, proposal, mutation, subscription, approval-required, and
    /// non-idempotent descriptors fail closed.
    ///
    /// # Errors
    ///
    /// Returns an error for stale result/lease/context binding, current access
    /// denial, a changed descriptor, malformed/oversized arguments or result,
    /// unavailable protection, a stale fence, or persistence failure. Resolver
    /// and egress/audit denials are returned as durable non-panicking outcomes.
    pub async fn execute_read_only(
        &self,
        lease: &AiRunLease,
        provider_result: &AiProviderCallResult,
        context: AiApplicationToolCallContext,
        route: AiToolResultEgressRoute,
    ) -> Result<AiPersistedApplicationToolCall, AiError> {
        match self
            .execute_unapproved(
                lease,
                provider_result,
                context,
                route,
                UnapprovedToolMode::ReadOnly,
            )
            .await?
        {
            AiConsequentialToolCallOutcome::Persisted(call) => Ok(*call),
            AiConsequentialToolCallOutcome::RecoveryRequired { .. } => Err(AiError::Conflict),
        }
    }

    /// Executes one exact generated mutation classified `Automatic`.
    ///
    /// The service compiles and freshly preauthorizes the closed plan, then
    /// atomically persists the exact capability/plan/fence before invoking the
    /// resolver. Any timeout or uncertain post-checkpoint execution closes the
    /// run as `RecoveryRequired`; automatic mutations are never replayed.
    ///
    /// # Errors
    ///
    /// Returns a safe error before the pre-effect checkpoint for invalid
    /// bindings or authority denial. After that boundary, execution ambiguity
    /// is returned as a durable recovery-required outcome.
    pub async fn execute_automatic_mutation(
        &self,
        lease: &AiRunLease,
        provider_result: &AiProviderCallResult,
        context: AiApplicationToolCallContext,
        route: AiToolResultEgressRoute,
    ) -> Result<AiConsequentialToolCallOutcome, AiError> {
        self.execute_unapproved(
            lease,
            provider_result,
            context,
            route,
            UnapprovedToolMode::AutomaticMutation,
        )
        .await
    }

    async fn execute_unapproved(
        &self,
        lease: &AiRunLease,
        provider_result: &AiProviderCallResult,
        context: AiApplicationToolCallContext,
        route: AiToolResultEgressRoute,
        mode: UnapprovedToolMode,
    ) -> Result<AiConsequentialToolCallOutcome, AiError> {
        self.validate_outer_binding(lease, provider_result, &context, &route)?;
        let provider_call = provider_result
            .tool_calls()
            .get(context.tool_call_index)
            .ok_or_else(|| AiError::InvalidInput("tool call index is out of bounds".to_owned()))?;
        let argument_bytes = serde_json::to_vec(provider_call.arguments())
            .map_err(|_| AiError::InvalidInput("invalid tool arguments".to_owned()))?;
        if argument_bytes.len() > self.limits.maximum_argument_bytes {
            return Err(AiError::InvalidInput(
                "tool arguments exceed deployment limit".to_owned(),
            ));
        }

        let session =
            AiSessionRecord::find_by_id(self.run_service.database(), &lease.session_id().0)
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
                .ok_or(AiError::NotFound)?;
        validate_session_binding(&session, lease, &context.scope)?;
        let principal = self.current_access(lease, &context.scope).await?;
        let policy = self
            .runtime
            .content_protection_policy_resolver()
            .resolve(principal.principal(), &context.scope)
            .await?;
        if !policy.ready || policy.scope != context.scope {
            return Err(AiError::RuntimeNotReady);
        }

        let id = AiToolCallId::new();
        let provider_call_key = provider_call_key(lease, provider_call.call_id());
        let argument_hash = canonical_json_hash(provider_call.arguments())?;
        let idempotency_key = matches!(mode, UnapprovedToolMode::ReadOnly)
            .then(|| format!("ai-tool:{provider_call_key}"));
        let invocation = GraphqlInvocationContext {
            run_id: lease.run_id(),
            tool_call_id: id,
            scope: context.scope.clone(),
            correlation_id: context.correlation_id.clone(),
            causation_id: context.causation_id.clone(),
            delegation_reference: context.delegation_reference.clone(),
            idempotency_key: idempotency_key.clone(),
        };
        let (descriptor, disclosure_fingerprint, request, generated_query, prepared_mutation) =
            match mode {
                UnapprovedToolMode::ReadOnly => {
                    if let Some(descriptor) = self
                        .runtime
                        .tool_catalog()
                        .descriptor(provider_call.tool_id())
                        .cloned()
                    {
                        let disclosure_fingerprint = self
                            .runtime
                            .tool_catalog()
                            .disclosure_schema(provider_call.tool_id())
                            .map(|schema| schema.fingerprint.clone())
                            .ok_or(AiError::Forbidden)?;
                        if descriptor.fingerprint != provider_call.tool_fingerprint()
                            || descriptor.operation_kind != AiToolOperationKind::Query
                            || descriptor.operation_domain != AiToolOperationDomain::Application
                            || descriptor.maturity != ToolMaturity::ReadOnly
                            || descriptor.risk != AiToolRisk::ReadOnly
                            || descriptor.approval != AiApprovalRule::None
                            || !descriptor.idempotent
                        {
                            return Err(AiError::Forbidden);
                        }
                        let contract = descriptor
                            .graphql_contract
                            .clone()
                            .ok_or(AiError::Forbidden)?;
                        let request = ToolGraphqlRequest {
                            document: descriptor.document.clone(),
                            operation_name: contract.operation_name.clone(),
                            contract,
                            variables: provider_call.arguments().clone(),
                            invocation,
                        };
                        self.runtime
                            .preauthorize_tool(
                                lease.principal_reference(),
                                &descriptor.id,
                                &request,
                            )
                            .await?;
                        (descriptor, disclosure_fingerprint, request, false, None)
                    } else {
                        let compiled = self.runtime.tool_catalog().compile_query_capability(
                            provider_call.tool_id(),
                            provider_call.tool_fingerprint(),
                            provider_call.arguments().clone(),
                        )?;
                        let (descriptor, disclosure, variables) = compiled.into_parts();
                        let contract = descriptor
                            .graphql_contract
                            .clone()
                            .ok_or(AiError::Forbidden)?;
                        let request = ToolGraphqlRequest {
                            document: descriptor.document.clone(),
                            operation_name: contract.operation_name.clone(),
                            contract,
                            variables,
                            invocation,
                        };
                        (
                            descriptor,
                            disclosure.fingerprint.clone(),
                            request,
                            true,
                            None,
                        )
                    }
                }
                UnapprovedToolMode::AutomaticMutation => {
                    let prepared = self.runtime.prepare_mutation_capability(
                        provider_call.tool_id(),
                        provider_call.tool_fingerprint(),
                        provider_call.arguments().clone(),
                        invocation,
                    )?;
                    if prepared.execution_policy()
                        != graphql_orm::graphql::orm::AiMutationExecutionPolicy::Automatic
                    {
                        return Err(AiError::Forbidden);
                    }
                    self.runtime
                        .preauthorize_prepared_mutation(lease.principal_reference(), &prepared)
                        .await?;
                    let descriptor = prepared.descriptor().clone();
                    let disclosure_fingerprint = descriptor
                        .graphql_contract
                        .as_ref()
                        .ok_or(AiError::Forbidden)?
                        .disclosure_schema_fingerprint
                        .clone();
                    let request = prepared.request().clone();
                    (
                        descriptor,
                        disclosure_fingerprint,
                        request,
                        false,
                        Some(prepared),
                    )
                }
            };
        let protected_arguments = self
            .protect(
                &policy,
                protection_context(
                    "graphql_orm_ai_tool_calls",
                    id.0,
                    "protected_arguments",
                    &context.scope,
                ),
                provider_call.arguments().clone(),
            )
            .await?;
        let started_event_id = Uuid::new_v4();
        let started_inbox_event_id = Uuid::new_v4();
        let started_payload = json!({
            "toolCallId": id.0,
            "runId": lease.run_id().0,
            "toolId": provider_call.tool_id().as_str(),
        });
        let protected_started_event = self
            .protect(
                &policy,
                protection_context(
                    "graphql_orm_ai_session_events",
                    started_event_id,
                    "protected_payload",
                    &context.scope,
                ),
                started_payload.clone(),
            )
            .await?;
        let protected_started_inbox_event = self
            .protect(
                &policy,
                protection_context(
                    "graphql_orm_ai_inbox_events",
                    started_inbox_event_id,
                    "protected_payload",
                    &context.scope,
                ),
                started_payload,
            )
            .await?;
        let active_lease = self
            .run_service
            .begin_tool_call(
                lease,
                PreparedToolCallStart {
                    id: id.0,
                    provider_call_key: provider_call_key.clone(),
                    provider_call_id: provider_call.call_id().to_owned(),
                    provider_kind: provider_result.provider_kind().as_str().to_owned(),
                    provider_model: provider_result.provider_model().to_owned(),
                    provider_response_id: provider_result.provider_response_id().map(str::to_owned),
                    budget_reservation_id: provider_result.budget_reservation_id().0,
                    provider_turn_index: i64::from(context.provider_turn_index),
                    tool_call_index: i64::try_from(context.tool_call_index).map_err(|_| {
                        AiError::InvalidInput("tool call index is invalid".to_owned())
                    })?,
                    tool_id: provider_call.tool_id().as_str().to_owned(),
                    tool_fingerprint: provider_call.tool_fingerprint().to_owned(),
                    protected_arguments,
                    argument_hash,
                    risk: risk_value(descriptor.risk).to_owned(),
                    idempotency_key: idempotency_key.clone(),
                    correlation_id: context.correlation_id.clone(),
                    causation_id: context.causation_id.clone(),
                    delegation_reference: context.delegation_reference.clone(),
                    started_event: Some(PreparedToolLifecycleEvent {
                        event_id: started_event_id,
                        inbox_event_id: started_inbox_event_id,
                        protected_event: protected_started_event,
                        protected_inbox_event: protected_started_inbox_event,
                    }),
                    expected_owner_principal_kind: session.owner_principal_kind.clone(),
                    expected_owner_subject: session.owner_subject.clone(),
                    expected_scope_kind: context.scope.kind.clone(),
                    expected_scope_id: context.scope.id.clone(),
                    expected_tenant_id: context.scope.tenant_id.clone(),
                },
            )
            .await?;
        let lease = &active_lease;

        if matches!(mode, UnapprovedToolMode::AutomaticMutation) {
            match self.run_service.cancellation(lease).await {
                Ok(Some(_)) => {
                    return self
                        .mark_automatic_recovery(
                            lease,
                            id,
                            provider_result.provider_response_id().map(str::to_owned),
                        )
                        .await;
                }
                Ok(None) => {}
                Err(error) => {
                    return match self
                        .mark_automatic_recovery(
                            lease,
                            id,
                            provider_result.provider_response_id().map(str::to_owned),
                        )
                        .await
                    {
                        Ok(outcome) => Ok(outcome),
                        Err(_) => Err(error),
                    };
                }
            }
        }

        let execution =
            tokio::time::timeout(self.limits.maximum_execution_time.unsigned_abs(), async {
                if generated_query {
                    self.runtime
                        .execute_query_capability(
                            lease.principal_reference(),
                            provider_call.tool_id(),
                            provider_call.tool_fingerprint(),
                            provider_call.arguments().clone(),
                            request.invocation.clone(),
                        )
                        .await
                } else if let Some(prepared) = prepared_mutation {
                    self.runtime
                        .execute_prepared_automatic_mutation(lease.principal_reference(), prepared)
                        .await
                } else {
                    self.runtime
                        .execute_tool(lease.principal_reference(), &descriptor.id, request)
                        .await
                }
            })
            .await
            .unwrap_or(Err(AiError::ToolExecutionFailed));
        let (
            state,
            model_output,
            classification,
            source_trust,
            authorization_code,
            policy_version,
            authorization_state_digest,
            application_audit_ref,
        ) = match execution {
            Ok(result) => (
                AiApplicationToolCallState::Completed,
                result.model_output(),
                result.disclosure().maximum_classification,
                AiSourceTrust::ResolverResult,
                "allowed".to_owned(),
                Some(result.policy_version().to_owned()),
                Some(result.authorization_state_digest().to_owned()),
                result.response().application_audit_ref.clone(),
            ),
            Err(_) if matches!(mode, UnapprovedToolMode::AutomaticMutation) => {
                return self
                    .mark_automatic_recovery(
                        lease,
                        id,
                        provider_result.provider_response_id().map(str::to_owned),
                    )
                    .await;
            }
            Err(_) => (
                AiApplicationToolCallState::ExecutionFailed,
                json!({"data": null, "errorCodes": ["AI_TOOL_EXECUTION_FAILED"]}),
                DataClassification::Public,
                AiSourceTrust::TrustedRuntime,
                "execution_failed".to_owned(),
                None,
                None,
                None,
            ),
        };
        #[cfg(test)]
        if let Err(error) =
            self.inject_automatic_mutation_fault(mode, AutomaticMutationTestFault::AfterResolver)
        {
            return match self
                .mark_automatic_recovery(
                    lease,
                    id,
                    provider_result.provider_response_id().map(str::to_owned),
                )
                .await
            {
                Ok(outcome) => Ok(outcome),
                Err(_) => Err(error),
            };
        }
        let finalization: Result<AiConsequentialToolCallOutcome, AiError> = async {
            #[cfg(test)]
            self.inject_automatic_mutation_fault(
                mode,
                AutomaticMutationTestFault::SerializeResult,
            )?;
            let output_bytes =
                serde_json::to_vec(&model_output).map_err(|_| AiError::ToolExecutionFailed)?;
            #[cfg(test)]
            self.inject_automatic_mutation_fault(mode, AutomaticMutationTestFault::BoundResult)?;
            if output_bytes.len() > self.limits.maximum_model_output_bytes {
                return Err(AiError::ToolExecutionFailed);
            }
            let outbound_bytes = output_bytes
                .len()
                .checked_add(provider_call.call_id().len())
                .and_then(|bytes| bytes.checked_add(provider_call.tool_id().as_str().len()))
                .ok_or_else(|| AiError::InvalidInput("tool result is too large".to_owned()))?;

            #[cfg(test)]
            self.inject_automatic_mutation_fault(
                mode,
                AutomaticMutationTestFault::ReauthorizeResult,
            )?;
            self.current_access(lease, &context.scope).await?;
            let manifest = AiEgressManifest {
                provider_profile_id: route.provider_profile_id,
                provider_kind: provider_result.provider_kind().as_str().to_owned(),
                model: provider_result.provider_model().to_owned(),
                destination: route.destination,
                destination_trust: route.destination_trust,
                capability: AiEgressCapability::ToolResult,
                scope: context.scope.clone(),
                session_id: Some(lease.session_id()),
                run_id: Some(lease.run_id()),
                sources: vec![AiDataSourceRef {
                    kind: "application_tool_result".to_owned(),
                    reference: id.0.to_string(),
                    classification,
                    trust: source_trust,
                }],
                estimated_bytes: u64::try_from(outbound_bytes)
                    .map_err(|_| AiError::InvalidInput("tool result is too large".to_owned()))?,
                estimated_tokens: 0,
                attachment_count: 0,
                purpose: route.purpose,
                retention: route.retention,
                residency: route.residency,
                policy_version: route.policy_version,
                consent_reference: route.consent_reference,
            };
            let decision = self
                .runtime
                .authorize_egress(lease.principal_reference(), &manifest)
                .await?;
            let audit_result = self.egress_audit.record(&manifest, &decision).await;
            let (final_state, model_input, decision_id, manifest_hash, final_authorization_code) =
                if audit_result.is_err() {
                    (
                        AiApplicationToolCallState::EgressAuditFailed,
                        None,
                        None,
                        None,
                        "egress_audit_failed".to_owned(),
                    )
                } else if decision.authorize(&manifest).is_err() {
                    (
                        AiApplicationToolCallState::EgressDenied,
                        None,
                        Some(decision.id.0),
                        Some(decision.manifest_hash.clone()),
                        "egress_denied".to_owned(),
                    )
                } else {
                    (
                        state,
                        Some(ModelInputBlock::ToolResult {
                            call_id: provider_call.call_id().to_owned(),
                            tool_id: provider_call.tool_id().as_str().to_owned(),
                            output: model_output.clone(),
                        }),
                        Some(decision.id.0),
                        Some(decision.manifest_hash.clone()),
                        authorization_code,
                    )
                };
            #[cfg(test)]
            self.inject_automatic_mutation_fault(mode, AutomaticMutationTestFault::ProtectResult)?;
            let protected_result = self
                .protect(
                    &policy,
                    protection_context(
                        "graphql_orm_ai_tool_calls",
                        id.0,
                        "protected_result",
                        &context.scope,
                    ),
                    model_output,
                )
                .await?;
            let event_id = Uuid::new_v4();
            let inbox_event_id = Uuid::new_v4();
            let protected_event = self
                .protect(
                    &policy,
                    protection_context(
                        "graphql_orm_ai_session_events",
                        event_id,
                        "protected_payload",
                        &context.scope,
                    ),
                    json!({
                        "toolCallId": id.0,
                        "runId": lease.run_id().0,
                        "toolId": provider_call.tool_id().as_str(),
                        "state": final_state.as_str(),
                    }),
                )
                .await?;
            let protected_inbox_event = self
                .protect(
                    &policy,
                    protection_context(
                        "graphql_orm_ai_inbox_events",
                        inbox_event_id,
                        "protected_payload",
                        &context.scope,
                    ),
                    json!({
                        "toolCallId": id.0,
                        "runId": lease.run_id().0,
                        "toolId": provider_call.tool_id().as_str(),
                        "state": final_state.as_str(),
                    }),
                )
                .await?;
            #[cfg(test)]
            self.inject_automatic_mutation_fault(mode, AutomaticMutationTestFault::FinishResult)?;
            let renewed = self
                .run_service
                .finish_tool_call(
                    lease,
                    PreparedToolCallFinish {
                        id: id.0,
                        state: final_state.as_str().to_owned(),
                        protected_result,
                        authorization_code: final_authorization_code,
                        authorization_policy_version: policy_version,
                        authorization_state_digest,
                        disclosure_schema_fingerprint: disclosure_fingerprint,
                        result_classification: classification_value(classification).to_owned(),
                        result_egress_decision_id: decision_id,
                        result_egress_manifest_hash: manifest_hash,
                        application_audit_ref,
                        event_id,
                        inbox_event_id,
                        protected_event,
                        protected_inbox_event,
                        correlation_id: context.correlation_id,
                        expected_provider_call_key: provider_call_key,
                        expected_tool_fingerprint: provider_call.tool_fingerprint().to_owned(),
                        expected_owner_principal_kind: session.owner_principal_kind,
                        expected_owner_subject: session.owner_subject,
                        expected_scope_kind: context.scope.kind,
                        expected_scope_id: context.scope.id,
                        expected_tenant_id: context.scope.tenant_id,
                    },
                )
                .await?;
            Ok(AiConsequentialToolCallOutcome::Persisted(Box::new(
                AiPersistedApplicationToolCall {
                    id,
                    provider_call_id: provider_call.call_id().to_owned(),
                    state: final_state,
                    model_input,
                    egress_manifest: decision_id.map(|_| manifest),
                    lease: renewed,
                },
            )))
        }
        .await;
        match finalization {
            Ok(outcome) => Ok(outcome),
            Err(error) if matches!(mode, UnapprovedToolMode::AutomaticMutation) => {
                match self
                    .mark_automatic_recovery(
                        lease,
                        id,
                        provider_result.provider_response_id().map(str::to_owned),
                    )
                    .await
                {
                    Ok(outcome) => Ok(outcome),
                    Err(_) => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn mark_automatic_recovery(
        &self,
        lease: &AiRunLease,
        tool_call_id: AiToolCallId,
        provider_response_id: Option<String>,
    ) -> Result<AiConsequentialToolCallOutcome, AiError> {
        self.run_service
            .finish(
                lease,
                AiRunCompletion::new(
                    AiRunState::RecoveryRequired,
                    "automatic_mutation_uncertain",
                    Some("automatic_mutation_uncertain".to_owned()),
                    provider_response_id,
                )?,
            )
            .await?;
        Ok(AiConsequentialToolCallOutcome::RecoveryRequired { tool_call_id })
    }

    fn validate_outer_binding(
        &self,
        lease: &AiRunLease,
        result: &AiProviderCallResult,
        context: &AiApplicationToolCallContext,
        route: &AiToolResultEgressRoute,
    ) -> Result<(), AiError> {
        route.validate()?;
        if !self.runtime.start_gate().is_ready()
            || result.session_id() != lease.session_id()
            || result.run_id() != lease.run_id()
            || result.attempt_id() != lease.attempt_id()
            || result.lease_generation() != lease.lease_generation()
            || (result.provider_response_id().is_none() && !result.uses_stateless_continuation())
            || context.provider_turn_index >= self.limits.maximum_provider_turns
            || context.tool_call_index >= self.limits.maximum_calls_per_turn
            || context.tool_call_index >= result.tool_calls().len()
        {
            return Err(AiError::Conflict);
        }
        Ok(())
    }

    async fn current_access(
        &self,
        lease: &AiRunLease,
        scope: &AiScope,
    ) -> Result<agql_auth::ResolvedPrincipal, AiError> {
        let principal = self
            .runtime
            .resolve_current_principal(lease.principal_reference())
            .await?;
        let now = self.clock.now();
        if principal.resolved_at() > now
            || now - principal.resolved_at() > self.limits.maximum_principal_age
            || principal
                .reference()
                .expires_at
                .is_some_and(|expires_at| expires_at <= now)
        {
            return Err(AiError::ReauthorizationFailed);
        }
        if !self
            .runtime
            .access_policy()
            .can_access_scope(principal.principal(), scope, AiSessionAction::Write)
            .await
            .is_allowed()
            || !self
                .runtime
                .access_policy()
                .can_access_session(
                    principal.principal(),
                    lease.session_id(),
                    AiSessionAction::Write,
                )
                .await
                .is_allowed()
        {
            return Err(AiError::Forbidden);
        }
        Ok(principal)
    }

    async fn protect(
        &self,
        policy: &crate::AiContentProtectionPolicy,
        context: ContentProtectionContext,
        value: serde_json::Value,
    ) -> Result<serde_json::Value, AiError> {
        let envelope = self
            .runtime
            .content_protector()
            .protect(policy, &context, value)
            .await
            .map_err(|error| match error {
                crate::ContentProtectionError::PolicyNotReady => AiError::RuntimeNotReady,
                _ => AiError::PersistenceFailed,
            })?;
        serde_json::to_value(envelope).map_err(|_| AiError::PersistenceFailed)
    }

    async fn open(
        &self,
        policy: &crate::AiContentProtectionPolicy,
        context: ContentProtectionContext,
        value: &serde_json::Value,
    ) -> Result<serde_json::Value, AiError> {
        let envelope: crate::ProtectedContentEnvelope =
            serde_json::from_value(value.clone()).map_err(|_| AiError::PersistenceFailed)?;
        self.runtime
            .content_protector()
            .open(policy, &context, &envelope)
            .await
            .map_err(|error| match error {
                crate::ContentProtectionError::PolicyNotReady => AiError::RuntimeNotReady,
                _ => AiError::PersistenceFailed,
            })
    }
}

/// ORM-backed supervised mutation lifecycle around exact one-shot approvals.
///
/// The service deliberately separates model-visible discovery, preview and
/// approval staging, atomic consumption, fresh resolver authorization, and
/// result egress. It never executes model-authored GraphQL or treats approval
/// as a replacement for current application authorization.
pub struct OrmAiConsequentialToolCallService {
    application_tools: OrmAiApplicationToolCallService,
    approval_service: OrmAiApprovalService,
    preview_builder: Arc<dyn AiCanonicalActionPreviewBuilder>,
    provider_session_service: Option<Arc<dyn crate::AiProviderSessionService>>,
}

impl OrmAiConsequentialToolCallService {
    /// Creates a supervised consequential tool service.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_service: OrmAiRunService,
        runtime: Arc<AiRuntime>,
        approval_service: OrmAiApprovalService,
        preview_builder: Arc<dyn AiCanonicalActionPreviewBuilder>,
        egress_audit: Arc<dyn AiEgressDecisionAudit>,
        clock: Arc<dyn Clock>,
        limits: AiApplicationToolCallLimits,
    ) -> Self {
        Self {
            application_tools: OrmAiApplicationToolCallService::new(
                run_service,
                runtime,
                egress_audit,
                clock,
                limits,
            ),
            approval_service,
            preview_builder,
            provider_session_service: None,
        }
    }

    /// Enables exact retained-session parking across approval waits.
    #[must_use]
    pub fn with_provider_session_service(
        mut self,
        service: Arc<dyn crate::AiProviderSessionService>,
    ) -> Self {
        self.provider_session_service = Some(service);
        self
    }

    /// Stages one exact provider-requested supervised application mutation and
    /// parks the run for human approval.
    ///
    /// The service rehydrates current access, validates exact provider/catalog
    /// binding, preauthorizes current host tool policy, asks trusted host code
    /// for a canonical version-bound preview, protects the arguments, creates
    /// the fenced tool row, and binds an expiring one-shot approval. No domain
    /// mutation executes in this method.
    ///
    /// # Errors
    ///
    /// Fails closed for stale provider/fence/session binding, a disabled or
    /// non-supervised descriptor, malformed/oversized arguments, current
    /// access or tool-policy denial, incomplete preview, unavailable content
    /// protection, invalid expiry, or persistence ambiguity. If persistence
    /// fails after the tool row is created but before approval parking, no
    /// application side effect has occurred and ordinary run recovery remains
    /// authoritative.
    pub async fn request_approval(
        &self,
        lease: &AiRunLease,
        provider_result: &AiProviderCallResult,
        context: AiApplicationToolCallContext,
        expires_at: time::OffsetDateTime,
        recent_mfa_required: bool,
    ) -> Result<AiRequestedConsequentialToolCall, AiError> {
        validate_provider_binding(&self.application_tools, lease, provider_result, &context)?;
        if provider_result.provider_session_claim().is_some()
            && self.provider_session_service.is_none()
        {
            return Err(AiError::RuntimeNotReady);
        }
        let provider_call = provider_result
            .tool_calls()
            .get(context.tool_call_index)
            .ok_or_else(|| AiError::InvalidInput("tool call index is out of bounds".to_owned()))?;
        let argument_bytes = serde_json::to_vec(provider_call.arguments())
            .map_err(|_| AiError::InvalidInput("invalid tool arguments".to_owned()))?;
        if argument_bytes.len() > self.application_tools.limits.maximum_argument_bytes {
            return Err(AiError::InvalidInput(
                "tool arguments exceed deployment limit".to_owned(),
            ));
        }
        let session = AiSessionRecord::find_by_id(
            self.application_tools.run_service.database(),
            &lease.session_id().0,
        )
        .await
        .map_err(|error| map_orm(OrmPublicError::from(error)))?
        .ok_or(AiError::NotFound)?;
        validate_session_binding(&session, lease, &context.scope)?;
        let current = self
            .application_tools
            .current_access(lease, &context.scope)
            .await?;
        let policy = self
            .application_tools
            .runtime
            .content_protection_policy_resolver()
            .resolve(current.principal(), &context.scope)
            .await?;
        if !policy.ready || policy.scope != context.scope {
            return Err(AiError::RuntimeNotReady);
        }

        let id = AiToolCallId::new();
        let provider_call_key = provider_call_key(lease, provider_call.call_id());
        let argument_hash = canonical_json_hash(provider_call.arguments())?;
        let idempotency_key = None;
        let invocation = GraphqlInvocationContext {
            run_id: lease.run_id(),
            tool_call_id: id,
            scope: context.scope.clone(),
            correlation_id: context.correlation_id.clone(),
            causation_id: context.causation_id.clone(),
            delegation_reference: context.delegation_reference.clone(),
            idempotency_key: None,
        };
        let (descriptor, request, preauthorization, binding_fingerprint) = if let Some(descriptor) =
            self.application_tools
                .runtime
                .tool_catalog()
                .descriptor(provider_call.tool_id())
                .cloned()
        {
            validate_supervised_descriptor(&descriptor, provider_call.tool_fingerprint())?;
            let request = build_tool_request(
                lease,
                id,
                &descriptor,
                provider_call.arguments().clone(),
                &context,
                None,
            )?;
            let preauthorization = self
                .application_tools
                .runtime
                .preauthorize_tool(lease.principal_reference(), &descriptor.id, &request)
                .await?;
            let fingerprint = descriptor.fingerprint.clone();
            (descriptor, request, preauthorization, fingerprint)
        } else {
            let prepared = self.application_tools.runtime.prepare_mutation_capability(
                provider_call.tool_id(),
                provider_call.tool_fingerprint(),
                provider_call.arguments().clone(),
                invocation,
            )?;
            if prepared.execution_policy()
                != graphql_orm::graphql::orm::AiMutationExecutionPolicy::ApprovalRequired
            {
                return Err(AiError::Forbidden);
            }
            let preauthorization = self
                .application_tools
                .runtime
                .preauthorize_prepared_mutation(lease.principal_reference(), &prepared)
                .await?;
            let descriptor = prepared.descriptor().clone();
            let request = prepared.request().clone();
            (
                descriptor,
                request,
                preauthorization,
                provider_call.tool_fingerprint().to_owned(),
            )
        };
        let preview = self
            .preview_builder
            .build_preview(preauthorization.principal(), &descriptor, &request)
            .await?;
        let binding = build_approval_binding(
            lease,
            id,
            &descriptor,
            &binding_fingerprint,
            &argument_hash,
            &context.scope,
            context.delegation_reference.clone(),
            &preauthorization,
            &preview,
        )?;
        let protected_arguments = self
            .application_tools
            .protect(
                &policy,
                protection_context(
                    "graphql_orm_ai_tool_calls",
                    id.0,
                    "protected_arguments",
                    &context.scope,
                ),
                provider_call.arguments().clone(),
            )
            .await?;
        let active_lease = self
            .application_tools
            .run_service
            .begin_tool_call(
                lease,
                PreparedToolCallStart {
                    id: id.0,
                    provider_call_key,
                    provider_call_id: provider_call.call_id().to_owned(),
                    provider_kind: provider_result.provider_kind().as_str().to_owned(),
                    provider_model: provider_result.provider_model().to_owned(),
                    provider_response_id: provider_result.provider_response_id().map(str::to_owned),
                    budget_reservation_id: provider_result.budget_reservation_id().0,
                    provider_turn_index: i64::from(context.provider_turn_index),
                    tool_call_index: i64::try_from(context.tool_call_index).map_err(|_| {
                        AiError::InvalidInput("tool call index is invalid".to_owned())
                    })?,
                    tool_id: descriptor.id.as_str().to_owned(),
                    tool_fingerprint: binding_fingerprint,
                    protected_arguments,
                    argument_hash,
                    risk: risk_value(descriptor.risk).to_owned(),
                    idempotency_key,
                    correlation_id: context.correlation_id,
                    causation_id: context.causation_id,
                    delegation_reference: context.delegation_reference,
                    started_event: None,
                    expected_owner_principal_kind: session.owner_principal_kind,
                    expected_owner_subject: session.owner_subject,
                    expected_scope_kind: context.scope.kind,
                    expected_scope_id: context.scope.id,
                    expected_tenant_id: context.scope.tenant_id,
                },
            )
            .await?;
        let approval_id = AiApprovalId::new();
        let park_request = if let (Some(_), Some(_)) = (
            &self.provider_session_service,
            provider_result.provider_session_claim(),
        ) {
            let request = async {
                let checkpoint_id = active_lease
                    .latest_checkpoint_id()
                    .ok_or(AiError::Conflict)?;
                let checkpoint = AiRunCheckpointRecord::find_by_id(
                    self.application_tools.run_service.database(),
                    &checkpoint_id,
                )
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
                .ok_or(AiError::Conflict)?;
                provider_result.provider_session_wait_park_request(
                    &active_lease,
                    crate::AiProviderSessionWaitIdentity::approval(approval_id),
                    checkpoint.id,
                    checkpoint.checkpoint_hash,
                )
            }
            .await;
            match request {
                Ok(request) => Some(request),
                Err(error) => {
                    return self
                        .converge_approval_staging_failure(
                            &active_lease,
                            id,
                            provider_result,
                            None,
                            None,
                            "provider_session_approval_request_invalid",
                            error,
                        )
                        .await;
                }
            }
        } else {
            None
        };
        let parked = if let (Some(service), Some(request)) =
            (&self.provider_session_service, park_request.as_ref())
        {
            match service.park_for_wait(&active_lease, request.clone()).await {
                Ok(parked) => Some(parked),
                Err(error) => {
                    return self
                        .converge_approval_staging_failure(
                            &active_lease,
                            id,
                            provider_result,
                            Some(request),
                            None,
                            "provider_session_approval_park_ambiguous",
                            error,
                        )
                        .await;
                }
            }
        } else {
            None
        };
        let cancelled = match self
            .application_tools
            .run_service
            .cancellation(&active_lease)
            .await
        {
            Ok(cancelled) => cancelled.is_some(),
            Err(error) => {
                return self
                    .converge_approval_staging_failure(
                        &active_lease,
                        id,
                        provider_result,
                        park_request.as_ref(),
                        parked.as_ref(),
                        "provider_session_approval_cancellation_unknown",
                        error,
                    )
                    .await;
            }
        };
        if cancelled {
            // Owner cancellation has already atomically terminalized the run,
            // active tool call, session stream and inbox stream. No resolver
            // side effect has occurred; only the retained provider cursor
            // needs exact quarantine before returning the stale-fence result.
            self.cleanup_approval_handoff(
                provider_result,
                park_request.as_ref(),
                parked.as_ref(),
                "provider_session_approval_wait_cancelled",
            )
            .await;
            return Err(AiError::Conflict);
        }
        let requested = self
            .approval_service
            .request_approval_with_id(
                &active_lease,
                approval_id,
                binding,
                preview,
                expires_at,
                recent_mfa_required,
            )
            .await;
        let requested = match requested {
            Ok(requested) => requested,
            Err(error) => {
                return self
                    .converge_approval_staging_failure(
                        &active_lease,
                        id,
                        provider_result,
                        park_request.as_ref(),
                        parked.as_ref(),
                        "provider_session_approval_staging_failed",
                        error,
                    )
                    .await;
            }
        };
        if let (Some(service), Some(parked)) = (&self.provider_session_service, parked.as_ref()) {
            // This confirmation is repairable from the exact durable approval
            // graph if the process stops after the waiting transaction wins.
            // The unconfirmed parked row and exact WaitingApproval graph are
            // durable. Restore reconciliation can confirm this exact pair
            // without replaying the provider call if this best-effort call is
            // ambiguous.
            let _ = service.confirm_parked_wait(parked).await;
        }
        Ok(AiRequestedConsequentialToolCall {
            tool_call_id: id,
            approval_id: requested.approval_id(),
            lease: requested.into_lease(),
        })
    }

    async fn cleanup_approval_handoff(
        &self,
        provider_result: &AiProviderCallResult,
        request: Option<&crate::AiProviderSessionWaitParkRequest>,
        parked: Option<&crate::AiProviderSessionParkedWait>,
        reason_code: &str,
    ) {
        let Some(service) = &self.provider_session_service else {
            return;
        };
        if let Some(parked) = parked {
            let _ = service
                .require_parked_wait_cleanup(parked, reason_code)
                .await;
        } else if let Some(request) = request {
            let _ = service
                .require_wait_handoff_cleanup(request, reason_code)
                .await;
        } else if let Some(claim) = provider_result.provider_session_claim() {
            let _ = service.require_cleanup(claim, reason_code).await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn converge_approval_staging_failure(
        &self,
        active_lease: &AiRunLease,
        tool_call_id: AiToolCallId,
        provider_result: &AiProviderCallResult,
        request: Option<&crate::AiProviderSessionWaitParkRequest>,
        parked: Option<&crate::AiProviderSessionParkedWait>,
        reason_code: &str,
        original: AiError,
    ) -> Result<AiRequestedConsequentialToolCall, AiError> {
        self.cleanup_approval_handoff(provider_result, request, parked, reason_code)
            .await;
        match self
            .mark_consequential_recovery(
                active_lease,
                tool_call_id,
                provider_result.provider_response_id().map(str::to_owned),
            )
            .await
        {
            Ok(_) => Err(AiError::Conflict),
            Err(_) => Err(original),
        }
    }

    /// Rebuilds and consumes one exact approval, then executes the ordinary
    /// registered resolver under freshly recomputed authorization.
    ///
    /// An execution/timeout/post-side-effect ambiguity terminally closes the
    /// run as `RecoveryRequired`; the consumed approval and mutation are never
    /// automatically replayed. A successful resolver result is bounded,
    /// statically classified, protected, separately egress-authorized and
    /// atomically fenced exactly like a read-only tool result.
    ///
    /// # Errors
    ///
    /// Fails closed for stale lease/approval/tool/resource/policy bindings,
    /// current access denial before consumption, malformed protected state,
    /// unavailable protection, or persistence failure. Resolver ambiguity is
    /// returned as a durable [`AiConsequentialToolCallOutcome::RecoveryRequired`]
    /// when the current fence can commit that terminal fact.
    pub async fn execute_approved(
        &self,
        lease: &AiRunLease,
        approval_id: AiApprovalId,
        tool_call_id: AiToolCallId,
        route: AiToolResultEgressRoute,
    ) -> Result<AiConsequentialToolCallOutcome, AiError> {
        route.validate()?;
        let call = AiToolCallRecord::find_by_id(
            self.application_tools.run_service.database(),
            &tool_call_id.0,
        )
        .await
        .map_err(|error| map_orm(OrmPublicError::from(error)))?
        .ok_or(AiError::NotFound)?;
        if call.run_id != lease.run_id().0
            || call.lease_generation != lease.lease_generation()
            || call.state != "waiting_approval"
            || call.approval_id != Some(approval_id.0)
            || call.completed_at.is_some()
            || call.protected_result.is_some()
        {
            return Err(AiError::Conflict);
        }
        let session = AiSessionRecord::find_by_id(
            self.application_tools.run_service.database(),
            &lease.session_id().0,
        )
        .await
        .map_err(|error| map_orm(OrmPublicError::from(error)))?
        .ok_or(AiError::NotFound)?;
        let scope = AiScope {
            kind: session.scope_kind.clone(),
            id: session.scope_id.clone(),
            tenant_id: session.tenant_id.clone(),
        };
        validate_session_binding(&session, lease, &scope)?;
        let tool_id = AiToolId::parse(call.tool_id.clone())?;
        let provider_kind = call
            .provider_kind
            .clone()
            .ok_or(AiError::PersistenceFailed)?;
        let provider_model = call
            .provider_model
            .clone()
            .ok_or(AiError::PersistenceFailed)?;
        let budget_reservation_id = call
            .budget_reservation_id
            .ok_or(AiError::PersistenceFailed)?;
        let correlation_id = call
            .correlation_id
            .clone()
            .ok_or(AiError::PersistenceFailed)?;
        let causation_id = call
            .causation_id
            .clone()
            .ok_or(AiError::PersistenceFailed)?;
        let reservation = AiBudgetReservationRecord::find_by_id(
            self.application_tools.run_service.database(),
            &budget_reservation_id,
        )
        .await
        .map_err(|error| map_orm(OrmPublicError::from(error)))?
        .ok_or(AiError::PersistenceFailed)?;
        if reservation.session_id != lease.session_id().0
            || reservation.run_id != lease.run_id().0
            || reservation.attempt_id != lease.attempt_id()
            || reservation.lease_generation != lease.lease_generation()
            || reservation.provider_kind != provider_kind
            || reservation.provider_model != provider_model
            || reservation.state != "committed"
            || reservation.actual_runs != Some(1)
            || reservation.reconciled_at.is_none()
        {
            return Err(AiError::PersistenceFailed);
        }
        let current = self.application_tools.current_access(lease, &scope).await?;
        let policy = self
            .application_tools
            .runtime
            .content_protection_policy_resolver()
            .resolve(current.principal(), &scope)
            .await?;
        if !policy.ready || policy.scope != scope {
            return Err(AiError::RuntimeNotReady);
        }
        let arguments = self
            .application_tools
            .open(
                &policy,
                protection_context(
                    "graphql_orm_ai_tool_calls",
                    tool_call_id.0,
                    "protected_arguments",
                    &scope,
                ),
                call.protected_arguments
                    .as_ref()
                    .ok_or(AiError::PersistenceFailed)?,
            )
            .await?;
        if canonical_json_hash(&arguments)? != call.argument_hash {
            return Err(AiError::PersistenceFailed);
        }
        let context = AiApplicationToolCallContext {
            provider_turn_index: u32::try_from(call.provider_turn_index)
                .map_err(|_| AiError::PersistenceFailed)?,
            tool_call_index: usize::try_from(call.tool_call_index)
                .map_err(|_| AiError::PersistenceFailed)?,
            scope: scope.clone(),
            correlation_id: correlation_id.clone(),
            causation_id: causation_id.clone(),
            delegation_reference: call.delegation_reference.clone(),
        };
        let invocation = GraphqlInvocationContext {
            run_id: lease.run_id(),
            tool_call_id,
            scope: scope.clone(),
            correlation_id: correlation_id.clone(),
            causation_id: causation_id.clone(),
            delegation_reference: call.delegation_reference.clone(),
            idempotency_key: call.idempotency_key.clone(),
        };
        let (descriptor, request, preauthorization, binding_fingerprint, prepared) =
            if let Some(descriptor) = self
                .application_tools
                .runtime
                .tool_catalog()
                .descriptor(&tool_id)
                .cloned()
            {
                validate_supervised_descriptor(&descriptor, &call.tool_fingerprint)?;
                let request = build_tool_request(
                    lease,
                    tool_call_id,
                    &descriptor,
                    arguments,
                    &context,
                    call.idempotency_key.clone(),
                )?;
                let preauthorization = self
                    .application_tools
                    .runtime
                    .preauthorize_tool(lease.principal_reference(), &descriptor.id, &request)
                    .await?;
                let fingerprint = descriptor.fingerprint.clone();
                (descriptor, request, preauthorization, fingerprint, None)
            } else {
                let prepared = self.application_tools.runtime.prepare_mutation_capability(
                    &tool_id,
                    &call.tool_fingerprint,
                    arguments,
                    invocation,
                )?;
                if prepared.execution_policy()
                    != graphql_orm::graphql::orm::AiMutationExecutionPolicy::ApprovalRequired
                {
                    return Err(AiError::Forbidden);
                }
                let preauthorization = self
                    .application_tools
                    .runtime
                    .preauthorize_prepared_mutation(lease.principal_reference(), &prepared)
                    .await?;
                let descriptor = prepared.descriptor().clone();
                let request = prepared.request().clone();
                (
                    descriptor,
                    request,
                    preauthorization,
                    call.tool_fingerprint.clone(),
                    Some(prepared),
                )
            };
        let disclosure_schema_fingerprint = descriptor
            .graphql_contract
            .as_ref()
            .ok_or(AiError::Forbidden)?
            .disclosure_schema_fingerprint
            .clone();
        let preview = self
            .preview_builder
            .build_preview(preauthorization.principal(), &descriptor, &request)
            .await?;
        let binding = build_approval_binding(
            lease,
            tool_call_id,
            &descriptor,
            &binding_fingerprint,
            &call.argument_hash,
            &scope,
            call.delegation_reference.clone(),
            &preauthorization,
            &preview,
        )?;
        let consumed = self
            .approval_service
            .consume_approval(lease, approval_id, &binding, &preview)
            .await?;
        let (approval, running_lease) = consumed.into_parts();
        let execution = if let Some(prepared) = prepared {
            tokio::time::timeout(
                self.application_tools
                    .limits
                    .maximum_execution_time
                    .unsigned_abs(),
                self.application_tools
                    .runtime
                    .execute_approved_prepared_mutation(
                        running_lease.principal_reference(),
                        prepared,
                        &approval,
                        &binding,
                    ),
            )
            .await
        } else {
            tokio::time::timeout(
                self.application_tools
                    .limits
                    .maximum_execution_time
                    .unsigned_abs(),
                self.application_tools.runtime.execute_approved_tool(
                    running_lease.principal_reference(),
                    &descriptor.id,
                    request,
                    &approval,
                    &binding,
                ),
            )
            .await
        };
        let result = match execution {
            Ok(Ok(result)) => result,
            Ok(Err(_)) | Err(_) => {
                return self
                    .mark_consequential_recovery(
                        &running_lease,
                        tool_call_id,
                        call.provider_response_id,
                    )
                    .await;
            }
        };
        let provider_response_id = call.provider_response_id.clone();
        if self
            .application_tools
            .current_access(&running_lease, &scope)
            .await
            .is_err()
        {
            return self
                .mark_consequential_recovery(
                    &running_lease,
                    tool_call_id,
                    provider_response_id.clone(),
                )
                .await;
        }
        let model_output = result.model_output();
        let output_bytes = match serde_json::to_vec(&model_output) {
            Ok(output) => output,
            Err(_) => {
                return self
                    .mark_consequential_recovery(&running_lease, tool_call_id, provider_response_id)
                    .await;
            }
        };
        if output_bytes.len() > self.application_tools.limits.maximum_model_output_bytes {
            return self
                .mark_consequential_recovery(
                    &running_lease,
                    tool_call_id,
                    provider_response_id.clone(),
                )
                .await;
        }
        let Some(outbound_bytes) = output_bytes
            .len()
            .checked_add(call.provider_call_id.len())
            .and_then(|bytes| bytes.checked_add(descriptor.id.as_str().len()))
        else {
            return self
                .mark_consequential_recovery(&running_lease, tool_call_id, provider_response_id)
                .await;
        };
        let estimated_bytes = match u64::try_from(outbound_bytes) {
            Ok(bytes) => bytes,
            Err(_) => {
                return self
                    .mark_consequential_recovery(&running_lease, tool_call_id, provider_response_id)
                    .await;
            }
        };
        let classification = result.disclosure().maximum_classification;
        let manifest = AiEgressManifest {
            provider_profile_id: route.provider_profile_id,
            provider_kind,
            model: provider_model,
            destination: route.destination,
            destination_trust: route.destination_trust,
            capability: AiEgressCapability::ToolResult,
            scope: scope.clone(),
            session_id: Some(running_lease.session_id()),
            run_id: Some(running_lease.run_id()),
            sources: vec![AiDataSourceRef {
                kind: "application_tool_result".to_owned(),
                reference: tool_call_id.0.to_string(),
                classification,
                trust: AiSourceTrust::ResolverResult,
            }],
            estimated_bytes,
            estimated_tokens: 0,
            attachment_count: 0,
            purpose: route.purpose,
            retention: route.retention,
            residency: route.residency,
            policy_version: route.policy_version,
            consent_reference: route.consent_reference,
        };
        let decision = match self
            .application_tools
            .runtime
            .authorize_egress(running_lease.principal_reference(), &manifest)
            .await
        {
            Ok(decision) => decision,
            Err(_) => {
                return self
                    .mark_consequential_recovery(&running_lease, tool_call_id, provider_response_id)
                    .await;
            }
        };
        let audit_result = self
            .application_tools
            .egress_audit
            .record(&manifest, &decision)
            .await;
        let (state, model_input, decision_id, manifest_hash, authorization_code) =
            if audit_result.is_err() {
                (
                    AiApplicationToolCallState::EgressAuditFailed,
                    None,
                    None,
                    None,
                    "egress_audit_failed".to_owned(),
                )
            } else if decision.authorize(&manifest).is_err() {
                (
                    AiApplicationToolCallState::EgressDenied,
                    None,
                    Some(decision.id.0),
                    Some(decision.manifest_hash.clone()),
                    "egress_denied".to_owned(),
                )
            } else {
                (
                    AiApplicationToolCallState::Completed,
                    Some(ModelInputBlock::ToolResult {
                        call_id: call.provider_call_id.clone(),
                        tool_id: descriptor.id.as_str().to_owned(),
                        output: model_output.clone(),
                    }),
                    Some(decision.id.0),
                    Some(decision.manifest_hash.clone()),
                    "allowed".to_owned(),
                )
            };
        let protected_result = match self
            .application_tools
            .protect(
                &policy,
                protection_context(
                    "graphql_orm_ai_tool_calls",
                    tool_call_id.0,
                    "protected_result",
                    &scope,
                ),
                model_output,
            )
            .await
        {
            Ok(result) => result,
            Err(_) => {
                return self
                    .mark_consequential_recovery(
                        &running_lease,
                        tool_call_id,
                        provider_response_id.clone(),
                    )
                    .await;
            }
        };
        let event_id = Uuid::new_v4();
        let inbox_event_id = Uuid::new_v4();
        let protected_event = match self
            .application_tools
            .protect(
                &policy,
                protection_context(
                    "graphql_orm_ai_session_events",
                    event_id,
                    "protected_payload",
                    &scope,
                ),
                json!({
                    "toolCallId": tool_call_id.0,
                    "runId": running_lease.run_id().0,
                    "toolId": descriptor.id.as_str(),
                    "state": state.as_str(),
                    "approvalId": approval_id.0,
                }),
            )
            .await
        {
            Ok(event) => event,
            Err(_) => {
                return self
                    .mark_consequential_recovery(
                        &running_lease,
                        tool_call_id,
                        provider_response_id.clone(),
                    )
                    .await;
            }
        };
        let protected_inbox_event = match self
            .application_tools
            .protect(
                &policy,
                protection_context(
                    "graphql_orm_ai_inbox_events",
                    inbox_event_id,
                    "protected_payload",
                    &scope,
                ),
                json!({
                    "toolCallId": tool_call_id.0,
                    "runId": running_lease.run_id().0,
                    "toolId": descriptor.id.as_str(),
                    "state": state.as_str(),
                    "approvalId": approval_id.0,
                }),
            )
            .await
        {
            Ok(event) => event,
            Err(_) => {
                return self
                    .mark_consequential_recovery(
                        &running_lease,
                        tool_call_id,
                        provider_response_id.clone(),
                    )
                    .await;
            }
        };
        let renewed = match self
            .application_tools
            .run_service
            .finish_tool_call(
                &running_lease,
                PreparedToolCallFinish {
                    id: tool_call_id.0,
                    state: state.as_str().to_owned(),
                    protected_result,
                    authorization_code,
                    authorization_policy_version: Some(result.policy_version().to_owned()),
                    authorization_state_digest: Some(
                        result.authorization_state_digest().to_owned(),
                    ),
                    disclosure_schema_fingerprint,
                    result_classification: classification_value(classification).to_owned(),
                    result_egress_decision_id: decision_id,
                    result_egress_manifest_hash: manifest_hash,
                    application_audit_ref: result.response().application_audit_ref.clone(),
                    event_id,
                    inbox_event_id,
                    protected_event,
                    protected_inbox_event,
                    correlation_id,
                    expected_provider_call_key: call.provider_call_key,
                    expected_tool_fingerprint: call.tool_fingerprint,
                    expected_owner_principal_kind: session.owner_principal_kind,
                    expected_owner_subject: session.owner_subject,
                    expected_scope_kind: scope.kind,
                    expected_scope_id: scope.id,
                    expected_tenant_id: scope.tenant_id,
                },
            )
            .await
        {
            Ok(lease) => lease,
            Err(error) => {
                return match self
                    .mark_consequential_recovery(&running_lease, tool_call_id, provider_response_id)
                    .await
                {
                    Ok(outcome) => Ok(outcome),
                    Err(_) => Err(error),
                };
            }
        };
        Ok(AiConsequentialToolCallOutcome::Persisted(Box::new(
            AiPersistedApplicationToolCall {
                id: tool_call_id,
                provider_call_id: call.provider_call_id,
                state,
                model_input,
                egress_manifest: decision_id.map(|_| manifest),
                lease: renewed,
            },
        )))
    }

    async fn mark_consequential_recovery(
        &self,
        lease: &AiRunLease,
        tool_call_id: AiToolCallId,
        provider_response_id: Option<String>,
    ) -> Result<AiConsequentialToolCallOutcome, AiError> {
        self.application_tools
            .run_service
            .finish(
                lease,
                AiRunCompletion::new(
                    AiRunState::RecoveryRequired,
                    "consequential_tool_uncertain",
                    Some("consequential_tool_uncertain".to_owned()),
                    provider_response_id,
                )?,
            )
            .await?;
        Ok(AiConsequentialToolCallOutcome::RecoveryRequired { tool_call_id })
    }
}

fn validate_provider_binding(
    service: &OrmAiApplicationToolCallService,
    lease: &AiRunLease,
    result: &AiProviderCallResult,
    context: &AiApplicationToolCallContext,
) -> Result<(), AiError> {
    if !service.runtime.start_gate().is_ready()
        || result.session_id() != lease.session_id()
        || result.run_id() != lease.run_id()
        || result.attempt_id() != lease.attempt_id()
        || result.lease_generation() != lease.lease_generation()
        || (result.provider_response_id().is_none() && !result.uses_stateless_continuation())
        || context.provider_turn_index >= service.limits.maximum_provider_turns
        || context.tool_call_index >= service.limits.maximum_calls_per_turn
        || context.tool_call_index >= result.tool_calls().len()
    {
        return Err(AiError::Conflict);
    }
    Ok(())
}

fn validate_supervised_descriptor(
    descriptor: &AiToolDescriptor,
    expected_fingerprint: &str,
) -> Result<(), AiError> {
    if descriptor.fingerprint != expected_fingerprint
        || descriptor.operation_kind != AiToolOperationKind::Mutation
        || descriptor.operation_domain != AiToolOperationDomain::Application
        || descriptor.maturity != ToolMaturity::SupervisedWrite
        || descriptor.approval != AiApprovalRule::OneShot
        || !matches!(
            descriptor.risk,
            AiToolRisk::LowRiskWrite | AiToolRisk::NonIdempotentWrite | AiToolRisk::HighImpact
        )
    {
        return Err(AiError::Forbidden);
    }
    Ok(())
}

fn build_tool_request(
    lease: &AiRunLease,
    tool_call_id: AiToolCallId,
    descriptor: &AiToolDescriptor,
    variables: serde_json::Value,
    context: &AiApplicationToolCallContext,
    idempotency_key: Option<String>,
) -> Result<ToolGraphqlRequest, AiError> {
    let contract = descriptor
        .graphql_contract
        .clone()
        .ok_or(AiError::Forbidden)?;
    Ok(ToolGraphqlRequest {
        document: descriptor.document.clone(),
        operation_name: contract.operation_name.clone(),
        contract,
        variables,
        invocation: GraphqlInvocationContext {
            run_id: lease.run_id(),
            tool_call_id,
            scope: context.scope.clone(),
            correlation_id: context.correlation_id.clone(),
            causation_id: context.causation_id.clone(),
            delegation_reference: context.delegation_reference.clone(),
            idempotency_key,
        },
    })
}

#[allow(clippy::too_many_arguments)]
fn build_approval_binding(
    lease: &AiRunLease,
    tool_call_id: AiToolCallId,
    descriptor: &AiToolDescriptor,
    binding_fingerprint: &str,
    argument_hash: &str,
    scope: &AiScope,
    delegation_reference: Option<String>,
    preauthorization: &crate::AiToolPreauthorization,
    preview: &AiCanonicalActionPreview,
) -> Result<AiApprovalBinding, AiError> {
    if preauthorization.tool_fingerprint() != descriptor.fingerprint
        || !valid_sha256_fingerprint(binding_fingerprint)
    {
        return Err(AiError::Conflict);
    }
    let binding = AiApprovalBinding {
        tool_call_id,
        session_id: lease.session_id(),
        scope: scope.clone(),
        tool_fingerprint: binding_fingerprint.to_owned(),
        argument_hash: argument_hash.to_owned(),
        operation: descriptor
            .graphql_contract
            .clone()
            .ok_or(AiError::Forbidden)?,
        principal_reference_fingerprint: AiApprovalBinding::principal_fingerprint(
            lease.principal_reference(),
        ),
        delegated_actor_subject: lease.principal_reference().actor_subject.clone(),
        delegation_reference,
        policy_version: preauthorization.policy_version().to_owned(),
        authorization_state_digest: preauthorization.authorization_state_digest().to_owned(),
        resources: preview.targets.clone(),
        preview_hash: preview.stable_hash(),
    };
    binding.validate(preview)?;
    Ok(binding)
}

fn valid_sha256_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn validate_session_binding(
    session: &AiSessionRecord,
    lease: &AiRunLease,
    scope: &AiScope,
) -> Result<(), AiError> {
    let expected_kind = match &lease.principal_reference().kind {
        PrincipalReferenceKind::UserSession => "user".to_owned(),
        PrincipalReferenceKind::ApiToken { principal_kind } => {
            format!("api_token:{principal_kind}")
        }
    };
    if session.id != lease.session_id().0
        || session.state != "active"
        || session.deleted_at.is_some()
        || session.owner_principal_kind != expected_kind
        || session.owner_subject != lease.principal_reference().subject
        || session.scope_kind != scope.kind
        || session.scope_id != scope.id
        || session.tenant_id != scope.tenant_id
        || lease
            .principal_reference()
            .tenant_id
            .as_ref()
            .is_some_and(|tenant_id| scope.tenant_id.as_ref() != Some(tenant_id))
    {
        return Err(AiError::Forbidden);
    }
    Ok(())
}

fn protection_context(
    entity: &str,
    row_id: Uuid,
    field: &str,
    scope: &AiScope,
) -> ContentProtectionContext {
    ContentProtectionContext {
        entity: entity.to_owned(),
        row_id: row_id.to_string(),
        field: field.to_owned(),
        scope: scope.clone(),
    }
}

pub(crate) fn provider_call_key(lease: &AiRunLease, provider_call_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(lease.run_id().0.as_bytes());
    hasher.update([0]);
    hasher.update(provider_call_id.as_bytes());
    hex::encode(hasher.finalize())
}

pub(crate) fn canonical_json_hash(value: &serde_json::Value) -> Result<String, AiError> {
    let canonical = canonical_json(value);
    let encoded = serde_json::to_vec(&canonical)
        .map_err(|_| AiError::InvalidInput("tool arguments are not canonical JSON".to_owned()))?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(values) => {
            let sorted = values
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect::<BTreeMap<_, _>>();
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        value => value.clone(),
    }
}

fn valid_audit_reference(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 1_024
        && value.bytes().all(|byte| !byte.is_ascii_control())
}

const fn classification_value(value: DataClassification) -> &'static str {
    match value {
        DataClassification::Public => "public",
        DataClassification::Internal => "internal",
        DataClassification::Confidential => "confidential",
        DataClassification::Restricted => "restricted",
        DataClassification::Secret => "secret",
    }
}

const fn risk_value(value: AiToolRisk) -> &'static str {
    match value {
        AiToolRisk::ReadOnly => "read_only",
        AiToolRisk::Proposal => "proposal",
        AiToolRisk::LowRiskWrite => "low_risk_write",
        AiToolRisk::NonIdempotentWrite => "non_idempotent_write",
        AiToolRisk::HighImpact => "high_impact",
        AiToolRisk::Secret => "secret",
    }
}

fn map_orm(error: OrmPublicError) -> AiError {
    match error.code {
        OrmErrorCode::InvalidInput
        | OrmErrorCode::CursorInvalid
        | OrmErrorCode::PageLimitExceeded => AiError::InvalidInput(error.message),
        OrmErrorCode::Unauthenticated | OrmErrorCode::Forbidden => AiError::Forbidden,
        OrmErrorCode::NotFound => AiError::NotFound,
        OrmErrorCode::Conflict | OrmErrorCode::ConstraintViolation => AiError::Conflict,
        OrmErrorCode::ServiceUnavailable
        | OrmErrorCode::InternalError
        | OrmErrorCode::AuthorizationMisconfigured => AiError::PersistenceFailed,
    }
}
