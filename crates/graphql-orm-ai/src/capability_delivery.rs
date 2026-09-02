//! Provider-neutral capability delivery, loading, and run fencing.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use agql_auth::{Clock, CurrentPrincipalResolver, PrincipalReference, ResolvedPrincipal};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    AiCapabilityIndex, AiCapabilityIndexEntry, AiCapabilityIndexSet,
    AiCapabilityIndexSetSearchResult, AiCapabilityKind, AiCapabilitySearchQuery, AiError, AiRunId,
    AiSessionId, AiToolId, ModelReasoningEffort, ModelToolDefinition, ProviderCapabilities,
    ProviderKind,
};

/// Frozen broker tool identifier for bounded capability discovery.
pub const AI_CAPABILITY_DISCOVER_TOOL_ID: &str = "graphql.capabilities.discover";
/// Frozen broker tool identifier for loading one exact capability contract.
pub const AI_CAPABILITY_DESCRIBE_TOOL_ID: &str = "graphql.capabilities.describe";
/// Frozen broker tool identifier for executing one loaded exact capability.
pub const AI_CAPABILITY_EXECUTE_TOOL_ID: &str = "graphql.capabilities.execute";

/// Current broker request/response envelope version.
pub const AI_CAPABILITY_BROKER_VERSION: u16 = 1;

/// Capability-definition delivery strategy selected by the coordinator.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiCapabilityDeliveryMode {
    /// Send a small exact definition set eagerly.
    EagerExact,
    /// Start with discovery and install selected definitions on continuation.
    ClientDeferred,
    /// Use a reviewed provider-native search/deferred-loading representation.
    ProviderDeferred,
    /// Keep a frozen discovery/describe/execute broker for the full session.
    FixedBroker,
}

/// Deployment bounds used when selecting a capability delivery strategy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiCapabilityDeliveryLimits {
    /// Maximum definitions admitted into eager mode.
    pub maximum_eager_definitions: u16,
    /// Maximum canonical definition bytes admitted into eager mode.
    pub maximum_eager_definition_bytes: u32,
    /// Maximum exact definitions installed after one discovery continuation.
    pub maximum_deferred_definitions: u16,
    /// Maximum bounded discovery results retained per fenced run.
    #[serde(default = "default_maximum_retained_searches")]
    pub maximum_retained_searches: u16,
    /// Maximum short-lived loaded bindings retained per fenced run.
    #[serde(default = "default_maximum_loaded_bindings")]
    pub maximum_loaded_bindings: u16,
    /// Maximum canonical bytes in one describe planning contract.
    #[serde(default = "default_maximum_describe_bytes")]
    pub maximum_describe_bytes: u32,
}

const fn default_maximum_retained_searches() -> u16 {
    4
}

const fn default_maximum_loaded_bindings() -> u16 {
    16
}

const fn default_maximum_describe_bytes() -> u32 {
    512 * 1024
}

impl Default for AiCapabilityDeliveryLimits {
    fn default() -> Self {
        Self {
            maximum_eager_definitions: 16,
            maximum_eager_definition_bytes: 256 * 1024,
            maximum_deferred_definitions: 8,
            maximum_retained_searches: default_maximum_retained_searches(),
            maximum_loaded_bindings: default_maximum_loaded_bindings(),
            maximum_describe_bytes: default_maximum_describe_bytes(),
        }
    }
}

impl AiCapabilityDeliveryLimits {
    /// Rejects a limit set that cannot represent a complete
    /// discover/describe/execute broker interaction.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] when any bound is zero or a
    /// broker bound is outside its compiled ceiling.
    pub fn validate(self) -> Result<(), AiError> {
        if self.maximum_eager_definitions == 0
            || self.maximum_eager_definition_bytes == 0
            || self.maximum_deferred_definitions == 0
            || self.maximum_deferred_definitions > self.maximum_loaded_bindings
            || self.maximum_retained_searches == 0
            || self.maximum_retained_searches > 64
            || self.maximum_loaded_bindings == 0
            || self.maximum_loaded_bindings > 256
            || self.maximum_describe_bytes < 1_024
            || self.maximum_describe_bytes > 4 * 1_024 * 1_024
        {
            return Err(AiError::InvalidConfiguration(
                "capability delivery limits are invalid".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Coordinator-selected exact capability delivery decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AiCapabilityDeliveryDecision {
    mode: AiCapabilityDeliveryMode,
}

impl AiCapabilityDeliveryDecision {
    /// Selected reviewed mode.
    pub const fn mode(self) -> AiCapabilityDeliveryMode {
        self.mode
    }
}

/// Exact initial tool surface produced for one coordinator-selected mode.
#[derive(Clone, Debug, PartialEq)]
pub struct AiCapabilityDeliverySurface {
    mode: AiCapabilityDeliveryMode,
    tools: Vec<ModelToolDefinition>,
}

impl AiCapabilityDeliverySurface {
    /// Selected mode.
    pub const fn mode(&self) -> AiCapabilityDeliveryMode {
        self.mode
    }

    /// Exact initial provider definitions. Client-deferred mode contains only
    /// discovery; provider-deferred mode contains filtered definitions marked
    /// for native deferred loading. A run-owned surface returned by
    /// [`AiCapabilityDeliveryTurn::current_surface`] also retains the exact
    /// static bootstrap definitions fingerprinted into its session binding.
    pub fn tools(&self) -> &[ModelToolDefinition] {
        &self.tools
    }
}

/// Builds the exact initial capability surface after host filtering.
///
/// Provider-deferred definitions are only the already-authorized candidates;
/// provider-native search therefore cannot discover a hidden or unauthorized
/// definition. Search and registration remain non-authoritative at execution.
///
/// # Errors
///
/// Rejects malformed index fingerprints, duplicate IDs/names, or malformed
/// exact definitions.
pub fn prepare_capability_delivery_surface(
    decision: AiCapabilityDeliveryDecision,
    index_fingerprint: &str,
    mut filtered_exact_definitions: Vec<ModelToolDefinition>,
) -> Result<AiCapabilityDeliverySurface, AiError> {
    if !crate::valid_sha256(index_fingerprint) {
        return Err(AiError::InvalidConfiguration(
            "capability delivery index fingerprint is invalid".to_owned(),
        ));
    }
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    for definition in &filtered_exact_definitions {
        definition.validate().map_err(|_| {
            AiError::InvalidConfiguration("capability definition is invalid".to_owned())
        })?;
        if matches!(
            definition.tool_id.as_str(),
            AI_CAPABILITY_DISCOVER_TOOL_ID
                | AI_CAPABILITY_DESCRIBE_TOOL_ID
                | AI_CAPABILITY_EXECUTE_TOOL_ID
        ) {
            return Err(AiError::InvalidConfiguration(
                "application definitions use a reserved capability-broker ID".to_owned(),
            ));
        }
        if !ids.insert(definition.tool_id.as_str())
            || !names.insert(definition.provider_name.as_str())
        {
            return Err(AiError::InvalidConfiguration(
                "capability definitions are ambiguous".to_owned(),
            ));
        }
    }
    filtered_exact_definitions.sort_by(|left, right| {
        left.tool_id
            .cmp(&right.tool_id)
            .then_with(|| left.provider_name.cmp(&right.provider_name))
    });
    let tools = match decision.mode {
        AiCapabilityDeliveryMode::EagerExact => filtered_exact_definitions,
        AiCapabilityDeliveryMode::ProviderDeferred => filtered_exact_definitions
            .into_iter()
            .map(|mut definition| {
                definition.defer_loading = true;
                definition
            })
            .collect(),
        AiCapabilityDeliveryMode::ClientDeferred => {
            vec![discovery_definition(index_fingerprint)]
        }
        AiCapabilityDeliveryMode::FixedBroker => fixed_broker_definitions(index_fingerprint),
    };
    Ok(AiCapabilityDeliverySurface {
        mode: decision.mode,
        tools,
    })
}

/// Selects only exact freshly loaded definitions for the next client-deferred
/// continuation. Unrelated catalogue definitions cannot be installed.
///
/// # Errors
///
/// Rejects count overflow, duplicates, malformed definitions, or any mismatch
/// between a crate-owned loaded binding and the supplied exact definition.
pub fn prepare_client_deferred_continuation(
    loaded: &[AiLoadedCapabilityBinding],
    mut exact_definitions: Vec<ModelToolDefinition>,
    limits: AiCapabilityDeliveryLimits,
) -> Result<Vec<ModelToolDefinition>, AiError> {
    if loaded.len() != exact_definitions.len()
        || loaded.len() > usize::from(limits.maximum_deferred_definitions)
    {
        return Err(AiError::InvalidInput(
            "deferred capability selection is invalid".to_owned(),
        ));
    }
    for definition in &mut exact_definitions {
        definition
            .validate()
            .map_err(|_| AiError::InvalidInput("deferred definition is invalid".to_owned()))?;
        definition.defer_loading = false;
        let matches = loaded.iter().filter(|binding| {
            binding.capability_id.as_str() == definition.tool_id
                && binding.capability_fingerprint == definition.fingerprint
        });
        if matches.count() != 1 {
            return Err(AiError::Forbidden);
        }
    }
    exact_definitions.sort_by(|left, right| left.tool_id.cmp(&right.tool_id));
    if exact_definitions
        .windows(2)
        .any(|pair| pair[0].tool_id == pair[1].tool_id)
    {
        return Err(AiError::InvalidInput(
            "deferred capability selection is ambiguous".to_owned(),
        ));
    }
    Ok(exact_definitions)
}

fn discovery_definition(index_fingerprint: &str) -> ModelToolDefinition {
    broker_definition(
        AI_CAPABILITY_DISCOVER_TOOL_ID,
        "graphql_capabilities_discover",
        "Search the current reviewed GraphQL read-query capability index.",
        index_fingerprint,
        json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "maxLength": 1024},
                "namespace": {"type": ["string", "null"], "maxLength": 256},
                "kind": {"type": ["string", "null"], "enum": ["generated_query", null]},
                "entityOrClass": {"type": ["string", "null"], "maxLength": 256},
                "maximumResults": {"type": "integer", "minimum": 1, "maximum": 32}
            },
            "required": ["text", "namespace", "kind", "entityOrClass", "maximumResults"],
            "additionalProperties": false
        }),
    )
}

fn fixed_broker_definitions(index_fingerprint: &str) -> Vec<ModelToolDefinition> {
    let discover = discovery_definition(index_fingerprint);
    let describe = broker_definition(
        AI_CAPABILITY_DESCRIBE_TOOL_ID,
        "graphql_capabilities_describe",
        "Load one exact current capability and return its bounded public planning contract.",
        index_fingerprint,
        json!({
            "type": "object",
            "properties": {
                "capabilityId": {"type": "string", "maxLength": 200},
                "candidateFingerprint": {"type": "string", "minLength": 64, "maxLength": 64}
            },
            "required": ["capabilityId", "candidateFingerprint"],
            "additionalProperties": false
        }),
    );
    let execute = broker_definition(
        AI_CAPABILITY_EXECUTE_TOOL_ID,
        "graphql_capabilities_execute",
        "Execute one previously loaded exact capability. Copy only fields admitted by the returned planSchema: flatten argument leaves into name/value entries, and omit optional wrapper fields that the planSchema does not expose.",
        index_fingerprint,
        json!({
            "type": "object",
            "properties": {
                "loadedReference": {
                    "type": "string", "minLength": 64, "maxLength": 64,
                    "description": "Exact loadedReference returned by the matching describe call."
                },
                "arguments": {
                    "type": "array", "maxItems": 64,
                    "description": "Optional root arguments admitted by planSchema.arguments. Flatten each supplied scalar leaf into one name/value entry using its dotted public path; omit this array when no root argument is needed.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string", "minLength": 1, "maxLength": 256,
                                "description": "Exact dotted public argument path admitted by planSchema.arguments."
                            },
                            "value": {
                                "type": ["string", "integer", "number", "boolean", "null"],
                                "description": "Scalar value for the exact argument path."
                            }
                        },
                        "required": ["name", "value"],
                        "additionalProperties": false
                    }
                },
                "selections": {
                    "type": "array", "maxItems": 256, "uniqueItems": true,
                    "description": "One or more exact scalar paths copied from planSchema.selections.items.enum.",
                    "items": {"type": "string", "maxLength": 512}
                },
                "relationshipArguments": {
                    "type": "array", "maxItems": 64,
                    "description": "Optional relationship arguments admitted by planSchema.relationshipArguments. Omit when that schema has no applicable relationship argument.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string", "minLength": 1, "maxLength": 512},
                            "arguments": {
                                "type": "array", "maxItems": 64,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "name": {"type": "string", "minLength": 1, "maxLength": 256},
                                        "value": {"type": ["string", "integer", "number", "boolean", "null"]}
                                    },
                                    "required": ["name", "value"],
                                    "additionalProperties": false
                                }
                            }
                        },
                        "required": ["path", "arguments"],
                        "additionalProperties": false
                    }
                },
                "relationshipMaximumItems": {
                    "type": "array", "maxItems": 64,
                    "description": "Optional relationship bounds admitted by planSchema.relationshipMaximumItems. Omit when the exact relationship path is not present there.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string", "minLength": 1, "maxLength": 512},
                            "maximumItems": {"type": "integer", "minimum": 1, "maximum": 10000}
                        },
                        "required": ["path", "maximumItems"],
                        "additionalProperties": false
                    }
                },
                "maximumItems": {
                    "type": ["integer", "null"], "minimum": 1, "maximum": 10000,
                    "description": "Optional root result bound. Supply a positive value only when planSchema exposes maximumItems; otherwise omit it."
                }
            },
            "required": ["loadedReference", "selections"],
            "additionalProperties": false
        }),
    );
    vec![discover, describe, execute]
}

