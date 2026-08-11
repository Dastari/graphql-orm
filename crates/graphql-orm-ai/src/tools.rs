//! Runtime registration and current-principal policy for canonical AI tools.

use std::collections::BTreeMap;

use agql_auth::ResolvedPrincipal;
use async_trait::async_trait;
use graphql_orm::graphql::orm::{GraphqlOperationCatalog, GraphqlOperationKind};
use serde::{Deserialize, Serialize};

#[cfg(any(feature = "sqlite", feature = "postgres"))]
use crate::ModelToolDefinition;
use crate::{
    AiApprovalRule, AiDisclosureSchema, AiError, AiGeneratedGraphqlOperationPolicy,
    AiGraphqlToolManifestCatalog, AiScope, AiToolDescriptor, AiToolId, AiToolOperationDomain,
    AiToolOperationKind, AiToolRisk, ToolGraphqlRequest, ToolMaturity,
    contains_forbidden_graphql_name,
};

const JSON_SCHEMA_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

/// Registered tool catalog. Registration does not enable tools.
#[derive(Clone, Debug, Default)]
pub struct AiToolCatalog {
    tools: BTreeMap<AiToolId, RegisteredAiTool>,
}

#[derive(Clone, Debug)]
struct RegisteredAiTool {
    descriptor: AiToolDescriptor,
    disclosure_schema: Option<AiDisclosureSchema>,
}

impl AiToolCatalog {
    /// Creates an empty catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a descriptor without exposing it.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::AlreadyExists`] for duplicate stable IDs.
    pub fn register(&mut self, descriptor: AiToolDescriptor) -> Result<(), AiError> {
        if descriptor.operation_kind != AiToolOperationKind::Internal {
            return Err(AiError::InvalidConfiguration(
                "application tools require a static disclosure schema".to_owned(),
            ));
        }
        self.register_validated(descriptor, None)
    }

    /// Registers a GraphQL tool with its exact static disclosure schema.
    /// Registration remains discovery only and does not enable the tool.
    ///
    /// # Errors
    ///
    /// Returns a safe error for duplicate IDs, forbidden operation domains,
    /// introspection/AI-control-plane documents, or stale contract bindings.
    pub fn register_with_disclosure(
        &mut self,
        descriptor: AiToolDescriptor,
        disclosure_schema: AiDisclosureSchema,
    ) -> Result<(), AiError> {
        if descriptor
            .graphql_contract
            .as_ref()
            .is_some_and(|contract| contract.generated_operation().is_some())
        {
            return Err(AiError::InvalidConfiguration(
                "generated operation bindings require catalog revalidation".to_owned(),
            ));
        }
        self.register_disclosed(descriptor, disclosure_schema)
    }

    /// Registers a generated GraphQL resolver after exact catalog and host
    /// domain revalidation.
    ///
    /// The contract must have been created with
    /// [`GraphqlOperationContract::with_generated_operation`]. This method
    /// re-resolves the current exposed catalog coordinate, verifies the
    /// catalog and operation fingerprints, proves that the server-authored
    /// document contains only that root field, and asks the host to classify
    /// the metadata as an application operation. Registration and metadata
    /// discovery still do not enable the tool or authorize resolver access.
    ///
    /// # Errors
    ///
    /// Returns a safe configuration error for a stale/hidden/ambiguous
    /// generated resolver, document or operation-kind drift, a denied host
    /// classification, subscriptions, or any ordinary disclosure/catalog
    /// validation failure.
    pub fn register_generated_with_disclosure(
        &mut self,
        descriptor: AiToolDescriptor,
        disclosure_schema: AiDisclosureSchema,
        operation_catalog: &GraphqlOperationCatalog,
        operation_policy: &dyn AiGeneratedGraphqlOperationPolicy,
    ) -> Result<(), AiError> {
        let contract = descriptor.graphql_contract.as_ref().ok_or_else(|| {
            AiError::InvalidConfiguration(
                "generated tools require an exact GraphQL operation contract".to_owned(),
            )
        })?;
        let operation = contract
            .resolve_generated_operation(operation_catalog, &descriptor.document)
            .map_err(|_| {
                AiError::InvalidConfiguration(
                    "generated GraphQL operation contract is stale".to_owned(),
                )
            })?;
        let kind_matches = matches!(
            (descriptor.operation_kind, operation.kind()),
            (AiToolOperationKind::Query, GraphqlOperationKind::Query)
                | (
                    AiToolOperationKind::Mutation,
                    GraphqlOperationKind::Mutation
                )
        );
        if descriptor.operation_domain != AiToolOperationDomain::Application
            || !kind_matches
            || !operation_policy.is_application_operation(operation)
        {
            return Err(AiError::InvalidConfiguration(
                "generated resolver is not an admitted application operation".to_owned(),
            ));
        }
        self.register_disclosed(descriptor, disclosure_schema)
    }

