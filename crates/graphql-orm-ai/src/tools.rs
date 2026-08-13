//! Runtime registration and current-principal policy for canonical AI tools.

use std::collections::BTreeMap;
use std::sync::Arc;

use agql_auth::ResolvedPrincipal;
use async_trait::async_trait;
use graphql_orm::graphql::orm::{
    AiMutationExecutionPolicy, GraphqlOperationCatalog, GraphqlOperationKind,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{AiApprovalRule, AiToolRisk, ModelToolDefinition};
use crate::{
    AiCompiledGraphqlMutation, AiCompiledGraphqlQuery, AiCompiledGraphqlSubscription,
    AiDisclosureSchema, AiError, AiGeneratedGraphqlOperationPolicy, AiGraphqlMutationCapability,
    AiGraphqlMutationCapabilityCatalog, AiGraphqlQueryCapability, AiGraphqlQueryCapabilityCatalog,
    AiGraphqlSubscriptionCapability, AiGraphqlSubscriptionCapabilityCatalog,
    AiGraphqlToolManifestCatalog, AiScope, AiToolDescriptor, AiToolId, AiToolOperationDomain,
    AiToolOperationKind, DataClassification, GraphqlExecutionTargetId, ToolGraphqlRequest,
    ToolMaturity, contains_forbidden_graphql_name,
};

const JSON_SCHEMA_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

/// Registered tool catalog. Registration does not enable tools.
#[derive(Clone, Debug, Default)]
pub struct AiToolCatalog {
    tools: BTreeMap<AiToolId, RegisteredAiTool>,
    query_capabilities: BTreeMap<AiToolId, AiGraphqlQueryCapability>,
    mutation_capabilities: BTreeMap<AiToolId, AiGraphqlMutationCapability>,
    subscription_capabilities: BTreeMap<AiToolId, AiGraphqlSubscriptionCapability>,
}

#[derive(Clone, Debug)]
struct RegisteredAiTool {
    descriptor: AiToolDescriptor,
    disclosure_schema: Option<AiDisclosureSchema>,
}

/// Explicit deployment policy for generated GraphQL capabilities on one
/// exact logical target and active schema/semantic graph.
///
/// This binding is not resolver authority. It only admits capability classes
/// to the ordinary fresh-principal, exact-operation, resolver, and disclosure
/// checks. Every capability class is disabled until explicitly enabled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiGeneratedGraphqlTargetPolicyBinding {
    target_id: GraphqlExecutionTargetId,
    finished_schema_fingerprint: String,
    semantic_catalog_fingerprint: String,
    queries: bool,
    automatic_mutations: bool,
    approval_required_mutations: bool,
    replayable_subscriptions: bool,
}

impl AiGeneratedGraphqlTargetPolicyBinding {
    /// Creates a default-deny binding for one exact active target contract.
    ///
    /// # Errors
    ///
    /// Returns an error unless both fingerprints are lowercase SHA-256 hex.
    pub fn new(
        target_id: GraphqlExecutionTargetId,
        finished_schema_fingerprint: impl Into<String>,
        semantic_catalog_fingerprint: impl Into<String>,
    ) -> Result<Self, AiError> {
        let binding = Self {
            target_id,
            finished_schema_fingerprint: finished_schema_fingerprint.into(),
            semantic_catalog_fingerprint: semantic_catalog_fingerprint.into(),
            queries: false,
            automatic_mutations: false,
            approval_required_mutations: false,
            replayable_subscriptions: false,
        };
        binding.validate()?;
        Ok(binding)
    }

    /// Explicitly admits generated read capabilities for this exact binding.
    pub fn allow_queries(mut self) -> Self {
        self.queries = true;
        self
    }

    /// Explicitly admits classified automatic mutations for this exact binding.
    pub fn allow_automatic_mutations(mut self) -> Self {
        self.automatic_mutations = true;
        self
    }

    /// Explicitly admits classified one-shot approval mutations for this binding.
    pub fn allow_approval_required_mutations(mut self) -> Self {
        self.approval_required_mutations = true;
        self
    }