fn broker_definition(
    tool_id: &str,
    provider_name: &str,
    description: &str,
    index_fingerprint: &str,
    parameters: serde_json::Value,
) -> ModelToolDefinition {
    ModelToolDefinition {
        tool_id: tool_id.to_owned(),
        provider_name: provider_name.to_owned(),
        fingerprint: hash_json(&json!({
            "version": 1,
            "tool_id": tool_id,
            "index_fingerprint": index_fingerprint,
            "parameters": parameters.clone(),
        })),
        description: description.to_owned(),
        parameters,
        strict: true,
        defer_loading: false,
    }
}

/// Selects the safest admissible delivery mode from provider declarations and
/// exact definition size. Prompt text cannot influence this decision.
///
/// # Errors
///
/// Returns a configuration error when no provider-declared mode can represent
/// the requested surface.
pub fn select_capability_delivery_mode(
    provider: &ProviderCapabilities,
    exact_definition_count: usize,
    exact_definition_bytes: usize,
    retained_definitions_frozen: bool,
    limits: AiCapabilityDeliveryLimits,
) -> Result<AiCapabilityDeliveryDecision, AiError> {
    limits.validate()?;
    let supported = &provider.capability_delivery_modes;
    let eager_fits = exact_definition_count <= usize::from(limits.maximum_eager_definitions)
        && exact_definition_bytes <= limits.maximum_eager_definition_bytes as usize;
    let selected = if retained_definitions_frozen
        && supported.contains(&AiCapabilityDeliveryMode::FixedBroker)
    {
        Some(AiCapabilityDeliveryMode::FixedBroker)
    } else if eager_fits && supported.contains(&AiCapabilityDeliveryMode::EagerExact) {
        Some(AiCapabilityDeliveryMode::EagerExact)
    } else if supported.contains(&AiCapabilityDeliveryMode::ProviderDeferred) {
        Some(AiCapabilityDeliveryMode::ProviderDeferred)
    } else if !retained_definitions_frozen
        && supported.contains(&AiCapabilityDeliveryMode::ClientDeferred)
    {
        Some(AiCapabilityDeliveryMode::ClientDeferred)
    } else if supported.contains(&AiCapabilityDeliveryMode::FixedBroker) {
        Some(AiCapabilityDeliveryMode::FixedBroker)
    } else {
        None
    };
    selected
        .map(|mode| AiCapabilityDeliveryDecision { mode })
        .ok_or_else(|| {
            AiError::InvalidConfiguration(
                "provider has no admissible capability delivery mode".to_owned(),
            )
        })
}

/// Execution-relevant retained provider-session definition binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiProviderCapabilitySessionBinding {
    /// Selected delivery mode.
    delivery_mode: AiCapabilityDeliveryMode,
    /// Canonical capability-index-set fingerprint.
    capability_index_fingerprint: String,
    /// Stable fingerprints of frozen static bootstrap tools.
    static_bootstrap_tool_fingerprints: BTreeSet<String>,
    /// Reviewed provider projection algorithm version.
    provider_projection_version: String,
    /// Exact model.
    model: String,
    /// Exact reasoning effort, including `Unspecified`.
    reasoning_effort: ModelReasoningEffort,
    /// Persisted provider registration identity.
    registration_identity: String,
    /// Complete session-definition fingerprint.
    fingerprint: String,
}

impl AiProviderCapabilitySessionBinding {
    /// Creates one immutable provider-session binding.
    ///
    /// # Errors
    ///
    /// Rejects missing, unbounded, or control-character-bearing values.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        delivery_mode: AiCapabilityDeliveryMode,
        capability_index_fingerprint: impl Into<String>,
        static_bootstrap_tool_fingerprints: BTreeSet<String>,
        provider_projection_version: impl Into<String>,
        model: impl Into<String>,
        reasoning_effort: ModelReasoningEffort,
        registration_identity: impl Into<String>,
    ) -> Result<Self, AiError> {
        let capability_index_fingerprint = capability_index_fingerprint.into();
        let provider_projection_version = provider_projection_version.into();
        let model = model.into();
        let registration_identity = registration_identity.into();
        for value in [provider_projection_version.as_str(), model.as_str()] {
            if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
                return Err(AiError::InvalidConfiguration(
                    "provider capability session binding is invalid".to_owned(),
                ));
            }
        }
        if !crate::valid_sha256(&capability_index_fingerprint)
            || !crate::valid_sha256(&registration_identity)
            || static_bootstrap_tool_fingerprints.len() > 64
            || static_bootstrap_tool_fingerprints
                .iter()
                .any(|value| !crate::valid_sha256(value))
        {
            return Err(AiError::InvalidConfiguration(
                "provider bootstrap tool binding is invalid".to_owned(),
            ));
        }
        let fingerprint = hash_json(&json!({
            "version": 1,
            "delivery_mode": delivery_mode,
            "capability_index_fingerprint": capability_index_fingerprint,
            "static_bootstrap_tool_fingerprints": static_bootstrap_tool_fingerprints,
            "provider_projection_version": provider_projection_version,
            "model": model,
            "reasoning_effort": reasoning_effort,
            "registration_identity": registration_identity,
        }));
        Ok(Self {
            delivery_mode,
            capability_index_fingerprint,
            static_bootstrap_tool_fingerprints,
            provider_projection_version,
            model,
            reasoning_effort,
            registration_identity,
            fingerprint,
        })
    }

    /// Coordinator-selected delivery mode.
    pub const fn delivery_mode(&self) -> AiCapabilityDeliveryMode {
        self.delivery_mode
    }

    /// Exact canonical capability-index-set fingerprint.
    pub fn capability_index_fingerprint(&self) -> &str {
        &self.capability_index_fingerprint
    }

    /// Exact canonical capability-index-set fingerprint.
    ///
    /// This is the explicit multi-target name for
    /// [`Self::capability_index_fingerprint`].
    pub fn capability_index_set_fingerprint(&self) -> &str {
        &self.capability_index_fingerprint
    }

    /// Exact frozen static bootstrap tool fingerprints.
    pub fn static_bootstrap_tool_fingerprints(&self) -> &BTreeSet<String> {
        &self.static_bootstrap_tool_fingerprints
    }

    /// Reviewed provider projection algorithm version.
    pub fn provider_projection_version(&self) -> &str {
        &self.provider_projection_version
    }

    /// Exact model bound into the retained session.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Exact reasoning effort bound into the retained session.
    pub const fn reasoning_effort(&self) -> ModelReasoningEffort {
        self.reasoning_effort
    }

    /// Immutable provider registration identity incorporated into this binding.
    pub fn registration_identity(&self) -> &str {
        &self.registration_identity
    }

    /// Complete immutable binding fingerprint.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

/// Exact run fence supplied when loading or executing one discovered entry.
#[derive(Clone, Debug)]
pub struct AiCapabilityRunBinding {
    /// Session identifier.
    pub session_id: AiSessionId,
    /// Run identifier.
    pub run_id: AiRunId,
    /// Exact attempt identifier.
    pub attempt_id: Uuid,
    /// Monotonic lease generation.
    pub lease_generation: i64,
    /// Provider family.
    pub provider_kind: ProviderKind,
    /// Exact provider capability-session fingerprint.
    pub provider_session_fingerprint: String,
}

/// Current host authorization decision for loading/executing one entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiCapabilityAuthorityDecision {
    /// Whether the current principal may use the exact entry in this run.
    pub allowed: bool,
    /// Safe current policy fingerprint/version bound into the handle.
    pub policy_fingerprint: String,
}

/// Fresh host policy applied after principal rehydration for every load and
/// again for every execution.
#[async_trait]
pub trait AiCapabilityAuthorityPolicy: Send + Sync {
    /// Applies current scope, classification, capability-kind, provider,
    /// session, and target policy.
    async fn authorize(
        &self,
        principal: &ResolvedPrincipal,
        owning_index: &AiCapabilityIndex,
        entry: &AiCapabilityIndexEntry,
        run: &AiCapabilityRunBinding,
    ) -> Result<AiCapabilityAuthorityDecision, AiError>;
}

/// Supplies the current exact index. Implementations may cache immutable
/// catalogue data but must never cache an authority decision.
pub trait AiCurrentCapabilityIndex: Send + Sync {
    /// Returns the current complete index for one logical run target.
    fn current_index(
        &self,
        run: &AiCapabilityRunBinding,
    ) -> Result<Arc<AiCapabilityIndex>, AiError>;
}

/// Supplies the current canonical set of independently compiled indexes.
///
/// Implementations may cache immutable catalogue data but must never cache an
/// authority decision. Every capability ID in the returned set has one exact
/// owning logical target. A single-index source automatically implements this
/// contract for backward-compatible consumers.
pub trait AiCurrentCapabilityIndexSet: Send + Sync {
    /// Returns the complete current multi-target index set for one run.
    fn current_index_set(
        &self,
        run: &AiCapabilityRunBinding,
    ) -> Result<Arc<AiCapabilityIndexSet>, AiError>;
}

impl<T> AiCurrentCapabilityIndexSet for T
where
    T: AiCurrentCapabilityIndex + ?Sized,
{
    fn current_index_set(
        &self,
        run: &AiCapabilityRunBinding,
    ) -> Result<Arc<AiCapabilityIndexSet>, AiError> {
        let index = self.current_index(run)?;
        Ok(Arc::new(AiCapabilityIndexSet::compile([index])?))
    }
}

/// Opaque short-lived crate-owned loaded capability proof.
///
/// Fields are private and the type is not deserializable, so a host or model
/// cannot construct fingerprints, substitute a target, or change a run fence.
#[derive(Clone, Debug)]
pub struct AiLoadedCapabilityBinding {
    nonce: Uuid,
    principal_reference_fingerprint: String,
    capability_id: AiToolId,
    capability_kind: AiCapabilityKind,
    capability_fingerprint: String,
    entry_fingerprint: String,
    index_set_fingerprint: String,
    index_fingerprint: String,
    schema_fingerprint: String,
    semantic_catalogue_fingerprint: String,
    target_policy_fingerprint: String,
    policy_fingerprint: String,
    session_id: AiSessionId,
    run_id: AiRunId,
    attempt_id: Uuid,
    lease_generation: i64,
    provider_kind: ProviderKind,
    provider_session_fingerprint: String,
    expires_at: OffsetDateTime,
}

impl AiLoadedCapabilityBinding {
    /// Stable capability ID carried to the ordinary durable tool broker.
    pub fn capability_id(&self) -> &AiToolId {
        &self.capability_id
    }

    /// Exact loaded capability fingerprint.
    pub fn capability_fingerprint(&self) -> &str {
        &self.capability_fingerprint
    }

    /// Opaque non-authoritative audit reference.
    pub fn audit_reference(&self) -> String {
        hash_json(&json!({
            "version": 1,
            "nonce": self.nonce,
            "entry": self.entry_fingerprint,
            "run": self.run_id,
            "attempt": self.attempt_id,
        }))
    }
}

/// Discovery and loaded-capability broker with fresh authority checks.
#[derive(Clone)]
pub struct AiCapabilityDiscoveryBroker {
    principal_resolver: Arc<dyn CurrentPrincipalResolver>,
    current_indexes: Arc<dyn AiCurrentCapabilityIndexSet>,
    authority: Arc<dyn AiCapabilityAuthorityPolicy>,
    clock: Arc<dyn Clock>,
    loaded_ttl: Duration,
}

impl AiCapabilityDiscoveryBroker {
    /// Creates a broker with a bounded short-lived loaded handle TTL.
    ///
    /// # Errors
    ///
    /// Rejects TTLs outside one second through five minutes.
    pub fn new(
        principal_resolver: Arc<dyn CurrentPrincipalResolver>,
        current_indexes: Arc<dyn AiCurrentCapabilityIndexSet>,
        authority: Arc<dyn AiCapabilityAuthorityPolicy>,
        clock: Arc<dyn Clock>,
        loaded_ttl: Duration,
    ) -> Result<Self, AiError> {
        if loaded_ttl < Duration::seconds(1) || loaded_ttl > Duration::minutes(5) {
            return Err(AiError::InvalidConfiguration(
                "loaded capability TTL is invalid".to_owned(),
            ));
        }
        Ok(Self {
            principal_resolver,
            current_indexes,
            authority,
            clock,
            loaded_ttl,
        })
    }