    fn register_disclosed(
        &mut self,
        descriptor: AiToolDescriptor,
        disclosure_schema: AiDisclosureSchema,
    ) -> Result<(), AiError> {
        if descriptor.operation_kind == AiToolOperationKind::Internal {
            return Err(AiError::InvalidConfiguration(
                "internal tools do not accept GraphQL disclosure schemas".to_owned(),
            ));
        }
        let contract = descriptor.graphql_contract.as_ref().ok_or_else(|| {
            AiError::InvalidConfiguration(
                "application tools require an exact GraphQL operation contract".to_owned(),
            )
        })?;
        if contract.disclosure_schema_fingerprint != disclosure_schema.fingerprint
            || contract.operation_name.trim().is_empty()
            || descriptor.result_projection.trim().is_empty()
            || disclosure_schema.maximum_list_bound() > descriptor.maximum_result_records
        {
            return Err(AiError::InvalidConfiguration(
                "tool disclosure or projection contract is stale".to_owned(),
            ));
        }
        self.register_validated(descriptor, Some(disclosure_schema))
    }

    pub(crate) fn register_compiled_manifest_entry(
        &mut self,
        descriptor: AiToolDescriptor,
        disclosure_schema: AiDisclosureSchema,
    ) -> Result<(), AiError> {
        self.register_disclosed(descriptor, disclosure_schema)
    }

    fn register_validated(
        &mut self,
        descriptor: AiToolDescriptor,
        disclosure_schema: Option<AiDisclosureSchema>,
    ) -> Result<(), AiError> {
        if !descriptor.has_valid_fingerprint()
            || descriptor
                .argument_schema
                .get("$schema")
                .and_then(serde_json::Value::as_str)
                != Some(JSON_SCHEMA_2020_12)
            || jsonschema::validator_for(&descriptor.argument_schema).is_err()
        {
            return Err(AiError::InvalidConfiguration(
                "tool arguments must use a valid JSON Schema 2020-12 contract".to_owned(),
            ));
        }
        if matches!(
            descriptor.operation_domain,
            AiToolOperationDomain::AiControlPlane | AiToolOperationDomain::SchemaIntrospection
        ) || contains_forbidden_graphql_name(&descriptor.document)
        {
            return Err(AiError::InvalidConfiguration(
                "AI control-plane and introspection operations cannot be tools".to_owned(),
            ));
        }
        if self.tools.contains_key(&descriptor.id) {
            return Err(AiError::AlreadyExists(descriptor.id.as_str().to_owned()));
        }
        self.tools.insert(
            descriptor.id.clone(),
            RegisteredAiTool {
                descriptor,
                disclosure_schema,
            },
        );
        Ok(())
    }

    /// Returns a descriptor by ID. This is discovery, not authorization.
    pub fn descriptor(&self, id: &AiToolId) -> Option<&AiToolDescriptor> {
        self.tools.get(id).map(|tool| &tool.descriptor)
    }

    /// Returns the static disclosure schema for a registered GraphQL tool.
    pub fn disclosure_schema(&self, id: &AiToolId) -> Option<&AiDisclosureSchema> {
        self.tools
            .get(id)
            .and_then(|tool| tool.disclosure_schema.as_ref())
    }

    /// Returns all registered descriptors.
    pub fn descriptors(&self) -> impl Iterator<Item = &AiToolDescriptor> {
        self.tools.values().map(|tool| &tool.descriptor)
    }

