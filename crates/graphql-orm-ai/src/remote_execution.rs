//! Short-lived delegated authority for private remote GraphQL execution.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use agql_auth::{Clock, ResolvedPrincipal};
use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    AiRegisteredToolExecutionBinding, AiRegisteredToolExecutionKind, AiRunId, AiScope,
    AiToolCallId, AiToolId, AiToolOperationKind, AuthenticatedGraphqlExecutor,
    GraphqlExecutionTarget, GraphqlExecutionTargetClass, GraphqlExecutionTargetId,
    GraphqlGeneratedOperationKind, GraphqlRequestContext, GraphqlRequestContextFactory,
    ToolExecutionError, ToolGraphqlRequest, ToolGraphqlResponse,
};

/// Maximum lifetime contract for authority minted for one private remote
/// GraphQL invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRemoteGraphqlExecutionLimits {
    authority_lifetime: Duration,
    maximum_principal_age: Duration,
}

impl AiRemoteGraphqlExecutionLimits {
    /// Creates bounded delegated-authority and principal-freshness limits.
    ///
    /// # Errors
    ///
    /// Returns [`ToolExecutionError::InvalidTarget`] unless both limits are
    /// positive and no greater than one hour.
    pub fn new(
        authority_lifetime: Duration,
        maximum_principal_age: Duration,
    ) -> Result<Self, ToolExecutionError> {
        if !authority_lifetime.is_positive()
            || authority_lifetime > Duration::hours(1)
            || !maximum_principal_age.is_positive()
            || maximum_principal_age > Duration::hours(1)
        {
            return Err(ToolExecutionError::InvalidTarget);
        }
        Ok(Self {
            authority_lifetime,
            maximum_principal_age,
        })
    }

    /// Maximum lifetime requested from the deployment issuer.
    pub const fn authority_lifetime(self) -> Duration {
        self.authority_lifetime
    }

    /// Maximum accepted age of the freshly resolved principal.
    pub const fn maximum_principal_age(self) -> Duration {
        self.maximum_principal_age
    }
}

/// Closed origin of an exact remote GraphQL capability binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiRemoteGraphqlCapabilityKind {
    /// Exact static descriptor admitted through current host tool policy.
    StaticOperation,
    /// Exact generated query admitted through active target policy.
    GeneratedQuery,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum AiRemoteGraphqlCapabilityBindingInner {
    StaticOperation {
        operation_kind: AiToolOperationKind,
        tool_id: AiToolId,
        tool_fingerprint: String,
    },
    GeneratedQuery {
        operation_kind: GraphqlGeneratedOperationKind,
        capability_id: AiToolId,
        capability_fingerprint: String,
        compiled_tool_fingerprint: String,
        target_id: GraphqlExecutionTargetId,
        finished_schema_fingerprint: String,
        semantic_fingerprint_algorithm: String,
        semantic_catalog_fingerprint: String,
        semantic_operation_fingerprint: String,
        root_field: String,
    },
}

/// Immutable crate-authored identity of the registered capability used for
/// one remote GraphQL invocation.
///
/// The remote adapter constructs this value only from the exact descriptor or
/// generated query binding already admitted by the authenticated runtime. It
/// is descriptive delegation evidence, not user authority, target policy, or
/// resolver authorization. Its private representation intentionally exposes
/// read-only accessors but no host constructor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AiRemoteGraphqlCapabilityBinding(AiRemoteGraphqlCapabilityBindingInner);

impl AiRemoteGraphqlCapabilityBinding {
    /// Returns the static or generated-query capability origin.
    pub const fn kind(&self) -> AiRemoteGraphqlCapabilityKind {
        match self.0 {
            AiRemoteGraphqlCapabilityBindingInner::StaticOperation { .. } => {
                AiRemoteGraphqlCapabilityKind::StaticOperation
            }
            AiRemoteGraphqlCapabilityBindingInner::GeneratedQuery { .. } => {
                AiRemoteGraphqlCapabilityKind::GeneratedQuery
            }
        }
    }

    /// Returns the exact static tool ID or generated capability ID.
    pub const fn capability_id(&self) -> &AiToolId {
        match &self.0 {
            AiRemoteGraphqlCapabilityBindingInner::StaticOperation { tool_id, .. } => tool_id,
            AiRemoteGraphqlCapabilityBindingInner::GeneratedQuery { capability_id, .. } => {
                capability_id
            }
        }
    }

    /// Returns the registered static descriptor fingerprint or generated
    /// provider capability fingerprint.
    pub fn capability_fingerprint(&self) -> &str {
        match &self.0 {
            AiRemoteGraphqlCapabilityBindingInner::StaticOperation {
                tool_fingerprint, ..
            } => tool_fingerprint,
            AiRemoteGraphqlCapabilityBindingInner::GeneratedQuery {
                capability_fingerprint,
                ..
            } => capability_fingerprint,
        }
    }

    /// Returns the exact compiled descriptor fingerprint. For static tools it
    /// is identical to [`Self::capability_fingerprint`].
    pub fn compiled_tool_fingerprint(&self) -> &str {
        match &self.0 {
            AiRemoteGraphqlCapabilityBindingInner::StaticOperation {
                tool_fingerprint, ..
            } => tool_fingerprint,
            AiRemoteGraphqlCapabilityBindingInner::GeneratedQuery {
                compiled_tool_fingerprint,
                ..
            } => compiled_tool_fingerprint,
        }
    }

    /// Returns the exact operation kind.
    pub const fn operation_kind(&self) -> AiToolOperationKind {
        match self.0 {
            AiRemoteGraphqlCapabilityBindingInner::StaticOperation { operation_kind, .. } => {
                operation_kind
            }
            AiRemoteGraphqlCapabilityBindingInner::GeneratedQuery { operation_kind, .. } => {
                match operation_kind {
                    GraphqlGeneratedOperationKind::Query => AiToolOperationKind::Query,
                    GraphqlGeneratedOperationKind::Mutation => AiToolOperationKind::Mutation,
                    GraphqlGeneratedOperationKind::Subscription => {
                        AiToolOperationKind::Subscription
                    }
                    _ => AiToolOperationKind::Internal,
                }
            }
        }
    }

    /// Returns the generated query's logical target, or `None` for a static
    /// descriptor.
    pub const fn generated_target_id(&self) -> Option<&GraphqlExecutionTargetId> {
        match &self.0 {
            AiRemoteGraphqlCapabilityBindingInner::GeneratedQuery { target_id, .. } => {
                Some(target_id)
            }
            AiRemoteGraphqlCapabilityBindingInner::StaticOperation { .. } => None,
        }
    }

    /// Returns the generated query's active finished-schema fingerprint.
    pub fn generated_finished_schema_fingerprint(&self) -> Option<&str> {
        match &self.0 {
            AiRemoteGraphqlCapabilityBindingInner::GeneratedQuery {
                finished_schema_fingerprint,
                ..
            } => Some(finished_schema_fingerprint),
            AiRemoteGraphqlCapabilityBindingInner::StaticOperation { .. } => None,
        }
    }

    /// Returns the generated query's semantic fingerprint algorithm.
    pub fn generated_semantic_fingerprint_algorithm(&self) -> Option<&str> {
        match &self.0 {
            AiRemoteGraphqlCapabilityBindingInner::GeneratedQuery {
                semantic_fingerprint_algorithm,
                ..
            } => Some(semantic_fingerprint_algorithm),
            AiRemoteGraphqlCapabilityBindingInner::StaticOperation { .. } => None,
        }
    }

