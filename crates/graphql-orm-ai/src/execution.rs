//! Authenticated application GraphQL execution contracts.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;

use agql_auth::{CurrentPrincipalResolver, PrincipalReference, ResolvedPrincipal};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    AiApprovalRule, AiRunId, AiScope, AiToolAuthorizationDecision, AiToolAuthorizationPolicy,
    AiToolCallId, AiToolDescriptor, AiToolId, AiToolOperationKind, GraphqlExecutionTargetId,
    GraphqlOperationContract, ToolExecutionError,
};

/// Deployment trust/routing class for authenticated GraphQL execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphqlExecutionTargetClass {
    /// Finished schema executing in the current process.
    Local,
    /// Private routed/composed GraphQL endpoint.
    PrivateRouted,
    /// Private direct service endpoint, disabled unless explicitly registered.
    PrivateDirect,
}

/// Non-secret deployment registration for one logical GraphQL destination.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphqlExecutionTarget {
    /// Stable logical ID used by server-owned tool descriptors.
    pub id: GraphqlExecutionTargetId,
    /// Local/routed/direct trust class.
    pub class: GraphqlExecutionTargetClass,
    /// Credential audience required for a remote target.
    pub audience: Option<String>,
    /// Resource type required for a remote target.
    pub resource_type: Option<String>,
    /// Resource identifier required for a remote target.
    pub resource_id: Option<String>,
    /// Exact compiled or registry schema fingerprint.
    pub schema_fingerprint: String,
}

impl GraphqlExecutionTarget {
    /// Validates a logical target without accepting or exposing a URL.
    ///
    /// # Errors
    ///
    /// Returns [`ToolExecutionError::InvalidTarget`] when a schema
    /// fingerprint is absent or a remote target lacks audience/resource
    /// binding.
    pub fn validate(&self) -> Result<(), ToolExecutionError> {
        if self.schema_fingerprint.trim().is_empty() {
            return Err(ToolExecutionError::InvalidTarget);
        }
        if self.class != GraphqlExecutionTargetClass::Local
            && (self.audience.as_deref().is_none_or(str::is_empty)
                || self.resource_type.as_deref().is_none_or(str::is_empty)
                || self.resource_id.as_deref().is_none_or(str::is_empty))
        {
            return Err(ToolExecutionError::InvalidTarget);
        }
        Ok(())
    }
}

/// Immutable deployment registry for logical GraphQL execution targets.
#[derive(Clone, Debug, Default)]
pub struct GraphqlExecutionTargetRegistry {
    targets: BTreeMap<GraphqlExecutionTargetId, GraphqlExecutionTarget>,
}

impl GraphqlExecutionTargetRegistry {
    /// Creates an empty registry. No target is implicitly trusted.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one validated logical target.
    ///
    /// # Errors
    ///
    /// Returns a safe error for invalid or duplicate target IDs.
    pub fn register(&mut self, target: GraphqlExecutionTarget) -> Result<(), ToolExecutionError> {
        target.validate()?;
        if self.targets.contains_key(&target.id) {
            return Err(ToolExecutionError::InvalidTarget);
        }
        self.targets.insert(target.id.clone(), target);
        Ok(())
    }

    /// Resolves a logical target without making its transport destination model-visible.
    pub fn target(&self, id: &GraphqlExecutionTargetId) -> Option<&GraphqlExecutionTarget> {
        self.targets.get(id)
    }

    fn validate_contract(
        &self,
        contract: &GraphqlOperationContract,
        document: &str,
    ) -> Result<&GraphqlExecutionTarget, ToolExecutionError> {
        let target = self
            .targets
            .get(&contract.target_id)
            .ok_or(ToolExecutionError::InvalidTarget)?;
        if target.schema_fingerprint != contract.schema_fingerprint
            || contract.document_hash != crate::stable_graphql_document_hash(document)
            || contract.operation_name.trim().is_empty()
            || contract.result_projection_fingerprint.trim().is_empty()
            || contract.disclosure_schema_fingerprint.trim().is_empty()
        {
            return Err(ToolExecutionError::StaleContract);
        }
        Ok(target)
    }
}