    pub(crate) fn validate_execution_request(
        &self,
        id: &AiToolId,
        request: &ToolGraphqlRequest,
        maximum_maturity: ToolMaturity,
    ) -> Result<(&AiToolDescriptor, &AiDisclosureSchema), AiError> {
        let registered = self.tools.get(id).ok_or(AiError::Forbidden)?;
        let descriptor = &registered.descriptor;
        let disclosure = registered
            .disclosure_schema
            .as_ref()
            .ok_or(AiError::Forbidden)?;
        if descriptor.maturity > maximum_maturity
            || descriptor.operation_kind == AiToolOperationKind::Internal
            || descriptor.document != request.document
            || descriptor.graphql_contract.as_ref() != Some(&request.contract)
            || request.operation_name != request.contract.operation_name
            || descriptor.result_projection != request.contract.result_projection_fingerprint
            || disclosure.fingerprint != request.contract.disclosure_schema_fingerprint
        {
            return Err(AiError::Forbidden);
        }
        let validator = jsonschema::validator_for(&descriptor.argument_schema).map_err(|_| {
            AiError::InvalidConfiguration("registered tool argument schema is invalid".to_owned())
        })?;
        if !validator.is_valid(&request.variables) {
            return Err(AiError::InvalidInput(
                "tool arguments do not match the registered schema".to_owned(),
            ));
        }
        Ok((descriptor, disclosure))
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn validate_read_only_model_definition(
        &self,
        definition: &ModelToolDefinition,
        policy: &AiToolPolicySet,
    ) -> Result<(), AiError> {
        let id = AiToolId::parse(definition.tool_id.clone())?;
        let descriptor = self.descriptor(&id).ok_or(AiError::Forbidden)?;
        if !policy.allows(descriptor)
            || descriptor.operation_kind != AiToolOperationKind::Query
            || descriptor.operation_domain != AiToolOperationDomain::Application
            || descriptor.maturity != ToolMaturity::ReadOnly
            || descriptor.risk != AiToolRisk::ReadOnly
            || descriptor.approval != AiApprovalRule::None
            || !descriptor.idempotent
            || definition.fingerprint != descriptor.fingerprint
            || definition.description != descriptor.description
            || definition.parameters != descriptor.argument_schema
        {
            return Err(AiError::Forbidden);
        }
        Ok(())
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn validate_supervised_model_definition(
        &self,
        definition: &ModelToolDefinition,
        policy: &AiToolPolicySet,
    ) -> Result<(), AiError> {
        let id = AiToolId::parse(definition.tool_id.clone())?;
        let descriptor = self.descriptor(&id).ok_or(AiError::Forbidden)?;
        let safe_read = descriptor.operation_kind == AiToolOperationKind::Query
            && descriptor.operation_domain == AiToolOperationDomain::Application
            && descriptor.maturity == ToolMaturity::ReadOnly
            && descriptor.risk == AiToolRisk::ReadOnly
            && descriptor.approval == AiApprovalRule::None
            && descriptor.idempotent;
        let supervised_write = descriptor.operation_kind == AiToolOperationKind::Mutation
            && descriptor.operation_domain == AiToolOperationDomain::Application
            && descriptor.maturity == ToolMaturity::SupervisedWrite
            && matches!(
                descriptor.risk,
                AiToolRisk::LowRiskWrite | AiToolRisk::NonIdempotentWrite | AiToolRisk::HighImpact
            )
            && descriptor.approval == AiApprovalRule::OneShot;
        if !policy.allows(descriptor)
            || !(safe_read || supervised_write)
            || definition.fingerprint != descriptor.fingerprint
            || definition.description != descriptor.description
            || definition.parameters != descriptor.argument_schema
        {
            return Err(AiError::Forbidden);
        }
        Ok(())
    }
}

impl AiGraphqlToolManifestCatalog for AiToolCatalog {
    fn register_generated_manifest_entry(
        &mut self,
        descriptor: AiToolDescriptor,
        disclosure_schema: AiDisclosureSchema,
        operation_catalog: &GraphqlOperationCatalog,
        operation_policy: &dyn AiGeneratedGraphqlOperationPolicy,
    ) -> Result<(), AiError> {
        self.register_generated_with_disclosure(
            descriptor,
            disclosure_schema,
            operation_catalog,
            operation_policy,
        )
    }

    fn register_custom_manifest_entry(
        &mut self,
        descriptor: AiToolDescriptor,
        disclosure_schema: AiDisclosureSchema,
    ) -> Result<(), AiError> {
        self.register_with_disclosure(descriptor, disclosure_schema)
    }

    fn register_aggregated_manifest_entry(
        &mut self,
        descriptor: AiToolDescriptor,
        disclosure_schema: AiDisclosureSchema,
    ) -> Result<(), AiError> {
        self.register_compiled_manifest_entry(descriptor, disclosure_schema)
    }
}

/// Persisted policy binding for one exact descriptor fingerprint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiToolPolicyBinding {
    /// Stable tool ID.
    pub tool_id: AiToolId,
    /// Reviewed descriptor fingerprint.
    pub fingerprint: String,
    /// Explicit enablement.
    pub enabled: bool,
}

/// Current host authorization outcome for one exact registered tool request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiToolAuthorizationDecision {
    allowed: bool,
    /// Stable non-sensitive reason code for audit and diagnostics.
    pub reason_code: String,
    /// Current host policy version used for the decision.
    pub policy_version: String,
    /// Current safe authorization-state digest used by approval workflows.
    pub authorization_state_digest: String,
}