    /// Searches current model-safe metadata after rehydrating the principal
    /// and applying current host policy to every candidate.
    ///
    /// # Errors
    ///
    /// Fails closed on rehydration, index, or policy failure. Search grants no
    /// load or execution authority.
    pub async fn search(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        query: &AiCapabilitySearchQuery,
    ) -> Result<AiCapabilityIndexSetSearchResult, AiError> {
        let principal = self.rehydrate(principal_reference).await?;
        let indexes = self.current_indexes.current_index_set(run)?;
        let mut result = indexes.search(query)?;
        let mut permitted = Vec::new();
        for candidate in result.candidates {
            let (index, entry) = indexes.entry(&candidate.id).ok_or(AiError::Forbidden)?;
            let decision = self
                .authority
                .authorize(&principal, index, entry, run)
                .await?;
            if decision.allowed {
                permitted.push(candidate);
            }
        }
        result.candidates = permitted;
        Ok(result)
    }

    /// Loads one exact candidate after fresh principal, current-index, target,
    /// schema, semantic, kind, provider, session, and policy verification.
    ///
    /// # Errors
    ///
    /// Fails closed for revocation, drift, substitution, expiry, or a denied
    /// current host decision.
    pub async fn load(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        search: &AiCapabilityIndexSetSearchResult,
        capability_id: &AiToolId,
    ) -> Result<AiLoadedCapabilityBinding, AiError> {
        let principal = self.rehydrate(principal_reference).await?;
        let indexes = self.current_indexes.current_index_set(run)?;
        verify_search_binding(&indexes, search)?;
        let candidate = search
            .candidates
            .iter()
            .find(|candidate| &candidate.id == capability_id)
            .ok_or(AiError::Forbidden)?;
        let (index, entry) = indexes.entry(capability_id).ok_or(AiError::Forbidden)?;
        if candidate.kind != entry.kind
            || candidate.capability_fingerprint != entry.capability_fingerprint
            || candidate.entry_fingerprint != entry.fingerprint
        {
            return Err(AiError::Forbidden);
        }
        let decision = self
            .authority
            .authorize(&principal, index, entry, run)
            .await?;
        if !decision.allowed || !valid_binding_value(&decision.policy_fingerprint) {
            return Err(AiError::Forbidden);
        }
        Ok(AiLoadedCapabilityBinding {
            nonce: Uuid::new_v4(),
            principal_reference_fingerprint: principal_reference_fingerprint(principal.reference()),
            capability_id: entry.id.clone(),
            capability_kind: entry.kind,
            capability_fingerprint: entry.capability_fingerprint.clone(),
            entry_fingerprint: entry.fingerprint.clone(),
            index_set_fingerprint: indexes.fingerprint().to_owned(),
            index_fingerprint: index.fingerprint().to_owned(),
            schema_fingerprint: index.schema_fingerprint().to_owned(),
            semantic_catalogue_fingerprint: index.semantic_catalogue_fingerprint().to_owned(),
            target_policy_fingerprint: index.target_policy_fingerprint().to_owned(),
            policy_fingerprint: decision.policy_fingerprint,
            session_id: run.session_id,
            run_id: run.run_id,
            attempt_id: run.attempt_id,
            lease_generation: run.lease_generation,
            provider_kind: run.provider_kind.clone(),
            provider_session_fingerprint: run.provider_session_fingerprint.clone(),
            expires_at: self.clock.now() + self.loaded_ttl,
        })
    }

    /// Revalidates a loaded handle immediately before ordinary broker
    /// execution. Permission removal after discovery or loading takes effect.
    ///
    /// # Errors
    ///
    /// Fails closed on expiry, revocation, drift, or any cross-run/provider/
    /// target/kind substitution.
    pub async fn authorize_execution(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        loaded: &AiLoadedCapabilityBinding,
    ) -> Result<(), AiError> {
        if principal_reference_fingerprint(principal_reference)
            != loaded.principal_reference_fingerprint
            || self.clock.now() > loaded.expires_at
            || loaded.session_id != run.session_id
            || loaded.run_id != run.run_id
            || loaded.attempt_id != run.attempt_id
            || run.lease_generation < loaded.lease_generation
            || loaded.provider_kind != run.provider_kind
            || loaded.provider_session_fingerprint != run.provider_session_fingerprint
        {
            return Err(AiError::Forbidden);
        }
        let principal = self.rehydrate(principal_reference).await?;
        let indexes = self.current_indexes.current_index_set(run)?;
        let (index, entry) = indexes
            .entry(&loaded.capability_id)
            .ok_or(AiError::Forbidden)?;
        if loaded.capability_kind != entry.kind
            || loaded.capability_fingerprint != entry.capability_fingerprint
            || loaded.entry_fingerprint != entry.fingerprint
            || loaded.index_set_fingerprint != indexes.fingerprint()
            || loaded.index_fingerprint != index.fingerprint()
            || loaded.schema_fingerprint != index.schema_fingerprint()
            || loaded.semantic_catalogue_fingerprint != index.semantic_catalogue_fingerprint()
            || loaded.target_policy_fingerprint != index.target_policy_fingerprint()
        {
            return Err(AiError::Forbidden);
        }
        let decision = self
            .authority
            .authorize(&principal, index, entry, run)
            .await?;
        if !decision.allowed || decision.policy_fingerprint != loaded.policy_fingerprint {
            return Err(AiError::Forbidden);
        }
        Ok(())
    }

    async fn rehydrate(
        &self,
        reference: &PrincipalReference,
    ) -> Result<ResolvedPrincipal, AiError> {
        self.principal_resolver
            .resolve(reference)
            .await
            .map_err(|_| AiError::ReauthorizationFailed)
    }
}

fn verify_search_binding(
    indexes: &AiCapabilityIndexSet,
    search: &AiCapabilityIndexSetSearchResult,
) -> Result<(), AiError> {
    if search.index_set_fingerprint != indexes.fingerprint() {
        return Err(AiError::Forbidden);
    }
    Ok(())
}

fn valid_binding_value(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

fn principal_reference_fingerprint(reference: &PrincipalReference) -> String {
    let encoded = serde_json::to_vec(reference)
        .expect("PrincipalReference contains only serializable safe reference values");
    hex::encode(Sha256::digest(encoded))
}

fn hash_json(value: &serde_json::Value) -> String {
    let encoded = serde_json::to_vec(value).unwrap_or_default();
    hex::encode(Sha256::digest(encoded))
}

/// One frozen capability-broker meta-operation.
///
/// The three operations are the complete crate-owned broker surface. Resolving
/// an operation from a tool identifier is discovery, not authorization: every
/// dispatch still rehydrates the principal, reapplies current host policy and,
/// for [`Self::Execute`] only, passes through ordinary resolver authorization.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiCapabilityBrokerOperation {
    /// Bounded authority-neutral search over the current compact index.
    Discover,
    /// Load one exact current capability and return its planning contract.
    Describe,
    /// Execute one previously loaded exact capability through a closed plan.
    Execute,
}

impl AiCapabilityBrokerOperation {
    /// Resolves the frozen broker operation for one stable tool identifier.
    ///
    /// Returns `None` for every ordinary registered capability.
    pub fn from_tool_id(tool_id: &AiToolId) -> Option<Self> {
        match tool_id.as_str() {
            AI_CAPABILITY_DISCOVER_TOOL_ID => Some(Self::Discover),
            AI_CAPABILITY_DESCRIBE_TOOL_ID => Some(Self::Describe),
            AI_CAPABILITY_EXECUTE_TOOL_ID => Some(Self::Execute),
            _ => None,
        }
    }

    /// Frozen stable tool identifier for this operation.
    pub const fn tool_id(self) -> &'static str {
        match self {
            Self::Discover => AI_CAPABILITY_DISCOVER_TOOL_ID,
            Self::Describe => AI_CAPABILITY_DESCRIBE_TOOL_ID,
            Self::Execute => AI_CAPABILITY_EXECUTE_TOOL_ID,
        }
    }

    /// Whether this operation can reach an application resolver.
    ///
    /// Only [`Self::Execute`] can. Discovery and describe return bounded
    /// authority-neutral metadata and grant no execution authority.
    pub const fn reaches_resolver(self) -> bool {
        matches!(self, Self::Execute)
    }
}

/// Returns the three frozen broker definitions for one exact compact index.
///
/// The definitions are crate-authored and deterministic: a host installs them
/// verbatim and cannot widen, rename, or re-fingerprint the broker surface.
///
/// # Errors
///
/// Returns [`AiError::InvalidConfiguration`] for a malformed index
/// fingerprint.
pub fn capability_broker_definitions(
    index_fingerprint: &str,
) -> Result<Vec<ModelToolDefinition>, AiError> {
    if !crate::valid_sha256(index_fingerprint) {
        return Err(AiError::InvalidConfiguration(
            "capability delivery index fingerprint is invalid".to_owned(),
        ));
    }
    Ok(fixed_broker_definitions(index_fingerprint))
}

/// Canonical serialized byte cost of one exact definition set.
///
/// This is the measurement `select_capability_delivery_mode` compares against
/// [`AiCapabilityDeliveryLimits::maximum_eager_definition_bytes`].
///
/// # Errors
///
/// Returns [`AiError::InvalidConfiguration`] when a definition cannot be
/// canonically serialized.
pub fn capability_definition_bytes(definitions: &[ModelToolDefinition]) -> Result<usize, AiError> {
    let mut total = 0usize;
    for definition in definitions {
        let encoded = serde_json::to_vec(definition).map_err(|_| {
            AiError::InvalidConfiguration("capability definition is invalid".to_owned())
        })?;
        total = total.checked_add(encoded.len()).ok_or_else(|| {
            AiError::InvalidConfiguration("capability definitions are too large".to_owned())
        })?;
    }
    Ok(total)
}

/// Selects the delivery mode and builds the exact initial surface in one step.
///
/// This is the single per-turn entry point: it measures the already-filtered
/// exact definitions, calls [`select_capability_delivery_mode`], then
/// [`prepare_capability_delivery_surface`]. Prompt text never reaches either
/// decision.
///
/// # Errors
///
/// Returns a safe error when no provider-declared mode can represent the
/// surface, the limits are invalid, or a definition is malformed.
pub fn plan_capability_delivery(
    provider: &ProviderCapabilities,
    index_fingerprint: &str,
    filtered_exact_definitions: Vec<ModelToolDefinition>,
    retained_definitions_frozen: bool,
    limits: AiCapabilityDeliveryLimits,
) -> Result<AiCapabilityDeliverySurface, AiError> {
    let bytes = capability_definition_bytes(&filtered_exact_definitions)?;
    let decision = select_capability_delivery_mode(
        provider,
        filtered_exact_definitions.len(),
        bytes,
        retained_definitions_frozen,
        limits,
    )?;
    prepare_capability_delivery_surface(decision, index_fingerprint, filtered_exact_definitions)
}

/// Derives one deterministic bounded provider alias for a capability ID.
///
/// Provider function names are limited to 64 bytes of `[A-Za-z0-9_-]`. The
/// alias substitutes unsafe bytes and, when truncation is required, appends a
/// stable short digest of the complete identifier, so two distinct
/// capabilities can never collide on one alias. The alias is a naming
/// convention only and grants nothing.
pub fn capability_provider_alias(tool_id: &AiToolId) -> String {
    let sanitized = tool_id
        .as_str()
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-') {
                char::from(byte)
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.len() <= 64 {
        return sanitized;
    }
    let digest = hex::encode(Sha256::digest(tool_id.as_str().as_bytes()));
    format!("{}_{}", &sanitized[..47], &digest[..16])
}

/// Observed broker turn/call amplification for one fenced run.
///
/// A novel capability costs one discover, one describe, and one execute call,
/// while a loaded capability costs one execute call. Completed-turn adapters
/// need one continuation provider turn per result; an in-turn dynamic-tool
/// adapter can perform the same broker calls inside one provider turn. These
/// counters measure application-tool calls, not provider turns.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AiCapabilityBrokerAmplification {
    /// Accepted discovery calls.
    pub discover_calls: u32,
    /// Accepted describe calls.
    pub describe_calls: u32,
    /// Accepted execute calls.
    pub execute_calls: u32,
}

impl AiCapabilityBrokerAmplification {
    /// Total accepted broker calls.
    pub const fn total_calls(self) -> u32 {
        self.discover_calls
            .saturating_add(self.describe_calls)
            .saturating_add(self.execute_calls)
    }
}

#[derive(Debug, Default)]
struct BrokerSessionState {
    searches: VecDeque<AiCapabilityIndexSetSearchResult>,
    loaded: VecDeque<(String, AiLoadedCapabilityBinding)>,
    deferred_installation_pending: bool,
    amplification: AiCapabilityBrokerAmplification,
}

/// Bounded process-local broker state for one fenced run.
///
/// The state retains only crate-owned discovery results and short-lived loaded
/// bindings so a later describe or execute can be matched to an exact earlier
/// candidate. It is never a durable authority and never substitutes for the
/// published default-deny catalogue: losing it fails the next describe or
/// execute closed with a bounded retryable stale-selection outcome and the
/// model rediscovers.
#[derive(Clone, Debug)]
pub struct AiCapabilityBrokerSession {
    inner: Arc<Mutex<BrokerSessionState>>,
    limits: AiCapabilityDeliveryLimits,
}