/// Invocation metadata linked into the host's normal audit context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphqlInvocationContext {
    /// Run causing the operation.
    pub run_id: AiRunId,
    /// Tool call causing the operation.
    pub tool_call_id: AiToolCallId,
    /// Application scope in which tool policy and resolver authorization run.
    pub scope: AiScope,
    /// Correlation identifier shared with the outer AI audit.
    pub correlation_id: String,
    /// Causal command/event identifier propagated into application audit.
    pub causation_id: String,
    /// Safe delegation/grant reference; never a bearer credential.
    pub delegation_reference: Option<String>,
    /// Optional idempotency key for a descriptor proven idempotent.
    pub idempotency_key: Option<String>,
}

/// Opaque host request context produced through the same factory used by
/// ordinary GraphQL transports.
#[derive(Clone)]
pub struct GraphqlRequestContext {
    inner: Arc<dyn Any + Send + Sync>,
}

/// Crate-authored non-secret provenance of a durable application tool invocation.
///
/// The tool lifecycle derives this value from its trusted run, provider result,
/// and registered request. It contains no prompt, command source, credential or
/// role snapshot and grants no authority. Hosts can inspect it when issuing
/// exact delegation without reading private runtime tables. Deserialized values
/// alone are not proof; only the registered execution binding can attach them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiToolExecutionProvenance {
    session_id: crate::AiSessionId,
    run_id: AiRunId,
    tool_call_id: AiToolCallId,
    attempt_id: uuid::Uuid,
    lease_generation: i64,
    provider_kind: crate::ProviderKind,
    provider_model: String,
    provider_profile_id: String,
    provider_call_id: String,
    provider_response_id: Option<String>,
    budget_reservation_id: crate::AiBudgetReservationId,
    execution_selection: Option<crate::AiSessionExecutionSelection>,
    provider_registration_fingerprint: Option<String>,
    tool_fingerprint: String,
    argument_hash: String,
    approval_id: Option<crate::AiApprovalId>,
    approval_binding_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    policy_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    authorization_state_digest: Option<String>,
}

impl AiToolExecutionProvenance {
    /// AI session owning the invocation.
    pub fn session_id(&self) -> crate::AiSessionId {
        self.session_id
    }

    /// Run owning the invocation.
    pub fn run_id(&self) -> AiRunId {
        self.run_id
    }

    /// Exact durable application tool call.
    pub fn tool_call_id(&self) -> AiToolCallId {
        self.tool_call_id
    }

    /// Provider execution attempt that produced the call.
    pub fn attempt_id(&self) -> uuid::Uuid {
        self.attempt_id
    }

    /// Provider execution fencing generation.
    pub fn lease_generation(&self) -> i64 {
        self.lease_generation
    }

    /// Actual provider family.
    pub fn provider_kind(&self) -> &crate::ProviderKind {
        &self.provider_kind
    }

    /// Actual provider model.
    pub fn provider_model(&self) -> &str {
        &self.provider_model
    }

    /// Exact provider profile from the authorized inference manifest.
    pub fn provider_profile_id(&self) -> &str {
        &self.provider_profile_id
    }

    /// Opaque provider-native call identifier.
    pub fn provider_call_id(&self) -> &str {
        &self.provider_call_id
    }

    /// Opaque provider response identifier, when available.
    pub fn provider_response_id(&self) -> Option<&str> {
        self.provider_response_id.as_deref()
    }

    /// Provider accounting correlation.
    pub fn budget_reservation_id(&self) -> crate::AiBudgetReservationId {
        self.budget_reservation_id
    }

    /// Immutable admitted session routing selection, when present.
    pub fn execution_selection(&self) -> Option<&crate::AiSessionExecutionSelection> {
        self.execution_selection.as_ref()
    }

    /// Exact retained-provider registration fingerprint, when present.
    pub fn provider_registration_fingerprint(&self) -> Option<&str> {
        self.provider_registration_fingerprint.as_deref()
    }

    /// Exact compiled registered tool fingerprint.
    pub fn tool_fingerprint(&self) -> &str {
        &self.tool_fingerprint
    }

    /// Canonical hash of the exact GraphQL variables.
    pub fn argument_hash(&self) -> &str {
        &self.argument_hash
    }

    /// Consumed one-shot approval, only for approved execution.
    pub fn approval_id(&self) -> Option<crate::AiApprovalId> {
        self.approval_id
    }

    /// Exact consumed approval envelope hash, when present.
    pub fn approval_binding_hash(&self) -> Option<&str> {
        self.approval_binding_hash.as_deref()
    }