    /// Returns the generated query's exact semantic-catalogue fingerprint.
    pub fn generated_semantic_catalog_fingerprint(&self) -> Option<&str> {
        match &self.0 {
            AiRemoteGraphqlCapabilityBindingInner::GeneratedQuery {
                semantic_catalog_fingerprint,
                ..
            } => Some(semantic_catalog_fingerprint),
            AiRemoteGraphqlCapabilityBindingInner::StaticOperation { .. } => None,
        }
    }

    /// Returns the generated query's exact semantic-operation fingerprint.
    pub fn generated_semantic_operation_fingerprint(&self) -> Option<&str> {
        match &self.0 {
            AiRemoteGraphqlCapabilityBindingInner::GeneratedQuery {
                semantic_operation_fingerprint,
                ..
            } => Some(semantic_operation_fingerprint),
            AiRemoteGraphqlCapabilityBindingInner::StaticOperation { .. } => None,
        }
    }

    /// Returns the generated query's exact public root field.
    pub fn generated_root_field(&self) -> Option<&str> {
        match &self.0 {
            AiRemoteGraphqlCapabilityBindingInner::GeneratedQuery { root_field, .. } => {
                Some(root_field)
            }
            AiRemoteGraphqlCapabilityBindingInner::StaticOperation { .. } => None,
        }
    }

    fn from_registered(
        registered: &AiRegisteredToolExecutionBinding,
        target: &GraphqlExecutionTarget,
        request: &ToolGraphqlRequest,
    ) -> Result<Self, ToolExecutionError> {
        if !valid_sha256(registered.tool_fingerprint()) {
            return Err(ToolExecutionError::StaleContract);
        }
        match registered.kind() {
            AiRegisteredToolExecutionKind::StaticOperation => {
                if registered.generated_capability_fingerprint().is_some()
                    || registered.operation_kind() != AiToolOperationKind::Query
                {
                    return Err(ToolExecutionError::StaleContract);
                }
                Ok(Self(
                    AiRemoteGraphqlCapabilityBindingInner::StaticOperation {
                        operation_kind: registered.operation_kind(),
                        tool_id: registered.tool_id().clone(),
                        tool_fingerprint: registered.tool_fingerprint().to_owned(),
                    },
                ))
            }
            AiRegisteredToolExecutionKind::GeneratedQuery => {
                let capability_fingerprint = registered
                    .generated_capability_fingerprint()
                    .filter(|value| valid_sha256(value))
                    .ok_or(ToolExecutionError::StaleContract)?;
                let semantic = request
                    .contract
                    .semantic_operation()
                    .ok_or(ToolExecutionError::StaleContract)?;
                if registered.operation_kind() != AiToolOperationKind::Query
                    || semantic.kind() != GraphqlGeneratedOperationKind::Query
                    || request.contract.target_id != target.id
                    || request.contract.schema_fingerprint != target.schema_fingerprint
                {
                    return Err(ToolExecutionError::StaleContract);
                }
                Ok(Self(
                    AiRemoteGraphqlCapabilityBindingInner::GeneratedQuery {
                        operation_kind: GraphqlGeneratedOperationKind::Query,
                        capability_id: registered.tool_id().clone(),
                        capability_fingerprint: capability_fingerprint.to_owned(),
                        compiled_tool_fingerprint: registered.tool_fingerprint().to_owned(),
                        target_id: target.id.clone(),
                        finished_schema_fingerprint: target.schema_fingerprint.clone(),
                        semantic_fingerprint_algorithm: semantic.fingerprint_algorithm().to_owned(),
                        semantic_catalog_fingerprint: semantic.catalog_fingerprint().to_owned(),
                        semantic_operation_fingerprint: semantic.operation_fingerprint().to_owned(),
                        root_field: semantic.field_name().to_owned(),
                    },
                ))
            }
            AiRegisteredToolExecutionKind::GeneratedMutation => {
                Err(ToolExecutionError::StaleContract)
            }
        }
    }
}

/// Redacted exact authority request for one server-authored private GraphQL
/// operation.
///
/// This value contains no bearer credential, user token, role list, or scope
/// snapshot. It binds the freshly resolved actor to one logical target,
/// operation contract, canonical variable hash, scope, run/tool call, audit
/// chain, and expiry. It is an issuer request, not proof that a returned token
/// actually contains equivalent claims; the trusted deployment issuer and
/// transport remain responsible for that enforcement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiRemoteGraphqlDelegationRequest {
    capability_binding: AiRemoteGraphqlCapabilityBinding,
    target_id: GraphqlExecutionTargetId,
    target_class: GraphqlExecutionTargetClass,
    audience: String,
    resource_type: String,
    resource_id: String,
    schema_fingerprint: String,
    operation_name: String,
    operation_document_hash: String,
    result_projection_fingerprint: String,
    disclosure_schema_fingerprint: String,
    argument_hash: String,
    principal_subject: String,
    actor_subject: Option<String>,
    scope: AiScope,
    run_id: AiRunId,
    tool_call_id: AiToolCallId,
    correlation_id: String,
    causation_id: String,
    delegation_reference: Option<String>,
    idempotency_key_hash: Option<String>,
    expires_at: OffsetDateTime,
}

impl AiRemoteGraphqlDelegationRequest {
    /// Exact crate-authored static or generated-query capability identity.
    pub const fn capability_binding(&self) -> &AiRemoteGraphqlCapabilityBinding {
        &self.capability_binding
    }

    /// Logical deployment target. No URL is included.
    pub fn target_id(&self) -> &GraphqlExecutionTargetId {
        &self.target_id
    }

    /// Private routed/direct trust class requested from the issuer.
    pub const fn target_class(&self) -> GraphqlExecutionTargetClass {
        self.target_class
    }

    /// Required delegated credential audience.
    pub fn audience(&self) -> &str {
        &self.audience
    }

    /// Required resource type.
    pub fn resource_type(&self) -> &str {
        &self.resource_type
    }

    /// Required resource identifier.
    pub fn resource_id(&self) -> &str {
        &self.resource_id
    }

    /// Fresh resolved principal subject represented by the delegated call.
    pub fn principal_subject(&self) -> &str {
        &self.principal_subject
    }

    /// Original actor subject for on-behalf-of work, when present.
    pub fn actor_subject(&self) -> Option<&str> {
        self.actor_subject.as_deref()
    }

    /// Exact application scope of the invocation.
    pub const fn scope(&self) -> &AiScope {
        &self.scope
    }

    /// Exact registered operation name.
    pub fn operation_name(&self) -> &str {
        &self.operation_name
    }

    /// Exact server-authored operation document hash.
    pub fn operation_document_hash(&self) -> &str {
        &self.operation_document_hash
    }

    /// Exact reviewed target schema fingerprint.
    pub fn schema_fingerprint(&self) -> &str {
        &self.schema_fingerprint
    }

    /// Exact reviewed result-projection fingerprint.
    pub fn result_projection_fingerprint(&self) -> &str {
        &self.result_projection_fingerprint
    }

    /// Exact reviewed static disclosure-schema fingerprint.
    pub fn disclosure_schema_fingerprint(&self) -> &str {
        &self.disclosure_schema_fingerprint
    }

    /// Canonical hash of the exact operation variables.
    pub fn argument_hash(&self) -> &str {
        &self.argument_hash
    }

    /// Run causing the delegated invocation.
    pub const fn run_id(&self) -> AiRunId {
        self.run_id
    }

    /// Tool call causing the delegated invocation.
    pub const fn tool_call_id(&self) -> AiToolCallId {
        self.tool_call_id
    }

