//! Authenticated application GraphQL execution contracts.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;

use agql_auth::{CurrentPrincipalResolver, PrincipalReference, ResolvedPrincipal};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    AiRunId, AiScope, AiToolAuthorizationDecision, AiToolAuthorizationPolicy, AiToolCallId,
    AiToolDescriptor, AiToolId, AiToolOperationKind, GraphqlExecutionTargetId,
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
}

/// Closed origin of a crate-authored registered tool execution binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiRegisteredToolExecutionKind {
    /// Exact static descriptor admitted through ordinary host tool policy.
    StaticOperation,
    /// Exact generated query capability admitted by active target policy.
    GeneratedQuery,
    /// Exact generated mutation capability. Remote delegated execution denies
    /// this kind unless a later separately reviewed contract admits it.
    GeneratedMutation,
}

impl AiRegisteredToolExecutionBinding {
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

    async fn execute_with_binding(
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
        if !authorization.is_complete_allow() {
            return Err(ToolExecutionError::Authorization);
        }
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
    pub(crate) async fn execute_bound(
        &self,
        principal_reference: &PrincipalReference,
        descriptor: &AiToolDescriptor,
        request: ToolGraphqlRequest,
        expected_policy_version: &str,
        expected_authorization_state_digest: &str,
    ) -> Result<(ToolGraphqlResponse, AiToolAuthorizationDecision), ToolExecutionError> {
        let binding = AiRegisteredToolExecutionBinding::static_operation(descriptor, &request)?;
        self.execute_registered_bound(
            principal_reference,
            descriptor,
            request,
            binding,
            expected_policy_version,
            expected_authorization_state_digest,
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
        let (principal, authorization) = self
            .preauthorize(principal_reference, descriptor, &request)
            .await?;
        if authorization.policy_version != expected_policy_version
            || authorization.authorization_state_digest != expected_authorization_state_digest
        {
            return Err(ToolExecutionError::Authorization);
        }
        let target = self
            .targets
            .validate_contract(&request.contract, &request.document)?;
        let context = self
            .context_factory
            .build_registered(&principal, target, &binding, &request)
            .await?;
        let response = self.executor.execute(context, request).await?;
        Ok((response, authorization))
    }
}