    /// Policy version recomputed by the bridge immediately before execution.
    /// Absent in the earlier durable origin record.
    pub fn policy_version(&self) -> Option<&str> {
        self.policy_version.as_deref()
    }

    /// Safe current authorization digest recomputed immediately before execution.
    pub fn authorization_state_digest(&self) -> Option<&str> {
        self.authorization_state_digest.as_deref()
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn from_provider_call(
        lease: &crate::AiRunLease,
        provider: &crate::AiProviderCallResult,
        provider_call_id: &str,
        descriptor: &AiToolDescriptor,
        request: &ToolGraphqlRequest,
        execution_selection: Option<crate::AiSessionExecutionSelection>,
    ) -> Result<Self, ToolExecutionError> {
        if provider.session_id() != lease.session_id()
            || provider.run_id() != lease.run_id()
            || provider.attempt_id() != lease.attempt_id()
            || provider.lease_generation() != lease.lease_generation()
            || request.invocation.run_id != lease.run_id()
            || execution_selection.as_ref().is_some_and(|selection| {
                selection.provider_kind() != *provider.provider_kind()
                    || selection.model() != provider.provider_model()
            })
        {
            return Err(ToolExecutionError::StaleContract);
        }
        let mut calls = provider
            .tool_calls()
            .iter()
            .filter(|call| call.call_id() == provider_call_id);
        let call = calls.next().ok_or(ToolExecutionError::StaleContract)?;
        if calls.next().is_some()
            || call.tool_id() != &descriptor.id
            || call.tool_fingerprint() != descriptor.fingerprint
            || call.arguments() != &request.variables
        {
            return Err(ToolExecutionError::StaleContract);
        }
        Ok(Self {
            session_id: lease.session_id(),
            run_id: lease.run_id(),
            tool_call_id: request.invocation.tool_call_id,
            attempt_id: lease.attempt_id(),
            lease_generation: lease.lease_generation(),
            provider_kind: provider.provider_kind().clone(),
            provider_model: provider.provider_model().to_owned(),
            provider_profile_id: provider
                .model_inference_manifest()
                .provider_profile_id
                .clone(),
            provider_call_id: call.call_id().to_owned(),
            provider_response_id: provider.provider_response_id().map(str::to_owned),
            budget_reservation_id: provider.budget_reservation_id(),
            execution_selection,
            provider_registration_fingerprint: provider
                .provider_session_claim()
                .map(|claim| claim.descriptor().registration_fingerprint().to_owned()),
            tool_fingerprint: descriptor.fingerprint.clone(),
            argument_hash: crate::tools::canonical_json_digest(&request.variables),
            approval_id: None,
            approval_binding_hash: None,
            policy_version: None,
            authorization_state_digest: None,
        })
    }

    pub(crate) fn with_consumed_approval(
        mut self,
        approval: &crate::ConsumedAiApproval,
        binding: &crate::AiApprovalBinding,
    ) -> Result<Self, ToolExecutionError> {
        if approval.binding_hash() != binding.stable_hash()
            || self.session_id != binding.session_id
            || self.tool_call_id != binding.tool_call_id
            || self.tool_fingerprint != binding.tool_fingerprint
            || self.argument_hash != binding.argument_hash
        {
            return Err(ToolExecutionError::StaleContract);
        }
        self.approval_id = Some(approval.approval_id());
        self.approval_binding_hash = Some(approval.binding_hash().to_owned());
        Ok(self)
    }

    fn matches_request(&self, fingerprint: &str, request: &ToolGraphqlRequest) -> bool {
        self.run_id == request.invocation.run_id
            && self.tool_call_id == request.invocation.tool_call_id
            && self.tool_fingerprint == fingerprint
            && self.argument_hash == crate::tools::canonical_json_digest(&request.variables)
            && self.approval_id.is_some() == self.approval_binding_hash.is_some()
    }
}

/// Crate-authored identity of the exact registered tool contract reaching an
/// authenticated GraphQL execution boundary.
///
/// This value is constructed only after the caller has selected the exact
/// registered descriptor. Generated-query bindings additionally require the
/// runtime's successful capability compilation and target-policy admission.
/// It is not user authority, resolver authority, or a substitute for the
/// current-principal policy decision performed by [`AuthenticatedToolBridge`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiRegisteredToolExecutionBinding {
    kind: AiRegisteredToolExecutionKind,
    tool_id: AiToolId,
    tool_fingerprint: String,
    operation_kind: AiToolOperationKind,
    generated_capability_fingerprint: Option<String>,
    operation_contract: GraphqlOperationContract,
    provenance: Option<AiToolExecutionProvenance>,
}