    /// Correlation identifier propagated into ordinary application audit.
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }

    /// Causation identifier propagated into ordinary application audit.
    pub fn causation_id(&self) -> &str {
        &self.causation_id
    }

    /// Safe delegation/grant reference, when the invocation has one.
    pub fn delegation_reference(&self) -> Option<&str> {
        self.delegation_reference.as_deref()
    }

    /// Hash of the invocation idempotency key, when present.
    ///
    /// The original idempotency value is deliberately not disclosed to the
    /// issuer request or its serialized audit form.
    pub fn idempotency_key_hash(&self) -> Option<&str> {
        self.idempotency_key_hash.as_deref()
    }

    /// Exclusive delegated-authority expiry.
    pub const fn expires_at(&self) -> OffsetDateTime {
        self.expires_at
    }

    /// Stable redacted hash suitable for issuer/transport audit correlation.
    pub fn stable_hash(&self) -> String {
        let encoded = serde_json::to_vec(self)
            .expect("AiRemoteGraphqlDelegationRequest contains serializable values");
        hex::encode(Sha256::digest(encoded))
    }

    fn build(
        principal: &ResolvedPrincipal,
        target: &GraphqlExecutionTarget,
        registered: &AiRegisteredToolExecutionBinding,
        request: &ToolGraphqlRequest,
        expires_at: OffsetDateTime,
    ) -> Result<Self, ToolExecutionError> {
        target.validate()?;
        if !matches!(
            target.class,
            GraphqlExecutionTargetClass::PrivateRouted | GraphqlExecutionTargetClass::PrivateDirect
        ) || request.contract.target_id != target.id
            || request.contract.schema_fingerprint != target.schema_fingerprint
            || request.contract.document_hash != stable_text_hash(&request.document)
            || request.operation_name != request.contract.operation_name
            || crate::contains_forbidden_graphql_name(&request.document)
            || request.invocation.scope.kind.trim().is_empty()
            || request.invocation.scope.id.trim().is_empty()
            || !valid_reference(&request.invocation.correlation_id)
            || !valid_reference(&request.invocation.causation_id)
            || request
                .invocation
                .delegation_reference
                .as_deref()
                .is_some_and(|value| !valid_reference(value))
            || request
                .invocation
                .idempotency_key
                .as_deref()
                .is_some_and(|value| !valid_reference(value))
            || !valid_reference(principal.principal().subject())
            || principal
                .reference()
                .actor_subject
                .as_deref()
                .is_some_and(|value| !valid_reference(value))
        {
            return Err(ToolExecutionError::InvalidTarget);
        }
        let audience = target
            .audience
            .clone()
            .filter(|value| valid_reference(value))
            .ok_or(ToolExecutionError::InvalidTarget)?;
        let resource_type = target
            .resource_type
            .clone()
            .filter(|value| valid_reference(value))
            .ok_or(ToolExecutionError::InvalidTarget)?;
        let resource_id = target
            .resource_id
            .clone()
            .filter(|value| valid_reference(value))
            .ok_or(ToolExecutionError::InvalidTarget)?;
        let capability_binding =
            AiRemoteGraphqlCapabilityBinding::from_registered(registered, target, request)?;
        Ok(Self {
            capability_binding,
            target_id: target.id.clone(),
            target_class: target.class,
            audience,
            resource_type,
            resource_id,
            schema_fingerprint: request.contract.schema_fingerprint.clone(),
            operation_name: request.contract.operation_name.clone(),
            operation_document_hash: request.contract.document_hash.clone(),
            result_projection_fingerprint: request.contract.result_projection_fingerprint.clone(),
            disclosure_schema_fingerprint: request.contract.disclosure_schema_fingerprint.clone(),
            argument_hash: canonical_json_hash(&request.variables)?,
            principal_subject: principal.principal().subject().to_owned(),
            actor_subject: principal.reference().actor_subject.clone(),
            scope: request.invocation.scope.clone(),
            run_id: request.invocation.run_id,
            tool_call_id: request.invocation.tool_call_id,
            correlation_id: request.invocation.correlation_id.clone(),
            causation_id: request.invocation.causation_id.clone(),
            delegation_reference: request.invocation.delegation_reference.clone(),
            idempotency_key_hash: request
                .invocation
                .idempotency_key
                .as_deref()
                .map(stable_text_hash),
            expires_at,
        })
    }

    fn matches(
        &self,
        target: &GraphqlExecutionTarget,
        registered: &AiRegisteredToolExecutionBinding,
        request: &ToolGraphqlRequest,
    ) -> Result<bool, ToolExecutionError> {
        Ok(self.capability_binding
            == AiRemoteGraphqlCapabilityBinding::from_registered(registered, target, request)?
            && self.target_id == target.id
            && self.target_class == target.class
            && target.audience.as_deref() == Some(self.audience.as_str())
            && target.resource_type.as_deref() == Some(self.resource_type.as_str())
            && target.resource_id.as_deref() == Some(self.resource_id.as_str())
            && self.schema_fingerprint == request.contract.schema_fingerprint
            && self.operation_name == request.contract.operation_name
            && self.operation_document_hash == request.contract.document_hash
            && self.result_projection_fingerprint == request.contract.result_projection_fingerprint
            && self.disclosure_schema_fingerprint == request.contract.disclosure_schema_fingerprint
            && self.argument_hash == canonical_json_hash(&request.variables)?
            && self.scope == request.invocation.scope
            && self.run_id == request.invocation.run_id
            && self.tool_call_id == request.invocation.tool_call_id
            && self.correlation_id == request.invocation.correlation_id
            && self.causation_id == request.invocation.causation_id
            && self.delegation_reference == request.invocation.delegation_reference
            && self.idempotency_key_hash
                == request
                    .invocation
                    .idempotency_key
                    .as_deref()
                    .map(stable_text_hash))
    }
}

/// Ephemeral delegated credential paired with its exact redacted authority
/// request.
///
/// The credential is secret and intentionally not serializable or cloneable.
/// Constructing this value asserts that the deployment issuer minted authority
/// no broader than the request; the crate cannot introspect proprietary token
/// claims. It must never be persisted, logged, returned to a model/client, or
/// reused for another operation.
pub struct AiRemoteGraphqlAuthority {
    request: AiRemoteGraphqlDelegationRequest,
    credential: SecretString,
}

impl AiRemoteGraphqlAuthority {
    /// Wraps a credential minted for exactly one delegation request.
    ///
    /// # Errors
    ///
    /// Returns [`ToolExecutionError::Authorization`] when the credential is
    /// empty. The trusted issuer remains responsible for matching its actual
    /// claims and expiry to the supplied request.
    pub fn for_request(
        request: &AiRemoteGraphqlDelegationRequest,
        credential: SecretString,
    ) -> Result<Self, ToolExecutionError> {
        if credential.expose_secret().trim().is_empty() {
            return Err(ToolExecutionError::Authorization);
        }
        Ok(Self {
            request: request.clone(),
            credential,
        })
    }

    /// Exact redacted request asserted by the issuer.
    pub const fn request(&self) -> &AiRemoteGraphqlDelegationRequest {
        &self.request
    }

    /// Ephemeral secret credential for the private transport only.
    ///
    /// Callers must not log, persist, clone into durable state, or expose this
    /// value to a model/client.
    pub const fn credential(&self) -> &SecretString {
        &self.credential
    }
}

impl fmt::Debug for AiRemoteGraphqlAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AiRemoteGraphqlAuthority")
            .field("request_hash", &self.request.stable_hash())
            .field("credential", &"[REDACTED]")
            .finish()
    }
}