    /// Explicitly admits bounded replayable subscriptions for this binding.
    pub fn allow_replayable_subscriptions(mut self) -> Self {
        self.replayable_subscriptions = true;
        self
    }

    /// Returns the exact logical target.
    pub fn target_id(&self) -> &GraphqlExecutionTargetId {
        &self.target_id
    }

    /// Returns the exact active finished-SDL fingerprint.
    pub fn finished_schema_fingerprint(&self) -> &str {
        &self.finished_schema_fingerprint
    }

    /// Returns the exact active semantic-catalogue fingerprint.
    pub fn semantic_catalog_fingerprint(&self) -> &str {
        &self.semantic_catalog_fingerprint
    }

    fn validate(&self) -> Result<(), AiError> {
        if !is_sha256(&self.finished_schema_fingerprint)
            || !is_sha256(&self.semantic_catalog_fingerprint)
        {
            return Err(AiError::InvalidConfiguration(
                "generated GraphQL target fingerprints are invalid".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Default-deny deployment policy for generated GraphQL capabilities.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AiGeneratedGraphqlTargetPolicySet(
    BTreeMap<GraphqlExecutionTargetId, AiGeneratedGraphqlTargetPolicyBinding>,
);

impl AiGeneratedGraphqlTargetPolicySet {
    /// Creates an empty default-deny target policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one exact target binding without replacing existing policy.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed fingerprints or a duplicate target.
    pub fn bind(&mut self, binding: AiGeneratedGraphqlTargetPolicyBinding) -> Result<(), AiError> {
        binding.validate()?;
        if self.0.contains_key(binding.target_id()) {
            return Err(AiError::AlreadyExists(
                binding.target_id().as_str().to_owned(),
            ));
        }
        self.0.insert(binding.target_id.clone(), binding);
        Ok(())
    }

    pub(crate) fn allows_query(&self, descriptor: &AiToolDescriptor) -> bool {
        self.matching(descriptor).is_some_and(|binding| {
            binding.queries
                && descriptor.operation_kind == AiToolOperationKind::Query
                && descriptor.maturity == ToolMaturity::ReadOnly
                && descriptor.risk == AiToolRisk::ReadOnly
                && descriptor.approval == AiApprovalRule::None
                && descriptor.idempotent
        })
    }

    pub(crate) fn allows_query_capability(&self, capability: &AiGraphqlQueryCapability) -> bool {
        self.0.get(capability.target_id()).is_some_and(|binding| {
            binding.queries
                && binding.finished_schema_fingerprint == capability.finished_schema_fingerprint()
                && binding.semantic_catalog_fingerprint == capability.semantic_catalog_fingerprint()
        })
    }

    pub(crate) fn allows_mutation(
        &self,
        descriptor: &AiToolDescriptor,
        policy: AiMutationExecutionPolicy,
    ) -> bool {
        self.matching(descriptor).is_some_and(|binding| {
            descriptor.operation_kind == AiToolOperationKind::Mutation
                && !descriptor.idempotent
                && descriptor.risk == AiToolRisk::NonIdempotentWrite
                && match policy {
                    AiMutationExecutionPolicy::Automatic => {
                        binding.automatic_mutations
                            && descriptor.maturity == ToolMaturity::AutonomousWrite
                            && descriptor.approval == AiApprovalRule::None
                    }
                    AiMutationExecutionPolicy::ApprovalRequired => {
                        binding.approval_required_mutations
                            && descriptor.maturity == ToolMaturity::SupervisedWrite
                            && descriptor.approval == AiApprovalRule::OneShot
                    }
                    AiMutationExecutionPolicy::Prohibited => false,
                }
        })
    }

    pub(crate) fn allows_mutation_capability(
        &self,
        capability: &AiGraphqlMutationCapability,
    ) -> bool {
        self.0.get(capability.target_id()).is_some_and(|binding| {
            binding.finished_schema_fingerprint == capability.finished_schema_fingerprint()
                && binding.semantic_catalog_fingerprint == capability.semantic_catalog_fingerprint()
                && match capability.execution_policy() {
                    AiMutationExecutionPolicy::Automatic => binding.automatic_mutations,
                    AiMutationExecutionPolicy::ApprovalRequired => {
                        binding.approval_required_mutations
                    }
                    AiMutationExecutionPolicy::Prohibited => false,
                }
        })
    }

    pub(crate) fn allows_subscription(&self, descriptor: &AiToolDescriptor) -> bool {
        self.matching(descriptor).is_some_and(|binding| {
            binding.replayable_subscriptions
                && descriptor.operation_kind == AiToolOperationKind::Subscription
                && descriptor.maturity == ToolMaturity::ReadOnly
                && descriptor.risk == AiToolRisk::ReadOnly
                && descriptor.approval == AiApprovalRule::None
                && descriptor.idempotent
        })
    }

    fn matching(
        &self,
        descriptor: &AiToolDescriptor,
    ) -> Option<&AiGeneratedGraphqlTargetPolicyBinding> {
        let contract = descriptor.graphql_contract.as_ref()?;
        let semantic = contract.semantic_operation()?;
        let binding = self.0.get(&contract.target_id)?;
        (binding.finished_schema_fingerprint == contract.schema_fingerprint
            && binding.semantic_catalog_fingerprint == semantic.catalog_fingerprint())
        .then_some(binding)
    }

    fn allows_generated_descriptor(&self, descriptor: &AiToolDescriptor) -> bool {
        self.allows_query(descriptor)
            || self.allows_mutation(descriptor, AiMutationExecutionPolicy::Automatic)
            || self.allows_mutation(descriptor, AiMutationExecutionPolicy::ApprovalRequired)
            || self.allows_subscription(descriptor)
    }

    /// Returns a deterministic fingerprint of the complete exact target policy.
    pub fn fingerprint(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"generated-graphql-target-policy-v1\0");
        for (target, binding) in &self.0 {
            for value in [
                target.as_str(),
                &binding.finished_schema_fingerprint,
                &binding.semantic_catalog_fingerprint,
            ] {
                digest.update(value.len().to_be_bytes());
                digest.update(value.as_bytes());
            }
            digest.update([
                u8::from(binding.queries),
                u8::from(binding.automatic_mutations),
                u8::from(binding.approval_required_mutations),
                u8::from(binding.replayable_subscriptions),
            ]);
        }
        hex::encode(digest.finalize())
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// Fresh-principal authorization adapter for exact generated GraphQL targets.
///
/// Generated semantic descriptors matching the target policy are admitted
/// without a per-capability ID list; static tools delegate to the host's
/// existing policy. A generated descriptor that is absent or stale in the
/// exact target policy is denied and cannot fall through to static policy.
#[derive(Clone)]
pub struct AiGeneratedGraphqlAuthorizationPolicy {
    targets: AiGeneratedGraphqlTargetPolicySet,
    static_tools: Arc<dyn AiToolAuthorizationPolicy>,
}

impl std::fmt::Debug for AiGeneratedGraphqlAuthorizationPolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiGeneratedGraphqlAuthorizationPolicy")
            .field("target_policy_fingerprint", &self.targets.fingerprint())
            .finish_non_exhaustive()
    }
}

impl AiGeneratedGraphqlAuthorizationPolicy {
    /// Creates a generated-target adapter with an independent static-tool fallback.
    pub fn new(
        targets: AiGeneratedGraphqlTargetPolicySet,
        static_tools: Arc<dyn AiToolAuthorizationPolicy>,
    ) -> Self {
        Self {
            targets,
            static_tools,
        }
    }

    /// Creates an adapter that denies all static application tools.
    pub fn generated_only(targets: AiGeneratedGraphqlTargetPolicySet) -> Self {
        Self::new(targets, Arc::new(DenyAllAiToolAuthorizationPolicy))
    }
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

    /// Registers a complete automatic query-capability set for discovery.
    ///
    /// Registration is all-or-nothing and grants no execution authority. The
    /// host must separately install current-principal, target, resolver, and
    /// disclosure policy before a compiled query can execute.
    ///
    /// # Errors
    ///
    /// Returns an error for a stable-ID collision with a static tool or
    /// previously registered automatic query capability.
    pub fn register_query_capability_catalog(
        &mut self,
        catalog: &AiGraphqlQueryCapabilityCatalog,
    ) -> Result<(), AiError> {
        for capability in catalog.capabilities() {
            if self.tools.contains_key(capability.id())
                || self.query_capabilities.contains_key(capability.id())
                || self.mutation_capabilities.contains_key(capability.id())
                || self.subscription_capabilities.contains_key(capability.id())
            {
                return Err(AiError::AlreadyExists(capability.id().as_str().to_owned()));
            }
        }
        self.query_capabilities.extend(
            catalog
                .capabilities()
                .map(|capability| (capability.id().clone(), capability.clone())),
        );
        Ok(())
    }

    /// Registers a complete classified mutation-capability set for discovery.
    ///
    /// Registration grants neither execution authority nor approval. The
    /// runtime must separately admit the exact target/schema/catalogue binding
    /// and apply the classified automatic or one-shot supervised path.
    ///
    /// # Errors
    ///
    /// Returns an error for a stable-ID collision with any registered tool or
    /// generated capability.
    pub fn register_mutation_capability_catalog(
        &mut self,
        catalog: &AiGraphqlMutationCapabilityCatalog,
    ) -> Result<(), AiError> {
        for capability in catalog.capabilities() {
            if self.tools.contains_key(capability.id())
                || self.query_capabilities.contains_key(capability.id())
                || self.mutation_capabilities.contains_key(capability.id())
                || self.subscription_capabilities.contains_key(capability.id())
            {
                return Err(AiError::AlreadyExists(capability.id().as_str().to_owned()));
            }
        }
        self.mutation_capabilities.extend(
            catalog
                .capabilities()
                .map(|capability| (capability.id().clone(), capability.clone())),
        );
        Ok(())
    }

    /// Registers a complete replayable subscription-capability set.
    ///
    /// Registration publishes only a closed typed plan contract. It neither
    /// registers a durable source nor grants execution or disclosure authority.
    ///
    /// # Errors
    ///
    /// Returns an error for a stable-ID collision with any registered tool or
    /// generated capability.
    pub fn register_subscription_capability_catalog(
        &mut self,
        catalog: &AiGraphqlSubscriptionCapabilityCatalog,
    ) -> Result<(), AiError> {
        for capability in catalog.capabilities() {
            if self.tools.contains_key(capability.id())
                || self.query_capabilities.contains_key(capability.id())
                || self.mutation_capabilities.contains_key(capability.id())
                || self.subscription_capabilities.contains_key(capability.id())
            {
                return Err(AiError::AlreadyExists(capability.id().as_str().to_owned()));
            }
        }
        self.subscription_capabilities.extend(
            catalog
                .capabilities()
                .map(|capability| (capability.id().clone(), capability.clone())),
        );
        Ok(())
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
    /// [`crate::GraphqlOperationContract::with_generated_operation`]. This method
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
            || disclosure_schema
                .maximum_graphql_record_count()
                .is_none_or(|records| records > u64::from(descriptor.maximum_result_records))
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
            || descriptor.browser_result_preview.is_some_and(|preview| {
                preview.maximum_bytes > descriptor.maximum_result_bytes
                    || preview.maximum_records > descriptor.maximum_result_records
                    || preview.maximum_classification > descriptor.maximum_classification
                    || preview.maximum_classification == DataClassification::Secret
            })
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
        if self.tools.contains_key(&descriptor.id)
            || self.query_capabilities.contains_key(&descriptor.id)
            || self.mutation_capabilities.contains_key(&descriptor.id)
            || self.subscription_capabilities.contains_key(&descriptor.id)
        {
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

    /// Returns all automatic query capabilities ordered by stable ID.
    ///
    /// This is discovery only and does not imply current authority.
    pub fn query_capabilities(&self) -> impl Iterator<Item = &AiGraphqlQueryCapability> {
        self.query_capabilities.values()
    }

    /// Returns classified mutation capabilities ordered by stable ID.
    ///
    /// This is discovery only and grants neither authority nor approval.
    pub fn mutation_capabilities(&self) -> impl Iterator<Item = &AiGraphqlMutationCapability> {
        self.mutation_capabilities.values()
    }

    /// Returns one exact classified mutation capability for revalidation.
    ///
    /// This remains discovery metadata and does not grant execution authority.
    pub fn mutation_capability(&self, id: &AiToolId) -> Option<&AiGraphqlMutationCapability> {
        self.mutation_capabilities.get(id)
    }

    /// Returns all registered replayable subscription capabilities in stable
    /// ID order. Discovery grants no source or user authority.
    pub fn subscription_capabilities(
        &self,
    ) -> impl Iterator<Item = &AiGraphqlSubscriptionCapability> {
        self.subscription_capabilities.values()
    }

    /// Projects one registered automatic query capability for a provider.
    ///
    /// The provider receives a closed typed plan schema. It never receives a
    /// GraphQL document, target, credential, or authorization rule.
    ///
    /// # Errors
    ///
    /// Returns a safe error for an absent capability or malformed provider
    /// alias.
    pub fn query_capability_model_definition(
        &self,
        id: &AiToolId,
        provider_name: impl Into<String>,
    ) -> Result<ModelToolDefinition, AiError> {
        let capability = self.query_capabilities.get(id).ok_or(AiError::Forbidden)?;
        let definition = ModelToolDefinition {
            tool_id: capability.id().as_str().to_owned(),
            provider_name: provider_name.into(),
            fingerprint: capability.fingerprint().to_owned(),
            description: capability.description().to_owned(),
            parameters: capability.argument_schema().clone(),
            strict: true,
        };
        definition.validate().map_err(|_| {
            AiError::InvalidConfiguration("automatic query provider alias is invalid".to_owned())
        })?;
        Ok(definition)
    }

    /// Compiles one provider-authored typed plan against the exact registered
    /// semantic capability.
    ///
    /// Compilation does not authorize or execute the query.
    ///
    /// # Errors
    ///
    /// Returns a safe error for an absent capability or invalid closed plan.
    pub fn compile_query_capability(
        &self,
        id: &AiToolId,
        expected_capability_fingerprint: &str,
        plan: serde_json::Value,
    ) -> Result<AiCompiledGraphqlQuery, AiError> {
        let capability = self.query_capabilities.get(id).ok_or(AiError::Forbidden)?;
        if capability.fingerprint() != expected_capability_fingerprint {
            return Err(AiError::Forbidden);
        }
        capability.compile(plan)
    }

    /// Projects one registered classified mutation capability for a provider.
    ///
    /// The provider receives only the closed typed plan schema and model-safe
    /// description. It receives no GraphQL document, target, credential,
    /// approval token, or authorization rule.
    ///
    /// # Errors
    ///
    /// Returns a safe error for an absent capability or malformed provider
    /// alias.
    pub fn mutation_capability_model_definition(
        &self,
        id: &AiToolId,
        provider_name: impl Into<String>,
    ) -> Result<ModelToolDefinition, AiError> {
        let capability = self
            .mutation_capabilities
            .get(id)
            .ok_or(AiError::Forbidden)?;
        let definition = ModelToolDefinition {
            tool_id: capability.id().as_str().to_owned(),
            provider_name: provider_name.into(),
            fingerprint: capability.fingerprint().to_owned(),
            description: capability.description().to_owned(),
            parameters: capability.argument_schema().clone(),
            strict: true,
        };
        definition.validate().map_err(|_| {
            AiError::InvalidConfiguration(
                "classified mutation provider alias is invalid".to_owned(),
            )
        })?;
        Ok(definition)
    }

    /// Compiles one provider-authored typed mutation plan against the exact
    /// registered semantic capability.
    ///
    /// Compilation neither authorizes nor executes the mutation and cannot
    /// satisfy one-shot approval.
    ///
    /// # Errors
    ///
    /// Returns a safe error for an absent capability, stale fingerprint, or
    /// invalid closed plan.
    pub fn compile_mutation_capability(
        &self,
        id: &AiToolId,
        expected_capability_fingerprint: &str,
        plan: serde_json::Value,
    ) -> Result<AiCompiledGraphqlMutation, AiError> {
        let capability = self
            .mutation_capabilities
            .get(id)
            .ok_or(AiError::Forbidden)?;
        if capability.fingerprint() != expected_capability_fingerprint {
            return Err(AiError::Forbidden);
        }
        capability.compile(plan)
    }

    /// Projects one registered subscription capability for a provider.
    ///
    /// The definition contains only the finite typed plan schema; it exposes
    /// no GraphQL document, target, cursor, transport, or credential.
    ///
    /// # Errors
    ///
    /// Returns an error for an absent capability or invalid provider alias.
    pub fn subscription_capability_model_definition(
        &self,
        id: &AiToolId,
        provider_name: impl Into<String>,
    ) -> Result<ModelToolDefinition, AiError> {
        let capability = self
            .subscription_capabilities
            .get(id)
            .ok_or(AiError::Forbidden)?;
        let definition = ModelToolDefinition {
            tool_id: capability.id().as_str().to_owned(),
            provider_name: provider_name.into(),
            fingerprint: capability.fingerprint().to_owned(),
            description: capability.description().to_owned(),
            parameters: capability.argument_schema().clone(),
            strict: true,
        };
        definition.validate().map_err(|_| {
            AiError::InvalidConfiguration(
                "automatic subscription provider alias is invalid".to_owned(),
            )
        })?;
        Ok(definition)
    }

    /// Compiles one closed provider-authored subscription plan.
    ///
    /// Compilation does not register a replay source, authorize a principal,
    /// open a subscription, persist a waiter, or disclose an event.
    ///
    /// # Errors
    ///
    /// Returns an error for an absent/stale capability or invalid typed plan.
    pub fn compile_subscription_capability(
        &self,
        id: &AiToolId,
        expected_capability_fingerprint: &str,
        plan: serde_json::Value,
    ) -> Result<AiCompiledGraphqlSubscription, AiError> {
        let capability = self
            .subscription_capabilities
            .get(id)
            .ok_or(AiError::Forbidden)?;
        if capability.fingerprint() != expected_capability_fingerprint {
            return Err(AiError::Forbidden);
        }
        capability.compile(plan)
    }

    /// Builds one provider-facing definition from the exact registered
    /// read-only descriptor.
    ///
    /// This is a canonical projection of catalog metadata, not a second tool
    /// declaration and not authorization. The caller supplies only the
    /// provider-safe alias used to correlate one model request; the stable ID,
    /// description, argument schema, and fingerprint are copied from the
    /// registered descriptor. Ordinary policy, current-principal, delegated
    /// authority, resolver, and disclosure checks still run when a plan is
    /// built and when a call executes.
    ///
    /// # Errors
    ///
    /// Returns a safe error when the tool is absent, is not an idempotent
    /// read-only application query, or the provider alias is malformed.
    pub fn read_only_model_definition(
        &self,
        id: &AiToolId,
        provider_name: impl Into<String>,
    ) -> Result<ModelToolDefinition, AiError> {
        let descriptor = self.descriptor(id).ok_or(AiError::Forbidden)?;
        if descriptor.operation_kind != AiToolOperationKind::Query
            || descriptor.operation_domain != AiToolOperationDomain::Application
            || descriptor.maturity != ToolMaturity::ReadOnly
            || descriptor.risk != AiToolRisk::ReadOnly
            || descriptor.approval != AiApprovalRule::None
            || !descriptor.idempotent
        {
            return Err(AiError::Forbidden);
        }
        let definition = ModelToolDefinition {
            tool_id: descriptor.id.as_str().to_owned(),
            provider_name: provider_name.into(),
            fingerprint: descriptor.fingerprint.clone(),
            description: descriptor.description.clone(),
            parameters: descriptor.argument_schema.clone(),
            strict: true,
        };
        definition.validate().map_err(|_| {
            AiError::InvalidConfiguration("provider-facing tool alias is invalid".to_owned())
        })?;
        Ok(definition)
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
    pub(crate) fn validate_generated_query_model_definition(
        &self,
        definition: &ModelToolDefinition,
        targets: &AiGeneratedGraphqlTargetPolicySet,
    ) -> Result<(), AiError> {
        let id = AiToolId::parse(definition.tool_id.clone())?;
        let capability = self.query_capabilities.get(&id).ok_or(AiError::Forbidden)?;
        if !targets.allows_query_capability(capability)
            || definition.fingerprint != capability.fingerprint()
            || definition.description != capability.description()
            || definition.parameters != *capability.argument_schema()
            || !definition.strict
        {
            return Err(AiError::Forbidden);
        }
        Ok(())
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn validate_generated_mutation_model_definition(
        &self,
        definition: &ModelToolDefinition,
        targets: &AiGeneratedGraphqlTargetPolicySet,
    ) -> Result<AiMutationExecutionPolicy, AiError> {
        let id = AiToolId::parse(definition.tool_id.clone())?;
        let capability = self
            .mutation_capabilities
            .get(&id)
            .ok_or(AiError::Forbidden)?;
        if !targets.allows_mutation_capability(capability)
            || definition.fingerprint != capability.fingerprint()
            || definition.description != capability.description()
            || definition.parameters != *capability.argument_schema()
            || !definition.strict
        {
            return Err(AiError::Forbidden);
        }
        Ok(capability.execution_policy())
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

#[async_trait]
impl AiToolAuthorizationPolicy for AiGeneratedGraphqlAuthorizationPolicy {
    async fn authorize(
        &self,
        principal: &ResolvedPrincipal,
        scope: &AiScope,
        descriptor: &AiToolDescriptor,
        variables: &serde_json::Value,
    ) -> AiToolAuthorizationDecision {
        if descriptor
            .graphql_contract
            .as_ref()
            .is_some_and(|contract| contract.semantic_operation().is_some())
        {
            if !self.targets.allows_generated_descriptor(descriptor) {
                return AiToolAuthorizationDecision::deny(
                    "generated_graphql_target_denied",
                    format!("generated-graphql-v1:{}", self.targets.fingerprint()),
                );
            }
            let policy_version = format!("generated-graphql-v1:{}", self.targets.fingerprint());
            let state = serde_json::json!({
                "policy": policy_version,
                "principal": principal.reference(),
                "scope": scope,
                "tool": descriptor.fingerprint,
                "variables": canonical_json_digest(variables),
            });
            let state_digest = serde_json::to_vec(&state)
                .map(|encoded| hex::encode(Sha256::digest(encoded)))
                .unwrap_or_default();
            if state_digest.is_empty() {
                return AiToolAuthorizationDecision::deny(
                    "generated_graphql_state_invalid",
                    policy_version,
                );
            }
            return AiToolAuthorizationDecision::allow(
                "generated_graphql_exact_target",
                policy_version,
                state_digest,
            );
        }
        self.static_tools
            .authorize(principal, scope, descriptor, variables)
            .await
    }
}

fn canonical_json_digest(value: &serde_json::Value) -> String {
    fn canonical(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(object) => serde_json::Value::Object(
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), canonical(value)))
                    .collect(),
            ),
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(canonical).collect())
            }
            _ => value.clone(),
        }
    }
    serde_json::to_vec(&canonical(value))
        .map(|encoded| hex::encode(Sha256::digest(encoded)))
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use agql_auth::{
        AccessTokenMetadata, AuthPrincipal, AuthUser, ResolvedPrincipal, SessionContext,
    };
    use serde_json::json;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;
    use crate::GraphqlOperationContract;

    const SCHEMA_FINGERPRINT: &str =
        "1111111111111111111111111111111111111111111111111111111111111111";
    const CATALOG_FINGERPRINT: &str =
        "2222222222222222222222222222222222222222222222222222222222222222";

    fn generated_query_descriptor(field: &str) -> AiToolDescriptor {
        let target = GraphqlExecutionTargetId::parse("inventory.graphql")
            .expect("test target should validate");
        let document = format!("query {field} {{ {field} }}");
        let contract = GraphqlOperationContract::new(
            target,
            SCHEMA_FINGERPRINT,
            field,
            &document,
            "projection-v1",
            "disclosure-v1",
        )
        .expect("test contract should validate");
        let mut value = serde_json::to_value(contract).expect("test contract should serialize");
        value["semantic_operation"] = json!({
            "fingerprint_algorithm": "graphql-orm-semantic-canonical-json-sha256-v1",
            "catalog_fingerprint": CATALOG_FINGERPRINT,
            "operation_fingerprint": "3333333333333333333333333333333333333333333333333333333333333333",
            "kind": "query",
            "field_name": field,
        });
        let contract = serde_json::from_value(value).expect("semantic contract should decode");
        AiToolDescriptor::new(
            format!("inventory.query.{}", field.to_ascii_lowercase()),
            format!("Read {field}."),
            AiToolOperationKind::Query,
            document,
            json!({
                "$schema": JSON_SCHEMA_2020_12,
                "type": "object",
                "additionalProperties": false,
            }),
        )
        .expect("test descriptor should validate")
        .with_graphql_contract(contract)
        .with_result_projection(field)
    }

    fn principal() -> ResolvedPrincipal {
        let principal = AuthPrincipal::User(AuthUser {
            user_id: "generated-query-user".to_owned(),
            session_id: Uuid::new_v4(),
            roles: Vec::new(),
            scopes: Vec::new(),
            session: SessionContext::default(),
            token_claims: AccessTokenMetadata {
                tenant_id: Some("generated-query-tenant".to_owned()),
                ..AccessTokenMetadata::default()
            },
        });
        ResolvedPrincipal::new(principal.reference(), principal, OffsetDateTime::now_utc())
            .expect("test principal should resolve")
    }

    fn target_policy(schema: &str, catalog: &str) -> AiGeneratedGraphqlTargetPolicySet {
        let mut policy = AiGeneratedGraphqlTargetPolicySet::new();
        policy
            .bind(
                AiGeneratedGraphqlTargetPolicyBinding::new(
                    GraphqlExecutionTargetId::parse("inventory.graphql")
                        .expect("test target should validate"),
                    schema,
                    catalog,
                )
                .expect("test target binding should validate")
                .allow_queries(),
            )
            .expect("test target should bind once");
        policy
    }

    #[tokio::test]
    async fn one_exact_target_policy_admits_new_query_roots_without_tool_ids() {
        let policy = AiGeneratedGraphqlAuthorizationPolicy::generated_only(target_policy(
            SCHEMA_FINGERPRINT,
            CATALOG_FINGERPRINT,
        ));
        let principal = principal();
        let scope =
            AiScope::new("tenant", "generated-query").with_tenant_id("generated-query-tenant");

        for descriptor in [
            generated_query_descriptor("ReadInventory"),
            generated_query_descriptor("ReadNewPublicRoot"),
        ] {
            let decision = policy
                .authorize(&principal, &scope, &descriptor, &json!({}))
                .await;
            assert!(decision.is_allowed());
            assert!(decision.is_complete_allow());
        }
    }

    #[tokio::test]
    async fn registration_alone_and_stale_target_contracts_remain_denied() {
        let descriptor = generated_query_descriptor("ReadInventory");
        let principal = principal();
        let scope =
            AiScope::new("tenant", "generated-query").with_tenant_id("generated-query-tenant");
        let unbound = AiGeneratedGraphqlAuthorizationPolicy::generated_only(
            AiGeneratedGraphqlTargetPolicySet::new(),
        );
        assert!(
            !unbound
                .authorize(&principal, &scope, &descriptor, &json!({}))
                .await
                .is_allowed()
        );
        let stale = AiGeneratedGraphqlAuthorizationPolicy::generated_only(target_policy(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            CATALOG_FINGERPRINT,
        ));
        assert!(
            !stale
                .authorize(&principal, &scope, &descriptor, &json!({}))
                .await
                .is_allowed()
        );
        let stale = AiGeneratedGraphqlAuthorizationPolicy::generated_only(target_policy(
            SCHEMA_FINGERPRINT,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ));
        assert!(
            !stale
                .authorize(&principal, &scope, &descriptor, &json!({}))
                .await
                .is_allowed()
        );
    }
}