/// Closed origin of a crate-authored registered tool execution binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiRegisteredToolExecutionKind {
    /// Exact static descriptor admitted through ordinary host tool policy.
    StaticOperation,
    /// Exact generated query capability admitted by active target policy.
    GeneratedQuery,
    /// Exact generated mutation capability. Remote delegated execution admits
    /// it only when the host explicitly enables registered mutations.
    GeneratedMutation,
}

impl AiRegisteredToolExecutionBinding {
    fn with_current_authorization(mut self, decision: &AiToolAuthorizationDecision) -> Self {
        if let Some(provenance) = self.provenance.as_mut() {
            provenance.policy_version = Some(decision.policy_version.clone());
            provenance.authorization_state_digest =
                Some(decision.authorization_state_digest.clone());
        }
        self
    }
    /// Returns whether the execution came from the static descriptor path or
    /// the generated-query capability path.
    pub const fn kind(&self) -> AiRegisteredToolExecutionKind {
        self.kind
    }

    /// Returns the exact registered static tool or generated capability ID.
    pub const fn tool_id(&self) -> &AiToolId {
        &self.tool_id
    }

    /// Returns the exact compiled descriptor fingerprint used for current host
    /// policy and ordinary resolver execution.
    pub fn tool_fingerprint(&self) -> &str {
        &self.tool_fingerprint
    }

    /// Returns the exact GraphQL operation kind.
    pub const fn operation_kind(&self) -> AiToolOperationKind {
        self.operation_kind
    }

    /// Returns the provider-visible registered generated-query capability
    /// fingerprint, or `None` for a static descriptor.
    pub fn generated_capability_fingerprint(&self) -> Option<&str> {
        self.generated_capability_fingerprint.as_deref()
    }

    /// Trusted durable lifecycle provenance, when execution originated there.
    pub fn provenance(&self) -> Option<&AiToolExecutionProvenance> {
        self.provenance.as_ref()
    }

    pub(crate) fn with_provenance(
        mut self,
        provenance: AiToolExecutionProvenance,
        request: &ToolGraphqlRequest,
    ) -> Result<Self, ToolExecutionError> {
        if !provenance.matches_request(&self.tool_fingerprint, request) {
            return Err(ToolExecutionError::StaleContract);
        }
        self.provenance = Some(provenance);
        Ok(self)
    }

    pub(crate) fn matches_request(&self, request: &ToolGraphqlRequest) -> bool {
        self.provenance
            .as_ref()
            .is_none_or(|provenance| provenance.matches_request(&self.tool_fingerprint, request))
            && self.operation_contract == request.contract
            && request.operation_name == self.operation_contract.operation_name
            && crate::stable_graphql_document_hash(&request.document)
                == self.operation_contract.document_hash
    }

    pub(crate) fn static_operation(
        descriptor: &AiToolDescriptor,
        request: &ToolGraphqlRequest,
    ) -> Result<Self, ToolExecutionError> {
        validate_descriptor_request_binding(descriptor, request)?;
        Ok(Self {
            kind: AiRegisteredToolExecutionKind::StaticOperation,
            tool_id: descriptor.id.clone(),
            tool_fingerprint: descriptor.fingerprint.clone(),
            operation_kind: descriptor.operation_kind,
            generated_capability_fingerprint: None,
            operation_contract: request.contract.clone(),
            provenance: None,
        })
    }

    pub(crate) fn generated_query(
        capability_id: &AiToolId,
        capability_fingerprint: &str,
        descriptor: &AiToolDescriptor,
        request: &ToolGraphqlRequest,
    ) -> Result<Self, ToolExecutionError> {
        validate_descriptor_request_binding(descriptor, request)?;
        let semantic = request
            .contract
            .semantic_operation()
            .ok_or(ToolExecutionError::StaleContract)?;
        if descriptor.id != *capability_id
            || descriptor.operation_kind != AiToolOperationKind::Query
            || semantic.kind().graphql_orm_kind()
                != graphql_orm::graphql::orm::GraphqlOperationKind::Query
            || !valid_sha256(capability_fingerprint)
        {
            return Err(ToolExecutionError::StaleContract);
        }
        Ok(Self {
            kind: AiRegisteredToolExecutionKind::GeneratedQuery,
            tool_id: capability_id.clone(),
            tool_fingerprint: descriptor.fingerprint.clone(),
            operation_kind: AiToolOperationKind::Query,
            generated_capability_fingerprint: Some(capability_fingerprint.to_owned()),
            operation_contract: request.contract.clone(),
            provenance: None,
        })
    }