/// Deployment issuer for fresh audience/resource/operation-bound remote
/// GraphQL authority.
#[async_trait]
pub trait AiRemoteGraphqlAuthorityIssuer: Send + Sync {
    /// Mints one ephemeral credential for the exact redacted request.
    ///
    /// The resolved principal contains current authority but no incoming bearer
    /// credential. Implementations must preserve the human actor, narrow rather
    /// than widen authority, and enforce the requested expiry.
    ///
    /// # Errors
    ///
    /// Returns a safe [`ToolExecutionError`] when current authority cannot be
    /// minted exactly; implementations must fail closed without returning a
    /// broader or partially bound credential.
    async fn issue(
        &self,
        principal: &ResolvedPrincipal,
        request: &AiRemoteGraphqlDelegationRequest,
    ) -> Result<AiRemoteGraphqlAuthority, ToolExecutionError>;
}

/// Private deployment transport for one exact remote GraphQL operation.
///
/// Implementations map the logical target ID to a fixed private allowlisted
/// destination. Neither a model nor `ToolGraphqlRequest` supplies a URL. A
/// direct-service route must enforce authority no broader than its equivalent
/// routed operation.
#[async_trait]
pub trait AiRemoteGraphqlTransport: Send + Sync {
    /// Sends one server-authored request with its ephemeral authority.
    ///
    /// # Errors
    ///
    /// Returns a safe [`ToolExecutionError`] when routing, authorization,
    /// transport, response bounds, or application execution fails.
    async fn execute(
        &self,
        target: &GraphqlExecutionTarget,
        authority: &AiRemoteGraphqlAuthority,
        request: ToolGraphqlRequest,
    ) -> Result<ToolGraphqlResponse, ToolExecutionError>;
}

struct AiRemoteGraphqlRequestContext {
    adapter_id: Uuid,
    target: GraphqlExecutionTarget,
    registered: AiRegisteredToolExecutionBinding,
    authority: AiRemoteGraphqlAuthority,
}

/// Canonical request-context factory and executor for private remote GraphQL
/// targets.
///
/// Use the same adapter as both [`GraphqlRequestContextFactory`] and
/// [`AuthenticatedGraphqlExecutor`]. The authenticated bridge still rehydrates
/// current principal/tool policy first. This adapter then mints a fresh exact
/// delegated credential and consumes it only through the private logical-route
/// transport.
#[derive(Clone)]
pub struct AiRemoteAuthenticatedGraphqlAdapter {
    adapter_id: Uuid,
    issuer: Arc<dyn AiRemoteGraphqlAuthorityIssuer>,
    transport: Arc<dyn AiRemoteGraphqlTransport>,
    clock: Arc<dyn Clock>,
    limits: AiRemoteGraphqlExecutionLimits,
}

impl AiRemoteAuthenticatedGraphqlAdapter {
    /// Creates a private remote GraphQL adapter.
    pub fn new(
        issuer: Arc<dyn AiRemoteGraphqlAuthorityIssuer>,
        transport: Arc<dyn AiRemoteGraphqlTransport>,
        clock: Arc<dyn Clock>,
        limits: AiRemoteGraphqlExecutionLimits,
    ) -> Self {
        Self {
            adapter_id: Uuid::new_v4(),
            issuer,
            transport,
            clock,
            limits,
        }
    }
}

#[async_trait]
impl GraphqlRequestContextFactory for AiRemoteAuthenticatedGraphqlAdapter {
    async fn build(
        &self,
        _principal: &ResolvedPrincipal,
        _target: &GraphqlExecutionTarget,
        _request: &ToolGraphqlRequest,
    ) -> Result<GraphqlRequestContext, ToolExecutionError> {
        Err(ToolExecutionError::RequestContext)
    }

    async fn build_registered(
        &self,
        principal: &ResolvedPrincipal,
        target: &GraphqlExecutionTarget,
        registered: &AiRegisteredToolExecutionBinding,
        request: &ToolGraphqlRequest,
    ) -> Result<GraphqlRequestContext, ToolExecutionError> {
        let now = self.clock.now();
        if principal.resolved_at() > now
            || now - principal.resolved_at() >= self.limits.maximum_principal_age
            || principal
                .reference()
                .expires_at
                .is_some_and(|expires_at| expires_at <= now)
        {
            return Err(ToolExecutionError::Reauthorization);
        }
        let maximum_expiry = now
            .checked_add(self.limits.authority_lifetime)
            .ok_or(ToolExecutionError::Authorization)?;
        let freshness_expiry = principal
            .resolved_at()
            .checked_add(self.limits.maximum_principal_age)
            .ok_or(ToolExecutionError::Authorization)?;
        let maximum_expiry = maximum_expiry.min(freshness_expiry);
        let expires_at = principal
            .reference()
            .expires_at
            .map_or(maximum_expiry, |principal_expiry| {
                principal_expiry.min(maximum_expiry)
            });
        let delegation = AiRemoteGraphqlDelegationRequest::build(
            principal, target, registered, request, expires_at,
        )?;
        let authority = self.issuer.issue(principal, &delegation).await?;
        let after_issuance = self.clock.now();
        if authority.request != delegation
            || authority.request.expires_at <= after_issuance
            || authority.request.expires_at > expires_at
        {
            return Err(ToolExecutionError::Authorization);
        }
        Ok(GraphqlRequestContext::new(AiRemoteGraphqlRequestContext {
            adapter_id: self.adapter_id,
            target: target.clone(),
            registered: registered.clone(),
            authority,
        }))
    }
}

#[async_trait]
impl AuthenticatedGraphqlExecutor for AiRemoteAuthenticatedGraphqlAdapter {
    async fn execute(
        &self,
        context: GraphqlRequestContext,
        request: ToolGraphqlRequest,
    ) -> Result<ToolGraphqlResponse, ToolExecutionError> {
        let context = context
            .downcast_ref::<AiRemoteGraphqlRequestContext>()
            .ok_or(ToolExecutionError::RequestContext)?;
        if context.adapter_id != self.adapter_id
            || self.clock.now() >= context.authority.request.expires_at
            || !context
                .authority
                .request
                .matches(&context.target, &context.registered, &request)?
        {
            return Err(ToolExecutionError::Authorization);
        }
        self.transport
            .execute(&context.target, &context.authority, request)
            .await
    }
}