impl AiCapabilityBrokerSession {
    /// Creates bounded broker state for one fenced run.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] for invalid delivery limits.
    pub fn new(limits: AiCapabilityDeliveryLimits) -> Result<Self, AiError> {
        limits.validate()?;
        Ok(Self {
            inner: Arc::new(Mutex::new(BrokerSessionState::default())),
            limits,
        })
    }

    /// Exact bounds applied to retained searches, loaded bindings, and one
    /// describe planning contract.
    pub const fn limits(&self) -> AiCapabilityDeliveryLimits {
        self.limits
    }

    /// Current accepted broker call counts for host-side run sizing.
    pub fn amplification(&self) -> AiCapabilityBrokerAmplification {
        self.state().amplification
    }

    /// Loaded bindings still retained for this run.
    ///
    /// The count is bounded by
    /// [`AiCapabilityDeliveryLimits::maximum_loaded_bindings`].
    pub fn loaded_binding_count(&self) -> usize {
        self.state().loaded.len()
    }

    /// Exact currently loaded bindings in load order.
    ///
    /// A client-deferred continuation installs exactly these capabilities
    /// through [`prepare_client_deferred_continuation`].
    pub fn loaded_bindings(&self) -> Vec<AiLoadedCapabilityBinding> {
        self.state()
            .loaded
            .iter()
            .map(|(_, binding)| binding.clone())
            .collect()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, BrokerSessionState> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record_search(&self, result: AiCapabilityIndexSetSearchResult) {
        let mut state = self.state();
        state.amplification.discover_calls = state.amplification.discover_calls.saturating_add(1);
        state.searches.push_back(result);
        while state.searches.len() > usize::from(self.limits.maximum_retained_searches) {
            state.searches.pop_front();
        }
    }

    fn candidate_search(
        &self,
        capability_id: &AiToolId,
        candidate_fingerprint: &str,
    ) -> Option<AiCapabilityIndexSetSearchResult> {
        self.state()
            .searches
            .iter()
            .rev()
            .find(|search| {
                search.candidates.iter().any(|candidate| {
                    &candidate.id == capability_id
                        && candidate.entry_fingerprint == candidate_fingerprint
                })
            })
            .cloned()
    }

    fn record_loaded(&self, reference: String, binding: AiLoadedCapabilityBinding) {
        let mut state = self.state();
        state.amplification.describe_calls = state.amplification.describe_calls.saturating_add(1);
        state.loaded.retain(|(existing, _)| existing != &reference);
        state.loaded.push_back((reference, binding));
        while state.loaded.len() > usize::from(self.limits.maximum_loaded_bindings) {
            state.loaded.pop_front();
        }
    }

    #[cfg_attr(feature = "mssql", allow(dead_code))]
    fn record_deferred_search(
        &self,
        result: AiCapabilityIndexSetSearchResult,
        loaded: Vec<(String, AiLoadedCapabilityBinding)>,
    ) {
        let mut state = self.state();
        state.amplification.discover_calls = state.amplification.discover_calls.saturating_add(1);
        state.searches.push_back(result);
        while state.searches.len() > usize::from(self.limits.maximum_retained_searches) {
            state.searches.pop_front();
        }
        state.loaded = loaded.into();
        state.deferred_installation_pending = true;
    }

    fn deferred_installation_pending(&self) -> bool {
        self.state().deferred_installation_pending
    }

    #[cfg_attr(feature = "mssql", allow(dead_code))]
    fn complete_deferred_installation(&self) {
        self.state().deferred_installation_pending = false;
    }

    fn loaded(&self, reference: &str) -> Option<AiLoadedCapabilityBinding> {
        self.state()
            .loaded
            .iter()
            .find(|(existing, _)| existing == reference)
            .map(|(_, binding)| binding.clone())
    }

    fn record_execute(&self) {
        let mut state = self.state();
        state.amplification.execute_calls = state.amplification.execute_calls.saturating_add(1);
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrokerDiscoverArguments {
    text: String,
    #[serde(default)]
    namespace: Option<String>,
    #[serde(default)]
    kind: Option<AiCapabilityKind>,
    #[serde(default)]
    entity_or_class: Option<String>,
    maximum_results: u16,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrokerDescribeArguments {
    capability_id: String,
    candidate_fingerprint: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrokerExecuteArgument {
    /// Dot-separated GraphQL input path. Decimal path components address
    /// bounded list positions, for example `filter.and.0.status.eq`.
    name: String,
    value: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrokerRelationshipArguments {
    path: String,
    #[serde(default)]
    arguments: Vec<BrokerExecuteArgument>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrokerRelationshipMaximumItems {
    path: String,
    maximum_items: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrokerExecuteArguments {
    loaded_reference: String,
    #[serde(default)]
    arguments: Vec<BrokerExecuteArgument>,
    selections: Vec<String>,
    #[serde(default)]
    relationship_arguments: Vec<BrokerRelationshipArguments>,
    #[serde(default)]
    relationship_maximum_items: Vec<BrokerRelationshipMaximumItems>,
    #[serde(default)]
    maximum_items: Option<u32>,
}

fn valid_graphql_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte == b'_' || byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

fn valid_broker_selection_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.chars().any(char::is_control)
        && value.split('.').count() <= 12
        && value.split('.').all(valid_graphql_name)
}

fn insert_broker_argument(
    root: &mut serde_json::Value,
    path: &str,
    value: serde_json::Value,
    seen: &mut BTreeSet<String>,
) -> Result<(), AiError> {
    if path.is_empty() || path.len() > 256 || !seen.insert(path.to_owned()) {
        return Err(malformed_broker_arguments());
    }
    if !matches!(
        value,
        serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_)
    ) {
        return Err(malformed_broker_arguments());
    }
    let segments = path.split('.').collect::<Vec<_>>();
    if segments.is_empty()
        || segments.len() > 12
        || !valid_graphql_name(segments[0])
        || segments.iter().any(|segment| {
            segment.is_empty()
                || (!valid_graphql_name(segment)
                    && segment.parse::<usize>().map_or(true, |index| index >= 64))
        })
    {
        return Err(malformed_broker_arguments());
    }
    insert_broker_argument_segments(root, &segments, value)
}

fn insert_broker_argument_segments(
    current: &mut serde_json::Value,
    segments: &[&str],
    value: serde_json::Value,
) -> Result<(), AiError> {
    let Some((segment, remaining)) = segments.split_first() else {
        return Err(malformed_broker_arguments());
    };
    let final_segment = remaining.is_empty();
    match current {
        serde_json::Value::Object(object) => {
            if !valid_graphql_name(segment) {
                return Err(malformed_broker_arguments());
            }
            if final_segment {
                if object.insert((*segment).to_owned(), value).is_some() {
                    return Err(malformed_broker_arguments());
                }
                return Ok(());
            }
            let next_is_index = remaining[0].parse::<usize>().is_ok();
            let child = object.entry((*segment).to_owned()).or_insert_with(|| {
                if next_is_index {
                    serde_json::Value::Array(Vec::new())
                } else {
                    serde_json::Value::Object(serde_json::Map::new())
                }
            });
            insert_broker_argument_segments(child, remaining, value)
        }
        serde_json::Value::Array(array) => {
            let index = segment
                .parse::<usize>()
                .ok()
                .filter(|index| *index < 64)
                .ok_or_else(malformed_broker_arguments)?;
            if array.len() <= index {
                array.resize(index + 1, serde_json::Value::Null);
            }
            if final_segment {
                if !array[index].is_null() {
                    return Err(malformed_broker_arguments());
                }
                array[index] = value;
                return Ok(());
            }
            if array[index].is_null() {
                array[index] = if remaining[0].parse::<usize>().is_ok() {
                    serde_json::Value::Array(Vec::new())
                } else {
                    serde_json::Value::Object(serde_json::Map::new())
                };
            }
            insert_broker_argument_segments(&mut array[index], remaining, value)
        }
        _ => Err(malformed_broker_arguments()),
    }
}

fn stale_selection() -> AiError {
    AiError::InvalidInput("broker capability selection is stale".to_owned())
}

fn malformed_broker_arguments() -> AiError {
    AiError::InvalidInput("broker arguments are invalid".to_owned())
}

/// Bounded authority-neutral planning contract for one loaded capability.
///
/// Holding a description proves that the exact current capability was loaded
/// under the current principal and current host policy at describe time. It is
/// not execution authority: [`AiCapabilityDiscoveryBroker::authorize_execution`]
/// runs again immediately before any resolver is reached.
#[derive(Clone, Debug)]
pub struct AiCapabilityDescription {
    loaded_reference: String,
    capability_id: AiToolId,
    capability_kind: AiCapabilityKind,
    capability_fingerprint: String,
    contract: serde_json::Value,
}

impl AiCapabilityDescription {
    /// Opaque non-authoritative reference the model presents to execute.
    pub fn loaded_reference(&self) -> &str {
        &self.loaded_reference
    }

    /// Stable capability identifier that was loaded.
    pub const fn capability_id(&self) -> &AiToolId {
        &self.capability_id
    }

    /// Capability family that was loaded.
    pub const fn capability_kind(&self) -> AiCapabilityKind {
        self.capability_kind
    }

    /// Exact loaded capability fingerprint.
    pub fn capability_fingerprint(&self) -> &str {
        &self.capability_fingerprint
    }

    /// Bounded model-visible planning contract.
    pub const fn contract(&self) -> &serde_json::Value {
        &self.contract
    }

    /// Attaches the exact compact plan schema of the loaded capability.
    ///
    /// The schema is the same closed provider-facing plan contract eager
    /// delivery would have sent, now delivered once for one capability instead
    /// of for the whole catalogue. An oversized schema is omitted rather than
    /// truncated, and the contract records that fact.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] when the contract can no
    /// longer be canonically serialized.
    pub fn with_plan_schema(
        mut self,
        plan_schema: serde_json::Value,
        maximum_bytes: u32,
    ) -> Result<Self, AiError> {
        {
            let Some(contract) = self.contract.as_object_mut() else {
                return Err(AiError::InvalidConfiguration(
                    "capability planning contract is invalid".to_owned(),
                ));
            };
            contract.insert("planSchema".to_owned(), plan_schema);
            contract.insert("planSchemaAvailable".to_owned(), json!(true));
        }
        let fits = serde_json::to_vec(&self.contract)
            .map(|encoded| encoded.len() <= maximum_bytes as usize)
            .unwrap_or(false);
        if fits {
            return Ok(self);
        }
        let contract = self
            .contract
            .as_object_mut()
            .expect("the planning contract was proven to be an object");
        contract.remove("planSchema");
        contract.insert("planSchemaAvailable".to_owned(), json!(false));
        if serde_json::to_vec(&self.contract)
            .map_or(true, |encoded| encoded.len() > maximum_bytes as usize)
        {
            return Err(AiError::InvalidConfiguration(
                "capability planning contract exceeds its deployment bound".to_owned(),
            ));
        }
        Ok(self)
    }

    /// Consumes the description and returns the one bounded model result.
    pub fn into_model_result(self) -> serde_json::Value {
        self.contract
    }
}

/// Authorized broker execution of one exact previously loaded capability.
///
/// Construction proves that the loaded binding is unexpired, still bound to
/// this run/attempt/lease/provider session, still matches the current index,
/// and still passes current host policy. Ordinary resolver authorization is
/// still applied by [`crate::AiRuntime::execute_query_capability`].
#[derive(Clone, Debug)]
pub struct AiCapabilityExecution {
    capability_id: AiToolId,
    capability_fingerprint: String,
    audit_reference: String,
    plan: serde_json::Value,
}

impl AiCapabilityExecution {
    /// Exact capability to execute.
    pub const fn capability_id(&self) -> &AiToolId {
        &self.capability_id
    }

    /// Exact capability fingerprint proven at load and re-proven at execution.
    pub fn capability_fingerprint(&self) -> &str {
        &self.capability_fingerprint
    }

    /// Opaque non-authoritative audit reference of the loaded binding.
    pub fn audit_reference(&self) -> &str {
        &self.audit_reference
    }

    /// Closed compact plan handed to the authoritative capability compiler.
    pub const fn plan(&self) -> &serde_json::Value {
        &self.plan
    }

    /// Consumes the execution and returns the closed compact plan.
    pub fn into_plan(self) -> serde_json::Value {
        self.plan
    }
}

fn candidate_value(candidate: &crate::AiCapabilitySearchCandidate) -> serde_json::Value {
    json!({
        "capabilityId": candidate.id.as_str(),
        "kind": candidate.kind,
        "name": candidate.name,
        "description": candidate.description,
        "namespace": candidate.namespace,
        "entity": candidate.entity_name,
        "operation": candidate.operation_name,
        "shape": candidate.operation_shape,
        "candidateFingerprint": candidate.entry_fingerprint,
    })
}

fn planning_contract(
    entry: &AiCapabilityIndexEntry,
    loaded_reference: &str,
    expires_in_seconds: i64,
) -> serde_json::Value {
    json!({
        "version": AI_CAPABILITY_BROKER_VERSION,
        "loadedReference": loaded_reference,
        "expiresInSeconds": expires_in_seconds,
        "capabilityId": entry.id.as_str(),
        "kind": entry.kind,
        "name": entry.name,
        "description": entry.description,
        "namespace": entry.namespace,
        "entity": entry.entity_name,
        "operation": entry.operation_name,
        "shape": entry.operation_shape,
        "resultClassification": entry.result_classification,
        "resultDescription": entry.result_description,
        "resultRecordCost": entry.result_record_cost,
        "risk": entry.risk,
        "approval": entry.approval,
        "scalarFields": entry.scalar_fields,
        "relationships": entry.relationships,
        "aggregates": entry.aggregate_features,
        "candidateFingerprint": entry.fingerprint,
    })
}

impl AiCapabilityDiscoveryBroker {
    /// Dispatches one frozen `graphql.capabilities.discover` call.
    ///
    /// The result is bounded authority-neutral generated-query metadata: it contains no
    /// argument schema, GraphQL document, target, resolver, or credential, and
    /// it grants no load or execution authority. Static tools remain exact
    /// bootstrap definitions, while mutations and subscriptions use their
    /// separate approval/wait contracts and cannot enter this broker. Every
    /// returned candidate has already passed current host policy for the
    /// current principal.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for malformed broker arguments and
    /// fails closed on rehydration, index, or policy failure.
    pub async fn dispatch_discover(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        session: &AiCapabilityBrokerSession,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, AiError> {
        let parsed: BrokerDiscoverArguments =
            serde_json::from_value(arguments.clone()).map_err(|_| malformed_broker_arguments())?;
        if parsed
            .kind
            .is_some_and(|kind| kind != AiCapabilityKind::GeneratedQuery)
        {
            return Err(malformed_broker_arguments());
        }
        let query = AiCapabilitySearchQuery {
            text: parsed.text,
            namespace: parsed.namespace,
            kind: Some(AiCapabilityKind::GeneratedQuery),
            entity_or_operation: parsed.entity_or_class,
            maximum_results: parsed.maximum_results,
        };
        let result = self.search(principal_reference, run, &query).await?;
        let value = json!({
            "version": AI_CAPABILITY_BROKER_VERSION,
            "indexFingerprint": result.index_set_fingerprint,
            "candidates": result
                .candidates
                .iter()
                .map(candidate_value)
                .collect::<Vec<_>>(),
        });
        session.record_search(result);
        Ok(value)
    }

    /// Dispatches client-deferred discovery and atomically loads exactly the
    /// generated-query candidates returned for the next continuation.
    ///
    /// Unlike the fixed broker, client-deferred delivery does not expose a
    /// separate describe operation. The returned candidate set is therefore
    /// constrained to generated reads, capped by the deferred-definition
    /// limit, freshly loaded under current authority, and installed verbatim
    /// by the coordinator after the durable discovery outcome commits.
    #[cfg_attr(feature = "mssql", allow(dead_code))]
    pub(crate) async fn dispatch_client_deferred_discover(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        session: &AiCapabilityBrokerSession,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, AiError> {
        let parsed: BrokerDiscoverArguments =
            serde_json::from_value(arguments.clone()).map_err(|_| malformed_broker_arguments())?;
        if parsed
            .kind
            .is_some_and(|kind| kind != AiCapabilityKind::GeneratedQuery)
            || parsed.maximum_results > session.limits().maximum_deferred_definitions
        {
            return Err(malformed_broker_arguments());
        }
        let query = AiCapabilitySearchQuery {
            text: parsed.text,
            namespace: parsed.namespace,
            kind: Some(AiCapabilityKind::GeneratedQuery),
            entity_or_operation: parsed.entity_or_class,
            maximum_results: parsed.maximum_results,
        };
        let result = self.search(principal_reference, run, &query).await?;
        if result.candidates.len() > usize::from(session.limits().maximum_deferred_definitions) {
            return Err(AiError::InvalidConfiguration(
                "deferred discovery exceeded its exact definition bound".to_owned(),
            ));
        }
        let mut loaded = Vec::with_capacity(result.candidates.len());
        for candidate in &result.candidates {
            let binding = self
                .load(principal_reference, run, &result, &candidate.id)
                .await?;
            loaded.push((binding.audit_reference(), binding));
        }
        let value = json!({
            "version": AI_CAPABILITY_BROKER_VERSION,
            "indexFingerprint": result.index_set_fingerprint,
            "candidates": result
                .candidates
                .iter()
                .map(candidate_value)
                .collect::<Vec<_>>(),
        });
        session.record_deferred_search(result, loaded);
        Ok(value)
    }

    /// Dispatches one frozen `graphql.capabilities.describe` call.
    ///
    /// The candidate must come from a discovery result retained for this run
    /// and its fingerprint must still match. A drifted index, an unknown
    /// identifier, and an identifier never returned by discovery are all
    /// reported as one bounded retryable stale selection, so describe cannot be
    /// used to probe for capabilities the current principal cannot see.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for malformed arguments or a stale
    /// selection, and [`AiError::Forbidden`] when current host policy no
    /// longer permits the exact candidate.
    pub async fn dispatch_describe(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        session: &AiCapabilityBrokerSession,
        arguments: &serde_json::Value,
    ) -> Result<AiCapabilityDescription, AiError> {
        let parsed: BrokerDescribeArguments =
            serde_json::from_value(arguments.clone()).map_err(|_| malformed_broker_arguments())?;
        if !crate::valid_sha256(&parsed.candidate_fingerprint) {
            return Err(malformed_broker_arguments());
        }
        let capability_id = AiToolId::parse(parsed.capability_id).map_err(|_| stale_selection())?;
        let search = session
            .candidate_search(&capability_id, &parsed.candidate_fingerprint)
            .ok_or_else(stale_selection)?;
        let indexes = self.current_indexes.current_index_set(run)?;
        verify_search_binding(&indexes, &search).map_err(|_| stale_selection())?;
        let loaded = self
            .load(principal_reference, run, &search, &capability_id)
            .await?;
        let (_, entry) = indexes.entry(&capability_id).ok_or_else(stale_selection)?;
        let loaded_reference = loaded.audit_reference();
        let expires_in_seconds = (loaded.expires_at - self.clock.now())
            .whole_seconds()
            .max(0);
        let contract = planning_contract(entry, &loaded_reference, expires_in_seconds);
        let description = AiCapabilityDescription {
            loaded_reference: loaded_reference.clone(),
            capability_id: loaded.capability_id.clone(),
            capability_kind: loaded.capability_kind,
            capability_fingerprint: loaded.capability_fingerprint.clone(),
            contract,
        };
        session.record_loaded(loaded_reference, loaded);
        Ok(description)
    }

    /// Dispatches one frozen `graphql.capabilities.execute` call up to, but
    /// not including, the resolver.
    ///
    /// Only a generated read capability is executable through the frozen
    /// broker; reviewed static tools, mutations, and subscriptions retain
    /// their own exact delivery and approval contracts and fail closed here.
    /// The returned closed plan contains public names and finite typed values
    /// only and is compiled by the authoritative schema-derived compiler.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for malformed arguments, an unknown
    /// loaded reference, or an expired binding, and [`AiError::Forbidden`] for
    /// a non-read capability or any revocation, drift, or substitution.
    pub async fn authorize_broker_execution(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        session: &AiCapabilityBrokerSession,
        arguments: &serde_json::Value,
    ) -> Result<AiCapabilityExecution, AiError> {
        let parsed: BrokerExecuteArguments =
            serde_json::from_value(arguments.clone()).map_err(|_| malformed_broker_arguments())?;
        if !crate::valid_sha256(&parsed.loaded_reference) {
            return Err(malformed_broker_arguments());
        }
        let loaded = session
            .loaded(&parsed.loaded_reference)
            .ok_or_else(stale_selection)?;
        if self.clock.now() > loaded.expires_at {
            return Err(stale_selection());
        }
        if loaded.capability_kind != AiCapabilityKind::GeneratedQuery {
            return Err(AiError::Forbidden);
        }
        self.authorize_execution(principal_reference, run, &loaded)
            .await?;
        if parsed.arguments.len() > 64
            || parsed.selections.len() > 256
            || parsed
                .selections
                .iter()
                .any(|selection| !valid_broker_selection_path(selection))
            || parsed.relationship_arguments.len() > 64
            || parsed.relationship_maximum_items.len() > 64
            || parsed
                .maximum_items
                .is_some_and(|maximum| !(1..=10_000).contains(&maximum))
        {
            return Err(malformed_broker_arguments());
        }
        let mut plan_arguments = serde_json::Value::Object(serde_json::Map::new());
        let mut seen_arguments = BTreeSet::new();
        for argument in parsed.arguments {
            insert_broker_argument(
                &mut plan_arguments,
                &argument.name,
                argument.value,
                &mut seen_arguments,
            )?;
        }
        let mut selections = parsed.selections;
        selections.sort_unstable();
        if selections.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(malformed_broker_arguments());
        }
        let mut relationship_arguments = serde_json::Map::new();
        for relationship in parsed.relationship_arguments {
            if !valid_broker_selection_path(&relationship.path)
                || relationship.arguments.len() > 64
                || relationship_arguments.contains_key(&relationship.path)
            {
                return Err(malformed_broker_arguments());
            }
            let mut arguments = serde_json::Value::Object(serde_json::Map::new());
            let mut seen = BTreeSet::new();
            for argument in relationship.arguments {
                insert_broker_argument(&mut arguments, &argument.name, argument.value, &mut seen)?;
            }
            relationship_arguments.insert(relationship.path, arguments);
        }
        let mut relationship_maximum_items = serde_json::Map::new();
        for relationship in parsed.relationship_maximum_items {
            if !valid_broker_selection_path(&relationship.path)
                || !(1..=10_000).contains(&relationship.maximum_items)
                || relationship_maximum_items
                    .insert(relationship.path, json!(relationship.maximum_items))
                    .is_some()
            {
                return Err(malformed_broker_arguments());
            }
        }
        let mut plan = serde_json::Map::from_iter([
            ("arguments".to_owned(), plan_arguments),
            ("selections".to_owned(), json!(selections)),
            (
                "relationshipArguments".to_owned(),
                serde_json::Value::Object(relationship_arguments),
            ),
            (
                "relationshipMaximumItems".to_owned(),
                serde_json::Value::Object(relationship_maximum_items),
            ),
        ]);
        if let Some(maximum_items) = parsed.maximum_items {
            plan.insert("maximumItems".to_owned(), json!(maximum_items));
        }
        session.record_execute();
        Ok(AiCapabilityExecution {
            capability_id: loaded.capability_id.clone(),
            capability_fingerprint: loaded.capability_fingerprint.clone(),
            audit_reference: loaded.audit_reference(),
            plan: serde_json::Value::Object(plan),
        })
    }
}

/// Crate-owned capability delivery for one fenced coordinator run.
///
/// The turn owns the provider surface: it selects the delivery mode from
/// provider declarations and exact definition size, mints the exact initial
/// definitions, and installs freshly loaded definitions on a client-deferred
/// continuation. A host installs the returned definitions verbatim, so prompt
/// text, model output, and host tool authoring cannot widen the surface.
///
/// The turn is not authority. Every broker call still rehydrates the current
/// principal, reapplies current host policy, and — for execution only — passes
/// through ordinary resolver authorization.
#[derive(Clone)]
pub struct AiCapabilityDeliveryTurn {
    mode: AiCapabilityDeliveryMode,
    index_fingerprint: String,
    session_binding: AiProviderCapabilitySessionBinding,
    broker: Arc<AiCapabilityDiscoveryBroker>,
    session: AiCapabilityBrokerSession,
    #[cfg_attr(feature = "mssql", allow(dead_code))]
    static_bootstrap_tools: Arc<Vec<ModelToolDefinition>>,
    tools: Arc<Mutex<Vec<ModelToolDefinition>>>,
}

impl std::fmt::Debug for AiCapabilityDeliveryTurn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiCapabilityDeliveryTurn")
            .field("mode", &self.mode)
            .field("index_fingerprint", &self.index_fingerprint)
            .field("binding", &self.session_binding.fingerprint())
            .finish_non_exhaustive()
    }
}

impl AiCapabilityDeliveryTurn {
    /// Selects the delivery mode and mints the exact initial surface.
    ///
    /// # Errors
    ///
    /// Returns a safe error when no provider-declared mode can represent the
    /// surface, a definition is malformed, or the retained capability-session
    /// binding does not describe the selected mode and compact index.
    pub fn select(
        provider: &ProviderCapabilities,
        index_fingerprint: &str,
        filtered_exact_definitions: Vec<ModelToolDefinition>,
        retained_definitions_frozen: bool,
        session_binding: AiProviderCapabilitySessionBinding,
        broker: Arc<AiCapabilityDiscoveryBroker>,
        session: AiCapabilityBrokerSession,
    ) -> Result<Self, AiError> {
        let static_bootstrap_tools = filtered_exact_definitions
            .iter()
            .filter(|definition| {
                session_binding
                    .static_bootstrap_tool_fingerprints()
                    .contains(&definition.fingerprint)
            })
            .cloned()
            .collect::<Vec<_>>();
        if static_bootstrap_tools.len()
            != session_binding.static_bootstrap_tool_fingerprints().len()
        {
            return Err(AiError::InvalidConfiguration(
                "capability static bootstrap binding is incomplete".to_owned(),
            ));
        }
        let surface = plan_capability_delivery(
            provider,
            index_fingerprint,
            filtered_exact_definitions,
            retained_definitions_frozen,
            session.limits(),
        )?;
        if session_binding.delivery_mode() != surface.mode()
            || session_binding.capability_index_fingerprint() != index_fingerprint
        {
            return Err(AiError::InvalidConfiguration(
                "capability session binding does not match the selected surface".to_owned(),
            ));
        }
        let mut tools = surface.tools().to_vec();
        if matches!(
            surface.mode(),
            AiCapabilityDeliveryMode::ClientDeferred | AiCapabilityDeliveryMode::FixedBroker
        ) {
            tools.extend(static_bootstrap_tools.iter().cloned());
            tools.sort_by(|left, right| left.tool_id.cmp(&right.tool_id));
        }
        Ok(Self {
            mode: surface.mode(),
            index_fingerprint: index_fingerprint.to_owned(),
            session_binding,
            broker,
            session,
            static_bootstrap_tools: Arc::new(static_bootstrap_tools),
            tools: Arc::new(Mutex::new(tools)),
        })
    }

    /// Coordinator-selected delivery mode for this run.
    pub const fn mode(&self) -> AiCapabilityDeliveryMode {
        self.mode
    }

    /// Exact canonical capability-index-set fingerprint bound into this surface.
    pub fn index_fingerprint(&self) -> &str {
        &self.index_fingerprint
    }

    /// Exact canonical capability-index-set fingerprint bound into this turn.
    pub fn index_set_fingerprint(&self) -> &str {
        &self.index_fingerprint
    }

    /// Immutable retained provider capability-session binding.
    pub const fn session_binding(&self) -> &AiProviderCapabilitySessionBinding {
        &self.session_binding
    }

    /// Broker used for every discover, describe, and execute dispatch.
    pub fn broker(&self) -> &Arc<AiCapabilityDiscoveryBroker> {
        &self.broker
    }

    /// Bounded process-local broker state for this run.
    pub const fn session(&self) -> &AiCapabilityBrokerSession {
        &self.session
    }

    /// Exact definitions a host must install in the next provider request.
    pub fn current_tools(&self) -> Vec<ModelToolDefinition> {
        self.tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Returns the exact current provider surface for initial or continuation
    /// plan construction.
    ///
    /// Client-deferred discovery can replace the installed exact definitions
    /// between turns. A planner must obtain this value from the same delivery
    /// turn passed by the coordinator rather than retaining an earlier surface
    /// or reconstructing one from public fields.
    pub fn current_surface(&self) -> AiCapabilityDeliverySurface {
        AiCapabilityDeliverySurface {
            mode: self.mode,
            tools: self.current_tools(),
        }
    }

    /// Whether an offered definition set is exactly the crate-owned surface.
    pub fn matches_offered_tools(&self, offered: &[ModelToolDefinition]) -> bool {
        let current = self.current_tools();
        current.len() == offered.len()
            && current
                .iter()
                .zip(offered.iter())
                .all(|(expected, actual)| expected == actual)
    }

    /// Whether another turn carries the exact same in-process run state.
    ///
    /// Matching fingerprints alone are insufficient on a continuation: a host
    /// must carry forward the crate-owned broker session and installed surface
    /// rather than recreate empty state under the same public configuration.
    #[cfg_attr(feature = "mssql", allow(dead_code))]
    pub(crate) fn shares_run_state(&self, other: &Self) -> bool {
        self.mode == other.mode
            && self.index_fingerprint == other.index_fingerprint
            && self.session_binding == other.session_binding
            && Arc::ptr_eq(&self.broker, &other.broker)
            && Arc::ptr_eq(&self.session.inner, &other.session.inner)
            && Arc::ptr_eq(&self.static_bootstrap_tools, &other.static_bootstrap_tools)
            && Arc::ptr_eq(&self.tools, &other.tools)
    }

    /// Whether the run currently needs freshly loaded exact definitions
    /// installed on the next continuation.
    pub fn requires_deferred_installation(&self) -> bool {
        self.mode == AiCapabilityDeliveryMode::ClientDeferred
            && self.session.deferred_installation_pending()
    }

    /// Installs the exact freshly loaded definitions for the next
    /// client-deferred continuation and returns the complete new surface.
    ///
    /// Discovery remains offered so a run can keep loading capabilities. Only
    /// definitions matching a crate-owned loaded binding are installed;
    /// unrelated catalogue definitions cannot be smuggled in.
    ///
    /// # Errors
    ///
    /// Returns a safe error unless the run is client-deferred and every
    /// supplied definition matches exactly one current loaded binding within
    /// the configured selection count.
    #[cfg_attr(feature = "mssql", allow(dead_code))]
    pub(crate) fn install_deferred_definitions(
        &self,
        exact_definitions: Vec<ModelToolDefinition>,
    ) -> Result<Vec<ModelToolDefinition>, AiError> {
        if self.mode != AiCapabilityDeliveryMode::ClientDeferred {
            return Err(AiError::Forbidden);
        }
        let loaded = self.session.loaded_bindings();
        let installed = prepare_client_deferred_continuation(
            &loaded,
            exact_definitions,
            self.session.limits(),
        )?;
        let mut tools = vec![discovery_definition(&self.index_fingerprint)];
        tools.extend(self.static_bootstrap_tools.iter().cloned());
        tools.extend(installed);
        tools.sort_by(|left, right| left.tool_id.cmp(&right.tool_id));
        let mut current = self.tools.lock().unwrap_or_else(PoisonError::into_inner);
        current.clone_from(&tools);
        self.session.complete_deferred_installation();
        Ok(tools)
    }

    /// Observed broker turn/call amplification for host-side run sizing.
    pub fn amplification(&self) -> AiCapabilityBrokerAmplification {
        self.session.amplification()
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
impl AiCapabilityRunBinding {
    /// Builds one exact broker run fence from a renewed run lease.
    ///
    /// The provider capability-session binding supplies the fingerprint, so a
    /// delivery-mode, index, bootstrap-tool, projection, model, or reasoning
    /// change invalidates every loaded binding taken under the previous
    /// binding rather than silently reusing it.
    pub fn from_lease(
        lease: &crate::AiRunLease,
        provider_kind: ProviderKind,
        session_binding: &AiProviderCapabilitySessionBinding,
    ) -> Self {
        Self {
            session_id: lease.session_id(),
            run_id: lease.run_id(),
            attempt_id: lease.attempt_id(),
            lease_generation: lease.lease_generation(),
            provider_kind,
            provider_session_fingerprint: session_binding.fingerprint().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        RwLock,
        atomic::{AtomicBool, Ordering},
    };

    use agql_auth::{AccessTokenMetadata, AuthPrincipal, AuthUser, FixedClock, SessionContext};
    use graphql_orm::graphql::orm::{
        GeneratedGraphqlOperationDescriptor, GraphqlEntitySemanticMetadata,
        GraphqlOperationCatalog, GraphqlOperationKind, GraphqlSemanticArgumentDescriptor,
        GraphqlSemanticCatalog, GraphqlSemanticClassification, GraphqlSemanticExport,
        GraphqlSemanticFieldMetadata, GraphqlSemanticOperationDescriptor, GraphqlSemanticTypeKind,
        GraphqlSemanticTypeRef,
    };

    use super::*;

    fn definition(id: &str) -> ModelToolDefinition {
        ModelToolDefinition {
            tool_id: id.to_owned(),
            provider_name: id.replace('.', "_"),
            fingerprint: hash_json(&json!({"id": id})),
            description: "Read one reviewed record.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
                "additionalProperties": false
            }),
            strict: true,
            defer_loading: false,
        }
    }

    fn principal() -> AuthPrincipal {
        AuthPrincipal::User(AuthUser {
            user_id: "capability-user".to_owned(),
            session_id: Uuid::from_u128(10),
            roles: Vec::new(),
            scopes: vec!["jim.read".to_owned()],
            session: SessionContext::default(),
            token_claims: AccessTokenMetadata {
                tenant_id: Some("tenant-1".to_owned()),
                ..AccessTokenMetadata::default()
            },
        })
    }

    fn index(target_policy: &str) -> Arc<AiCapabilityIndex> {
        let operation_catalogue = GraphqlOperationCatalog::compose(Vec::<(
            &'static [GeneratedGraphqlOperationDescriptor],
            bool,
            bool,
        )>::new());
        let semantics = GraphqlSemanticCatalog::compose(Vec::new(), &operation_catalogue)
            .expect("semantic catalogue");
        let descriptor = crate::AiToolDescriptor::new(
            "jim.jobs_list",
            "List the latest Jim jobs.",
            crate::AiToolOperationKind::Query,
            "query ReviewedJobs { jobs }",
            json!({
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }),
        )
        .expect("descriptor")
        .with_maximum_classification(crate::DataClassification::Public);
        Arc::new(
            AiCapabilityIndex::compile(
                crate::GraphqlExecutionTargetId::parse("jim-production").expect("target"),
                "schema-v1",
                &semantics,
                None,
                None,
                None,
                [descriptor],
                target_policy,
                crate::AiCapabilityIndexLimits::default(),
            )
            .expect("capability index"),
        )
    }

    fn generated_index(target_policy: &str) -> Arc<AiCapabilityIndex> {
        generated_index_for(
            "generated-read-application",
            "generated-read",
            target_policy,
        )
    }

    fn generated_index_for(
        target_id: &str,
        profile_id: &str,
        target_policy: &str,
    ) -> Arc<AiCapabilityIndex> {
        const SDL: &str = r#"
            schema { query: Query }
            type Query { GeneratedRecord(recordId: ID!): Record! }
            type Record { recordId: ID!, subject: String! }
        "#;
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
        let target = crate::GraphqlExecutionTargetId::parse(target_id)
            .expect("generated target should validate");
        let catalogue = crate::AiGraphqlQueryCapabilityCatalog::compile(
            profile_id,
            target.clone(),
            SDL,
            &semantics,
            crate::AiGraphqlQueryCapabilityLimits::default(),
        )
        .expect("generated query capabilities should compile");
        Arc::new(
            AiCapabilityIndex::compile(
                target,
                catalogue.finished_schema_fingerprint(),
                &semantics,
                Some(&catalogue),
                None,
                None,
                [],
                target_policy,
                crate::AiCapabilityIndexLimits::default(),
            )
            .expect("generated capability index"),
        )
    }

    struct Resolver(AuthPrincipal);

    #[async_trait]
    impl CurrentPrincipalResolver for Resolver {
        async fn resolve(
            &self,
            reference: &PrincipalReference,
        ) -> agql_auth::AuthResult<ResolvedPrincipal> {
            ResolvedPrincipal::new(
                reference.clone(),
                self.0.clone(),
                OffsetDateTime::UNIX_EPOCH,
            )
        }
    }

    struct CurrentIndex(RwLock<Arc<AiCapabilityIndex>>);

    impl AiCurrentCapabilityIndex for CurrentIndex {
        fn current_index(
            &self,
            _run: &AiCapabilityRunBinding,
        ) -> Result<Arc<AiCapabilityIndex>, AiError> {
            self.0
                .read()
                .map(|index| Arc::clone(&index))
                .map_err(|_| AiError::PersistenceFailed)
        }
    }

    struct CurrentIndexes(RwLock<Arc<AiCapabilityIndexSet>>);

    impl AiCurrentCapabilityIndexSet for CurrentIndexes {
        fn current_index_set(
            &self,
            _run: &AiCapabilityRunBinding,
        ) -> Result<Arc<AiCapabilityIndexSet>, AiError> {
            self.0
                .read()
                .map(|indexes| Arc::clone(&indexes))
                .map_err(|_| AiError::PersistenceFailed)
        }
    }

    struct Authority {
        allowed: AtomicBool,
        policy_fingerprint: RwLock<String>,
    }

    #[async_trait]
    impl AiCapabilityAuthorityPolicy for Authority {
        async fn authorize(
            &self,
            _principal: &ResolvedPrincipal,
            _owning_index: &AiCapabilityIndex,
            _entry: &AiCapabilityIndexEntry,
            _run: &AiCapabilityRunBinding,
        ) -> Result<AiCapabilityAuthorityDecision, AiError> {
            Ok(AiCapabilityAuthorityDecision {
                allowed: self.allowed.load(Ordering::SeqCst),
                policy_fingerprint: self
                    .policy_fingerprint
                    .read()
                    .map_err(|_| AiError::PersistenceFailed)?
                    .clone(),
            })
        }
    }

    fn run_binding() -> AiCapabilityRunBinding {
        AiCapabilityRunBinding {
            session_id: AiSessionId::new(),
            run_id: AiRunId::new(),
            attempt_id: Uuid::new_v4(),
            lease_generation: 1,
            provider_kind: ProviderKind::OpenAi,
            provider_session_fingerprint: "provider-session-v1".to_owned(),
        }
    }

    #[test]
    fn frozen_retained_and_large_native_surfaces_select_distinct_modes() {
        let mut capabilities = ProviderCapabilities {
            custom_tools: true,
            ..ProviderCapabilities::default()
        };
        capabilities.capability_delivery_modes = BTreeSet::from([
            AiCapabilityDeliveryMode::EagerExact,
            AiCapabilityDeliveryMode::ProviderDeferred,
            AiCapabilityDeliveryMode::FixedBroker,
        ]);
        let limits = AiCapabilityDeliveryLimits::default();
        assert_eq!(
            select_capability_delivery_mode(&capabilities, 2, 1_024, false, limits)
                .expect("small surface should select")
                .mode(),
            AiCapabilityDeliveryMode::EagerExact
        );
        assert_eq!(
            select_capability_delivery_mode(&capabilities, 55, 4_917_706, false, limits)
                .expect("large surface should select")
                .mode(),
            AiCapabilityDeliveryMode::ProviderDeferred
        );
        assert_eq!(
            select_capability_delivery_mode(&capabilities, 55, 4_917_706, true, limits)
                .expect("frozen surface should select")
                .mode(),
            AiCapabilityDeliveryMode::FixedBroker
        );

        capabilities.capability_delivery_modes =
            BTreeSet::from([AiCapabilityDeliveryMode::ClientDeferred]);
        assert!(matches!(
            select_capability_delivery_mode(&capabilities, 55, 4_917_706, true, limits),
            Err(AiError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn deferred_and_fixed_surfaces_remain_small_and_mode_exact() {
        let fingerprint = "a".repeat(64);
        let definitions = (0..55)
            .map(|index| {
                let mut definition = definition(&format!("jim.generated.{index:02}"));
                definition.parameters["properties"]["id"]["description"] =
                    json!("x".repeat(90_000));
                definition
            })
            .collect::<Vec<_>>();
        assert!(
            definitions
                .iter()
                .map(|definition| serde_json::to_vec(definition).expect("definition").len())
                .sum::<usize>()
                >= 4_900_000
        );
        let provider_deferred = prepare_capability_delivery_surface(
            AiCapabilityDeliveryDecision {
                mode: AiCapabilityDeliveryMode::ProviderDeferred,
            },
            &fingerprint,
            definitions.clone(),
        )
        .expect("native deferred surface should project");
        assert!(
            provider_deferred
                .tools()
                .iter()
                .all(|tool| tool.defer_loading)
        );

        let client = prepare_capability_delivery_surface(
            AiCapabilityDeliveryDecision {
                mode: AiCapabilityDeliveryMode::ClientDeferred,
            },
            &fingerprint,
            definitions.clone(),
        )
        .expect("client discovery surface should project");
        assert_eq!(client.tools().len(), 1);
        assert_eq!(client.tools()[0].tool_id, "graphql.capabilities.discover");
        assert!(
            serde_json::to_vec(client.tools())
                .expect("client surface")
                .len()
                < 4_096
        );

        let fixed = prepare_capability_delivery_surface(
            AiCapabilityDeliveryDecision {
                mode: AiCapabilityDeliveryMode::FixedBroker,
            },
            &fingerprint,
            definitions,
        )
        .expect("fixed broker should project");
        assert_eq!(fixed.tools().len(), 3);
        assert!(
            serde_json::to_vec(fixed.tools())
                .expect("fixed surface")
                .len()
                < 12_288
        );
        assert!(
            fixed
                .tools()
                .iter()
                .all(|tool| serde_json::to_vec(tool).is_ok_and(|bytes| bytes.len() < 4_096))
        );
    }

    #[test]
    fn run_delivery_keeps_exact_static_bootstrap_while_generated_reads_defer() {
        let principal = principal();
        let index = index("target-policy-v1");
        let index_set =
            AiCapabilityIndexSet::compile([Arc::clone(&index)]).expect("capability index set");
        let broker = Arc::new(
            AiCapabilityDiscoveryBroker::new(
                Arc::new(Resolver(principal)),
                Arc::new(CurrentIndex(RwLock::new(index.clone()))),
                Arc::new(Authority {
                    allowed: AtomicBool::new(true),
                    policy_fingerprint: RwLock::new("current-policy-v1".to_owned()),
                }),
                Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
                Duration::seconds(30),
            )
            .expect("broker"),
        );
        let static_tool = definition("jim.jobs_list");
        let binding = AiProviderCapabilitySessionBinding::new(
            AiCapabilityDeliveryMode::FixedBroker,
            index_set.fingerprint(),
            BTreeSet::from([static_tool.fingerprint.clone()]),
            "provider-projection-v1",
            "gpt-test",
            ModelReasoningEffort::Unspecified,
            "f".repeat(64),
        )
        .expect("session binding");
        let capabilities = ProviderCapabilities {
            custom_tools: true,
            capability_delivery_modes: BTreeSet::from([AiCapabilityDeliveryMode::FixedBroker]),
            ..ProviderCapabilities::default()
        };
        let delivery = AiCapabilityDeliveryTurn::select(
            &capabilities,
            index_set.fingerprint(),
            vec![static_tool.clone()],
            true,
            binding,
            broker,
            AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
                .expect("broker session"),
        )
        .expect("fixed delivery");
        let surface = delivery.current_surface();
        assert_eq!(surface.mode(), AiCapabilityDeliveryMode::FixedBroker);
        assert_eq!(surface.tools().len(), 4);
        assert!(surface.tools().contains(&static_tool));
        assert!(
            surface
                .tools()
                .iter()
                .any(|definition| { definition.tool_id == AI_CAPABILITY_DISCOVER_TOOL_ID })
        );
    }

    #[test]
    fn application_definition_cannot_claim_a_reserved_broker_id() {
        let mut reserved = definition(AI_CAPABILITY_EXECUTE_TOOL_ID);
        reserved.provider_name = "application_execute".to_owned();
        assert!(matches!(
            prepare_capability_delivery_surface(
                AiCapabilityDeliveryDecision {
                    mode: AiCapabilityDeliveryMode::EagerExact,
                },
                &"a".repeat(64),
                vec![reserved],
            ),
            Err(AiError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn retained_binding_changes_for_every_execution_relevant_dimension() {
        let make = |mode, effort, model: &str| {
            AiProviderCapabilitySessionBinding::new(
                mode,
                "a".repeat(64),
                BTreeSet::from(["b".repeat(64)]),
                "openai-strict/v1",
                model,
                effort,
                "c".repeat(64),
            )
            .expect("binding should validate")
        };
        let base = make(
            AiCapabilityDeliveryMode::FixedBroker,
            ModelReasoningEffort::Unspecified,
            "gpt-test",
        );
        assert_ne!(
            base.fingerprint(),
            make(
                AiCapabilityDeliveryMode::ProviderDeferred,
                ModelReasoningEffort::Unspecified,
                "gpt-test"
            )
            .fingerprint()
        );
        assert_ne!(
            base.fingerprint(),
            make(
                AiCapabilityDeliveryMode::FixedBroker,
                ModelReasoningEffort::High,
                "gpt-test"
            )
            .fingerprint()
        );
        assert_ne!(
            base.fingerprint(),
            make(
                AiCapabilityDeliveryMode::FixedBroker,
                ModelReasoningEffort::Unspecified,
                "gpt-next"
            )
            .fingerprint()
        );
    }

    #[tokio::test]
    async fn discovery_handles_fail_closed_on_revocation_drift_and_substitution() {
        let principal = principal();
        let principal_reference = principal.reference();
        let current_index = Arc::new(CurrentIndex(RwLock::new(index("target-policy-v1"))));
        let authority = Arc::new(Authority {
            allowed: AtomicBool::new(true),
            policy_fingerprint: RwLock::new("current-policy-v1".to_owned()),
        });
        let clock = Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH));
        let broker = AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal)),
            current_index.clone(),
            authority.clone(),
            clock.clone(),
            Duration::seconds(30),
        )
        .expect("broker");
        let run = run_binding();
        let search = broker
            .search(
                &principal_reference,
                &run,
                &AiCapabilitySearchQuery {
                    text: "latest Jim jobs".to_owned(),
                    namespace: Some("jim".to_owned()),
                    kind: Some(AiCapabilityKind::ReviewedStatic),
                    entity_or_operation: None,
                    maximum_results: 1,
                },
            )
            .await
            .expect("authorized search");
        let id = search.candidates[0].id.clone();
        let loaded = broker
            .load(&principal_reference, &run, &search, &id)
            .await
            .expect("authorized load");
        broker
            .authorize_execution(&principal_reference, &run, &loaded)
            .await
            .expect("current binding");

        let mut renewed_run = run.clone();
        renewed_run.lease_generation += 1;
        broker
            .authorize_execution(&principal_reference, &renewed_run, &loaded)
            .await
            .expect("a monotonic lease renewal must preserve the exact run binding");
        let mut stale_run = run.clone();
        stale_run.lease_generation -= 1;
        assert!(matches!(
            broker
                .authorize_execution(&principal_reference, &stale_run, &loaded)
                .await,
            Err(AiError::Forbidden)
        ));

        let mut substituted_principal = principal_reference.clone();
        substituted_principal.subject = "another-user".to_owned();
        assert!(matches!(
            broker
                .authorize_execution(&substituted_principal, &run, &loaded)
                .await,
            Err(AiError::Forbidden)
        ));

        authority.allowed.store(false, Ordering::SeqCst);
        assert!(matches!(
            broker
                .authorize_execution(&principal_reference, &run, &loaded)
                .await,
            Err(AiError::Forbidden)
        ));
        authority.allowed.store(true, Ordering::SeqCst);

        let mut substituted_run = run_binding();
        substituted_run.provider_kind = ProviderKind::Anthropic;
        assert!(matches!(
            broker
                .authorize_execution(&principal_reference, &substituted_run, &loaded)
                .await,
            Err(AiError::Forbidden)
        ));

        *current_index.0.write().expect("index write") = index("target-policy-v2");
        assert!(matches!(
            broker
                .authorize_execution(&principal_reference, &run, &loaded)
                .await,
            Err(AiError::Forbidden)
        ));

        *current_index.0.write().expect("index write") = index("target-policy-v1");
        *authority.policy_fingerprint.write().expect("policy write") =
            "current-policy-v2".to_owned();
        assert!(matches!(
            broker
                .authorize_execution(&principal_reference, &run, &loaded)
                .await,
            Err(AiError::Forbidden)
        ));

        *authority.policy_fingerprint.write().expect("policy write") =
            "current-policy-v1".to_owned();
        clock.advance_seconds(31);
        assert!(matches!(
            broker
                .authorize_execution(&principal_reference, &run, &loaded)
                .await,
            Err(AiError::Forbidden)
        ));

        let mut kind_substitution = search;
        kind_substitution.candidates[0].kind = AiCapabilityKind::GeneratedQuery;
        assert!(matches!(
            broker
                .load(&principal_reference, &run, &kind_substitution, &id)
                .await,
            Err(AiError::Forbidden)
        ));
    }

    #[tokio::test]
    async fn broker_searches_and_reauthorizes_across_exact_owning_indexes() {
        let first = generated_index_for("first-application", "first-read", "first-policy-v1");
        let second = generated_index_for("second-application", "second-read", "second-policy-v1");
        let first_id = first.entries().next().expect("first entry").id.clone();
        let second_id = second.entries().next().expect("second entry").id.clone();
        assert_ne!(first_id, second_id);
        let initial_set = Arc::new(
            AiCapabilityIndexSet::compile([Arc::clone(&first), Arc::clone(&second)])
                .expect("multi-target set"),
        );
        let current_indexes = Arc::new(CurrentIndexes(RwLock::new(initial_set)));
        let principal = principal();
        let principal_reference = principal.reference();
        let broker = AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal)),
            current_indexes.clone(),
            Arc::new(Authority {
                allowed: AtomicBool::new(true),
                policy_fingerprint: RwLock::new("current-policy-v1".to_owned()),
            }),
            Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
            Duration::seconds(30),
        )
        .expect("broker");
        let run = run_binding();
        let search = broker
            .search(
                &principal_reference,
                &run,
                &AiCapabilitySearchQuery {
                    text: "reviewed record".to_owned(),
                    namespace: None,
                    kind: Some(AiCapabilityKind::GeneratedQuery),
                    entity_or_operation: None,
                    maximum_results: 2,
                },
            )
            .await
            .expect("cross-target search");
        assert_eq!(search.candidates.len(), 2);
        let loaded = broker
            .load(&principal_reference, &run, &search, &second_id)
            .await
            .expect("second-target load");
        broker
            .authorize_execution(&principal_reference, &run, &loaded)
            .await
            .expect("second-target execution authorization");

        let mut substituted = search.clone();
        let first_fingerprint = substituted
            .candidates
            .iter()
            .find(|candidate| candidate.id == first_id)
            .expect("first candidate")
            .entry_fingerprint
            .clone();
        substituted
            .candidates
            .iter_mut()
            .find(|candidate| candidate.id == second_id)
            .expect("second candidate")
            .entry_fingerprint = first_fingerprint;
        assert!(matches!(
            broker
                .load(&principal_reference, &run, &substituted, &second_id)
                .await,
            Err(AiError::Forbidden)
        ));

        let drifted_second =
            generated_index_for("second-application", "second-read", "second-policy-v2");
        *current_indexes.0.write().expect("set write") =
            Arc::new(AiCapabilityIndexSet::compile([first, drifted_second]).expect("drifted set"));
        assert!(matches!(
            broker
                .authorize_execution(&principal_reference, &run, &loaded)
                .await,
            Err(AiError::Forbidden)
        ));
    }

    #[tokio::test]
    async fn broker_applies_authority_to_each_exact_owning_target() {
        struct TargetAuthority;

        #[async_trait]
        impl AiCapabilityAuthorityPolicy for TargetAuthority {
            async fn authorize(
                &self,
                _principal: &ResolvedPrincipal,
                owning_index: &AiCapabilityIndex,
                _entry: &AiCapabilityIndexEntry,
                _run: &AiCapabilityRunBinding,
            ) -> Result<AiCapabilityAuthorityDecision, AiError> {
                Ok(AiCapabilityAuthorityDecision {
                    allowed: owning_index.target_id().as_str() == "second-application",
                    policy_fingerprint: format!(
                        "{}-current-policy",
                        owning_index.target_id().as_str()
                    ),
                })
            }
        }

        let first = generated_index_for("first-application", "first-read", "first-policy-v1");
        let second = generated_index_for("second-application", "second-read", "second-policy-v1");
        let first_id = first.entries().next().expect("first entry").id.clone();
        let second_id = second.entries().next().expect("second entry").id.clone();
        let indexes = Arc::new(CurrentIndexes(RwLock::new(Arc::new(
            AiCapabilityIndexSet::compile([first, second]).expect("multi-target set"),
        ))));
        let principal = principal();
        let principal_reference = principal.reference();
        let broker = AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal)),
            indexes,
            Arc::new(TargetAuthority),
            Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
            Duration::seconds(30),
        )
        .expect("broker");
        let run = run_binding();
        let search = broker
            .search(
                &principal_reference,
                &run,
                &AiCapabilitySearchQuery {
                    text: "reviewed record".to_owned(),
                    namespace: None,
                    kind: Some(AiCapabilityKind::GeneratedQuery),
                    entity_or_operation: None,
                    maximum_results: 2,
                },
            )
            .await
            .expect("target-filtered search");
        assert_eq!(search.candidates.len(), 1);
        assert_eq!(search.candidates[0].id, second_id);
        assert_ne!(search.candidates[0].id, first_id);
        let loaded = broker
            .load(
                &principal_reference,
                &run,
                &search,
                &search.candidates[0].id,
            )
            .await
            .expect("authorized owning target should load");
        broker
            .authorize_execution(&principal_reference, &run, &loaded)
            .await
            .expect("authorized owning target should reauthorize");
    }

    #[tokio::test]
    async fn client_deferred_discovery_loads_only_exact_bounded_generated_queries() {
        let principal = principal();
        let principal_reference = principal.reference();
        let current_index = Arc::new(CurrentIndex(RwLock::new(generated_index(
            "target-policy-v1",
        ))));
        let authority = Arc::new(Authority {
            allowed: AtomicBool::new(true),
            policy_fingerprint: RwLock::new("current-policy-v1".to_owned()),
        });
        let broker = Arc::new(
            AiCapabilityDiscoveryBroker::new(
                Arc::new(Resolver(principal)),
                current_index.clone(),
                authority.clone(),
                Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
                Duration::seconds(30),
            )
            .expect("broker"),
        );
        let limits = AiCapabilityDeliveryLimits::default();
        let session = AiCapabilityBrokerSession::new(limits).expect("session");
        let index_fingerprint =
            AiCapabilityIndexSet::compile([current_index.0.read().expect("index read").clone()])
                .expect("index set")
                .fingerprint()
                .to_owned();
        let binding = AiProviderCapabilitySessionBinding::new(
            AiCapabilityDeliveryMode::ClientDeferred,
            &index_fingerprint,
            BTreeSet::new(),
            "provider-projection-v1",
            "gpt-test",
            ModelReasoningEffort::Unspecified,
            "f".repeat(64),
        )
        .expect("session binding");
        let delivery = AiCapabilityDeliveryTurn::select(
            &ProviderCapabilities {
                custom_tools: true,
                capability_delivery_modes: BTreeSet::from([
                    AiCapabilityDeliveryMode::ClientDeferred,
                ]),
                ..ProviderCapabilities::default()
            },
            &index_fingerprint,
            Vec::new(),
            false,
            binding,
            broker.clone(),
            session.clone(),
        )
        .expect("client-deferred delivery");
        let value = broker
            .dispatch_client_deferred_discover(
                &principal_reference,
                &run_binding(),
                &session,
                &json!({
                    "text": "reviewed record",
                    "kind": "generated_query",
                    "maximumResults": 1
                }),
            )
            .await
            .expect("bounded generated discovery");
        assert_eq!(value["candidates"].as_array().map(Vec::len), Some(1));
        assert_eq!(session.loaded_binding_count(), 1);
        assert_eq!(session.amplification().discover_calls, 1);
        assert_eq!(session.amplification().describe_calls, 0);
        assert!(delivery.requires_deferred_installation());
        let candidate = &value["candidates"][0];
        let mut loaded_definition = definition(
            candidate["capabilityId"]
                .as_str()
                .expect("candidate should expose an ID"),
        );
        loaded_definition.fingerprint = session.loaded_bindings()[0]
            .capability_fingerprint()
            .to_owned();
        delivery
            .install_deferred_definitions(vec![loaded_definition])
            .expect("exact loaded definition should install");
        assert_eq!(delivery.current_tools().len(), 2);
        assert!(!delivery.requires_deferred_installation());

        authority.allowed.store(false, Ordering::SeqCst);
        let empty = broker
            .dispatch_client_deferred_discover(
                &principal_reference,
                &run_binding(),
                &session,
                &json!({
                    "text": "reviewed record",
                    "kind": "generated_query",
                    "maximumResults": 1
                }),
            )
            .await
            .expect("an empty currently-authorized search is valid");
        assert_eq!(empty["candidates"].as_array().map(Vec::len), Some(0));
        assert!(delivery.requires_deferred_installation());
        delivery
            .install_deferred_definitions(Vec::new())
            .expect("empty discovery should clear stale definitions");
        assert_eq!(delivery.current_tools().len(), 1);
        assert_eq!(
            delivery.current_tools()[0].tool_id,
            AI_CAPABILITY_DISCOVER_TOOL_ID
        );
        assert!(!delivery.requires_deferred_installation());
        authority.allowed.store(true, Ordering::SeqCst);

        assert!(matches!(
            broker
                .dispatch_client_deferred_discover(
                    &principal_reference,
                    &run_binding(),
                    &session,
                    &json!({
                        "text": "record",
                        "kind": "reviewed_static",
                        "maximumResults": 1
                    }),
                )
                .await,
            Err(AiError::InvalidInput(_))
        ));
        assert!(matches!(
            broker
                .dispatch_client_deferred_discover(
                    &principal_reference,
                    &run_binding(),
                    &session,
                    &json!({
                        "text": "record",
                        "maximumResults": limits.maximum_deferred_definitions + 1
                    }),
                )
                .await,
            Err(AiError::InvalidInput(_))
        ));
    }

    #[test]
    fn fixed_broker_argument_paths_build_closed_nested_inputs() {
        let mut value = serde_json::Value::Object(serde_json::Map::new());
        let mut seen = BTreeSet::new();
        insert_broker_argument(
            &mut value,
            "filter.and.0.status.eq",
            json!("open"),
            &mut seen,
        )
        .expect("nested filter should be representable");
        insert_broker_argument(&mut value, "filter.and.1.priority.gte", json!(2), &mut seen)
            .expect("second bounded list item should be representable");
        assert_eq!(
            value,
            json!({
                "filter": {
                    "and": [
                        {"status": {"eq": "open"}},
                        {"priority": {"gte": 2}}
                    ]
                }
            })
        );

        assert!(matches!(
            insert_broker_argument(
                &mut value,
                "filter.and.0.status.eq",
                json!("closed"),
                &mut seen,
            ),
            Err(AiError::InvalidInput(_))
        ));
        assert!(matches!(
            insert_broker_argument(
                &mut value,
                "filter.and.64.status.eq",
                json!("closed"),
                &mut seen,
            ),
            Err(AiError::InvalidInput(_))
        ));
        assert!(matches!(
            insert_broker_argument(&mut value, "filter.raw", json!({"sql": "never"}), &mut seen,),
            Err(AiError::InvalidInput(_))
        ));
    }

    #[test]
    fn fixed_broker_execute_admits_the_minimal_schema_aligned_wrapper() {
        let definitions = fixed_broker_definitions(&"a".repeat(64));
        let execute = definitions
            .iter()
            .find(|definition| definition.tool_id == AI_CAPABILITY_EXECUTE_TOOL_ID)
            .expect("fixed execute definition");
        let validator = jsonschema::validator_for(&execute.parameters)
            .expect("fixed execute parameters should be valid JSON Schema");
        let minimal = json!({
            "loadedReference": "b".repeat(64),
            "selections": ["records.id"]
        });
        assert!(validator.is_valid(&minimal));
        let parsed: BrokerExecuteArguments =
            serde_json::from_value(minimal).expect("minimal execute wrapper should decode");
        assert!(parsed.arguments.is_empty());
        assert!(parsed.relationship_arguments.is_empty());
        assert!(parsed.relationship_maximum_items.is_empty());
        assert_eq!(parsed.maximum_items, None);

        assert_eq!(
            execute.parameters["required"],
            json!(["loadedReference", "selections"])
        );
        assert!(execute.description.contains("returned planSchema"));
        assert!(
            execute.parameters["properties"]["maximumItems"]["description"]
                .as_str()
                .is_some_and(|description| description.contains("only when planSchema exposes"))
        );
    }

    #[tokio::test]
    async fn describe_exposes_compiler_owned_result_record_cost_bounds() {
        let principal = principal();
        let principal_reference = principal.reference();
        let current_index = Arc::new(CurrentIndex(RwLock::new(generated_index(
            "target-policy-v1",
        ))));
        let broker = AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal)),
            current_index,
            Arc::new(Authority {
                allowed: AtomicBool::new(true),
                policy_fingerprint: RwLock::new("current-policy-v1".to_owned()),
            }),
            Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
            Duration::seconds(30),
        )
        .expect("broker");
        let session = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
            .expect("broker session");
        let run = run_binding();
        let discovery = broker
            .dispatch_discover(
                &principal_reference,
                &run,
                &session,
                &json!({
                    "text": "reviewed application record",
                    "kind": "generated_query",
                    "maximumResults": 1
                }),
            )
            .await
            .expect("discovery");
        let candidate = &discovery["candidates"][0];
        let description = broker
            .dispatch_describe(
                &principal_reference,
                &run,
                &session,
                &json!({
                    "capabilityId": candidate["capabilityId"],
                    "candidateFingerprint": candidate["candidateFingerprint"]
                }),
            )
            .await
            .expect("description");
        assert_eq!(
            description.contract()["resultRecordCost"],
            json!({
                "maximumRootRecords": 1,
                "maximumTotalRecords": 100,
                "rootBoundRequired": false
            })
        );
    }

    #[test]
    fn describe_bounds_the_complete_planning_contract_without_truncation() {
        let description = AiCapabilityDescription {
            loaded_reference: "a".repeat(64),
            capability_id: AiToolId::parse("jim.query.records.auto").expect("capability ID"),
            capability_kind: AiCapabilityKind::GeneratedQuery,
            capability_fingerprint: "b".repeat(64),
            contract: json!({"loadedReference": "a".repeat(64)}),
        };
        let bounded = description
            .clone()
            .with_plan_schema(
                json!({
                    "type": "object",
                    "properties": {},
                    "required": [],
                    "additionalProperties": false
                }),
                1_024,
            )
            .expect("small complete contract should fit");
        assert_eq!(bounded.contract()["planSchemaAvailable"], true);
        assert!(bounded.contract().get("planSchema").is_some());

        let oversized = description
            .with_plan_schema(json!({"description": "x".repeat(2_048)}), 1_024)
            .expect("oversized schema should become an explicit unavailable contract");
        assert_eq!(oversized.contract()["planSchemaAvailable"], false);
        assert!(oversized.contract().get("planSchema").is_none());
        assert!(serde_json::to_vec(oversized.contract()).is_ok_and(|value| value.len() <= 1_024));

        let oversized_metadata = AiCapabilityDescription {
            loaded_reference: "a".repeat(64),
            capability_id: AiToolId::parse("jim.query.records.auto").expect("capability ID"),
            capability_kind: AiCapabilityKind::GeneratedQuery,
            capability_fingerprint: "b".repeat(64),
            contract: json!({"description": "x".repeat(2_048)}),
        };
        assert!(matches!(
            oversized_metadata.with_plan_schema(json!({"type": "object"}), 1_024),
            Err(AiError::InvalidConfiguration(_))
        ));
    }
}