    pub(crate) fn generated_mutation(
        capability_id: &AiToolId,
        capability_fingerprint: &str,
        descriptor: &AiToolDescriptor,
        request: &ToolGraphqlRequest,
    ) -> Result<Self, ToolExecutionError> {
        validate_descriptor_request_binding(descriptor, request)?;
        let semantic = request
            .contract
            .semantic_operation()
            .ok_or(ToolExecutionError::StaleContract)?;
        if descriptor.id != *capability_id
            || descriptor.operation_kind != AiToolOperationKind::Mutation
            || semantic.kind().graphql_orm_kind()
                != graphql_orm::graphql::orm::GraphqlOperationKind::Mutation
            || !valid_sha256(capability_fingerprint)
        {
            return Err(ToolExecutionError::StaleContract);
        }
        Ok(Self {
            kind: AiRegisteredToolExecutionKind::GeneratedMutation,
            tool_id: capability_id.clone(),
            tool_fingerprint: descriptor.fingerprint.clone(),
            operation_kind: AiToolOperationKind::Mutation,
            generated_capability_fingerprint: Some(capability_fingerprint.to_owned()),
            operation_contract: request.contract.clone(),
            provenance: None,
        })
    }
}

fn validate_descriptor_request_binding(
    descriptor: &AiToolDescriptor,
    request: &ToolGraphqlRequest,
) -> Result<(), ToolExecutionError> {
    if !descriptor.has_valid_fingerprint()
        || descriptor.operation_kind == AiToolOperationKind::Internal
        || descriptor.document != request.document
        || descriptor.graphql_contract.as_ref() != Some(&request.contract)
        || descriptor.result_projection != request.contract.result_projection_fingerprint
        || request.operation_name != request.contract.operation_name
    {
        return Err(ToolExecutionError::StaleContract);
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl GraphqlRequestContext {
    /// Wraps a host-specific request context.
    pub fn new<T>(context: T) -> Self
    where
        T: Any + Send + Sync,
    {
        Self {
            inner: Arc::new(context),
        }
    }

    /// Downcasts to the host-specific context type.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.inner.downcast_ref()
    }
}

/// Server-authored GraphQL operation request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolGraphqlRequest {
    /// Static server-authored operation document.
    pub document: String,
    /// Operation name.
    pub operation_name: String,
    /// Exact target/schema/document/projection/disclosure binding.
    pub contract: GraphqlOperationContract,
    /// Schema-validated variables.
    pub variables: serde_json::Value,
    /// Invocation/audit metadata.
    pub invocation: GraphqlInvocationContext,
}

/// Bounded normalized GraphQL result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolGraphqlResponse {
    /// Projected JSON result.
    pub data: serde_json::Value,
    /// Safe stable public error codes.
    pub error_codes: Vec<String>,
    /// Host application audit reference, when emitted.
    pub application_audit_ref: Option<String>,
}

/// Canonical host request-context factory shared with normal HTTP/WS paths.
#[async_trait]
pub trait GraphqlRequestContextFactory: Send + Sync {
    /// Builds the complete auth, DB-auth, loader, rate-limit, request, and audit
    /// envelope for the exact server-authored request.
    ///
    /// Receiving the complete request lets remote factories bind delegated
    /// authority to the operation document, variables, projection, disclosure,
    /// run/tool identity, and audit chain before execution.
    async fn build(
        &self,
        principal: &ResolvedPrincipal,
        target: &GraphqlExecutionTarget,
        request: &ToolGraphqlRequest,
    ) -> Result<GraphqlRequestContext, ToolExecutionError>;

    /// Builds the canonical envelope with the crate-authored identity of the
    /// exact registered descriptor or generated-query capability.
    ///
    /// The default preserves existing local context factories by delegating to
    /// [`Self::build`]. Security-sensitive remote factories override this hook
    /// to bind short-lived delegated authority to `binding`. Callers cannot
    /// construct a generated binding independently of capability compilation
    /// and target-policy validation.
    async fn build_registered(
        &self,
        principal: &ResolvedPrincipal,
        target: &GraphqlExecutionTarget,
        binding: &AiRegisteredToolExecutionBinding,
        request: &ToolGraphqlRequest,
    ) -> Result<GraphqlRequestContext, ToolExecutionError> {
        let _ = binding;
        self.build(principal, target, request).await
    }
}