fn canonical_json_hash(value: &serde_json::Value) -> Result<String, ToolExecutionError> {
    let encoded = serde_json::to_vec(&canonical_json(value))
        .map_err(|_| ToolExecutionError::Authorization)?;
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

fn stable_text_hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn valid_reference(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 1_024
        && value.bytes().all(|byte| !byte.is_ascii_control())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use agql_auth::{
        AccessTokenMetadata, AuthPrincipal, AuthUser, CurrentPrincipalResolver, FixedClock,
        ResolvedPrincipal, SessionContext,
    };
    use secrecy::ExposeSecret;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::{
        AiGraphqlQueryCapabilityCatalog, AiGraphqlQueryCapabilityLimits,
        AiToolAuthorizationDecision, AiToolAuthorizationPolicy, AiToolDescriptor,
        AiToolOperationKind, GraphqlInvocationContext, GraphqlOperationContract,
    };
    use graphql_orm::graphql::orm::{
        GraphqlEntitySemanticMetadata, GraphqlOperationCatalog, GraphqlOperationKind,
        GraphqlSemanticArgumentDescriptor, GraphqlSemanticCatalog, GraphqlSemanticClassification,
        GraphqlSemanticExport, GraphqlSemanticFieldMetadata, GraphqlSemanticOperationDescriptor,
        GraphqlSemanticTypeKind, GraphqlSemanticTypeRef,
    };

    struct Resolver {
        principal: AuthPrincipal,
        now: OffsetDateTime,
    }

    #[async_trait]
    impl CurrentPrincipalResolver for Resolver {
        async fn resolve(
            &self,
            reference: &agql_auth::PrincipalReference,
        ) -> agql_auth::AuthResult<ResolvedPrincipal> {
            ResolvedPrincipal::new(reference.clone(), self.principal.clone(), self.now)
        }
    }

    struct RevocableResolver {
        principal: AuthPrincipal,
        now: OffsetDateTime,
        revoked: Arc<AtomicBool>,
    }

    #[async_trait]
    impl CurrentPrincipalResolver for RevocableResolver {
        async fn resolve(
            &self,
            reference: &agql_auth::PrincipalReference,
        ) -> agql_auth::AuthResult<ResolvedPrincipal> {
            if self.revoked.load(Ordering::SeqCst) {
                return Err(agql_auth::AuthError::Forbidden);
            }
            ResolvedPrincipal::new(reference.clone(), self.principal.clone(), self.now)
        }
    }

    struct AllowTools;

    #[async_trait]
    impl AiToolAuthorizationPolicy for AllowTools {
        async fn authorize(
            &self,
            _principal: &ResolvedPrincipal,
            _scope: &AiScope,
            _descriptor: &AiToolDescriptor,
            _variables: &serde_json::Value,
        ) -> AiToolAuthorizationDecision {
            AiToolAuthorizationDecision::allow("remote_test", "policy-v1", "state-v1")
        }
    }

    struct Issuer {
        calls: AtomicUsize,
        hashes: Mutex<Vec<String>>,
        bindings: Mutex<Vec<AiRemoteGraphqlCapabilityBinding>>,
    }

    #[async_trait]
    impl AiRemoteGraphqlAuthorityIssuer for Issuer {
        async fn issue(
            &self,
            _principal: &ResolvedPrincipal,
            request: &AiRemoteGraphqlDelegationRequest,
        ) -> Result<AiRemoteGraphqlAuthority, ToolExecutionError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.hashes
                .lock()
                .expect("issuer hashes should lock")
                .push(request.stable_hash());
            self.bindings
                .lock()
                .expect("issuer bindings should lock")
                .push(request.capability_binding().clone());
            AiRemoteGraphqlAuthority::for_request(
                request,
                SecretString::from("ephemeral-test-authority".to_owned()),
            )
        }
    }

    struct Transport {
        calls: AtomicUsize,
    }

    struct IssuerCrossingFreshnessBoundary {
        clock: Arc<FixedClock>,
    }

    #[async_trait]
    impl AiRemoteGraphqlAuthorityIssuer for IssuerCrossingFreshnessBoundary {
        async fn issue(
            &self,
            _principal: &ResolvedPrincipal,
            request: &AiRemoteGraphqlDelegationRequest,
        ) -> Result<AiRemoteGraphqlAuthority, ToolExecutionError> {
            self.clock.advance_seconds(10);
            AiRemoteGraphqlAuthority::for_request(
                request,
                SecretString::from("already-expired-authority".to_owned()),
            )
        }
    }

    #[async_trait]
    impl AiRemoteGraphqlTransport for Transport {
        async fn execute(
            &self,
            target: &GraphqlExecutionTarget,
            authority: &AiRemoteGraphqlAuthority,
            request: ToolGraphqlRequest,
        ) -> Result<ToolGraphqlResponse, ToolExecutionError> {
            assert_eq!(target.id.as_str(), "private-router");
            assert_eq!(
                authority.credential().expose_secret(),
                if authority.request().capability_binding().kind()
                    == AiRemoteGraphqlCapabilityKind::GeneratedQuery
                {
                    "exact-generated-authority"
                } else {
                    "ephemeral-test-authority"
                }
            );
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolGraphqlResponse {
                data: json!({"recordId": request.variables.get("recordId")}),
                error_codes: Vec::new(),
                application_audit_ref: Some("remote-audit-1".to_owned()),
            })
        }
    }

    fn fixture() -> (
        AuthPrincipal,
        GraphqlExecutionTarget,
        AiToolDescriptor,
        ToolGraphqlRequest,
        Arc<Issuer>,
        Arc<Transport>,
        Arc<FixedClock>,
    ) {
        let now =
            OffsetDateTime::from_unix_timestamp(2_000_000_000).expect("test time should validate");
        let principal = AuthPrincipal::User(AuthUser {
            user_id: "remote-user".to_owned(),
            session_id: Uuid::new_v4(),
            roles: vec!["reader".to_owned()],
            scopes: vec!["records:read".to_owned()],
            session: SessionContext::default(),
            token_claims: AccessTokenMetadata {
                tenant_id: Some("tenant-1".to_owned()),
                ..AccessTokenMetadata::default()
            },
        });
        let target_id =
            GraphqlExecutionTargetId::parse("private-router").expect("target ID should parse");
        let target = GraphqlExecutionTarget {
            id: target_id.clone(),
            class: GraphqlExecutionTargetClass::PrivateRouted,
            audience: Some("private-graphql".to_owned()),
            resource_type: Some("tenant".to_owned()),
            resource_id: Some("tenant-1".to_owned()),
            schema_fingerprint: "schema-v1".to_owned(),
        };
        let document = "query ReadRecord($recordId: ID!) { record(id: $recordId) { recordId } }";
        let contract = GraphqlOperationContract::new(
            target_id,
            "schema-v1",
            "ReadRecord",
            document,
            "projection-v1",
            "disclosure-v1",
        )
        .expect("contract should validate");
        let descriptor = AiToolDescriptor::new(
            "records.remote_read",
            "Read one record through a private router",
            AiToolOperationKind::Query,
            document,
            json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {"recordId": {"type": "string"}},
                "required": ["recordId"],
                "additionalProperties": false
            }),
        )
        .expect("descriptor should validate")
        .with_result_projection("projection-v1")
        .with_graphql_contract(contract.clone());
        let request = ToolGraphqlRequest {
            document: document.to_owned(),
            operation_name: "ReadRecord".to_owned(),
            contract,
            variables: json!({"recordId": "54"}),
            invocation: GraphqlInvocationContext {
                run_id: AiRunId::new(),
                tool_call_id: AiToolCallId::new(),
                scope: AiScope::new("tenant", "tenant-1").with_tenant_id("tenant-1"),
                correlation_id: "remote-correlation".to_owned(),
                causation_id: "remote-causation".to_owned(),
                delegation_reference: Some("grant-reference-1".to_owned()),
                idempotency_key: Some("remote-idempotency-1".to_owned()),
            },
        };
        (
            principal,
            target,
            descriptor,
            request,
            Arc::new(Issuer {
                calls: AtomicUsize::new(0),
                hashes: Mutex::new(Vec::new()),
                bindings: Mutex::new(Vec::new()),
            }),
            Arc::new(Transport {
                calls: AtomicUsize::new(0),
            }),
            Arc::new(FixedClock::new(now)),
        )
    }

    fn static_binding(
        descriptor: &AiToolDescriptor,
        request: &ToolGraphqlRequest,
    ) -> AiRegisteredToolExecutionBinding {
        AiRegisteredToolExecutionBinding::static_operation(descriptor, request)
            .expect("static test binding should validate")
    }

    const GENERATED_QUERY_SDL: &str = r#"
        schema { query: Query }
        type Query { GeneratedRecord(recordId: ID!): Record! }
        type Record { recordId: ID!, subject: String! }
    "#;

    fn generated_query_catalog() -> (GraphqlSemanticCatalog, AiGraphqlQueryCapabilityCatalog) {
        let scalar = |field_name: &str, scalar_name: &str| GraphqlSemanticFieldMetadata {
            field_name: field_name.to_owned(),
            description: format!("Reviewed public {field_name}."),
            type_ref: GraphqlSemanticTypeRef::named(
                scalar_name,
                GraphqlSemanticTypeKind::Scalar,
                false,
            ),
            selectable: true,
            filter_operators: Vec::new(),
            sortable: false,
            groupable: false,
            aggregate_operators: Vec::new(),
            aggregate_value_kind: None,
            relationship: None,
            classification: GraphqlSemanticClassification::Internal,
            export: GraphqlSemanticExport::Exportable,
            has_field_policy: false,
        };
        let entity = GraphqlEntitySemanticMetadata {
            entity_name: "Record".to_owned(),
            description: "A reviewed application record.".to_owned(),
            default_classification: GraphqlSemanticClassification::Internal,
            fields: vec![scalar("recordId", "ID"), scalar("subject", "String")].into_boxed_slice(),
        };
        let operation = GraphqlSemanticOperationDescriptor::custom(
            GraphqlOperationKind::Query,
            "GeneratedRecord",
            "Read one reviewed application record.",
            vec![GraphqlSemanticArgumentDescriptor {
                graphql_name: "recordId".to_owned(),
                description: "Reviewed record identifier.".to_owned(),
                type_ref: GraphqlSemanticTypeRef::named(
                    "ID",
                    GraphqlSemanticTypeKind::Scalar,
                    false,
                ),
            }],
            GraphqlSemanticTypeRef::named("Record", GraphqlSemanticTypeKind::Object, false),
            true,
        )
        .expect("test query semantics should validate");
        let semantics = GraphqlSemanticCatalog::compose_with_custom(
            [entity],
            &GraphqlOperationCatalog::compose(std::iter::empty()),
            [operation],
        )
        .expect("test query catalogue should validate");
        let catalog = AiGraphqlQueryCapabilityCatalog::compile(
            "generated-read",
            GraphqlExecutionTargetId::parse("private-router")
                .expect("generated read target should validate"),
            GENERATED_QUERY_SDL,
            &semantics,
            AiGraphqlQueryCapabilityLimits::default(),
        )
        .expect("generated query capabilities should compile");
        (semantics, catalog)
    }

    struct ExactGeneratedIssuer {
        expected_capability_id: AiToolId,
        expected_capability_fingerprint: String,
        expected_root: String,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl AiRemoteGraphqlAuthorityIssuer for ExactGeneratedIssuer {
        async fn issue(
            &self,
            _principal: &ResolvedPrincipal,
            request: &AiRemoteGraphqlDelegationRequest,
        ) -> Result<AiRemoteGraphqlAuthority, ToolExecutionError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let binding = request.capability_binding();
            if binding.kind() != AiRemoteGraphqlCapabilityKind::GeneratedQuery
                || binding.capability_id() != &self.expected_capability_id
                || binding.capability_fingerprint() != self.expected_capability_fingerprint
                || binding.generated_target_id() != Some(request.target_id())
                || binding.generated_finished_schema_fingerprint()
                    != Some(request.schema_fingerprint())
                || binding.generated_root_field() != Some(self.expected_root.as_str())
                || binding.operation_kind() != AiToolOperationKind::Query
            {
                return Err(ToolExecutionError::Authorization);
            }
            AiRemoteGraphqlAuthority::for_request(
                request,
                SecretString::from("exact-generated-authority".to_owned()),
            )
        }
    }

    #[tokio::test]
    async fn bridge_mints_exact_ephemeral_authority_for_private_route() {
        let (principal, target, descriptor, request, issuer, transport, clock) = fixture();
        let adapter = Arc::new(AiRemoteAuthenticatedGraphqlAdapter::new(
            issuer.clone(),
            transport.clone(),
            clock,
            AiRemoteGraphqlExecutionLimits::new(Duration::seconds(30), Duration::seconds(30))
                .expect("limits should validate"),
        ));
        let mut targets = crate::GraphqlExecutionTargetRegistry::new();
        targets.register(target).expect("target should register");
        let bridge = crate::AuthenticatedToolBridge::new(
            Arc::new(Resolver {
                principal: principal.clone(),
                now: OffsetDateTime::from_unix_timestamp(2_000_000_000)
                    .expect("test time should validate"),
            }),
            Arc::new(AllowTools),
            adapter.clone(),
            adapter,
            targets,
        );
        let (response, authorization) = bridge
            .execute(&principal.reference(), &descriptor, request)
            .await
            .expect("private remote bridge should execute");
        assert_eq!(response.data, json!({"recordId": "54"}));
        assert_eq!(authorization.policy_version, "policy-v1");
        assert_eq!(issuer.calls.load(Ordering::SeqCst), 1);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        assert_eq!(issuer.hashes.lock().expect("hashes should lock").len(), 1);
        let bindings = issuer.bindings.lock().expect("bindings should lock");
        assert_eq!(bindings.len(), 1);
        assert_eq!(
            bindings[0].kind(),
            AiRemoteGraphqlCapabilityKind::StaticOperation
        );
        assert_eq!(bindings[0].capability_id().as_str(), "records.remote_read");
        assert_eq!(
            bindings[0].capability_fingerprint(),
            bindings[0].compiled_tool_fingerprint()
        );
        assert_eq!(bindings[0].operation_kind(), AiToolOperationKind::Query);
        assert!(bindings[0].generated_target_id().is_none());
    }

    #[tokio::test]
    async fn generated_query_delegation_is_exact_and_name_conventions_grant_nothing() {
        let (principal, mut target, _static_descriptor, _static_request, _, transport, clock) =
            fixture();
        let (semantics, catalog) = generated_query_catalog();
        let capability = catalog
            .capabilities()
            .next()
            .expect("one generated query should compile");
        target.schema_fingerprint = catalog.finished_schema_fingerprint().to_owned();
        let compiled = capability
            .compile(json!({
                "arguments": {"recordId": "54"},
                "fields": {"recordId": true, "subject": true},
                "relationships": {}
            }))
            .expect("generated query plan should compile");
        let capability_id = capability.id().clone();
        let capability_fingerprint = capability.fingerprint().to_owned();
        let (descriptor, _disclosure, variables) = compiled.into_parts();
        let contract = descriptor
            .graphql_contract
            .clone()
            .expect("generated query should bind GraphQL");
        let request = ToolGraphqlRequest {
            document: descriptor.document.clone(),
            operation_name: contract.operation_name.clone(),
            contract,
            variables,
            invocation: GraphqlInvocationContext {
                run_id: AiRunId::new(),
                tool_call_id: AiToolCallId::new(),
                scope: AiScope::new("tenant", "tenant-1").with_tenant_id("tenant-1"),
                correlation_id: "generated-correlation".to_owned(),
                causation_id: "generated-causation".to_owned(),
                delegation_reference: Some("generated-grant-reference".to_owned()),
                idempotency_key: None,
            },
        };
        let registered = AiRegisteredToolExecutionBinding::generated_query(
            &capability_id,
            &capability_fingerprint,
            &descriptor,
            &request,
        )
        .expect("generated execution binding should validate");
        let issuer = Arc::new(ExactGeneratedIssuer {
            expected_capability_id: capability_id.clone(),
            expected_capability_fingerprint: capability_fingerprint.clone(),
            expected_root: "GeneratedRecord".to_owned(),
            calls: AtomicUsize::new(0),
        });
        let adapter = Arc::new(AiRemoteAuthenticatedGraphqlAdapter::new(
            issuer.clone(),
            transport.clone(),
            clock.clone(),
            AiRemoteGraphqlExecutionLimits::new(Duration::seconds(30), Duration::seconds(30))
                .expect("limits should validate"),
        ));
        let mut targets = crate::GraphqlExecutionTargetRegistry::new();
        targets
            .register(target.clone())
            .expect("target should register");
        let revoked = Arc::new(AtomicBool::new(false));
        let bridge = crate::AuthenticatedToolBridge::new(
            Arc::new(RevocableResolver {
                principal: principal.clone(),
                now: clock.now(),
                revoked: revoked.clone(),
            }),
            Arc::new(AllowTools),
            adapter.clone(),
            adapter.clone(),
            targets,
        );
        bridge
            .execute_generated_query(
                &principal.reference(),
                &capability_id,
                &capability_fingerprint,
                &descriptor,
                request.clone(),
            )
            .await
            .expect("exact generated query should reach the private route");
        assert_eq!(issuer.calls.load(Ordering::SeqCst), 1);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        revoked.store(true, Ordering::SeqCst);
        assert!(matches!(
            bridge
                .execute_generated_query(
                    &principal.reference(),
                    &capability_id,
                    &capability_fingerprint,
                    &descriptor,
                    request.clone(),
                )
                .await,
            Err(ToolExecutionError::Reauthorization)
        ));
        assert_eq!(issuer.calls.load(Ordering::SeqCst), 1);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);

        let resolved = ResolvedPrincipal::new(principal.reference(), principal, clock.now())
            .expect("principal should resolve");
        let delegation = AiRemoteGraphqlDelegationRequest::build(
            &resolved,
            &target,
            &registered,
            &request,
            clock.now() + Duration::seconds(30),
        )
        .expect("exact generated delegation should build");
        let binding = delegation.capability_binding();
        assert_eq!(
            binding.kind(),
            AiRemoteGraphqlCapabilityKind::GeneratedQuery
        );
        assert_eq!(binding.capability_id(), &capability_id);
        assert_eq!(binding.capability_fingerprint(), capability_fingerprint);
        assert_eq!(
            binding.generated_semantic_catalog_fingerprint(),
            Some(semantics.fingerprint.as_str())
        );
        assert_eq!(
            binding.generated_semantic_operation_fingerprint(),
            Some(semantics.operations[0].fingerprint.as_str())
        );
        assert_eq!(binding.generated_root_field(), Some("GeneratedRecord"));
        assert_eq!(
            binding.generated_semantic_fingerprint_algorithm(),
            request
                .contract
                .semantic_operation()
                .map(|semantic| semantic.fingerprint_algorithm())
        );

        let encoded = serde_json::to_value(&delegation)
            .expect("delegation request should serialize without protected content");
        let original_hash = delegation.stable_hash();
        let mut cross_kind = encoded.clone();
        cross_kind["capability_binding"]["kind"] = json!("static_operation");
        assert!(
            serde_json::from_value::<AiRemoteGraphqlDelegationRequest>(cross_kind).is_err(),
            "a generated binding cannot be reinterpreted as a static operation"
        );
        for (field, replacement) in [
            ("operation_kind", json!("mutation")),
            ("capability_id", json!("generated-read.other")),
            (
                "capability_fingerprint",
                json!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            ),
            (
                "compiled_tool_fingerprint",
                json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            ),
            ("target_id", json!("private-other")),
            (
                "finished_schema_fingerprint",
                json!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"),
            ),
            (
                "semantic_fingerprint_algorithm",
                json!("invented-semantic-fingerprint-v1"),
            ),
            (
                "semantic_catalog_fingerprint",
                json!("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"),
            ),
            (
                "semantic_operation_fingerprint",
                json!("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
            ),
            ("root_field", json!("OtherRoot")),
        ] {
            let mut tampered = encoded.clone();
            tampered["capability_binding"][field] = replacement;
            let tampered: AiRemoteGraphqlDelegationRequest = serde_json::from_value(tampered)
                .expect("structurally valid tampered request should decode for rejection");
            assert_ne!(tampered.stable_hash(), original_hash, "{field}");
            assert!(
                !tampered
                    .matches(&target, &registered, &request)
                    .expect("tampered comparison should remain safe")
            );
        }
        let serialized = serde_json::to_string(&delegation)
            .expect("delegation request should serialize for safe audit");
        let debug = format!("{delegation:?}");
        for forbidden in [
            "exact-generated-authority",
            "bearer",
            "roles",
            "scopes",
            "prompt",
            "provider_secret",
            "resolver_data",
        ] {
            assert!(!serialized.contains(forbidden));
            assert!(!debug.contains(forbidden));
        }

        let invented_document =
            "query AiQuery_invented { GeneratedRecord(recordId: \"54\") { recordId } }";
        let invented_contract = GraphqlOperationContract::new(
            target.id.clone(),
            target.schema_fingerprint.clone(),
            "AiQuery_invented",
            invented_document,
            "invented-projection",
            "invented-disclosure",
        )
        .expect("invented static contract should be structurally valid");
        let invented_descriptor = AiToolDescriptor::new(
            "records.invented_dynamic_name",
            "An invented dynamic-looking static operation.",
            AiToolOperationKind::Query,
            invented_document,
            json!({"type": "object", "additionalProperties": false}),
        )
        .expect("invented descriptor should be structurally valid")
        .with_result_projection("invented-projection")
        .with_graphql_contract(invented_contract.clone());
        let invented_request = ToolGraphqlRequest {
            document: invented_document.to_owned(),
            operation_name: "AiQuery_invented".to_owned(),
            contract: invented_contract,
            variables: json!({}),
            invocation: request.invocation,
        };
        let invented_binding = static_binding(&invented_descriptor, &invented_request);
        assert!(matches!(
            adapter
                .build_registered(&resolved, &target, &invented_binding, &invented_request,)
                .await,
            Err(ToolExecutionError::Authorization)
        ));
        assert_eq!(issuer.calls.load(Ordering::SeqCst), 2);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn remote_capability_binding_rejects_non_read_operations_before_issuance() {
        let (principal, target, descriptor, request, issuer, transport, clock) = fixture();
        let adapter = AiRemoteAuthenticatedGraphqlAdapter::new(
            issuer.clone(),
            transport.clone(),
            clock,
            AiRemoteGraphqlExecutionLimits::new(Duration::seconds(30), Duration::seconds(30))
                .expect("limits should validate"),
        );
        let resolved = ResolvedPrincipal::new(
            principal.reference(),
            principal,
            OffsetDateTime::from_unix_timestamp(2_000_000_000).expect("test time should validate"),
        )
        .expect("principal should resolve");
        for operation_kind in [
            AiToolOperationKind::Mutation,
            AiToolOperationKind::Subscription,
        ] {
            let non_read = AiToolDescriptor::new(
                descriptor.id.as_str(),
                descriptor.description.clone(),
                operation_kind,
                descriptor.document.clone(),
                descriptor.argument_schema.clone(),
            )
            .expect("non-read descriptor should remain structurally valid")
            .with_result_projection(descriptor.result_projection.clone())
            .with_graphql_contract(request.contract.clone());
            let binding = static_binding(&non_read, &request);
            assert!(matches!(
                adapter
                    .build_registered(&resolved, &target, &binding, &request)
                    .await,
                Err(ToolExecutionError::StaleContract)
            ));
        }

        let (query_semantics, _) = generated_query_catalog();
        let mutation_semantics = GraphqlSemanticCatalog::compose_with_custom(
            query_semantics.entities.iter().cloned(),
            &GraphqlOperationCatalog::compose(std::iter::empty()),
            [GraphqlSemanticOperationDescriptor::custom(
                GraphqlOperationKind::Mutation,
                "ChangeRecord",
                "Change one reviewed application record.",
                Vec::new(),
                GraphqlSemanticTypeRef::named("Record", GraphqlSemanticTypeKind::Object, false),
                true,
            )
            .expect("test mutation semantics should validate")],
        )
        .expect("test mutation catalogue should validate");
        let mutation_document = "mutation AiMutation_test { ChangeRecord { recordId subject } }";
        let mutation_contract = GraphqlOperationContract::new(
            target.id.clone(),
            target.schema_fingerprint.clone(),
            "AiMutation_test",
            mutation_document,
            "mutation-projection",
            "mutation-disclosure",
        )
        .expect("mutation contract should validate")
        .with_semantic_operation_kind(
            &mutation_semantics,
            GraphqlOperationKind::Mutation,
            "ChangeRecord",
            mutation_document,
        )
        .expect("mutation semantic binding should validate");
        let mutation_descriptor = AiToolDescriptor::new(
            "generated-mutation.test",
            "A generated mutation which remote read delegation must reject.",
            AiToolOperationKind::Mutation,
            mutation_document,
            json!({"type": "object", "additionalProperties": false}),
        )
        .expect("generated mutation descriptor should validate")
        .with_result_projection("mutation-projection")
        .with_graphql_contract(mutation_contract.clone());
        let mutation_request = ToolGraphqlRequest {
            document: mutation_document.to_owned(),
            operation_name: "AiMutation_test".to_owned(),
            contract: mutation_contract,
            variables: json!({}),
            invocation: request.invocation.clone(),
        };
        let mutation_binding = AiRegisteredToolExecutionBinding::generated_mutation(
            &mutation_descriptor.id,
            &"a".repeat(64),
            &mutation_descriptor,
            &mutation_request,
        )
        .expect("generated mutation binding should validate internally");
        assert!(matches!(
            adapter
                .build_registered(&resolved, &target, &mutation_binding, &mutation_request,)
                .await,
            Err(ToolExecutionError::StaleContract)
        ));
        assert_eq!(issuer.calls.load(Ordering::SeqCst), 0);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn swapped_request_is_rejected_before_private_transport() {
        let (principal, target, descriptor, request, issuer, transport, clock) = fixture();
        let adapter = AiRemoteAuthenticatedGraphqlAdapter::new(
            issuer,
            transport.clone(),
            clock,
            AiRemoteGraphqlExecutionLimits::new(Duration::seconds(30), Duration::seconds(30))
                .expect("limits should validate"),
        );
        let resolved = ResolvedPrincipal::new(
            principal.reference(),
            principal,
            OffsetDateTime::from_unix_timestamp(2_000_000_000).expect("test time should validate"),
        )
        .expect("principal should resolve");
        let context = adapter
            .build_registered(
                &resolved,
                &target,
                &static_binding(&descriptor, &request),
                &request,
            )
            .await
            .expect("context should build");
        let mut swapped = request;
        swapped.variables = json!({"recordId": "55"});
        assert!(matches!(
            adapter.execute(context, swapped).await,
            Err(ToolExecutionError::Authorization)
        ));
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn expired_authority_is_rejected_before_private_transport() {
        let (principal, target, descriptor, request, issuer, transport, clock) = fixture();
        let adapter = AiRemoteAuthenticatedGraphqlAdapter::new(
            issuer,
            transport.clone(),
            clock.clone(),
            AiRemoteGraphqlExecutionLimits::new(Duration::seconds(30), Duration::seconds(30))
                .expect("limits should validate"),
        );
        let resolved = ResolvedPrincipal::new(principal.reference(), principal, clock.now())
            .expect("principal should resolve");
        let context = adapter
            .build_registered(
                &resolved,
                &target,
                &static_binding(&descriptor, &request),
                &request,
            )
            .await
            .expect("context should build");

        clock.advance_seconds(30);

        assert!(matches!(
            adapter.execute(context, request).await,
            Err(ToolExecutionError::Authorization)
        ));
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn context_from_another_adapter_is_rejected() {
        let (principal, target, descriptor, request, issuer, transport, clock) = fixture();
        let limits =
            AiRemoteGraphqlExecutionLimits::new(Duration::seconds(30), Duration::seconds(30))
                .expect("limits should validate");
        let first = AiRemoteAuthenticatedGraphqlAdapter::new(
            issuer.clone(),
            transport.clone(),
            clock.clone(),
            limits,
        );
        let second = AiRemoteAuthenticatedGraphqlAdapter::new(
            issuer,
            transport.clone(),
            clock.clone(),
            limits,
        );
        let resolved = ResolvedPrincipal::new(principal.reference(), principal, clock.now())
            .expect("principal should resolve");
        let context = first
            .build_registered(
                &resolved,
                &target,
                &static_binding(&descriptor, &request),
                &request,
            )
            .await
            .expect("context should build");

        assert!(matches!(
            second.execute(context, request).await,
            Err(ToolExecutionError::Authorization)
        ));
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn stale_principal_and_recursive_document_fail_before_issuance() {
        let (principal, target, descriptor, mut request, issuer, transport, clock) = fixture();
        let adapter = AiRemoteAuthenticatedGraphqlAdapter::new(
            issuer.clone(),
            transport,
            clock.clone(),
            AiRemoteGraphqlExecutionLimits::new(Duration::seconds(30), Duration::seconds(10))
                .expect("limits should validate"),
        );
        let stale = ResolvedPrincipal::new(
            principal.reference(),
            principal.clone(),
            clock.now() - Duration::seconds(11),
        )
        .expect("principal should resolve");
        assert!(matches!(
            adapter
                .build_registered(
                    &stale,
                    &target,
                    &static_binding(&descriptor, &request),
                    &request,
                )
                .await,
            Err(ToolExecutionError::Reauthorization)
        ));

        let binding = static_binding(&descriptor, &request);
        let recursive_document = "query InspectAi { __schema { queryType { name } } }";
        request.document = recursive_document.to_owned();
        request.contract = GraphqlOperationContract::new(
            target.id.clone(),
            target.schema_fingerprint.clone(),
            "InspectAi",
            recursive_document,
            "projection-v1",
            "disclosure-v1",
        )
        .expect("contract should validate");
        request.operation_name = "InspectAi".to_owned();
        let current = ResolvedPrincipal::new(principal.reference(), principal, clock.now())
            .expect("principal should resolve");
        assert!(matches!(
            adapter
                .build_registered(&current, &target, &binding, &request,)
                .await,
            Err(ToolExecutionError::InvalidTarget)
        ));
        assert_eq!(issuer.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn issuance_cannot_cross_the_principal_freshness_boundary() {
        let (principal, target, descriptor, request, _issuer, transport, clock) = fixture();
        let adapter = AiRemoteAuthenticatedGraphqlAdapter::new(
            Arc::new(IssuerCrossingFreshnessBoundary {
                clock: clock.clone(),
            }),
            transport.clone(),
            clock.clone(),
            AiRemoteGraphqlExecutionLimits::new(Duration::seconds(30), Duration::seconds(10))
                .expect("limits should validate"),
        );
        let resolved = ResolvedPrincipal::new(principal.reference(), principal, clock.now())
            .expect("principal should resolve");

        assert!(matches!(
            adapter
                .build_registered(
                    &resolved,
                    &target,
                    &static_binding(&descriptor, &request),
                    &request,
                )
                .await,
            Err(ToolExecutionError::Authorization)
        ));
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    }
}