impl AiToolAuthorizationDecision {
    /// Creates an allowed current-principal decision.
    pub fn allow(
        reason_code: impl Into<String>,
        policy_version: impl Into<String>,
        authorization_state_digest: impl Into<String>,
    ) -> Self {
        Self {
            allowed: true,
            reason_code: reason_code.into(),
            policy_version: policy_version.into(),
            authorization_state_digest: authorization_state_digest.into(),
        }
    }

    /// Creates a denied current-principal decision.
    pub fn deny(reason_code: impl Into<String>, policy_version: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason_code: reason_code.into(),
            policy_version: policy_version.into(),
            authorization_state_digest: String::new(),
        }
    }

    /// Returns whether current host policy allowed this exact request.
    pub const fn is_allowed(&self) -> bool {
        self.allowed
    }

    pub(crate) fn is_complete_allow(&self) -> bool {
        self.allowed
            && !self.reason_code.trim().is_empty()
            && !self.policy_version.trim().is_empty()
            && !self.authorization_state_digest.trim().is_empty()
    }
}

/// Fresh, principal-aware host policy for registered application tool calls.
///
/// This is evaluated after principal rehydration for every execution. The
/// ordinary resolver authorization path still runs afterward and remains
/// authoritative.
#[async_trait]
pub trait AiToolAuthorizationPolicy: Send + Sync {
    /// Authorizes the exact registered descriptor, scope, and validated
    /// variables using the freshly resolved principal.
    async fn authorize(
        &self,
        principal: &ResolvedPrincipal,
        scope: &AiScope,
        descriptor: &AiToolDescriptor,
        variables: &serde_json::Value,
    ) -> AiToolAuthorizationDecision;
}

/// Fail-closed tool policy suitable as an explicit disabled implementation.
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyAllAiToolAuthorizationPolicy;

#[async_trait]
impl AiToolAuthorizationPolicy for DenyAllAiToolAuthorizationPolicy {
    async fn authorize(
        &self,
        _principal: &ResolvedPrincipal,
        _scope: &AiScope,
        _descriptor: &AiToolDescriptor,
        _variables: &serde_json::Value,
    ) -> AiToolAuthorizationDecision {
        AiToolAuthorizationDecision::deny("default_deny", "deny-all")
    }
}

/// Scope tool policy. Absence always means disabled.
#[derive(Clone, Debug)]
pub struct AiToolPolicySet {
    maximum_maturity: ToolMaturity,
    bindings: BTreeMap<AiToolId, AiToolPolicyBinding>,
}

impl AiToolPolicySet {
    /// Creates an empty, default-deny policy with a deployment/scope maturity
    /// ceiling.
    pub fn new(maximum_maturity: ToolMaturity) -> Self {
        Self {
            maximum_maturity,
            bindings: BTreeMap::new(),
        }
    }

    /// Adds/replaces an explicit policy binding.
    pub fn bind(&mut self, binding: AiToolPolicyBinding) {
        self.bindings.insert(binding.tool_id.clone(), binding);
    }

    /// Returns whether the exact current descriptor is enabled within the
    /// maturity ceiling.
    pub fn allows(&self, descriptor: &AiToolDescriptor) -> bool {
        descriptor.maturity <= self.maximum_maturity
            && self.bindings.get(&descriptor.id).is_some_and(|binding| {
                binding.enabled && binding.fingerprint == descriptor.fingerprint
            })
    }
}