/// Executes a server-authored operation against the composed host schema.
#[async_trait]
pub trait AuthenticatedGraphqlExecutor: Send + Sync {
    /// Executes with the canonical host request context.
    async fn execute(
        &self,
        context: GraphqlRequestContext,
        request: ToolGraphqlRequest,
    ) -> Result<ToolGraphqlResponse, ToolExecutionError>;
}

/// Security-preserving bridge that always rehydrates before constructing and
/// executing a tool request.
#[derive(Clone)]
pub struct AuthenticatedToolBridge {
    principal_resolver: Arc<dyn CurrentPrincipalResolver>,
    authorization_policy: Arc<dyn AiToolAuthorizationPolicy>,
    context_factory: Arc<dyn GraphqlRequestContextFactory>,
    executor: Arc<dyn AuthenticatedGraphqlExecutor>,
    targets: GraphqlExecutionTargetRegistry,
}

impl AuthenticatedToolBridge {
    /// Creates a bridge from host implementations.
    pub fn new(
        principal_resolver: Arc<dyn CurrentPrincipalResolver>,
        authorization_policy: Arc<dyn AiToolAuthorizationPolicy>,
        context_factory: Arc<dyn GraphqlRequestContextFactory>,
        executor: Arc<dyn AuthenticatedGraphqlExecutor>,
        targets: GraphqlExecutionTargetRegistry,
    ) -> Self {
        Self {
            principal_resolver,
            authorization_policy,
            context_factory,
            executor,
            targets,
        }
    }

    /// Rehydrates the principal, builds the canonical request envelope, and
    /// executes the static request.
    pub async fn execute(
        &self,
        principal_reference: &PrincipalReference,
        descriptor: &AiToolDescriptor,
        request: ToolGraphqlRequest,
    ) -> Result<(ToolGraphqlResponse, AiToolAuthorizationDecision), ToolExecutionError> {
        let binding = AiRegisteredToolExecutionBinding::static_operation(descriptor, &request)?;
        self.execute_with_binding(principal_reference, descriptor, request, binding)
            .await
    }

    pub(crate) async fn execute_generated_query(
        &self,
        principal_reference: &PrincipalReference,
        capability_id: &AiToolId,
        capability_fingerprint: &str,
        descriptor: &AiToolDescriptor,
        request: ToolGraphqlRequest,
    ) -> Result<(ToolGraphqlResponse, AiToolAuthorizationDecision), ToolExecutionError> {
        let binding = AiRegisteredToolExecutionBinding::generated_query(
            capability_id,
            capability_fingerprint,
            descriptor,
            &request,
        )?;
        self.execute_with_binding(principal_reference, descriptor, request, binding)
            .await
    }

    pub(crate) async fn execute_generated_mutation(
        &self,
        principal_reference: &PrincipalReference,
        capability_id: &AiToolId,
        capability_fingerprint: &str,
        descriptor: &AiToolDescriptor,
        request: ToolGraphqlRequest,
    ) -> Result<(ToolGraphqlResponse, AiToolAuthorizationDecision), ToolExecutionError> {
        let binding = AiRegisteredToolExecutionBinding::generated_mutation(
            capability_id,
            capability_fingerprint,
            descriptor,
            &request,
        )?;
        self.execute_with_binding(principal_reference, descriptor, request, binding)
            .await
    }

    pub(crate) async fn execute_with_binding(
        &self,
        principal_reference: &PrincipalReference,
        descriptor: &AiToolDescriptor,
        request: ToolGraphqlRequest,
        binding: AiRegisteredToolExecutionBinding,
    ) -> Result<(ToolGraphqlResponse, AiToolAuthorizationDecision), ToolExecutionError> {
        if request.operation_name != request.contract.operation_name {
            return Err(ToolExecutionError::StaleContract);
        }
        let target = self
            .targets
            .validate_contract(&request.contract, &request.document)?;
        let principal = self
            .principal_resolver
            .resolve(principal_reference)
            .await
            .map_err(|_| ToolExecutionError::Reauthorization)?;
        let authorization = self
            .authorization_policy
            .authorize(
                &principal,
                &request.invocation.scope,
                descriptor,
                &request.variables,
            )
            .await;
        if !authorization.is_complete_allow()
            || authorization.approval_requirement() != AiApprovalRule::None
        {
            return Err(ToolExecutionError::Authorization);
        }
        let binding = binding.with_current_authorization(&authorization);
        let context = self
            .context_factory
            .build_registered(&principal, target, &binding, &request)
            .await?;
        let response = self.executor.execute(context, request).await?;
        Ok((response, authorization))
    }

    /// Rehydrates and authorizes an exact registered request without invoking
    /// its resolver.
    ///
    /// This is only a current host tool-policy decision. It does not prove
    /// resolver authorization, unchanged application resources, approval, or
    /// successful execution.
    pub(crate) async fn preauthorize(
        &self,
        principal_reference: &PrincipalReference,
        descriptor: &AiToolDescriptor,
        request: &ToolGraphqlRequest,
    ) -> Result<(ResolvedPrincipal, AiToolAuthorizationDecision), ToolExecutionError> {
        if request.operation_name != request.contract.operation_name {
            return Err(ToolExecutionError::StaleContract);
        }
        self.targets
            .validate_contract(&request.contract, &request.document)?;
        let principal = self
            .principal_resolver
            .resolve(principal_reference)
            .await
            .map_err(|_| ToolExecutionError::Reauthorization)?;
        let authorization = self
            .authorization_policy
            .authorize(
                &principal,
                &request.invocation.scope,
                descriptor,
                &request.variables,
            )
            .await;
        if !authorization.is_complete_allow() {
            return Err(ToolExecutionError::Authorization);
        }
        Ok((principal, authorization))
    }

    /// Executes only when a newly recomputed host policy decision still
    /// matches the policy version and safe authorization-state digest bound to
    /// a consumed one-shot approval.
    #[cfg(test)]
    pub(crate) async fn execute_bound(
        &self,
        principal_reference: &PrincipalReference,
        descriptor: &AiToolDescriptor,
        request: ToolGraphqlRequest,
        expected_policy_version: &str,
        expected_authorization_state_digest: &str,
        required_approval: Option<AiApprovalRule>,
    ) -> Result<(ToolGraphqlResponse, AiToolAuthorizationDecision), ToolExecutionError> {
        let binding = AiRegisteredToolExecutionBinding::static_operation(descriptor, &request)?;
        self.execute_registered_bound_requiring(
            principal_reference,
            descriptor,
            request,
            binding,
            expected_policy_version,
            expected_authorization_state_digest,
            required_approval,
        )
        .await
    }

    pub(crate) async fn execute_registered_bound(
        &self,
        principal_reference: &PrincipalReference,
        descriptor: &AiToolDescriptor,
        request: ToolGraphqlRequest,
        binding: AiRegisteredToolExecutionBinding,
        expected_policy_version: &str,
        expected_authorization_state_digest: &str,
    ) -> Result<(ToolGraphqlResponse, AiToolAuthorizationDecision), ToolExecutionError> {
        self.execute_registered_bound_requiring(
            principal_reference,
            descriptor,
            request,
            binding,
            expected_policy_version,
            expected_authorization_state_digest,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn execute_registered_bound_requiring(
        &self,
        principal_reference: &PrincipalReference,
        descriptor: &AiToolDescriptor,
        request: ToolGraphqlRequest,
        binding: AiRegisteredToolExecutionBinding,
        expected_policy_version: &str,
        expected_authorization_state_digest: &str,
        required_approval: Option<AiApprovalRule>,
    ) -> Result<(ToolGraphqlResponse, AiToolAuthorizationDecision), ToolExecutionError> {
        let (principal, authorization) = self
            .preauthorize(principal_reference, descriptor, &request)
            .await?;
        if authorization.policy_version != expected_policy_version
            || authorization.authorization_state_digest != expected_authorization_state_digest
            || required_approval
                .is_some_and(|required| authorization.approval_requirement() != required)
        {
            return Err(ToolExecutionError::Authorization);
        }
        let target = self
            .targets
            .validate_contract(&request.contract, &request.document)?;
        let binding = binding.with_current_authorization(&authorization);
        let context = self
            .context_factory
            .build_registered(&principal, target, &binding, &request)
            .await?;
        let response = self.executor.execute(context, request).await?;
        Ok((response, authorization))
    }
}
