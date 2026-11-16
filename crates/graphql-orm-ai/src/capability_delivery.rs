//! Provider-neutral capability delivery, loading, and run fencing.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use agql_auth::{Clock, CurrentPrincipalResolver, PrincipalReference, ResolvedPrincipal};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    AiApprovalRule, AiCapabilityIndex, AiCapabilityIndexEntry, AiCapabilityIndexSet,
    AiCapabilityIndexSetSearchResult, AiCapabilityKind, AiCapabilityRankedMatch,
    AiCapabilitySearchQuery, AiError, AiRunId, AiSessionId, AiSharedTypePlanSchema, AiToolId,
    AiToolRisk, DataClassification, ModelReasoningEffort, ModelToolDefinition,
    ProviderCapabilities, ProviderKind,
};

/// Frozen broker tool identifier for bounded capability discovery.
pub const AI_CAPABILITY_DISCOVER_TOOL_ID: &str = "graphql.capabilities.discover";
/// Frozen broker tool identifier for loading one exact capability contract.
pub const AI_CAPABILITY_DESCRIBE_TOOL_ID: &str = "graphql.capabilities.describe";
/// Frozen broker tool identifier for executing one loaded exact capability.
pub const AI_CAPABILITY_EXECUTE_TOOL_ID: &str = "graphql.capabilities.execute";
/// Frozen broker tool identifier for expanding one named type of a loaded
/// capability's signature (broker version 2).
pub const AI_CAPABILITY_DESCRIBE_TYPE_TOOL_ID: &str = "graphql.capabilities.describe_type";

/// Current broker protocol version.
///
/// Version 2 adds the `describe_type` operation, returns compact typed
/// signatures from `describe` instead of the plan schema, discovers one
/// entity capability per entity, and takes the entity's `operation` at
/// `describe_type` and `execute`. The version is hashed into the fixed broker
/// definitions and every fixed-broker session binding, so a session bound
/// under version 1 rebinds once.
pub const AI_CAPABILITY_BROKER_VERSION: u16 = 2;

/// Maximum reviewed static tools one run may reach through the fixed broker.
///
/// Brokered static tools are not provider definitions, so this bound is
/// independent of the 128 exact static bootstrap fingerprints a retained
/// provider session admits.
pub const AI_CAPABILITY_MAXIMUM_BROKERED_STATIC_TOOLS: usize = 8_192;

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
    /// Prefer the fixed broker over provider-native deferred loading when the
    /// exact definitions exceed the eager limits. On by default; set `false`
    /// (or call [`Self::with_provider_deferred_for_large_surfaces`]) to keep
    /// `ProviderDeferred` for large surfaces.
    #[serde(
        default = "default_prefer_fixed_broker_for_large_surfaces",
        skip_serializing_if = "is_true"
    )]
    pub prefer_fixed_broker_for_large_surfaces: bool,
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

// Serde's `skip_serializing_if` passes the field by reference.
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_true(value: &bool) -> bool {
    *value
}

const fn default_prefer_fixed_broker_for_large_surfaces() -> bool {
    true
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
            prefer_fixed_broker_for_large_surfaces: default_prefer_fixed_broker_for_large_surfaces(
            ),
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
    /// Opts out of the fixed-broker preference: a provider that supports
    /// `ProviderDeferred` keeps native deferred loading above the eager
    /// limits, as before broker version 2.
    ///
    /// The selected mode is part of the retained session binding, so
    /// changing this setting rebinds retained sessions.
    #[must_use]
    pub const fn with_provider_deferred_for_large_surfaces(mut self) -> Self {
        self.prefer_fixed_broker_for_large_surfaces = false;
        self
    }

    /// Rejects a limit set that cannot represent a complete
    /// discover/describe/describe-type/execute broker interaction.
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
#[derive(Clone, Debug)]
pub struct AiCapabilityDeliverySurface {
    mode: AiCapabilityDeliveryMode,
    tools: Vec<ModelToolDefinition>,
    #[cfg(any(test, feature = "sqlite", feature = "postgres"))]
    offering: Option<AiCapabilityOfferingSnapshot>,
}

#[derive(Debug)]
struct AiCapabilityOfferingState {
    tools: Vec<ModelToolDefinition>,
    static_fingerprints: BTreeSet<String>,
    generation: u64,
    frozen_plans: usize,
}

#[cfg(any(test, feature = "sqlite", feature = "postgres"))]
#[derive(Clone, Debug)]
struct AiCapabilityOfferingSnapshot {
    state: Arc<Mutex<AiCapabilityOfferingState>>,
    generation: u64,
}

impl PartialEq for AiCapabilityDeliverySurface {
    fn eq(&self, other: &Self) -> bool {
        self.mode == other.mode && self.tools == other.tools
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AiCapabilityOfferingPlanGuard(Arc<AiCapabilityOfferingPlanFreeze>);

#[derive(Debug)]
struct AiCapabilityOfferingPlanFreeze {
    state: Arc<Mutex<AiCapabilityOfferingState>>,
    generation: u64,
}

impl Drop for AiCapabilityOfferingPlanFreeze {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.frozen_plans = state.frozen_plans.saturating_sub(1);
    }
}

impl AiCapabilityOfferingPlanGuard {
    #[cfg_attr(feature = "mssql", allow(dead_code))]
    pub(crate) fn validate(&self) -> Result<(), AiError> {
        let state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.generation != self.0.generation {
            return Err(AiError::Conflict);
        }
        Ok(())
    }
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

    #[cfg(any(test, feature = "sqlite", feature = "postgres"))]
    pub(crate) fn freeze_offering(&self) -> Result<Option<AiCapabilityOfferingPlanGuard>, AiError> {
        let Some(snapshot) = &self.offering else {
            return Ok(None);
        };
        let mut state = snapshot
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.generation != snapshot.generation || state.tools != self.tools {
            return Err(AiError::Conflict);
        }
        state.frozen_plans = state.frozen_plans.checked_add(1).ok_or(AiError::Conflict)?;
        Ok(Some(AiCapabilityOfferingPlanGuard(Arc::new(
            AiCapabilityOfferingPlanFreeze {
                state: snapshot.state.clone(),
                generation: snapshot.generation,
            },
        ))))
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
                | AI_CAPABILITY_DESCRIBE_TYPE_TOOL_ID
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
            vec![discovery_definition(Some(index_fingerprint))]
        }
        AiCapabilityDeliveryMode::FixedBroker => fixed_broker_definitions(),
    };
    Ok(AiCapabilityDeliverySurface {
        mode: decision.mode,
        tools,
        #[cfg(any(test, feature = "sqlite", feature = "postgres"))]
        offering: None,
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

/// `index_fingerprint` is `Some` only for client-deferred delivery, whose
/// installed definitions are catalogue-derived. The fixed broker passes `None`:
/// its three definitions depend on the broker protocol version alone, so a
/// catalogue change never alters the retained provider surface.
fn discovery_definition(index_fingerprint: Option<&str>) -> ModelToolDefinition {
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
            "required": ["text", "maximumResults"],
            "additionalProperties": false
        }),
    )
}

fn fixed_discovery_definition() -> ModelToolDefinition {
    broker_definition(
        AI_CAPABILITY_DISCOVER_TOOL_ID,
        "graphql_capabilities_discover",
        "Search the current reviewed read capabilities. A generated_entity candidate covers every read operation of one entity; its operations list names them.",
        None,
        json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "maxLength": 1024},
                "namespace": {"type": ["string", "null"], "maxLength": 256},
                "kind": {
                    "type": ["string", "null"],
                    "enum": ["generated_query", "generated_entity", "reviewed_static", null],
                    "description": "generated_query: every generated read; generated_entity: entity capabilities only; reviewed_static: reviewed tools only."
                },
                "entityOrClass": {"type": ["string", "null"], "maxLength": 256},
                "maximumResults": {"type": "integer", "minimum": 1, "maximum": 32}
            },
            "required": ["text", "maximumResults"],
            "additionalProperties": false
        }),
    )
}

fn fixed_broker_definitions() -> Vec<ModelToolDefinition> {
    let index_fingerprint = None;
    let discover = fixed_discovery_definition();
    let describe = broker_definition(
        AI_CAPABILITY_DESCRIBE_TOOL_ID,
        "graphql_capabilities_describe",
        "Load one exact current capability. Returns a loadedReference and one compact typed signature per operation; expand any named type with graphql_capabilities_describe_type.",
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
    let describe_type = broker_definition(
        AI_CAPABILITY_DESCRIBE_TYPE_TOOL_ID,
        "graphql_capabilities_describe_type",
        "Expand one named type of a loaded capability's signature: an input type from inputTypes, select, or select.<relationship path>. Long types are paged with offset.",
        index_fingerprint,
        json!({
            "type": "object",
            "properties": {
                "loadedReference": {
                    "type": "string", "minLength": 64, "maxLength": 64,
                    "description": "Exact loadedReference returned by the matching describe call."
                },
                "operation": {
                    "type": ["string", "null"], "minLength": 1, "maxLength": 256,
                    "description": "Operation of an entity capability, from its describe result. Omit for a capability with one operation."
                },
                "type": {"type": "string", "minLength": 1, "maxLength": 512},
                "offset": {"type": ["integer", "null"], "minimum": 0, "maximum": 1000000},
                "limit": {"type": ["integer", "null"], "minimum": 1, "maximum": 128}
            },
            "required": ["loadedReference", "type"],
            "additionalProperties": false
        }),
    );
    let execute = broker_definition(
        AI_CAPABILITY_EXECUTE_TOOL_ID,
        "graphql_capabilities_execute",
        "Execute one operation of a previously loaded exact capability. Use only argument and selection paths its signature and expanded types admit: flatten argument leaves into name/value entries with dotted paths, and omit optional fields you do not need.",
        index_fingerprint,
        json!({
            "type": "object",
            "properties": {
                "loadedReference": {
                    "type": "string", "minLength": 64, "maxLength": 64,
                    "description": "Exact loadedReference returned by the matching describe call."
                },
                "operation": {
                    "type": ["string", "null"], "minLength": 1, "maxLength": 256,
                    "description": "Operation of an entity capability, from its describe result. Omit for a capability with one operation."
                },
                "arguments": {
                    "type": "array", "maxItems": 64,
                    "description": "Optional root arguments of the signature. Flatten each supplied scalar leaf into one name/value entry using its dotted public path, for example where.status.eq; omit this array when no root argument is needed.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string", "minLength": 1, "maxLength": 256,
                                "description": "Exact dotted public argument path admitted by the signature."
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
                    "description": "One or more exact dotted scalar paths from the select type and its select.<path> expansions.",
                    "items": {"type": "string", "maxLength": 512}
                },
                "relationshipArguments": {
                    "type": "array", "maxItems": 64,
                    "description": "Optional arguments of a selected relationship path, as listed by its select type. Omit when none applies.",
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
                    "description": "Optional bounds of selected relationship paths whose select type lists maximumItems. Omit otherwise.",
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
                    "description": "Root result bound. Supply a positive value when the operation lists maximumItems; otherwise omit it."
                }
            },
            "required": ["loadedReference", "selections"],
            "additionalProperties": false
        }),
    );
    vec![discover, describe, describe_type, execute]
}

fn broker_definition(
    tool_id: &str,
    provider_name: &str,
    description: &str,
    index_fingerprint: Option<&str>,
    parameters: serde_json::Value,
) -> ModelToolDefinition {
    let fingerprint = match index_fingerprint {
        Some(index_fingerprint) => hash_json(&json!({
            "version": 1,
            "tool_id": tool_id,
            "index_fingerprint": index_fingerprint,
            "parameters": parameters.clone(),
        })),
        None => hash_json(&json!({
            "version": 2,
            "tool_id": tool_id,
            "broker_version": AI_CAPABILITY_BROKER_VERSION,
            "parameters": parameters.clone(),
        })),
    };
    ModelToolDefinition {
        tool_id: tool_id.to_owned(),
        provider_name: provider_name.to_owned(),
        fingerprint,
        description: description.to_owned(),
        parameters,
        strict: true,
        defer_loading: false,
    }
}

/// Selects the safest admissible delivery mode from provider declarations and
/// exact definition size. Prompt text cannot influence this decision.
///
/// Above the eager limits a provider that supports both `ProviderDeferred`
/// and `FixedBroker` gets the constant-size fixed broker by default, so no
/// selected mode's request size grows with the catalogue. Clear
/// [`AiCapabilityDeliveryLimits::prefer_fixed_broker_for_large_surfaces`] to
/// keep native deferred loading. The selected mode is part of the retained
/// session binding, so changing it rebinds retained sessions.
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
        if limits.prefer_fixed_broker_for_large_surfaces
            && !eager_fits
            && supported.contains(&AiCapabilityDeliveryMode::FixedBroker)
        {
            // Never choose a mode whose request size grows with the
            // catalogue while a constant-size one is available.
            Some(AiCapabilityDeliveryMode::FixedBroker)
        } else {
            Some(AiCapabilityDeliveryMode::ProviderDeferred)
        }
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
    /// Canonical capability-index-set fingerprint for catalogue-derived
    /// modes; `None` for the fixed broker.
    capability_index_fingerprint: Option<String>,
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
    /// `EagerExact`, `ProviderDeferred` and `ClientDeferred` definitions are
    /// derived from the catalogue, so their binding includes
    /// `capability_index_fingerprint` and rotates when the catalogue changes.
    ///
    /// A `FixedBroker` binding covers the broker protocol version
    /// ([`AI_CAPABILITY_BROKER_VERSION`]), the static bootstrap surface, the
    /// delivery mode and the provider identity only. The supplied index
    /// fingerprint is validated and then dropped: it is neither stored nor
    /// hashed, so a schema or catalogue change keeps the retained session.
    /// Describe and execute remain authoritative: each revalidates the current
    /// index, schema, entry and policy fingerprints and refuses a drifted
    /// selection with the recoverable [`AI_CAPABILITY_STALE`] outcome.
    ///
    /// # Errors
    ///
    /// Rejects missing, unbounded, or control-character-bearing values and more
    /// than 128 static bootstrap fingerprints. Provider-specific definition and
    /// byte limits still apply independently to the complete projected surface.
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
            || static_bootstrap_tool_fingerprints.len() > 128
            || static_bootstrap_tool_fingerprints
                .iter()
                .any(|value| !crate::valid_sha256(value))
        {
            return Err(AiError::InvalidConfiguration(
                "provider bootstrap tool binding is invalid".to_owned(),
            ));
        }
        let (capability_index_fingerprint, fingerprint) =
            if delivery_mode == AiCapabilityDeliveryMode::FixedBroker {
                let fingerprint = hash_json(&json!({
                    "version": 2,
                    "delivery_mode": delivery_mode,
                    "broker_version": AI_CAPABILITY_BROKER_VERSION,
                    "static_bootstrap_tool_fingerprints": static_bootstrap_tool_fingerprints,
                    "provider_projection_version": provider_projection_version,
                    "model": model,
                    "reasoning_effort": reasoning_effort,
                    "registration_identity": registration_identity,
                }));
                (None, fingerprint)
            } else {
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
                (Some(capability_index_fingerprint), fingerprint)
            };
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

    /// Exact canonical capability-index-set fingerprint bound into a
    /// catalogue-derived delivery mode.
    ///
    /// Returns `None` for [`AiCapabilityDeliveryMode::FixedBroker`], whose
    /// binding does not depend on the catalogue.
    pub fn capability_index_fingerprint(&self) -> Option<&str> {
        self.capability_index_fingerprint.as_deref()
    }

    /// Exact canonical capability-index-set fingerprint.
    ///
    /// This is the explicit multi-target name for
    /// [`Self::capability_index_fingerprint`].
    pub fn capability_index_set_fingerprint(&self) -> Option<&str> {
        self.capability_index_fingerprint.as_deref()
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

/// Number of per-entry [`AiCapabilityAuthorityPolicy::authorize`] calls the
/// default [`AiCapabilityAuthorityPolicy::authorize_discovery`] keeps in flight.
pub const AI_CAPABILITY_DISCOVERY_AUTHORIZATION_CONCURRENCY: usize = 16;

/// Stable [`AiError::InvalidInput`] message returned when one discovery query
/// matches more entries than the broker's configured
/// [`AiCapabilityDiscoveryBroker::with_maximum_discovery_authorizations`].
///
/// The model should narrow the query. The refusal depends only on how many
/// entries match, permitted or not, and is decided before any policy call.
pub const AI_CAPABILITY_DISCOVERY_TOO_BROAD: &str =
    "capability discovery query matches too many capabilities; narrow the query";

/// Current host discovery decision bound to the exact ranked entry it decides.
///
/// The broker refuses a batch unless decision `i` names the capability of
/// ranked match `i`, so a host cannot attach one entry's decision to another.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiCapabilityDiscoveryDecision {
    /// Exact capability this decision applies to.
    pub capability_id: AiToolId,
    /// Whether the current principal may see and load the capability.
    pub allowed: bool,
}

impl AiCapabilityDiscoveryDecision {
    /// Binds one decision to the exact ranked match it decides.
    pub fn for_match(candidate: &AiCapabilityRankedMatch<'_>, allowed: bool) -> Self {
        Self {
            capability_id: candidate.entry().id.clone(),
            allowed,
        }
    }
}

/// Fresh host policy applied after principal rehydration for every load and
/// again for every execution.
///
/// A denial is `Ok` with `allowed: false`. Return `Err` only when policy
/// cannot be evaluated, for example an unavailable store. Discovery fails the
/// whole search on any `Err`, so returning `Err(Forbidden)` for an entry the
/// principal may not see turns search success into a signal that a hidden
/// entry matched the query.
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

    /// Applies current policy to every ranked discovery match of one search.
    ///
    /// The broker passes every entry that matches the query, in rank order and
    /// before the query's result ceiling, and truncates only after this
    /// returns. A restricted principal therefore receives its permitted
    /// candidates even when unpermitted entries rank above them. When every
    /// policy call succeeds, the amount of policy work depends on the query
    /// and catalogue, never on which candidates the principal may use.
    ///
    /// One search can therefore evaluate up to the index set's
    /// `maximum_entries` matches (16,384 by default). Bound it with
    /// [`AiCapabilityDiscoveryBroker::with_maximum_discovery_authorizations`].
    ///
    /// The result must hold exactly one decision per match, in the same order,
    /// and decision `i` must carry the `capability_id` of `matches[i]`; build
    /// each with [`AiCapabilityDiscoveryDecision::for_match`]. The broker
    /// refuses a missing, extra, duplicated or reordered decision with
    /// [`AiError::Forbidden`] and returns no candidates.
    ///
    /// The default calls [`Self::authorize`] for each match, keeping up to
    /// [`AI_CAPABILITY_DISCOVERY_AUTHORIZATION_CONCURRENCY`] calls in flight on
    /// the calling task, and stops at the first error. Override it to decide
    /// the batch from one current lookup. An override must apply the same
    /// current decision [`Self::authorize`] would and must not cache an
    /// authority decision across searches.
    ///
    /// # Errors
    ///
    /// Returns the first policy failure. A denial is not a failure: report it
    /// as `allowed: false`, because any `Err` fails the whole search and so
    /// reveals that the failing entry matched.
    async fn authorize_discovery(
        &self,
        principal: &ResolvedPrincipal,
        matches: &[AiCapabilityRankedMatch<'_>],
        run: &AiCapabilityRunBinding,
    ) -> Result<Vec<AiCapabilityDiscoveryDecision>, AiError> {
        let mut pending = futures::stream::FuturesOrdered::new();
        let mut remaining = matches.iter();
        let mut decisions = Vec::with_capacity(matches.len());
        loop {
            while pending.len() < AI_CAPABILITY_DISCOVERY_AUTHORIZATION_CONCURRENCY {
                let Some(candidate) = remaining.next() else {
                    break;
                };
                pending.push_back(self.authorize(
                    principal,
                    candidate.index(),
                    candidate.entry(),
                    run,
                ));
            }
            let Some(decision) = pending.next().await else {
                break;
            };
            let candidate = &matches[decisions.len()];
            decisions.push(AiCapabilityDiscoveryDecision::for_match(
                candidate,
                decision?.allowed,
            ));
        }
        Ok(decisions)
    }
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
    /// Operations of an entity capability current host policy admitted at
    /// load, in operation-name order. Empty for every other kind.
    operations: Vec<AiLoadedCapabilityOperation>,
}

/// One operation of a loaded entity capability, as admitted at load.
#[derive(Clone, Debug)]
struct AiLoadedCapabilityOperation {
    operation_name: String,
    capability_id: AiToolId,
    capability_fingerprint: String,
    entry_fingerprint: String,
    policy_fingerprint: String,
}

/// The exact executable root a broker call resolved from a loaded binding.
struct LoadedTarget {
    capability_id: AiToolId,
    capability_kind: AiCapabilityKind,
    capability_fingerprint: String,
    operation_name: String,
}

impl AiLoadedCapabilityBinding {
    /// Operations of a loaded entity capability that current host policy
    /// admitted at load, in operation-name order. Empty for every other kind.
    pub fn operation_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.operations
            .iter()
            .map(|operation| operation.operation_name.as_str())
    }

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

#[derive(Clone)]
struct CachedCapabilityDiscovery {
    principal_fingerprint: String,
    session_id: AiSessionId,
    search: AiCapabilityIndexSetSearchResult,
    expires_at: OffsetDateTime,
}

#[derive(Clone)]
struct CapabilityDiscoveryCache {
    entries: Arc<Mutex<VecDeque<CachedCapabilityDiscovery>>>,
    ttl: Duration,
    maximum_searches: usize,
}

/// Discovery and loaded-capability broker with fresh authority checks.
#[derive(Clone)]
pub struct AiCapabilityDiscoveryBroker {
    principal_resolver: crate::principal_projection::AiCurrentPrincipalResolver,
    current_indexes: Arc<dyn AiCurrentCapabilityIndexSet>,
    authority: Arc<dyn AiCapabilityAuthorityPolicy>,
    clock: Arc<dyn Clock>,
    loaded_ttl: Duration,
    discovery_cache: Option<CapabilityDiscoveryCache>,
    maximum_discovery_authorizations: Option<usize>,
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
            principal_resolver: crate::principal_projection::AiCurrentPrincipalResolver::from(
                principal_resolver,
            )
            .with_clock(clock.clone()),
            current_indexes,
            authority,
            clock,
            loaded_ttl,
            discovery_cache: None,
            maximum_discovery_authorizations: None,
        })
    }

    /// Enables bounded process-local reuse of discovery candidates across runs.
    ///
    /// Cache entries belong to one principal reference and AI session. Only
    /// authority-neutral discovery metadata is retained: describe still checks
    /// the current index, rehydrates the principal and authorizes the candidate,
    /// then issues a new short-lived binding for the current fenced run.
    /// Execution bindings, permissions and tool results are never cached here.
    /// Cloned brokers share this cache; replacing the broker starts it empty.
    /// Entries expire after an absolute TTL and the oldest searches are evicted
    /// at capacity. The cache is disabled unless this builder is called.
    ///
    /// # Errors
    ///
    /// Rejects TTLs outside one second through seven days or capacities outside
    /// one through 1,024 searches. Each search retains the canonical search bounds.
    pub fn with_discovery_cache(
        mut self,
        ttl: Duration,
        maximum_searches: u16,
    ) -> Result<Self, AiError> {
        if ttl < Duration::seconds(1)
            || ttl > Duration::days(7)
            || !(1..=1_024).contains(&maximum_searches)
        {
            return Err(AiError::InvalidConfiguration(
                "capability discovery cache bounds are invalid".to_owned(),
            ));
        }
        self.discovery_cache = Some(CapabilityDiscoveryCache {
            entries: Arc::new(Mutex::new(VecDeque::new())),
            ttl,
            maximum_searches: usize::from(maximum_searches),
        });
        Ok(self)
    }

    /// Caps how many ranked matches one discovery query may send to host
    /// policy. Unset by default, so every match is evaluated.
    ///
    /// A query that matches more entries than `maximum` is refused before any
    /// policy call with [`AiError::InvalidInput`] carrying
    /// [`AI_CAPABILITY_DISCOVERY_TOO_BROAD`]. The refusal depends only on the
    /// total number of matches, permitted or not, which search latency
    /// already reflects; it never depends on a policy decision. The cap is
    /// applied before truncation so a restricted principal still receives
    /// every permitted candidate of an admitted query.
    ///
    /// # Errors
    ///
    /// Rejects a maximum of zero.
    pub fn with_maximum_discovery_authorizations(mut self, maximum: u32) -> Result<Self, AiError> {
        if maximum == 0 {
            return Err(AiError::InvalidConfiguration(
                "capability discovery authorization bound is invalid".to_owned(),
            ));
        }
        self.maximum_discovery_authorizations =
            Some(usize::try_from(maximum).unwrap_or(usize::MAX));
        Ok(self)
    }

    fn cache_discovery(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        search: &AiCapabilityIndexSetSearchResult,
    ) {
        let Some(cache) = &self.discovery_cache else {
            return;
        };
        let now = self.clock.now();
        let mut entries = cache.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|entry| entry.expires_at > now);
        entries.push_back(CachedCapabilityDiscovery {
            principal_fingerprint: discovery_reference_fingerprint(principal_reference),
            session_id: run.session_id,
            search: search.clone(),
            expires_at: now + cache.ttl,
        });
        while entries.len() > cache.maximum_searches {
            entries.pop_front();
        }
    }

    fn cached_discovery(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        capability_id: &AiToolId,
        candidate_fingerprint: &str,
    ) -> Option<AiCapabilityIndexSetSearchResult> {
        let cache = self.discovery_cache.as_ref()?;
        let now = self.clock.now();
        let fingerprint = discovery_reference_fingerprint(principal_reference);
        let mut entries = cache.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|entry| entry.expires_at > now);
        entries
            .iter()
            .rev()
            .find(|entry| {
                entry.principal_fingerprint == fingerprint
                    && entry.session_id == run.session_id
                    && entry.search.candidates.iter().any(|candidate| {
                        &candidate.id == capability_id
                            && candidate.entry_fingerprint == candidate_fingerprint
                    })
            })
            .map(|entry| entry.search.clone())
    }

    /// Searches current model-safe metadata after rehydrating the principal
    /// and applying current host policy to every candidate.
    ///
    /// Policy runs over the complete ranking through
    /// [`AiCapabilityAuthorityPolicy::authorize_discovery`] and the result
    /// ceiling is applied afterwards, so the result holds the highest-ranked
    /// candidates the principal may use. Each decision must name the exact
    /// ranked entry it decides. The result carries no score, count or
    /// continuation that could reveal unpermitted entries.
    ///
    /// # Errors
    ///
    /// Fails closed on rehydration, index, or policy failure, and with
    /// [`AiError::Forbidden`] when the batch decisions do not match the
    /// ranking one to one. Returns [`AiError::InvalidInput`] carrying
    /// [`AI_CAPABILITY_DISCOVERY_TOO_BROAD`] when a configured authorization
    /// bound is exceeded. Search grants no load or execution authority.
    pub async fn search(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        query: &AiCapabilitySearchQuery,
    ) -> Result<AiCapabilityIndexSetSearchResult, AiError> {
        self.search_admitted(principal_reference, run, query, false, |_| true)
            .await
    }

    /// [`Self::search`] over the entries `admit` accepts. An entry `admit`
    /// refuses is removed before the authorization bound and before host
    /// policy, exactly as if the index did not hold it. With `per_root`, an
    /// entity entry is replaced by the root entries it groups
    /// ([`AiCapabilityIndexSet::ranked_operation_matches`]).
    async fn search_admitted(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        query: &AiCapabilitySearchQuery,
        per_root: bool,
        admit: impl Fn(&AiCapabilityIndexEntry) -> bool,
    ) -> Result<AiCapabilityIndexSetSearchResult, AiError> {
        let principal = self.rehydrate(principal_reference).await?;
        let indexes = self.current_indexes.current_index_set(run)?;
        let mut ranked = if per_root {
            indexes.ranked_operation_matches(query)?
        } else {
            indexes.ranked_matches(query)?
        };
        ranked.retain(|candidate| admit(candidate.entry()));
        if self
            .maximum_discovery_authorizations
            .is_some_and(|maximum| ranked.len() > maximum)
        {
            return Err(AiError::InvalidInput(
                AI_CAPABILITY_DISCOVERY_TOO_BROAD.to_owned(),
            ));
        }
        let decisions = self
            .authority
            .authorize_discovery(&principal, &ranked, run)
            .await?;
        if decisions.len() != ranked.len()
            || ranked
                .iter()
                .zip(&decisions)
                .any(|(candidate, decision)| decision.capability_id != candidate.entry().id)
        {
            return Err(AiError::Forbidden);
        }
        let candidates = ranked
            .iter()
            .zip(decisions)
            .filter(|(_, decision)| decision.allowed)
            .take(usize::from(query.maximum_results))
            .map(|(candidate, _)| candidate.candidate())
            .collect();
        Ok(AiCapabilityIndexSetSearchResult {
            index_set_fingerprint: indexes.fingerprint().to_owned(),
            candidates,
        })
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
        // Each operation of an entity capability is its own root entry, and
        // host policy decides each one. Only admitted operations are loaded.
        let mut operations = Vec::new();
        if entry.kind == AiCapabilityKind::GeneratedEntity {
            for member in &entry.operations {
                let root = index
                    .entry(&member.capability_id)
                    .ok_or(AiError::Forbidden)?;
                let member_decision = self
                    .authority
                    .authorize(&principal, index, root, run)
                    .await?;
                if member_decision.allowed
                    && valid_binding_value(&member_decision.policy_fingerprint)
                {
                    operations.push(AiLoadedCapabilityOperation {
                        operation_name: member.operation_name.clone(),
                        capability_id: root.id.clone(),
                        capability_fingerprint: root.capability_fingerprint.clone(),
                        entry_fingerprint: root.fingerprint.clone(),
                        policy_fingerprint: member_decision.policy_fingerprint,
                    });
                }
            }
            if operations.is_empty() {
                return Err(AiError::Forbidden);
            }
        }
        Ok(AiLoadedCapabilityBinding {
            operations,
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
    /// Fails closed with [`AiError::Forbidden`] on expiry, revocation, or any
    /// cross-run/provider/principal substitution. Drift of the capability,
    /// entry, index, schema, catalogue, target-policy or host-policy
    /// fingerprint since the handle was loaded also fails closed, with
    /// [`AiError::InvalidInput`] carrying [`AI_CAPABILITY_STALE`]: nothing
    /// executes and the caller must describe the capability again. Drift is
    /// checked only after the handle is proven to belong to this principal
    /// and run, and a current denial is never reported as drift when the
    /// fingerprints are unchanged.
    pub async fn authorize_execution(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        loaded: &AiLoadedCapabilityBinding,
    ) -> Result<(), AiError> {
        self.authorize_loaded(principal_reference, run, loaded)
            .await
            .map(|_| ())
    }

    async fn authorize_loaded(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        loaded: &AiLoadedCapabilityBinding,
    ) -> Result<(ResolvedPrincipal, Arc<AiCapabilityIndexSet>), AiError> {
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
        {
            let (index, entry) = indexes
                .entry(&loaded.capability_id)
                .ok_or_else(stale_capability)?;
            if loaded.capability_kind != entry.kind {
                return Err(AiError::Forbidden);
            }
            if loaded.capability_fingerprint != entry.capability_fingerprint
                || loaded.entry_fingerprint != entry.fingerprint
                || loaded.index_set_fingerprint != indexes.fingerprint()
                || loaded.index_fingerprint != index.fingerprint()
                || loaded.schema_fingerprint != index.schema_fingerprint()
                || loaded.semantic_catalogue_fingerprint != index.semantic_catalogue_fingerprint()
                || loaded.target_policy_fingerprint != index.target_policy_fingerprint()
            {
                return Err(stale_capability());
            }
            let decision = self
                .authority
                .authorize(&principal, index, entry, run)
                .await?;
            if !decision.allowed {
                return Err(AiError::Forbidden);
            }
            if decision.policy_fingerprint != loaded.policy_fingerprint {
                return Err(stale_capability());
            }
        }
        Ok((principal, indexes))
    }

    /// Resolves and authorizes the root one broker call addresses.
    ///
    /// An entity capability needs `operation` unless exactly one of its
    /// operations was loaded; any other capability has one operation, its
    /// own, and `operation` may only name it. The entity is authorized as a
    /// whole and then the chosen root entry again, with its own fingerprints
    /// and host policy, so the root is never reached on the entity's
    /// decision alone.
    async fn authorize_loaded_operation(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        loaded: &AiLoadedCapabilityBinding,
        operation: Option<&str>,
    ) -> Result<LoadedTarget, AiError> {
        let selected = if loaded.capability_kind == AiCapabilityKind::GeneratedEntity {
            let selected = match operation {
                Some(name) => loaded
                    .operations
                    .iter()
                    .find(|candidate| candidate.operation_name == name),
                None if loaded.operations.len() == 1 => loaded.operations.first(),
                None => None,
            };
            Some(selected.ok_or_else(invalid_operation)?)
        } else {
            None
        };
        let (principal, indexes) = self
            .authorize_loaded(principal_reference, run, loaded)
            .await?;
        let (index, entry) = indexes
            .entry(&loaded.capability_id)
            .ok_or_else(stale_capability)?;
        let Some(selected) = selected else {
            if operation.is_some_and(|name| name != entry.operation_name) {
                return Err(invalid_operation());
            }
            return Ok(LoadedTarget {
                capability_id: loaded.capability_id.clone(),
                capability_kind: loaded.capability_kind,
                capability_fingerprint: loaded.capability_fingerprint.clone(),
                operation_name: entry.operation_name.clone(),
            });
        };
        let root = index
            .operation_entry(&loaded.capability_id, &selected.operation_name)
            .ok_or_else(stale_capability)?;
        if root.id != selected.capability_id
            || root.kind != AiCapabilityKind::GeneratedQuery
            || root.capability_fingerprint != selected.capability_fingerprint
            || root.fingerprint != selected.entry_fingerprint
        {
            return Err(stale_capability());
        }
        let decision = self
            .authority
            .authorize(&principal, index, root, run)
            .await?;
        if !decision.allowed {
            return Err(AiError::Forbidden);
        }
        if decision.policy_fingerprint != selected.policy_fingerprint {
            return Err(stale_capability());
        }
        Ok(LoadedTarget {
            capability_id: root.id.clone(),
            capability_kind: root.kind,
            capability_fingerprint: root.capability_fingerprint.clone(),
            operation_name: root.operation_name.clone(),
        })
    }

    async fn rehydrate(
        &self,
        reference: &PrincipalReference,
    ) -> Result<ResolvedPrincipal, AiError> {
        crate::principal_projection::resolve_current_reference(
            self.principal_resolver.as_ref(),
            reference,
        )
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

// Discovery metadata is reusable across freshly admitted work deadlines. Every
// other principal binding remains part of the cache key. Loaded execution handles
// deliberately retain the complete reference fingerprint, including its deadline.
fn discovery_reference_fingerprint(reference: &PrincipalReference) -> String {
    let mut identity = reference.clone();
    identity.expires_at = None;
    principal_reference_fingerprint(&identity)
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
/// The four operations are the complete crate-owned broker surface. Resolving
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
    /// Expand one named type of a loaded capability's signature.
    DescribeType,
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
            AI_CAPABILITY_DESCRIBE_TYPE_TOOL_ID => Some(Self::DescribeType),
            AI_CAPABILITY_EXECUTE_TOOL_ID => Some(Self::Execute),
            _ => None,
        }
    }

    /// Frozen stable tool identifier for this operation.
    pub const fn tool_id(self) -> &'static str {
        match self {
            Self::Discover => AI_CAPABILITY_DISCOVER_TOOL_ID,
            Self::Describe => AI_CAPABILITY_DESCRIBE_TOOL_ID,
            Self::DescribeType => AI_CAPABILITY_DESCRIBE_TYPE_TOOL_ID,
            Self::Execute => AI_CAPABILITY_EXECUTE_TOOL_ID,
        }
    }

    /// Whether this operation can reach an application resolver.
    ///
    /// Only [`Self::Execute`] can. Discovery, describe and describe-type
    /// return bounded authority-neutral metadata and grant no execution
    /// authority.
    pub const fn reaches_resolver(self) -> bool {
        matches!(self, Self::Execute)
    }
}

/// Returns the four frozen fixed-broker definitions: discover, describe,
/// describe-type and execute.
///
/// The definitions are crate-authored and deterministic: a host installs them
/// verbatim and cannot widen, rename, or re-fingerprint the broker surface.
/// They depend only on [`AI_CAPABILITY_BROKER_VERSION`] and never on a
/// catalogue or schema fingerprint, so they are identical before and after any
/// catalogue change.
pub fn capability_broker_definitions() -> Vec<ModelToolDefinition> {
    fixed_broker_definitions()
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
    /// Accepted describe-type calls.
    #[serde(default)]
    pub describe_type_calls: u32,
    /// Accepted execute calls.
    pub execute_calls: u32,
}

impl AiCapabilityBrokerAmplification {
    /// Total accepted broker calls.
    pub const fn total_calls(self) -> u32 {
        self.discover_calls
            .saturating_add(self.describe_calls)
            .saturating_add(self.describe_type_calls)
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
/// model rediscovers unless the broker has a matching unexpired discovery cache entry.
#[derive(Clone, Debug)]
pub struct AiCapabilityBrokerSession {
    inner: Arc<Mutex<BrokerSessionState>>,
    limits: AiCapabilityDeliveryLimits,
    brokered_static_tools: Arc<BTreeSet<String>>,
    inline_describe_schema: bool,
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
            brokered_static_tools: Arc::new(BTreeSet::new()),
            inline_describe_schema: false,
        })
    }

    /// Admits the listed reviewed static read tools to this run's fixed
    /// broker, by exact descriptor fingerprint.
    ///
    /// A brokered static tool is found with discover, loaded with describe
    /// and run with execute, like a generated query. It is not a provider
    /// definition, so it does not count against the 128 exact static
    /// bootstrap fingerprints of a retained provider session, and changing
    /// this list never changes the session binding. Offer a tool either as a
    /// bootstrap definition or through the broker, not both.
    ///
    /// The list is run configuration, not authority. Each brokered call still
    /// rehydrates the principal, applies host capability policy to the exact
    /// index entry at discover, describe and execute, and then runs through
    /// ordinary tool authorization and the tool's static disclosure schema.
    /// The exact-definition review contract is kept: the fingerprint must
    /// equal the indexed and the registered descriptor fingerprint at every
    /// step, or the call fails closed.
    ///
    /// Only an approval-free, read-only, idempotent application query whose
    /// maximum classification is below `Restricted` and which declares no
    /// result media can be brokered. A listed tool that is anything else is
    /// never discoverable and never executable through the broker; keep it a
    /// bootstrap definition. A brokered call carries no
    /// [`crate::AiToolExecutionProvenance`], so a host executor that requires
    /// provenance refuses it.
    ///
    /// Build the list from the same current hierarchical rules as the turn:
    /// the coordinator refuses a turn whose rules do not admit every listed
    /// fingerprint as an approval-free read tool.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] for a malformed fingerprint
    /// or more than [`AI_CAPABILITY_MAXIMUM_BROKERED_STATIC_TOOLS`] entries.
    pub fn with_brokered_static_tools(
        mut self,
        descriptor_fingerprints: BTreeSet<String>,
    ) -> Result<Self, AiError> {
        if descriptor_fingerprints.len() > AI_CAPABILITY_MAXIMUM_BROKERED_STATIC_TOOLS
            || descriptor_fingerprints
                .iter()
                .any(|fingerprint| !crate::valid_sha256(fingerprint))
        {
            return Err(AiError::InvalidConfiguration(
                "brokered static tool binding is invalid".to_owned(),
            ));
        }
        self.brokered_static_tools = Arc::new(descriptor_fingerprints);
        Ok(self)
    }

    /// Exact descriptor fingerprints of the reviewed static tools this run
    /// may reach through the fixed broker.
    pub fn brokered_static_tool_fingerprints(&self) -> &BTreeSet<String> {
        &self.brokered_static_tools
    }

    /// Makes describe also return each operation's complete inline compact
    /// plan schema as `planSchema`, beside its signature.
    ///
    /// By default describe returns only compact typed signatures and the
    /// model expands named types with `describe_type`. Use the inline form
    /// only for a model that cannot follow a type name. The schema is dropped
    /// again when the description would exceed
    /// [`AiCapabilityDeliveryLimits::maximum_describe_bytes`].
    #[must_use]
    pub const fn with_inline_describe_schema(mut self) -> Self {
        self.inline_describe_schema = true;
        self
    }

    /// Whether describe also returns the inline compact plan schema.
    pub const fn uses_inline_describe_schema(&self) -> bool {
        self.inline_describe_schema
    }

    fn brokers_static_entry(&self, entry: &AiCapabilityIndexEntry) -> bool {
        brokerable_static_entry(entry)
            && self
                .brokered_static_tools
                .contains(&entry.capability_fingerprint)
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

    fn record_describe_type(&self) {
        let mut state = self.state();
        state.amplification.describe_type_calls =
            state.amplification.describe_type_calls.saturating_add(1);
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
struct BrokerDescribeTypeArguments {
    loaded_reference: String,
    #[serde(default)]
    operation: Option<String>,
    #[serde(rename = "type")]
    type_name: String,
    #[serde(default)]
    offset: Option<u32>,
    #[serde(default)]
    limit: Option<u16>,
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
    operation: Option<String>,
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

/// Safe [`AiError::InvalidInput`] message for a broker selection this run does
/// not hold: an identifier discovery never returned to it, an unknown or
/// expired `loadedReference`, or lost process-local broker state.
///
/// The model-facing code is `selection_stale`
/// ([`crate::AiApplicationToolFailureCode::SelectionStale`]). It says nothing
/// about whether the named capability exists, so it cannot be used to probe
/// for capabilities the current principal cannot see.
pub const AI_CAPABILITY_SELECTION_STALE: &str = "broker capability selection is stale";

/// Safe [`AiError::InvalidInput`] message for a selection the run does hold
/// whose index, schema, catalogue, entry, capability or policy fingerprint no
/// longer matches the current published state.
///
/// The model-facing code is `capability_stale`
/// ([`crate::AiApplicationToolFailureCode::CapabilityStale`]): the earlier
/// candidate, planning contract and `loadedReference` are void, and the model
/// must discover and describe again. It is reported only for a candidate the
/// current principal was already shown or a capability it already loaded, so
/// it reveals no capability the principal could not see.
pub const AI_CAPABILITY_STALE: &str = "broker capability is stale";

/// Safe [`AiError::InvalidInput`] message for a `describe_type` or `execute`
/// call whose `operation` is not one the loaded capability's description
/// returned, or that omits it where the capability has several.
///
/// The model-facing code is `invalid_arguments` with a fixed correction. It
/// is reported only for a capability the run holds, after its loaded
/// reference was matched, and names no operation.
pub const AI_CAPABILITY_OPERATION_INVALID: &str =
    "broker capability operation is not one the description returned";

/// Safe [`AiError::InvalidInput`] message for a `describe_type` call naming a
/// type the loaded operation's signature does not define.
///
/// The model-facing code is `invalid_arguments` with a fixed correction.
pub const AI_CAPABILITY_TYPE_UNKNOWN: &str =
    "broker capability type is not defined by the signature";

/// Whether index metadata admits a reviewed static tool to the fixed broker.
/// The registered descriptor is checked again before describe and execute.
fn brokerable_static_entry(entry: &AiCapabilityIndexEntry) -> bool {
    entry.kind == AiCapabilityKind::ReviewedStatic
        && entry.risk == AiToolRisk::ReadOnly
        && entry.approval == AiApprovalRule::None
        && entry.result_classification < DataClassification::Restricted
}

fn stale_selection() -> AiError {
    AiError::InvalidInput(AI_CAPABILITY_SELECTION_STALE.to_owned())
}

fn stale_capability() -> AiError {
    AiError::InvalidInput(AI_CAPABILITY_STALE.to_owned())
}

fn invalid_operation() -> AiError {
    AiError::InvalidInput(AI_CAPABILITY_OPERATION_INVALID.to_owned())
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
    operations: Vec<AiCapabilityDescribedOperation>,
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

    /// Operations of the description, in the order of the contract's
    /// `operations` list: every admitted operation of an entity capability,
    /// or the capability's own single operation.
    pub fn operations(&self) -> &[AiCapabilityDescribedOperation] {
        &self.operations
    }

    /// Attaches one signature summary to each operation, in
    /// [`Self::operations`] order, typically
    /// [`crate::AiCapabilitySignature::summary`].
    ///
    /// The members of each summary are merged into that operation of the
    /// contract. When the contract would exceed `maximum_bytes`, any
    /// `planSchema` member is dropped first; if it still does not fit, every
    /// signature is dropped and `signaturesAvailable` is `false`. Nothing is
    /// truncated.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] when the number of summaries
    /// differs from the number of operations, a summary is not an object, or
    /// even the contract without signatures exceeds `maximum_bytes`.
    pub fn with_operation_signatures(
        mut self,
        summaries: Vec<serde_json::Value>,
        maximum_bytes: u32,
    ) -> Result<Self, AiError> {
        let invalid =
            || AiError::InvalidConfiguration("capability planning contract is invalid".to_owned());
        if summaries.len() != self.operations.len() {
            return Err(invalid());
        }
        let base = self.contract.clone();
        let fits = |contract: &serde_json::Value| {
            serde_json::to_vec(contract)
                .is_ok_and(|encoded| encoded.len() <= maximum_bytes as usize)
        };
        let merge = |contract: &mut serde_json::Value, keep_plan_schema: bool| {
            let operations = contract
                .get_mut("operations")
                .and_then(serde_json::Value::as_array_mut)
                .ok_or_else(invalid)?;
            if operations.len() != summaries.len() {
                return Err(invalid());
            }
            for (operation, summary) in operations.iter_mut().zip(&summaries) {
                let (Some(target), Some(source)) = (operation.as_object_mut(), summary.as_object())
                else {
                    return Err(invalid());
                };
                for (key, value) in source {
                    if key == "planSchema" && !keep_plan_schema {
                        continue;
                    }
                    if key != "operation" {
                        target.insert(key.clone(), value.clone());
                    }
                }
            }
            Ok(())
        };
        for keep_plan_schema in [true, false] {
            let mut contract = base.clone();
            merge(&mut contract, keep_plan_schema)?;
            if fits(&contract) {
                self.contract = contract;
                return Ok(self);
            }
        }
        let mut contract = base;
        if let Some(object) = contract.as_object_mut() {
            object.insert("signaturesAvailable".to_owned(), json!(false));
        }
        if !fits(&contract) {
            return Err(AiError::InvalidConfiguration(
                "capability planning contract exceeds its deployment bound".to_owned(),
            ));
        }
        self.contract = contract;
        Ok(self)
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

    /// Attaches the plan schema of the loaded capability with each repeated
    /// input type defined once under `planSchema.types`.
    ///
    /// The contract is the one [`Self::with_plan_schema`] would attach, in
    /// fewer bytes: a `{"$ref": "#/types/<Name>"}` stands for the named
    /// definition, and the execute arguments are unchanged. The same size
    /// bound and omission rule apply.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] when the contract can no
    /// longer be canonically serialized.
    pub fn with_shared_type_plan_schema(
        mut self,
        plan_schema: &AiSharedTypePlanSchema,
        maximum_bytes: u32,
    ) -> Result<Self, AiError> {
        if !plan_schema.types().is_empty()
            && let Some(contract) = self.contract.as_object_mut()
        {
            contract.insert(
                "planSchemaTypeReferences".to_owned(),
                json!(
                    "A {\"$ref\": \"#/types/<Name>\"} in planSchema stands for planSchema.types.<Name>."
                ),
            );
        }
        self.with_plan_schema(plan_schema.to_model_schema(), maximum_bytes)
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
    capability_kind: AiCapabilityKind,
    capability_fingerprint: String,
    audit_reference: String,
    plan: serde_json::Value,
}

impl AiCapabilityExecution {
    /// Exact capability to execute.
    pub const fn capability_id(&self) -> &AiToolId {
        &self.capability_id
    }

    /// Capability family to execute: a generated query, or a reviewed static
    /// tool the run admitted to the broker.
    pub const fn capability_kind(&self) -> AiCapabilityKind {
        self.capability_kind
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
    ///
    /// For a reviewed static tool this is the tool's argument object, which
    /// the registered argument schema validates before the resolver runs.
    pub const fn plan(&self) -> &serde_json::Value {
        &self.plan
    }

    /// Consumes the execution and returns the closed compact plan, or the
    /// argument object of a reviewed static tool.
    pub fn into_plan(self) -> serde_json::Value {
        self.plan
    }
}

fn candidate_value(candidate: &crate::AiCapabilitySearchCandidate) -> serde_json::Value {
    let mut value = json!({
        "capabilityId": candidate.id.as_str(),
        "kind": candidate.kind,
        "name": candidate.name,
        "description": candidate.description,
        "namespace": candidate.namespace,
        "entity": candidate.entity_name,
        "operation": candidate.operation_name,
        "shape": candidate.operation_shape,
        "candidateFingerprint": candidate.entry_fingerprint,
    });
    if !candidate.operations.is_empty()
        && let Some(object) = value.as_object_mut()
    {
        object.remove("operation");
        object.insert(
            "operations".to_owned(),
            candidate
                .operations
                .iter()
                .map(|operation| {
                    json!({"operation": operation.operation_name, "shape": operation.operation_shape})
                })
                .collect(),
        );
    }
    value
}

/// Model-facing usage note of every broker version 2 description.
const DESCRIBE_USAGE: &str = "Each operation has a typed signature. Expand a named type from inputTypes, select or select.<path> with graphql_capabilities_describe_type (loadedReference, operation, type). Execute with loadedReference, operation, arguments as dotted leaf paths (for example where.status.eq) and selections as dotted scalar paths.";

fn planning_contract(
    entry: &AiCapabilityIndexEntry,
    loaded: &AiLoadedCapabilityBinding,
    loaded_reference: &str,
    expires_in_seconds: i64,
) -> serde_json::Value {
    let operations = if entry.kind == AiCapabilityKind::GeneratedEntity {
        loaded
            .operations
            .iter()
            .filter_map(|operation| {
                let member = entry
                    .operations
                    .iter()
                    .find(|member| member.capability_id == operation.capability_id)?;
                Some(json!({
                    "operation": member.operation_name,
                    "shape": member.operation_shape,
                    "description": member.description,
                    "resultRecordCost": member.result_record_cost,
                }))
            })
            .collect::<Vec<_>>()
    } else {
        vec![json!({
            "operation": entry.operation_name,
            "shape": entry.operation_shape,
            "description": entry.description,
            "resultRecordCost": entry.result_record_cost,
        })]
    };
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
        "resultClassification": entry.result_classification,
        "resultDescription": entry.result_description,
        "resultRecordCost": entry.result_record_cost,
        "risk": entry.risk,
        "approval": entry.approval,
        "aggregates": entry.aggregate_features,
        "candidateFingerprint": entry.fingerprint,
        "usage": DESCRIBE_USAGE,
        "operations": operations,
    })
}

/// One operation of a described capability: the exact root whose signature
/// the host attaches to the description.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiCapabilityDescribedOperation {
    operation_name: String,
    capability_id: AiToolId,
    capability_kind: AiCapabilityKind,
    capability_fingerprint: String,
}

impl AiCapabilityDescribedOperation {
    /// Public operation name the model passes as `operation`.
    pub fn operation_name(&self) -> &str {
        &self.operation_name
    }

    /// Exact root capability of this operation.
    pub const fn capability_id(&self) -> &AiToolId {
        &self.capability_id
    }

    /// Family of the root: a generated query, or a brokered static tool.
    pub const fn capability_kind(&self) -> AiCapabilityKind {
        self.capability_kind
    }

    /// Exact root capability fingerprint.
    pub fn capability_fingerprint(&self) -> &str {
        &self.capability_fingerprint
    }
}

/// Authorized `describe_type` request: one named type of one loaded
/// operation's signature.
///
/// Construction proves the loaded reference belongs to this run and the
/// capability and operation still pass current fingerprint and host-policy
/// checks. The host renders the page from the registered capability with
/// [`crate::AiCapabilitySignature::describe_type`].
#[derive(Clone, Debug)]
pub struct AiCapabilityTypeRequest {
    operation: AiCapabilityDescribedOperation,
    type_name: String,
    offset: u32,
    limit: u16,
    /// Index descriptions of the capability's top-level scalar fields and
    /// relationships, added to the root selection page.
    field_descriptions: BTreeMap<String, String>,
}

impl AiCapabilityTypeRequest {
    /// Exact root operation the type belongs to.
    pub const fn operation(&self) -> &AiCapabilityDescribedOperation {
        &self.operation
    }

    /// Requested type name.
    pub fn type_name(&self) -> &str {
        &self.type_name
    }

    /// First field or value of the requested page.
    pub const fn offset(&self) -> u32 {
        self.offset
    }

    /// Maximum fields or values in the requested page.
    pub const fn limit(&self) -> u16 {
        self.limit
    }

    /// Renders the page from the operation's signature.
    ///
    /// The root selection page also carries each top-level field's
    /// model-safe index description.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] carrying
    /// [`AI_CAPABILITY_TYPE_UNKNOWN`] when the signature defines no such type.
    pub fn render(
        &self,
        signature: &crate::AiCapabilitySignature,
    ) -> Result<serde_json::Value, AiError> {
        let mut page = signature
            .describe_type(&self.type_name, self.offset, self.limit)
            .ok_or_else(|| AiError::InvalidInput(AI_CAPABILITY_TYPE_UNKNOWN.to_owned()))?;
        if self.type_name == crate::AI_CAPABILITY_SELECTION_TYPE
            && let Some(fields) = page
                .get_mut("fields")
                .and_then(serde_json::Value::as_array_mut)
        {
            for field in fields {
                let description = field
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|name| self.field_descriptions.get(name))
                    .filter(|description| !description.is_empty())
                    .cloned();
                if let (Some(object), Some(description)) = (field.as_object_mut(), description) {
                    object.insert("description".to_owned(), json!(description));
                }
            }
        }
        if let Some(object) = page.as_object_mut() {
            object.insert("operation".to_owned(), json!(self.operation.operation_name));
        }
        Ok(page)
    }
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
        let brokers_static = !session.brokered_static_tools.is_empty();
        // Generated reads are entity capabilities plus any root without an
        // entity. Without brokered static tools the search covers only
        // those; with them, an unfiltered search covers both families.
        let generated = |entry: &AiCapabilityIndexEntry| {
            matches!(
                entry.kind,
                AiCapabilityKind::GeneratedEntity | AiCapabilityKind::GeneratedQuery
            )
        };
        let admit: Box<dyn Fn(&AiCapabilityIndexEntry) -> bool + Send + Sync> = match parsed.kind {
            Some(AiCapabilityKind::GeneratedQuery) => Box::new(generated),
            Some(AiCapabilityKind::GeneratedEntity) => {
                Box::new(|entry: &AiCapabilityIndexEntry| {
                    entry.kind == AiCapabilityKind::GeneratedEntity
                })
            }
            Some(AiCapabilityKind::ReviewedStatic) if brokers_static => {
                let session = session.clone();
                Box::new(move |entry: &AiCapabilityIndexEntry| session.brokers_static_entry(entry))
            }
            Some(_) => return Err(malformed_broker_arguments()),
            None => {
                let session = session.clone();
                Box::new(move |entry: &AiCapabilityIndexEntry| {
                    generated(entry) || session.brokers_static_entry(entry)
                })
            }
        };
        let query = AiCapabilitySearchQuery {
            text: parsed.text,
            namespace: parsed.namespace,
            kind: None,
            entity_or_operation: parsed.entity_or_class,
            maximum_results: parsed.maximum_results,
        };
        let result = self
            .search_admitted(principal_reference, run, &query, false, admit)
            .await?;
        let value = json!({
            "version": AI_CAPABILITY_BROKER_VERSION,
            "indexFingerprint": result.index_set_fingerprint,
            "candidates": result
                .candidates
                .iter()
                .map(candidate_value)
                .collect::<Vec<_>>(),
        });
        self.cache_discovery(principal_reference, run, &result);
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
        // Client-deferred delivery installs one exact definition per root,
        // so it ranks roots: an entity's roots stand in for the entity.
        let result = self
            .search_admitted(principal_reference, run, &query, true, |_| true)
            .await?;
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
    /// The candidate must come from discovery retained for this run or, when
    /// enabled, the unexpired cache for this principal and AI session. Its
    /// fingerprint must still match. An unknown identifier and an identifier
    /// never returned to this run by discovery are both reported as one
    /// bounded retryable [`AI_CAPABILITY_SELECTION_STALE`], so describe cannot
    /// be used to probe for capabilities the current principal cannot see.
    /// A candidate this run does hold whose index set has since changed is
    /// reported as the distinct retryable [`AI_CAPABILITY_STALE`], telling the
    /// model to discover and describe again.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for malformed arguments, an unheld
    /// selection ([`AI_CAPABILITY_SELECTION_STALE`]) or a drifted one
    /// ([`AI_CAPABILITY_STALE`]), and [`AiError::Forbidden`] when current host
    /// policy no longer permits the exact candidate.
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
            .or_else(|| {
                self.cached_discovery(
                    principal_reference,
                    run,
                    &capability_id,
                    &parsed.candidate_fingerprint,
                )
            })
            .ok_or_else(stale_selection)?;
        let indexes = self.current_indexes.current_index_set(run)?;
        // The run holds this exact candidate, so reporting catalogue drift
        // discloses nothing the principal was not already shown.
        verify_search_binding(&indexes, &search).map_err(|_| stale_capability())?;
        let (_, entry) = indexes.entry(&capability_id).ok_or_else(stale_capability)?;
        // A static tool is loadable only while this run brokers its exact
        // fingerprint. A cached discovery from a run that did, or any other
        // capability family, is the same stale selection as an unknown ID.
        if !matches!(
            entry.kind,
            AiCapabilityKind::GeneratedEntity | AiCapabilityKind::GeneratedQuery
        ) && !session.brokers_static_entry(entry)
        {
            return Err(stale_selection());
        }
        let loaded = self
            .load(principal_reference, run, &search, &capability_id)
            .await?;
        let loaded_reference = loaded.audit_reference();
        let expires_in_seconds = (loaded.expires_at - self.clock.now())
            .whole_seconds()
            .max(0);
        let contract = planning_contract(entry, &loaded, &loaded_reference, expires_in_seconds);
        let operations = if loaded.capability_kind == AiCapabilityKind::GeneratedEntity {
            loaded
                .operations
                .iter()
                .map(|operation| AiCapabilityDescribedOperation {
                    operation_name: operation.operation_name.clone(),
                    capability_id: operation.capability_id.clone(),
                    capability_kind: AiCapabilityKind::GeneratedQuery,
                    capability_fingerprint: operation.capability_fingerprint.clone(),
                })
                .collect()
        } else {
            vec![AiCapabilityDescribedOperation {
                operation_name: entry.operation_name.clone(),
                capability_id: loaded.capability_id.clone(),
                capability_kind: loaded.capability_kind,
                capability_fingerprint: loaded.capability_fingerprint.clone(),
            }]
        };
        let description = AiCapabilityDescription {
            loaded_reference: loaded_reference.clone(),
            capability_id: loaded.capability_id.clone(),
            capability_kind: loaded.capability_kind,
            capability_fingerprint: loaded.capability_fingerprint.clone(),
            operations,
            contract,
        };
        session.record_loaded(loaded_reference, loaded);
        Ok(description)
    }

    /// Dispatches one frozen `graphql.capabilities.describe_type` call up to
    /// the rendering of the page.
    ///
    /// The loaded reference must be one this run holds and has not expired;
    /// the capability and the chosen operation are then checked against the
    /// current index and host policy exactly as at execute. The returned
    /// request names the exact root whose signature the host renders.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for malformed arguments, an unheld
    /// or expired reference ([`AI_CAPABILITY_SELECTION_STALE`]), drift
    /// ([`AI_CAPABILITY_STALE`]) or an operation the description did not
    /// return ([`AI_CAPABILITY_OPERATION_INVALID`]), and
    /// [`AiError::Forbidden`] for revocation or substitution.
    pub async fn dispatch_describe_type(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        session: &AiCapabilityBrokerSession,
        arguments: &serde_json::Value,
    ) -> Result<AiCapabilityTypeRequest, AiError> {
        let parsed: BrokerDescribeTypeArguments =
            serde_json::from_value(arguments.clone()).map_err(|_| malformed_broker_arguments())?;
        if !crate::valid_sha256(&parsed.loaded_reference)
            || parsed.type_name.is_empty()
            || parsed.type_name.len() > 512
            || parsed.type_name.chars().any(char::is_control)
            || parsed
                .operation
                .as_ref()
                .is_some_and(|operation| !valid_graphql_name(operation))
            || parsed.offset.is_some_and(|offset| offset > 1_000_000)
            || parsed
                .limit
                .is_some_and(|limit| !(1..=crate::AI_CAPABILITY_TYPE_PAGE_MAXIMUM).contains(&limit))
        {
            return Err(malformed_broker_arguments());
        }
        let loaded = session
            .loaded(&parsed.loaded_reference)
            .ok_or_else(stale_selection)?;
        if self.clock.now() > loaded.expires_at {
            return Err(stale_selection());
        }
        if loaded.capability_kind == AiCapabilityKind::ReviewedStatic
            && !session
                .brokered_static_tools
                .contains(&loaded.capability_fingerprint)
        {
            return Err(AiError::Forbidden);
        }
        let target = self
            .authorize_loaded_operation(
                principal_reference,
                run,
                &loaded,
                parsed.operation.as_deref(),
            )
            .await?;
        let field_descriptions = self
            .current_indexes
            .current_index_set(run)?
            .entry(&loaded.capability_id)
            .map(|(_, entry)| {
                entry
                    .scalar_fields
                    .iter()
                    .map(|field| (field.name.clone(), field.description.clone()))
                    .chain(entry.relationships.iter().map(|relationship| {
                        (relationship.name.clone(), relationship.description.clone())
                    }))
                    .collect()
            })
            .unwrap_or_default();
        session.record_describe_type();
        Ok(AiCapabilityTypeRequest {
            field_descriptions,
            operation: AiCapabilityDescribedOperation {
                operation_name: target.operation_name,
                capability_id: target.capability_id,
                capability_kind: target.capability_kind,
                capability_fingerprint: target.capability_fingerprint,
            },
            type_name: parsed.type_name,
            offset: parsed.offset.unwrap_or(0),
            limit: parsed
                .limit
                .unwrap_or(crate::AI_CAPABILITY_TYPE_PAGE_DEFAULT),
        })
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
    /// Returns [`AiError::InvalidInput`] for malformed arguments, for an
    /// unknown loaded reference or an expired binding
    /// ([`AI_CAPABILITY_SELECTION_STALE`]), and for a loaded capability whose
    /// index, schema, catalogue, entry or policy fingerprint has drifted
    /// ([`AI_CAPABILITY_STALE`]). Returns [`AiError::Forbidden`] for a non-read
    /// capability or any revocation or substitution.
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
        let static_tool = match loaded.capability_kind {
            AiCapabilityKind::GeneratedQuery | AiCapabilityKind::GeneratedEntity => false,
            AiCapabilityKind::ReviewedStatic
                if session
                    .brokered_static_tools
                    .contains(&loaded.capability_fingerprint) =>
            {
                true
            }
            _ => return Err(AiError::Forbidden),
        };
        if parsed
            .operation
            .as_ref()
            .is_some_and(|operation| !valid_graphql_name(operation))
        {
            return Err(malformed_broker_arguments());
        }
        let target = self
            .authorize_loaded_operation(
                principal_reference,
                run,
                &loaded,
                parsed.operation.as_deref(),
            )
            .await?;
        if static_tool {
            let indexes = self.current_indexes.current_index_set(run)?;
            if !indexes
                .entry(&loaded.capability_id)
                .is_some_and(|(_, entry)| session.brokers_static_entry(entry))
            {
                return Err(AiError::Forbidden);
            }
        }
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
        if static_tool {
            // A static tool has a fixed server-authored document: it takes
            // arguments only.
            if !selections.is_empty()
                || !relationship_arguments.is_empty()
                || !relationship_maximum_items.is_empty()
                || parsed.maximum_items.is_some()
            {
                return Err(malformed_broker_arguments());
            }
            session.record_execute();
            return Ok(AiCapabilityExecution {
                capability_id: target.capability_id,
                capability_kind: target.capability_kind,
                capability_fingerprint: target.capability_fingerprint,
                audit_reference: loaded.audit_reference(),
                plan: plan_arguments,
            });
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
            capability_id: target.capability_id,
            capability_kind: target.capability_kind,
            capability_fingerprint: target.capability_fingerprint,
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
    tools: Arc<Mutex<AiCapabilityOfferingState>>,
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
        // A fixed-broker binding carries no index fingerprint: its surface is
        // catalogue-independent and describe/execute revalidate the index.
        if session_binding.delivery_mode() != surface.mode()
            || session_binding
                .capability_index_fingerprint()
                .is_some_and(|bound| bound != index_fingerprint)
        {
            return Err(AiError::InvalidConfiguration(
                "capability session binding does not match the selected surface".to_owned(),
            ));
        }
        // Only the fixed broker has the describe and execute operations a
        // static tool needs, and a tool is offered one way, never both.
        if !session.brokered_static_tools.is_empty()
            && (surface.mode() != AiCapabilityDeliveryMode::FixedBroker
                || !session
                    .brokered_static_tools
                    .is_disjoint(session_binding.static_bootstrap_tool_fingerprints()))
        {
            return Err(AiError::InvalidConfiguration(
                "brokered static tools do not match the selected surface".to_owned(),
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
        let static_fingerprints = static_bootstrap_tools
            .iter()
            .map(|definition| definition.fingerprint.clone())
            .collect();
        Ok(Self {
            mode: surface.mode(),
            index_fingerprint: index_fingerprint.to_owned(),
            session_binding,
            broker,
            session,
            static_bootstrap_tools: Arc::new(static_bootstrap_tools),
            tools: Arc::new(Mutex::new(AiCapabilityOfferingState {
                tools,
                static_fingerprints,
                generation: 0,
                frozen_plans: 0,
            })),
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

    /// Whether `admits` accepts every reviewed static tool this run may reach
    /// through the fixed broker. The coordinator uses it to hold brokered
    /// static tools to the same hierarchical rules as offered ones.
    #[cfg_attr(feature = "mssql", allow(dead_code))]
    pub(crate) fn brokered_static_tools_admitted(&self, admits: impl Fn(&str) -> bool) -> bool {
        self.session
            .brokered_static_tools
            .iter()
            .all(|fingerprint| admits(fingerprint))
    }

    /// Exact definitions a host must install in the next provider request.
    pub fn current_tools(&self) -> Vec<ModelToolDefinition> {
        self.tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .tools
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
        let state = self.tools.lock().unwrap_or_else(PoisonError::into_inner);
        AiCapabilityDeliverySurface {
            mode: self.mode,
            tools: state.tools.clone(),
            #[cfg(any(test, feature = "sqlite", feature = "postgres"))]
            offering: Some(AiCapabilityOfferingSnapshot {
                state: self.tools.clone(),
                generation: state.generation,
            }),
        }
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn require_unfrozen_static_offering(&self) -> Result<(), AiError> {
        let state = self.tools.lock().unwrap_or_else(PoisonError::into_inner);
        if state.frozen_plans != 0 {
            return Err(AiError::Conflict);
        }
        Ok(())
    }

    /// Selects the static bootstrap definitions offered on the next provider turn.
    ///
    /// Only fingerprints in the original immutable session binding may be selected. The
    /// same broker, loaded definitions and shared run state survive reduction or reoffering.
    /// This is disclosure configuration, not execution authority. Call before constructing
    /// the next plan; existing plans freeze their offered set until all plan clones are dropped.
    /// Retained providers require the SDK's confirmed cleanup and fresh-thread transition
    /// before a changed set can be installed remotely.
    ///
    /// # Errors
    /// Returns [`AiError::Forbidden`] for unsupported modes or unknown fingerprints and
    /// [`AiError::Conflict`] when a changed set would alter a frozen plan.
    pub fn set_static_bootstrap_offering(
        &self,
        selected_fingerprints: &BTreeSet<String>,
    ) -> Result<(), AiError> {
        if !matches!(
            self.mode,
            AiCapabilityDeliveryMode::FixedBroker | AiCapabilityDeliveryMode::ClientDeferred
        ) || !selected_fingerprints
            .is_subset(self.session_binding.static_bootstrap_tool_fingerprints())
        {
            return Err(AiError::Forbidden);
        }
        let mut state = self.tools.lock().unwrap_or_else(PoisonError::into_inner);
        if state.static_fingerprints == *selected_fingerprints {
            return Ok(());
        }
        if state.frozen_plans != 0 {
            return Err(AiError::Conflict);
        }
        let generation = state.generation.checked_add(1).ok_or(AiError::Conflict)?;
        let all_static = self.session_binding.static_bootstrap_tool_fingerprints();
        state
            .tools
            .retain(|definition| !all_static.contains(&definition.fingerprint));
        state.tools.extend(
            self.static_bootstrap_tools
                .iter()
                .filter(|definition| selected_fingerprints.contains(&definition.fingerprint))
                .cloned(),
        );
        state
            .tools
            .sort_by(|left, right| left.tool_id.cmp(&right.tool_id));
        state.static_fingerprints = selected_fingerprints.clone();
        state.generation = generation;
        Ok(())
    }

    /// Exact crate-authored definition of one broker operation in this
    /// turn's delivery mode, or `None` when the mode does not offer it.
    ///
    /// The fixed broker's definitions are catalogue-independent; the
    /// client-deferred discovery definition stays bound to the index.
    #[cfg_attr(feature = "mssql", allow(dead_code))]
    pub(crate) fn broker_definition(
        &self,
        operation: AiCapabilityBrokerOperation,
    ) -> Option<ModelToolDefinition> {
        match self.mode {
            AiCapabilityDeliveryMode::FixedBroker => fixed_broker_definitions()
                .into_iter()
                .find(|definition| definition.tool_id == operation.tool_id()),
            AiCapabilityDeliveryMode::ClientDeferred => (operation
                == AiCapabilityBrokerOperation::Discover)
                .then(|| discovery_definition(Some(&self.index_fingerprint))),
            AiCapabilityDeliveryMode::EagerExact | AiCapabilityDeliveryMode::ProviderDeferred => {
                None
            }
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
        let mut current = self.tools.lock().unwrap_or_else(PoisonError::into_inner);
        if current.frozen_plans != 0 {
            return Err(AiError::Conflict);
        }
        let generation = current.generation.checked_add(1).ok_or(AiError::Conflict)?;
        let mut tools = vec![discovery_definition(Some(&self.index_fingerprint))];
        tools.extend(
            self.static_bootstrap_tools
                .iter()
                .filter(|definition| {
                    current
                        .static_fingerprints
                        .contains(&definition.fingerprint)
                })
                .cloned(),
        );
        tools.extend(installed);
        tools.sort_by(|left, right| left.tool_id.cmp(&right.tool_id));
        current.tools.clone_from(&tools);
        current.generation = generation;
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

    #[test]
    fn discovery_optional_filters_match_the_broker_contract() {
        let definition = discovery_definition(Some("index"));
        let validator = jsonschema::validator_for(&definition.parameters).unwrap();
        for arguments in [
            json!({"text":"reviewed record", "maximumResults":8}),
            json!({"text":"reviewed record", "maximumResults":8, "namespace":null, "kind":null, "entityOrClass":null}),
        ] {
            assert!(validator.is_valid(&arguments));
            let parsed: BrokerDiscoverArguments = serde_json::from_value(arguments).unwrap();
            assert!(
                parsed.namespace.is_none()
                    && parsed.kind.is_none()
                    && parsed.entity_or_class.is_none()
            );
            assert_eq!(parsed.maximum_results, 8);
        }
        for arguments in [
            json!({"text":"record"}),
            json!({"text":"record", "maximumResults":0}),
            json!({"text":"record", "maximumResults":33}),
            json!({"text":"record", "maximumResults":8, "kind":"mutation"}),
            json!({"text":"record", "maximumResults":8, "unknown":true}),
        ] {
            assert!(!validator.is_valid(&arguments));
        }
    }

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
        generated_index_with_schema(target_id, profile_id, target_policy, None, false)
    }

    /// The `Record` entity with two query roots, `GeneratedRecord` and
    /// `RecordBySubject`, grouped into one entity capability.
    fn two_root_entity_index(target_policy: &str) -> Arc<AiCapabilityIndex> {
        generated_index_with_schema(
            "generated-read-application",
            "generated-read",
            target_policy,
            None,
            true,
        )
    }

    /// `added_field` publishes one more `Record` scalar: a real schema change
    /// with a different schema, catalogue, entry and index fingerprint.
    /// `second_root` adds the `RecordBySubject` root of the same entity.
    fn generated_index_with_schema(
        target_id: &str,
        profile_id: &str,
        target_policy: &str,
        added_field: Option<&str>,
        second_root: bool,
    ) -> Arc<AiCapabilityIndex> {
        let added_sdl = added_field
            .map(|field| format!(", {field}: String!"))
            .unwrap_or_default();
        let second_root_sdl = if second_root {
            ", RecordBySubject(subject: String!): Record!"
        } else {
            ""
        };
        let sdl = format!(
            "schema {{ query: Query }}
            type Query {{ GeneratedRecord(recordId: ID!): Record!{second_root_sdl} }}
            type Record {{ recordId: ID!, subject: String!{added_sdl} }}"
        );
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
            filter_input: None,
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
            generated_input_members: Vec::new(),
            entity_name: "Record".to_owned(),
            description: "A reviewed application record.".to_owned(),
            default_classification: GraphqlSemanticClassification::Internal,
            fields: [scalar("recordId", "ID"), scalar("subject", "String")]
                .into_iter()
                .chain(added_field.map(|field| scalar(field, "String")))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            label_fields: Vec::new(),
            record_version: None,
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
        let second_operation = second_root.then(|| {
            GraphqlSemanticOperationDescriptor::custom(
                GraphqlOperationKind::Query,
                "RecordBySubject",
                "Read one reviewed application record by its subject.",
                vec![GraphqlSemanticArgumentDescriptor {
                    graphql_name: "subject".to_owned(),
                    description: "Reviewed record subject.".to_owned(),
                    type_ref: GraphqlSemanticTypeRef::named(
                        "String",
                        GraphqlSemanticTypeKind::Scalar,
                        false,
                    ),
                }],
                GraphqlSemanticTypeRef::named("Record", GraphqlSemanticTypeKind::Object, false),
                true,
            )
            .expect("second query semantics should validate")
        });
        let semantics = GraphqlSemanticCatalog::compose_with_custom(
            [entity],
            &GraphqlOperationCatalog::compose(std::iter::empty()),
            std::iter::once(operation).chain(second_operation),
        )
        .expect("test query catalogue should validate");
        let target = crate::GraphqlExecutionTargetId::parse(target_id)
            .expect("generated target should validate");
        let catalogue = crate::AiGraphqlQueryCapabilityCatalog::compile(
            profile_id,
            target.clone(),
            &sdl,
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
    fn large_surfaces_default_to_the_fixed_broker_and_hosts_can_opt_out() {
        let provider = ProviderCapabilities {
            capability_delivery_modes: BTreeSet::from([
                AiCapabilityDeliveryMode::ProviderDeferred,
                AiCapabilityDeliveryMode::FixedBroker,
            ]),
            ..ProviderCapabilities::default()
        };
        let select = |limits| {
            select_capability_delivery_mode(&provider, 55, 4_917_706, false, limits)
                .unwrap()
                .mode()
        };
        // Default: no mode whose request size grows with the catalogue.
        assert!(AiCapabilityDeliveryLimits::default().prefer_fixed_broker_for_large_surfaces);
        assert_eq!(
            select(AiCapabilityDeliveryLimits::default()),
            AiCapabilityDeliveryMode::FixedBroker
        );
        // Opt-out: native deferred loading as before, with the same binding
        // fingerprint a host that never opted in had.
        let opted_out =
            AiCapabilityDeliveryLimits::default().with_provider_deferred_for_large_surfaces();
        assert_eq!(
            select(opted_out),
            AiCapabilityDeliveryMode::ProviderDeferred
        );
        let binding = |mode| {
            AiProviderCapabilitySessionBinding::new(
                mode,
                "a".repeat(64),
                BTreeSet::new(),
                "v1",
                "model",
                ModelReasoningEffort::default(),
                "b".repeat(64),
            )
            .unwrap()
        };
        assert_eq!(
            binding(select(opted_out)).fingerprint(),
            "525f9e792f18b8e566073f853724cd03161da7535a59dd5d9d9c1420e501f9dc"
        );
        // A small surface stays eager, and a provider without the fixed
        // broker keeps native deferred loading.
        let eager = ProviderCapabilities {
            capability_delivery_modes: BTreeSet::from([
                AiCapabilityDeliveryMode::EagerExact,
                AiCapabilityDeliveryMode::ProviderDeferred,
                AiCapabilityDeliveryMode::FixedBroker,
            ]),
            ..ProviderCapabilities::default()
        };
        assert_eq!(
            select_capability_delivery_mode(
                &eager,
                2,
                1_024,
                false,
                AiCapabilityDeliveryLimits::default()
            )
            .unwrap()
            .mode(),
            AiCapabilityDeliveryMode::EagerExact
        );
        let native_only = ProviderCapabilities {
            capability_delivery_modes: BTreeSet::from([AiCapabilityDeliveryMode::ProviderDeferred]),
            ..ProviderCapabilities::default()
        };
        assert_eq!(
            select_capability_delivery_mode(
                &native_only,
                55,
                4_917_706,
                false,
                AiCapabilityDeliveryLimits::default()
            )
            .unwrap()
            .mode(),
            AiCapabilityDeliveryMode::ProviderDeferred
        );
        // The default serializes without the field and a configuration
        // stored without it reads as the new default; an explicit opt-out
        // round-trips.
        let default_value = serde_json::to_value(AiCapabilityDeliveryLimits::default()).unwrap();
        assert!(
            default_value
                .get("prefer_fixed_broker_for_large_surfaces")
                .is_none()
        );
        let stored: AiCapabilityDeliveryLimits = serde_json::from_value(json!({
            "maximum_eager_definitions": 16,
            "maximum_eager_definition_bytes": 262_144,
            "maximum_deferred_definitions": 8
        }))
        .unwrap();
        assert!(stored.prefer_fixed_broker_for_large_surfaces);
        let opted_out_value = serde_json::to_value(opted_out).unwrap();
        assert_eq!(
            opted_out_value["prefer_fixed_broker_for_large_surfaces"],
            false
        );
        assert_eq!(
            serde_json::from_value::<AiCapabilityDeliveryLimits>(opted_out_value).unwrap(),
            opted_out
        );
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
        let limits = AiCapabilityDeliveryLimits {
            prefer_fixed_broker_for_large_surfaces: true,
            ..limits
        };
        // A surface above the eager limits never selects the mode that sends
        // every definition while the provider also offers the fixed broker.
        assert_eq!(
            select_capability_delivery_mode(&capabilities, 55, 4_917_706, false, limits)
                .expect("large surface should select")
                .mode(),
            AiCapabilityDeliveryMode::FixedBroker
        );
        assert_eq!(
            select_capability_delivery_mode(
                &capabilities,
                usize::from(limits.maximum_eager_definitions) + 1,
                1_024,
                false,
                limits
            )
            .expect("count above the eager limit should select")
            .mode(),
            AiCapabilityDeliveryMode::FixedBroker
        );
        // Without a declared fixed broker, native deferred loading remains
        // the only admissible mode for the same surface.
        let mut native_only = capabilities.clone();
        native_only
            .capability_delivery_modes
            .remove(&AiCapabilityDeliveryMode::FixedBroker);
        assert_eq!(
            select_capability_delivery_mode(&native_only, 55, 4_917_706, false, limits)
                .expect("native-only surface should select")
                .mode(),
            AiCapabilityDeliveryMode::ProviderDeferred
        );
        // A surface within the eager limits keeps native deferred loading
        // when the provider declares no eager mode.
        let mut no_eager = capabilities.clone();
        no_eager
            .capability_delivery_modes
            .remove(&AiCapabilityDeliveryMode::EagerExact);
        assert_eq!(
            select_capability_delivery_mode(&no_eager, 2, 1_024, false, limits)
                .expect("small deferred surface should select")
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
        assert_eq!(fixed.tools().len(), 4);
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
        assert_eq!(surface.tools().len(), 5);
        assert!(surface.tools().contains(&static_tool));
        #[cfg(feature = "provider-grok-acp")]
        {
            let registration = crate::providers::AiGrokAcpRegistration::new(
                "grok-binding-test".into(),
                "grok-4.7".into(),
                "a".repeat(64),
                "1.0.40".into(),
                "isolated-test".into(),
                crate::ModelReasoningEffortProfile::new(
                    "grok-4.7",
                    [ModelReasoningEffort::Low],
                    ModelReasoningEffort::Low,
                )
                .unwrap(),
                ModelReasoningEffort::Low,
                "Authorized reads only.".into(),
                delivery.current_tools(),
                16_384,
                2_048,
                8,
            )
            .unwrap();
            let binding = AiProviderCapabilitySessionBinding::new(
                AiCapabilityDeliveryMode::FixedBroker,
                index_set.fingerprint(),
                BTreeSet::from([static_tool.fingerprint.clone()]),
                "grok-acp-v1",
                registration.model(),
                registration.reasoning_effort(),
                registration.identity(),
            )
            .unwrap();
            assert!(
                registration
                    .clone()
                    .with_capability_binding(binding.clone())
                    .is_ok()
            );
            let wrong_static = AiProviderCapabilitySessionBinding::new(
                AiCapabilityDeliveryMode::FixedBroker,
                index_set.fingerprint(),
                BTreeSet::new(),
                "grok-acp-v1",
                registration.model(),
                registration.reasoning_effort(),
                registration.identity(),
            )
            .unwrap();
            assert!(
                registration
                    .clone()
                    .with_capability_binding(wrong_static)
                    .is_err()
            );
            let wrong_index = AiProviderCapabilitySessionBinding::new(
                AiCapabilityDeliveryMode::FixedBroker,
                "d".repeat(64),
                BTreeSet::from([static_tool.fingerprint.clone()]),
                "grok-acp-v1",
                registration.model(),
                registration.reasoning_effort(),
                registration.identity(),
            )
            .unwrap();
            // 6A: a FixedBroker binding is catalogue-independent. A different
            // index fingerprint is validated and dropped, so the binding is
            // identical and still admitted; describe/execute revalidate drift.
            assert_eq!(wrong_index.fingerprint(), binding.fingerprint());
            assert_eq!(wrong_index.capability_index_fingerprint(), None);
            assert!(registration.with_capability_binding(wrong_index).is_ok());
        }
        assert!(
            surface
                .tools()
                .iter()
                .any(|definition| { definition.tool_id == AI_CAPABILITY_DISCOVER_TOOL_ID })
        );
        let clone = delivery.clone();
        assert!(delivery.shares_run_state(&clone));
        assert!(matches!(
            clone.set_static_bootstrap_offering(&BTreeSet::from(["0".repeat(64)])),
            Err(AiError::Forbidden)
        ));
        assert!(delivery.matches_offered_tools(surface.tools()));
        let frozen = surface.freeze_offering().unwrap().unwrap();
        let frozen_clone = frozen.clone();
        assert!(matches!(
            clone.set_static_bootstrap_offering(&BTreeSet::new()),
            Err(AiError::Conflict)
        ));
        drop(frozen);
        assert!(
            clone
                .set_static_bootstrap_offering(&BTreeSet::new())
                .is_err()
        );
        drop(frozen_clone);
        clone
            .set_static_bootstrap_offering(&BTreeSet::new())
            .unwrap();
        assert_eq!(delivery.current_tools().len(), 4);
        assert!(delivery.shares_run_state(&clone));
        assert!(matches!(surface.freeze_offering(), Err(AiError::Conflict)));
        delivery
            .set_static_bootstrap_offering(&BTreeSet::from([static_tool.fingerprint.clone()]))
            .unwrap();
        assert_eq!(delivery.current_tools(), surface.tools());
        // Restoring the same definitions does not revive a stale snapshot generation.
        assert!(matches!(surface.freeze_offering(), Err(AiError::Conflict)));
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
    fn provider_binding_accepts_large_exact_bootstrap_and_keeps_a_finite_limit() {
        let make = |count: usize| {
            AiProviderCapabilitySessionBinding::new(
                AiCapabilityDeliveryMode::FixedBroker,
                "a".repeat(64),
                (0..count).map(|index| format!("{index:064x}")).collect(),
                "test-projection/v1",
                "test-model",
                ModelReasoningEffort::Low,
                "b".repeat(64),
            )
        };
        for count in [64, 65, 86, 128] {
            let binding = make(count).expect("bounded larger host bootstrap should be admitted");
            assert_eq!(binding.static_bootstrap_tool_fingerprints().len(), count);
        }
        assert!(make(129).is_err());
        assert_ne!(
            make(85).unwrap().fingerprint(),
            make(86).unwrap().fingerprint()
        );
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

    fn large_static_index(entry_count: usize) -> Arc<AiCapabilityIndex> {
        let operation_catalogue = GraphqlOperationCatalog::compose(Vec::<(
            &'static [GeneratedGraphqlOperationDescriptor],
            bool,
            bool,
        )>::new());
        let semantics = GraphqlSemanticCatalog::compose(Vec::new(), &operation_catalogue)
            .expect("semantic catalogue");
        let descriptors = (0..entry_count).map(|position| {
            crate::AiToolDescriptor::new(
                format!("catalogue.record_{position:04}"),
                "Read one reviewed catalogue record.",
                crate::AiToolOperationKind::Query,
                "query ReviewedRecord { record }",
                json!({
                    "type": "object",
                    "properties": {},
                    "required": [],
                    "additionalProperties": false
                }),
            )
            .expect("descriptor")
            .with_maximum_classification(crate::DataClassification::Public)
        });
        Arc::new(
            AiCapabilityIndex::compile(
                crate::GraphqlExecutionTargetId::parse("catalogue-application").expect("target"),
                "schema-v1",
                &semantics,
                None,
                None,
                None,
                descriptors,
                "target-policy-v1",
                crate::AiCapabilityIndexLimits::default(),
            )
            .expect("large capability index"),
        )
    }

    struct PermittedSet {
        permitted: BTreeSet<String>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl PermittedSet {
        fn new<'a>(permitted: impl IntoIterator<Item = &'a str>) -> Self {
            Self {
                permitted: permitted.into_iter().map(str::to_owned).collect(),
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl AiCapabilityAuthorityPolicy for PermittedSet {
        async fn authorize(
            &self,
            _principal: &ResolvedPrincipal,
            _owning_index: &AiCapabilityIndex,
            entry: &AiCapabilityIndexEntry,
            _run: &AiCapabilityRunBinding,
        ) -> Result<AiCapabilityAuthorityDecision, AiError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(AiCapabilityAuthorityDecision {
                allowed: self.permitted.contains(entry.id.as_str()),
                policy_fingerprint: "current-policy-v1".to_owned(),
            })
        }
    }

    async fn search_with(
        index: &Arc<AiCapabilityIndex>,
        authority: Arc<PermittedSet>,
        maximum_results: u16,
    ) -> (Vec<String>, usize) {
        let principal = principal();
        let principal_reference = principal.reference();
        let broker = AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal)),
            Arc::new(CurrentIndex(RwLock::new(Arc::clone(index)))),
            authority.clone(),
            Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
            Duration::seconds(30),
        )
        .expect("broker");
        let result = broker
            .search(
                &principal_reference,
                &run_binding(),
                &AiCapabilitySearchQuery {
                    text: "reviewed catalogue record".to_owned(),
                    namespace: None,
                    kind: Some(AiCapabilityKind::ReviewedStatic),
                    entity_or_operation: None,
                    maximum_results,
                },
            )
            .await
            .expect("authorized search");
        (
            result
                .candidates
                .iter()
                .map(|candidate| candidate.id.as_str().to_owned())
                .collect(),
            authority.calls.load(Ordering::SeqCst),
        )
    }

    #[tokio::test]
    async fn restricted_search_fills_from_permitted_low_ranked_candidates() {
        const ENTRIES: usize = 1_500;
        let index = large_static_index(ENTRIES);
        let low_ranked = [
            "catalogue.record_1497",
            "catalogue.record_1498",
            "catalogue.record_1499",
        ];

        let (restricted, restricted_calls) =
            search_with(&index, Arc::new(PermittedSet::new(low_ranked)), 8).await;
        assert_eq!(
            restricted, low_ranked,
            "permitted matches below the unfiltered top-k must still be returned in rank order"
        );

        let sparse = (0..ENTRIES)
            .step_by(100)
            .map(|position| format!("catalogue.record_{position:04}"))
            .collect::<Vec<_>>();
        let (sparse_result, sparse_calls) = search_with(
            &index,
            Arc::new(PermittedSet::new(sparse.iter().map(String::as_str))),
            8,
        )
        .await;
        assert_eq!(sparse_result, sparse[..8]);

        let everything = (0..ENTRIES)
            .map(|position| format!("catalogue.record_{position:04}"))
            .collect::<Vec<_>>();
        let (unrestricted, unrestricted_calls) = search_with(
            &index,
            Arc::new(PermittedSet::new(everything.iter().map(String::as_str))),
            8,
        )
        .await;
        assert_eq!(unrestricted, everything[..8]);

        let (denied, denied_calls) = search_with(&index, Arc::new(PermittedSet::new([])), 8).await;
        assert!(denied.is_empty());

        // Policy work depends on the query and catalogue only, never on which
        // candidates the caller may use, so latency cannot reveal hidden ones.
        assert_eq!(restricted_calls, ENTRIES);
        assert_eq!(sparse_calls, ENTRIES);
        assert_eq!(unrestricted_calls, ENTRIES);
        assert_eq!(denied_calls, ENTRIES);
    }

    #[derive(Clone, Copy, Debug)]
    enum BatchShape {
        Aligned,
        Short,
        Reordered,
        Extra,
        Duplicate,
    }

    struct BatchAuthority {
        shape: BatchShape,
        batches: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl AiCapabilityAuthorityPolicy for BatchAuthority {
        async fn authorize(
            &self,
            _principal: &ResolvedPrincipal,
            _owning_index: &AiCapabilityIndex,
            _entry: &AiCapabilityIndexEntry,
            _run: &AiCapabilityRunBinding,
        ) -> Result<AiCapabilityAuthorityDecision, AiError> {
            panic!("search must use the batch decision");
        }

        async fn authorize_discovery(
            &self,
            _principal: &ResolvedPrincipal,
            matches: &[AiCapabilityRankedMatch<'_>],
            _run: &AiCapabilityRunBinding,
        ) -> Result<Vec<AiCapabilityDiscoveryDecision>, AiError> {
            self.batches.fetch_add(1, Ordering::SeqCst);
            // Every decision is individually correct for the entry it names;
            // only its position relative to the ranking differs.
            let mut decisions = matches
                .iter()
                .map(|candidate| {
                    AiCapabilityDiscoveryDecision::for_match(
                        candidate,
                        candidate.entry().id.as_str().ends_with('7'),
                    )
                })
                .collect::<Vec<_>>();
            match self.shape {
                BatchShape::Aligned => {}
                BatchShape::Short => {
                    decisions.pop();
                }
                BatchShape::Reordered => {
                    // Grouped and concatenated out of rank order, same length.
                    // Position 0 would otherwise receive record_0007's allow.
                    decisions.swap(0, 7);
                }
                BatchShape::Extra => {
                    let last = decisions.last().cloned().expect("matches");
                    decisions.push(last);
                }
                BatchShape::Duplicate => {
                    decisions[1] = decisions[0].clone();
                }
            }
            Ok(decisions)
        }
    }

    fn batch_broker(
        index: &Arc<AiCapabilityIndex>,
        authority: Arc<dyn AiCapabilityAuthorityPolicy>,
    ) -> AiCapabilityDiscoveryBroker {
        AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal())),
            Arc::new(CurrentIndex(RwLock::new(Arc::clone(index)))),
            authority,
            Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
            Duration::seconds(30),
        )
        .expect("broker")
    }

    fn catalogue_query(maximum_results: u16) -> AiCapabilitySearchQuery {
        AiCapabilitySearchQuery {
            text: "reviewed catalogue record".to_owned(),
            namespace: None,
            kind: Some(AiCapabilityKind::ReviewedStatic),
            entity_or_operation: None,
            maximum_results,
        }
    }

    #[tokio::test]
    async fn batch_discovery_decisions_must_name_each_ranked_entry_in_order() {
        let index = large_static_index(200);
        let principal_reference = principal().reference();
        for shape in [
            BatchShape::Aligned,
            BatchShape::Short,
            BatchShape::Reordered,
            BatchShape::Extra,
            BatchShape::Duplicate,
        ] {
            let authority = Arc::new(BatchAuthority {
                shape,
                batches: std::sync::atomic::AtomicUsize::new(0),
            });
            let result = batch_broker(&index, authority.clone())
                .search(&principal_reference, &run_binding(), &catalogue_query(3))
                .await;
            assert_eq!(authority.batches.load(Ordering::SeqCst), 1);
            match shape {
                BatchShape::Aligned => assert_eq!(
                    result
                        .expect("aligned batch search")
                        .candidates
                        .iter()
                        .map(|candidate| candidate.id.as_str())
                        .collect::<Vec<_>>(),
                    [
                        "catalogue.record_0007",
                        "catalogue.record_0017",
                        "catalogue.record_0027"
                    ]
                ),
                _ => assert!(
                    matches!(result, Err(AiError::Forbidden)),
                    "{shape:?} batch must be refused"
                ),
            }
        }
    }

    #[tokio::test]
    async fn discovery_authorization_bound_refuses_broad_queries_before_policy() {
        let index = large_static_index(200);
        let principal_reference = principal().reference();
        let authority = Arc::new(PermittedSet::new(["catalogue.record_0199"]));
        let bounded = batch_broker(&index, authority.clone())
            .with_maximum_discovery_authorizations(199)
            .expect("bound");
        assert!(matches!(
            bounded
                .search(&principal_reference, &run_binding(), &catalogue_query(8))
                .await,
            Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_DISCOVERY_TOO_BROAD
        ));
        assert_eq!(authority.calls.load(Ordering::SeqCst), 0);

        let admitted = batch_broker(&index, authority.clone())
            .with_maximum_discovery_authorizations(200)
            .expect("bound");
        let result = admitted
            .search(&principal_reference, &run_binding(), &catalogue_query(8))
            .await
            .expect("bounded search");
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].id.as_str(), "catalogue.record_0199");
        assert_eq!(authority.calls.load(Ordering::SeqCst), 200);

        assert!(matches!(
            batch_broker(&index, authority).with_maximum_discovery_authorizations(0),
            Err(AiError::InvalidConfiguration(_))
        ));
    }

    struct InFlightAuthority {
        current: std::sync::atomic::AtomicUsize,
        peak: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl AiCapabilityAuthorityPolicy for InFlightAuthority {
        async fn authorize(
            &self,
            _principal: &ResolvedPrincipal,
            _owning_index: &AiCapabilityIndex,
            entry: &AiCapabilityIndexEntry,
            _run: &AiCapabilityRunBinding,
        ) -> Result<AiCapabilityAuthorityDecision, AiError> {
            let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            // Later-ranked entries finish first, so ordering cannot rely on
            // completion order.
            let position = entry
                .id
                .as_str()
                .rsplit('_')
                .next()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or_default();
            for _ in 0..(200 - position) % 7 {
                tokio::task::yield_now().await;
            }
            self.current.fetch_sub(1, Ordering::SeqCst);
            Ok(AiCapabilityAuthorityDecision {
                allowed: entry.id.as_str().ends_with('3'),
                policy_fingerprint: "current-policy-v1".to_owned(),
            })
        }
    }

    #[tokio::test]
    async fn default_discovery_policy_is_concurrent_bounded_and_rank_ordered() {
        let index = large_static_index(200);
        let authority = Arc::new(InFlightAuthority {
            current: std::sync::atomic::AtomicUsize::new(0),
            peak: std::sync::atomic::AtomicUsize::new(0),
        });
        let result = batch_broker(&index, authority.clone())
            .search(
                &principal().reference(),
                &run_binding(),
                &catalogue_query(4),
            )
            .await
            .expect("concurrent search");
        assert_eq!(
            result
                .candidates
                .iter()
                .map(|candidate| candidate.id.as_str())
                .collect::<Vec<_>>(),
            [
                "catalogue.record_0003",
                "catalogue.record_0013",
                "catalogue.record_0023",
                "catalogue.record_0033"
            ]
        );
        let peak = authority.peak.load(Ordering::SeqCst);
        assert!(
            peak > 1 && peak <= AI_CAPABILITY_DISCOVERY_AUTHORIZATION_CONCURRENCY,
            "peak in-flight policy calls was {peak}"
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
            Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_STALE
        ));

        *current_index.0.write().expect("index write") = index("target-policy-v1");
        *authority.policy_fingerprint.write().expect("policy write") =
            "current-policy-v2".to_owned();
        assert!(matches!(
            broker
                .authorize_execution(&principal_reference, &run, &loaded)
                .await,
            Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_STALE
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
                    kind: Some(AiCapabilityKind::GeneratedEntity),
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
            Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_STALE
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
                    kind: Some(AiCapabilityKind::GeneratedEntity),
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
        let static_tool = definition("preview.semantic");
        let binding = AiProviderCapabilitySessionBinding::new(
            AiCapabilityDeliveryMode::ClientDeferred,
            &index_fingerprint,
            BTreeSet::from([static_tool.fingerprint.clone()]),
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
            vec![static_tool.clone()],
            false,
            binding,
            broker.clone(),
            session.clone(),
        )
        .expect("client-deferred delivery");
        delivery
            .set_static_bootstrap_offering(&BTreeSet::new())
            .unwrap();
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
        let loaded_tools = delivery.current_tools();
        delivery
            .set_static_bootstrap_offering(&BTreeSet::from([static_tool.fingerprint.clone()]))
            .unwrap();
        assert_eq!(delivery.current_tools().len(), 3);
        assert!(
            loaded_tools
                .iter()
                .all(|tool| delivery.current_tools().contains(tool))
        );
        delivery
            .set_static_bootstrap_offering(&BTreeSet::new())
            .unwrap();
        assert_eq!(delivery.current_tools(), loaded_tools);

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
        let definitions = fixed_broker_definitions();
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
        assert!(
            execute
                .description
                .contains("signature and expanded types admit")
        );
        assert!(
            execute.parameters["properties"]["maximumItems"]["description"]
                .as_str()
                .is_some_and(|description| description.contains("lists maximumItems"))
        );
    }

    struct DiscoveryCacheFixture {
        broker: AiCapabilityDiscoveryBroker,
        principal: PrincipalReference,
        run: AiCapabilityRunBinding,
        clock: Arc<FixedClock>,
        authority: Arc<Authority>,
        index: Arc<CurrentIndex>,
    }

    impl DiscoveryCacheFixture {
        fn new(capacity: u16) -> Self {
            let principal = principal();
            let reference = principal.reference();
            let clock = Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH));
            let authority = Arc::new(Authority {
                allowed: AtomicBool::new(true),
                policy_fingerprint: RwLock::new("current-policy-v1".to_owned()),
            });
            let index = Arc::new(CurrentIndex(RwLock::new(generated_index(
                "target-policy-v1",
            ))));
            let broker = AiCapabilityDiscoveryBroker::new(
                Arc::new(Resolver(principal)),
                index.clone(),
                authority.clone(),
                clock.clone(),
                Duration::seconds(30),
            )
            .expect("broker")
            .with_discovery_cache(Duration::days(1), capacity)
            .expect("cache");
            Self {
                broker,
                principal: reference,
                run: run_binding(),
                clock,
                authority,
                index,
            }
        }

        async fn discover(&self, run: &AiCapabilityRunBinding) -> serde_json::Value {
            let result = self.broker.dispatch_discover(
                &self.principal, run, &Self::session(),
                &json!({"text": "reviewed application record", "kind": "generated_query", "maximumResults": 1}),
            ).await.expect("discovery");
            json!({
                "capabilityId": result["candidates"][0]["capabilityId"],
                "candidateFingerprint": result["candidates"][0]["candidateFingerprint"]
            })
        }

        fn session() -> AiCapabilityBrokerSession {
            AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
                .expect("run state")
        }
    }

    #[tokio::test]
    async fn discovery_cache_reuses_metadata_across_runs_with_fresh_execution_fences() {
        let mut fixture = DiscoveryCacheFixture::new(4);
        fixture.principal.expires_at = Some(fixture.clock.now() + Duration::minutes(75));
        let candidate = fixture.discover(&fixture.run).await;
        let first_session = DiscoveryCacheFixture::session();
        let first = fixture
            .broker
            .dispatch_describe(&fixture.principal, &fixture.run, &first_session, &candidate)
            .await
            .expect("cached candidate describes in first run");
        let mut changed_deadline = fixture.principal.clone();
        changed_deadline.expires_at = Some(fixture.clock.now() + Duration::minutes(90));
        assert!(matches!(fixture.broker.authorize_broker_execution(
            &changed_deadline, &fixture.run, &first_session,
            &json!({"loadedReference": first.loaded_reference(), "selections": ["records.id"]}),
        ).await, Err(AiError::Forbidden)));
        fixture.clock.advance_seconds(3_600);
        fixture.principal.expires_at = Some(fixture.clock.now() + Duration::minutes(75));
        let mut next_run = fixture.run.clone();
        next_run.run_id = AiRunId(Uuid::from_u128(200));
        next_run.attempt_id = Uuid::from_u128(201);
        let next_session = DiscoveryCacheFixture::session();
        let next = fixture
            .broker
            .clone()
            .dispatch_describe(&fixture.principal, &next_run, &next_session, &candidate)
            .await
            .expect("one-hour-old candidate describes without another discovery");
        assert_eq!(next_session.amplification().discover_calls, 0);
        assert_eq!(next_session.amplification().describe_calls, 1);
        assert_ne!(first.loaded_reference(), next.loaded_reference());
        let execute =
            json!({"loadedReference": next.loaded_reference(), "selections": ["records.id"]});
        fixture
            .broker
            .authorize_broker_execution(&fixture.principal, &next_run, &next_session, &execute)
            .await
            .expect("new run has fresh execution authority");
        let old_execute =
            json!({"loadedReference": first.loaded_reference(), "selections": ["records.id"]});
        assert!(matches!(
            fixture
                .broker
                .authorize_broker_execution(
                    &fixture.principal,
                    &next_run,
                    &next_session,
                    &old_execute,
                )
                .await,
            Err(AiError::InvalidInput(_))
        ));
        assert!(matches!(
            fixture
                .broker
                .authorize_broker_execution(
                    &fixture.principal,
                    &fixture.run,
                    &next_session,
                    &execute,
                )
                .await,
            Err(AiError::Forbidden)
        ));
        fixture.authority.allowed.store(false, Ordering::SeqCst);
        assert!(matches!(
            fixture
                .broker
                .authorize_broker_execution(&fixture.principal, &next_run, &next_session, &execute,)
                .await,
            Err(AiError::Forbidden)
        ));
        assert!(matches!(
            fixture
                .broker
                .dispatch_describe(
                    &fixture.principal,
                    &next_run,
                    &DiscoveryCacheFixture::session(),
                    &candidate,
                )
                .await,
            Err(AiError::Forbidden)
        ));
    }

    fn schema_changed_index() -> Arc<AiCapabilityIndex> {
        generated_index_with_schema(
            "generated-read-application",
            "generated-read",
            "target-policy-v1",
            Some("note"),
            false,
        )
    }

    #[test]
    fn fixed_broker_binding_and_definitions_survive_a_schema_change() {
        let before = generated_index("target-policy-v1");
        let after = schema_changed_index();
        assert_ne!(before.schema_fingerprint(), after.schema_fingerprint());
        let before_set = AiCapabilityIndexSet::compile([before]).expect("index set before");
        let after_set = AiCapabilityIndexSet::compile([after]).expect("index set after");
        assert_ne!(before_set.fingerprint(), after_set.fingerprint());

        let static_tool = definition("reviewed.records_list");
        let binding = |mode, indexes: &AiCapabilityIndexSet| {
            AiProviderCapabilitySessionBinding::new(
                mode,
                indexes.fingerprint(),
                BTreeSet::from([static_tool.fingerprint.clone()]),
                "provider-projection-v1",
                "model-test",
                ModelReasoningEffort::Unspecified,
                "f".repeat(64),
            )
            .expect("session binding")
        };
        let retained = binding(AiCapabilityDeliveryMode::FixedBroker, &before_set);
        let current = binding(AiCapabilityDeliveryMode::FixedBroker, &after_set);
        assert_eq!(retained, current);
        assert_eq!(retained.fingerprint(), current.fingerprint());
        assert_eq!(retained.capability_index_fingerprint(), None);
        assert_eq!(retained.capability_index_set_fingerprint(), None);

        // Catalogue-derived modes still rotate with the catalogue.
        for mode in [
            AiCapabilityDeliveryMode::EagerExact,
            AiCapabilityDeliveryMode::ProviderDeferred,
            AiCapabilityDeliveryMode::ClientDeferred,
        ] {
            let before = binding(mode, &before_set);
            assert_eq!(
                before.capability_index_fingerprint(),
                Some(before_set.fingerprint())
            );
            assert_ne!(
                before.fingerprint(),
                binding(mode, &after_set).fingerprint()
            );
        }
        assert_ne!(
            discovery_definition(Some(before_set.fingerprint())),
            discovery_definition(Some(after_set.fingerprint()))
        );

        // The binding retained from before the change selects the surface for
        // the changed catalogue, and that surface is byte-identical.
        let capabilities = ProviderCapabilities {
            custom_tools: true,
            capability_delivery_modes: BTreeSet::from([AiCapabilityDeliveryMode::FixedBroker]),
            ..ProviderCapabilities::default()
        };
        let select = |indexes: &AiCapabilityIndexSet| {
            let fixture = DiscoveryCacheFixture::new(1);
            AiCapabilityDeliveryTurn::select(
                &capabilities,
                indexes.fingerprint(),
                vec![static_tool.clone()],
                true,
                retained.clone(),
                Arc::new(fixture.broker),
                DiscoveryCacheFixture::session(),
            )
            .expect("retained fixed-broker binding should select")
        };
        let before_turn = select(&before_set);
        let after_turn = select(&after_set);
        assert_eq!(before_turn.current_tools(), after_turn.current_tools());
        assert!(after_turn.matches_offered_tools(&before_turn.current_tools()));
        let mut broker_tools = capability_broker_definitions();
        broker_tools.push(static_tool.clone());
        broker_tools.sort_by(|left, right| left.tool_id.cmp(&right.tool_id));
        assert_eq!(after_turn.current_tools(), broker_tools);
        for operation in [
            AiCapabilityBrokerOperation::Discover,
            AiCapabilityBrokerOperation::Describe,
            AiCapabilityBrokerOperation::Execute,
        ] {
            assert_eq!(
                before_turn.broker_definition(operation),
                after_turn.broker_definition(operation)
            );
        }
    }

    #[tokio::test]
    async fn schema_change_is_a_recoverable_capability_stale_distinct_from_an_unheld_selection() {
        use crate::AiApplicationToolFailureCode as Code;
        let code = |error: &AiError| crate::classify_safe_application_tool_error(error);
        let fixture = DiscoveryCacheFixture::new(4);
        let candidate = fixture.discover(&fixture.run).await;
        let session = DiscoveryCacheFixture::session();
        let described = fixture
            .broker
            .dispatch_describe(&fixture.principal, &fixture.run, &session, &candidate)
            .await
            .expect("candidate describes before the schema change");
        let execute =
            json!({"loadedReference": described.loaded_reference(), "selections": ["recordId"]});
        fixture
            .broker
            .authorize_broker_execution(&fixture.principal, &fixture.run, &session, &execute)
            .await
            .expect("loaded capability executes before the schema change");

        // A selection the run never held is the generic outcome, before and
        // after the change, whether or not the identifier exists.
        let unknown = json!({"loadedReference": "e".repeat(64), "selections": ["recordId"]});
        let mut forged = candidate.clone();
        forged["candidateFingerprint"] = json!("f".repeat(64));
        let mut absent = candidate.clone();
        absent["capabilityId"] = json!("generated.never_published");
        let assert_unheld = |error: AiError| {
            assert!(
                matches!(&error, AiError::InvalidInput(message)
                    if message == AI_CAPABILITY_SELECTION_STALE),
                "{error:?}"
            );
            assert_eq!(code(&error), Some(Code::SelectionStale));
        };
        let unheld = || async {
            assert_unheld(
                fixture
                    .broker
                    .authorize_broker_execution(
                        &fixture.principal,
                        &fixture.run,
                        &session,
                        &unknown,
                    )
                    .await
                    .expect_err("an unknown loaded reference is refused"),
            );
            for selection in [&forged, &absent] {
                assert_unheld(
                    fixture
                        .broker
                        .dispatch_describe(&fixture.principal, &fixture.run, &session, selection)
                        .await
                        .expect_err("an unheld candidate is refused"),
                );
            }
        };
        unheld().await;

        *fixture.index.0.write().expect("index") = schema_changed_index();

        let assert_stale = |error: AiError| {
            assert!(
                matches!(&error, AiError::InvalidInput(message) if message == AI_CAPABILITY_STALE),
                "{error:?}"
            );
            assert_eq!(code(&error), Some(Code::CapabilityStale));
        };
        assert_stale(
            fixture
                .broker
                .authorize_broker_execution(&fixture.principal, &fixture.run, &session, &execute)
                .await
                .expect_err("a loaded capability is stale after the schema change"),
        );
        assert_stale(
            fixture
                .broker
                .dispatch_describe(&fixture.principal, &fixture.run, &session, &candidate)
                .await
                .expect_err("a held candidate is stale after the schema change"),
        );
        unheld().await;
        assert!(Code::CapabilityStale.retryable() && Code::SelectionStale.retryable());
        assert_ne!(
            Code::CapabilityStale.as_str(),
            Code::SelectionStale.as_str()
        );

        // The same run and provider-session binding recover by discovering
        // and describing again; nothing is rebound.
        let candidate = fixture.discover(&fixture.run).await;
        let described = fixture
            .broker
            .dispatch_describe(&fixture.principal, &fixture.run, &session, &candidate)
            .await
            .expect("the changed capability describes again");
        let execute =
            json!({"loadedReference": described.loaded_reference(), "selections": ["note"]});
        fixture
            .broker
            .authorize_broker_execution(&fixture.principal, &fixture.run, &session, &execute)
            .await
            .expect("the re-described capability executes under the same binding");

        // A changed host-policy fingerprint that still allows the capability
        // is drift; a denial is never reported as drift.
        *fixture
            .authority
            .policy_fingerprint
            .write()
            .expect("policy") = "current-policy-v2".to_owned();
        assert_stale(
            fixture
                .broker
                .authorize_broker_execution(&fixture.principal, &fixture.run, &session, &execute)
                .await
                .expect_err("a changed policy fingerprint voids the loaded contract"),
        );
        *fixture
            .authority
            .policy_fingerprint
            .write()
            .expect("policy") = "current-policy-v1".to_owned();
        fixture.authority.allowed.store(false, Ordering::SeqCst);
        assert!(matches!(
            fixture
                .broker
                .authorize_broker_execution(&fixture.principal, &fixture.run, &session, &execute)
                .await,
            Err(AiError::Forbidden)
        ));
    }

    #[tokio::test]
    async fn discovery_cache_isolates_principals_sessions_and_rejects_index_drift() {
        let fixture = DiscoveryCacheFixture::new(4);
        let candidate = fixture.discover(&fixture.run).await;
        let mut other_run = fixture.run.clone();
        other_run.session_id = AiSessionId(Uuid::from_u128(999));
        assert!(matches!(
            fixture
                .broker
                .dispatch_describe(
                    &fixture.principal,
                    &other_run,
                    &DiscoveryCacheFixture::session(),
                    &candidate,
                )
                .await,
            Err(AiError::InvalidInput(_))
        ));
        let mut other = principal();
        if let AuthPrincipal::User(user) = &mut other {
            user.user_id = "another-user".to_owned();
        }
        assert!(matches!(
            fixture
                .broker
                .dispatch_describe(
                    &other.reference(),
                    &fixture.run,
                    &DiscoveryCacheFixture::session(),
                    &candidate,
                )
                .await,
            Err(AiError::InvalidInput(_))
        ));
        let mut forged = candidate.clone();
        forged["candidateFingerprint"] = json!("f".repeat(64));
        assert!(matches!(
            fixture
                .broker
                .dispatch_describe(
                    &fixture.principal,
                    &fixture.run,
                    &DiscoveryCacheFixture::session(),
                    &forged,
                )
                .await,
            Err(AiError::InvalidInput(_))
        ));
        *fixture.index.0.write().expect("index") = generated_index("target-policy-v2");
        assert!(matches!(
            fixture
                .broker
                .dispatch_describe(
                    &fixture.principal,
                    &fixture.run,
                    &DiscoveryCacheFixture::session(),
                    &candidate,
                )
                .await,
            Err(AiError::InvalidInput(_))
        ));
    }

    #[tokio::test]
    async fn discovery_cache_expires_evicts_and_stays_optional() {
        let fixture = DiscoveryCacheFixture::new(1);
        let candidate = fixture.discover(&fixture.run).await;
        fixture.clock.advance_seconds(86_400);
        assert!(matches!(
            fixture
                .broker
                .dispatch_describe(
                    &fixture.principal,
                    &fixture.run,
                    &DiscoveryCacheFixture::session(),
                    &candidate,
                )
                .await,
            Err(AiError::InvalidInput(_))
        ));
        fixture.discover(&fixture.run).await;
        let mut other_run = fixture.run.clone();
        other_run.session_id = AiSessionId(Uuid::from_u128(999));
        fixture.discover(&other_run).await;
        assert!(matches!(
            fixture
                .broker
                .dispatch_describe(
                    &fixture.principal,
                    &fixture.run,
                    &DiscoveryCacheFixture::session(),
                    &candidate,
                )
                .await,
            Err(AiError::InvalidInput(_))
        ));
        fixture
            .broker
            .dispatch_describe(
                &fixture.principal,
                &other_run,
                &DiscoveryCacheFixture::session(),
                &candidate,
            )
            .await
            .expect("newest entry survives eviction");
        let mut uncached = fixture.broker.clone();
        uncached.discovery_cache = None;
        assert!(matches!(
            uncached
                .dispatch_describe(
                    &fixture.principal,
                    &other_run,
                    &DiscoveryCacheFixture::session(),
                    &candidate,
                )
                .await,
            Err(AiError::InvalidInput(_))
        ));
        for (ttl, capacity) in [
            (Duration::ZERO, 1),
            (Duration::days(8), 1),
            (Duration::days(1), 0),
            (Duration::days(1), 1_025),
        ] {
            assert!(
                uncached
                    .clone()
                    .with_discovery_cache(ttl, capacity)
                    .is_err()
            );
        }
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
            operations: Vec::new(),
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
            operations: Vec::new(),
            contract: json!({"description": "x".repeat(2_048)}),
        };
        assert!(matches!(
            oversized_metadata.with_plan_schema(json!({"type": "object"}), 1_024),
            Err(AiError::InvalidConfiguration(_))
        ));
    }

    fn static_broker_fixture() -> (
        AiCapabilityDiscoveryBroker,
        PrincipalReference,
        AiCapabilityRunBinding,
        AiCapabilityIndexEntry,
    ) {
        let principal = principal();
        let principal_reference = principal.reference();
        let indexes = AiCapabilityIndexSet::compile([
            index("target-policy-v1"),
            generated_index("target-policy-v1"),
        ])
        .expect("index set");
        let static_entry = indexes
            .indexes()
            .flat_map(|index| index.entries())
            .find(|entry| entry.kind == AiCapabilityKind::ReviewedStatic)
            .expect("reviewed static entry")
            .clone();
        let broker = AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal)),
            Arc::new(CurrentIndexes(RwLock::new(Arc::new(indexes)))),
            Arc::new(Authority {
                allowed: AtomicBool::new(true),
                policy_fingerprint: RwLock::new("current-policy-v1".to_owned()),
            }),
            Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
            Duration::seconds(30),
        )
        .expect("broker");
        (broker, principal_reference, run_binding(), static_entry)
    }

    fn brokering<S: AsRef<str>>(fingerprints: &[S]) -> AiCapabilityBrokerSession {
        AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
            .expect("broker session")
            .with_brokered_static_tools(
                fingerprints
                    .iter()
                    .map(|fingerprint| fingerprint.as_ref().to_owned())
                    .collect(),
            )
            .expect("brokered static tools")
    }

    fn candidate_kinds(discovery: &serde_json::Value) -> BTreeSet<String> {
        discovery["candidates"]
            .as_array()
            .expect("candidates")
            .iter()
            .map(|candidate| candidate["kind"].as_str().expect("kind").to_owned())
            .collect()
    }

    #[tokio::test]
    async fn a_static_tool_reaches_the_broker_only_in_a_run_that_admits_its_fingerprint() {
        let (broker, principal, run, static_entry) = static_broker_fixture();
        let search = json!({"text": "list the latest jobs", "maximumResults": 8});

        // Default run: the static tool is in the index and never surfaces.
        let default = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
            .expect("broker session");
        let discovery = broker
            .dispatch_discover(&principal, &run, &default, &search)
            .await
            .expect("default discovery");
        assert!(!candidate_kinds(&discovery).contains("reviewed_static"));
        assert!(matches!(
            broker
                .dispatch_discover(
                    &principal,
                    &run,
                    &default,
                    &json!({"text": "jobs", "kind": "reviewed_static", "maximumResults": 8}),
                )
                .await,
            Err(AiError::InvalidInput(_))
        ));
        // A run that brokers some other fingerprint does not admit this one.
        let other = brokering(&[&"e".repeat(64)]);
        let discovery = broker
            .dispatch_discover(&principal, &run, &other, &search)
            .await
            .expect("other discovery");
        assert!(!candidate_kinds(&discovery).contains("reviewed_static"));

        let session = brokering(&[&static_entry.capability_fingerprint]);
        let discovery = broker
            .dispatch_discover(
                &principal,
                &run,
                &session,
                &json!({"text": "list the latest jobs", "kind": "reviewed_static", "maximumResults": 8}),
            )
            .await
            .expect("brokered discovery");
        let candidate = &discovery["candidates"][0];
        assert_eq!(candidate["capabilityId"], static_entry.id.as_str());
        assert_eq!(candidate["kind"], "reviewed_static");
        // An unfiltered search still returns generated queries.
        let mixed = broker
            .dispatch_discover(
                &principal,
                &run,
                &session,
                &json!({"text": "reviewed application record jobs", "maximumResults": 8}),
            )
            .await
            .expect("mixed discovery");
        assert_eq!(
            candidate_kinds(&mixed),
            BTreeSet::from(["generated_entity".to_owned(), "reviewed_static".to_owned()])
        );
        // Mutations and subscriptions stay outside the broker.
        for kind in ["generated_mutation", "generated_subscription"] {
            assert!(matches!(
                broker
                    .dispatch_discover(
                        &principal,
                        &run,
                        &session,
                        &json!({"text": "jobs", "kind": kind, "maximumResults": 8}),
                    )
                    .await,
                Err(AiError::InvalidInput(_))
            ));
        }

        let describe = json!({
            "capabilityId": candidate["capabilityId"],
            "candidateFingerprint": candidate["candidateFingerprint"],
        });
        // A candidate this run did not broker is a stale selection, even when
        // its search result is at hand.
        default.record_search(
            session
                .candidate_search(&static_entry.id, &static_entry.fingerprint)
                .expect("recorded search"),
        );
        assert!(matches!(
            broker
                .dispatch_describe(&principal, &run, &default, &describe)
                .await,
            Err(AiError::InvalidInput(message)) if message.contains("stale")
        ));

        let description = broker
            .dispatch_describe(&principal, &run, &session, &describe)
            .await
            .expect("brokered description");
        assert_eq!(
            description.capability_kind(),
            AiCapabilityKind::ReviewedStatic
        );
        assert_eq!(
            description.capability_fingerprint(),
            static_entry.capability_fingerprint
        );
        assert_eq!(description.contract()["risk"], json!(static_entry.risk));
        assert_eq!(
            description.contract()["approval"],
            json!(static_entry.approval)
        );
        let loaded_reference = description.loaded_reference().to_owned();

        let execution = broker
            .authorize_broker_execution(
                &principal,
                &run,
                &session,
                &json!({
                    "loadedReference": loaded_reference,
                    "arguments": [
                        {"name": "limit", "value": 5},
                        {"name": "filter.status", "value": "open"}
                    ],
                    "selections": []
                }),
            )
            .await
            .expect("brokered execution");
        assert_eq!(
            execution.capability_kind(),
            AiCapabilityKind::ReviewedStatic
        );
        assert_eq!(execution.capability_id(), &static_entry.id);
        assert_eq!(
            execution.capability_fingerprint(),
            static_entry.capability_fingerprint
        );
        // The plan of a static tool is its argument object and nothing else.
        assert_eq!(
            execution.plan(),
            &json!({"limit": 5, "filter": {"status": "open"}})
        );

        // A static tool has a fixed document: it takes no selection or bound.
        for arguments in [
            json!({"loadedReference": loaded_reference, "selections": ["id"]}),
            json!({"loadedReference": loaded_reference, "selections": [], "maximumItems": 5}),
            json!({
                "loadedReference": loaded_reference,
                "selections": [],
                "relationshipMaximumItems": [{"path": "items", "maximumItems": 5}]
            }),
        ] {
            assert!(matches!(
                broker
                    .authorize_broker_execution(&principal, &run, &session, &arguments)
                    .await,
                Err(AiError::InvalidInput(_))
            ));
        }

        // The same loaded binding in run state that no longer brokers the
        // tool does not execute.
        let revoked = AiCapabilityBrokerSession {
            inner: Arc::clone(&session.inner),
            limits: session.limits,
            brokered_static_tools: Arc::new(BTreeSet::new()),
            inline_describe_schema: false,
        };
        assert!(matches!(
            broker
                .authorize_broker_execution(
                    &principal,
                    &run,
                    &revoked,
                    &json!({"loadedReference": loaded_reference, "selections": []}),
                )
                .await,
            Err(AiError::Forbidden)
        ));
    }

    #[tokio::test]
    async fn a_static_tool_that_is_not_an_approval_free_low_classification_read_is_never_brokered()
    {
        let principal = principal();
        let principal_reference = principal.reference();
        let semantics = GraphqlSemanticCatalog::compose(
            Vec::new(),
            &GraphqlOperationCatalog::compose(Vec::<(
                &'static [GeneratedGraphqlOperationDescriptor],
                bool,
                bool,
            )>::new()),
        )
        .expect("semantic catalogue");
        let descriptor = |id: &str| {
            crate::AiToolDescriptor::new(
                id,
                "List reviewed warehouse records.",
                crate::AiToolOperationKind::Query,
                "query Reviewed { records }",
                json!({"type": "object", "properties": {}, "additionalProperties": false}),
            )
            .expect("descriptor")
        };
        let plain = descriptor("warehouse.plain")
            .with_maximum_classification(crate::DataClassification::Confidential);
        let restricted = descriptor("warehouse.restricted")
            .with_maximum_classification(crate::DataClassification::Restricted);
        let supervised = descriptor("warehouse.supervised")
            .with_maximum_classification(crate::DataClassification::Public)
            .with_risk(crate::AiToolRisk::ReadOnly, crate::AiApprovalRule::OneShot);
        let fingerprints =
            [&plain, &restricted, &supervised].map(|descriptor| descriptor.fingerprint.clone());
        let index = Arc::new(
            AiCapabilityIndex::compile(
                crate::GraphqlExecutionTargetId::parse("warehouse").expect("target"),
                "schema-v1",
                &semantics,
                None,
                None,
                None,
                [plain, restricted, supervised],
                "target-policy-v1",
                crate::AiCapabilityIndexLimits::default(),
            )
            .expect("capability index"),
        );
        let broker = AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal)),
            Arc::new(CurrentIndex(RwLock::new(index))),
            Arc::new(Authority {
                allowed: AtomicBool::new(true),
                policy_fingerprint: RwLock::new("current-policy-v1".to_owned()),
            }),
            Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
            Duration::seconds(30),
        )
        .expect("broker");
        let session = brokering(&fingerprints.each_ref().map(String::as_str));
        let discovery = broker
            .dispatch_discover(
                &principal_reference,
                &run_binding(),
                &session,
                &json!({"text": "reviewed warehouse records", "maximumResults": 8}),
            )
            .await
            .expect("discovery");
        assert_eq!(
            discovery["candidates"]
                .as_array()
                .expect("candidates")
                .iter()
                .map(|candidate| candidate["capabilityId"].as_str().expect("id"))
                .collect::<Vec<_>>(),
            ["warehouse.plain"]
        );
    }

    #[test]
    fn brokered_static_tools_need_the_fixed_broker_and_are_never_also_bootstrap_tools() {
        let (broker, _, _, static_entry) = static_broker_fixture();
        let broker = Arc::new(broker);
        let index_fingerprint = "a".repeat(64);
        let bootstrap = definition("records.lookup");
        let select = |modes: &[AiCapabilityDeliveryMode],
                      mode: AiCapabilityDeliveryMode,
                      bootstrap_fingerprints: BTreeSet<String>,
                      session: AiCapabilityBrokerSession| {
            AiCapabilityDeliveryTurn::select(
                &ProviderCapabilities {
                    custom_tools: true,
                    capability_delivery_modes: modes.iter().copied().collect(),
                    ..ProviderCapabilities::default()
                },
                &index_fingerprint,
                vec![bootstrap.clone()],
                mode == AiCapabilityDeliveryMode::FixedBroker,
                AiProviderCapabilitySessionBinding::new(
                    mode,
                    index_fingerprint.clone(),
                    bootstrap_fingerprints,
                    "provider-projection-v1",
                    "gpt-test",
                    ModelReasoningEffort::Unspecified,
                    "f".repeat(64),
                )
                .expect("session binding"),
                Arc::clone(&broker),
                session,
            )
        };
        let brokered = || brokering(&[&static_entry.capability_fingerprint]);

        let delivery = select(
            &[AiCapabilityDeliveryMode::FixedBroker],
            AiCapabilityDeliveryMode::FixedBroker,
            BTreeSet::from([bootstrap.fingerprint.clone()]),
            brokered(),
        )
        .expect("fixed broker with a brokered static tool");
        // The brokered tool adds no provider definition.
        assert_eq!(delivery.current_tools().len(), 5);
        assert_eq!(
            delivery.session().brokered_static_tool_fingerprints(),
            &BTreeSet::from([static_entry.capability_fingerprint.clone()])
        );

        assert!(matches!(
            select(
                &[AiCapabilityDeliveryMode::EagerExact],
                AiCapabilityDeliveryMode::EagerExact,
                BTreeSet::new(),
                brokered(),
            ),
            Err(AiError::InvalidConfiguration(_))
        ));
        assert!(matches!(
            select(
                &[AiCapabilityDeliveryMode::FixedBroker],
                AiCapabilityDeliveryMode::FixedBroker,
                BTreeSet::from([bootstrap.fingerprint.clone()]),
                brokering(&[&bootstrap.fingerprint]),
            ),
            Err(AiError::InvalidConfiguration(_))
        ));
        assert!(
            AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
                .expect("broker session")
                .with_brokered_static_tools(BTreeSet::from(["not-a-fingerprint".to_owned()]))
                .is_err()
        );
    }

    #[test]
    fn describe_attaches_the_shared_type_plan_schema_within_the_same_bound() {
        let filter = json!({
            "type": "object",
            "properties": {
                "eq": {"type": ["string", "null"], "maxLength": 4096},
                "ne": {"type": ["string", "null"], "maxLength": 4096},
                "contains": {"type": ["string", "null"], "maxLength": 4096},
            },
            "required": [],
            "additionalProperties": false,
        });
        let inline = json!({
            "type": "object",
            "properties": {
                "arguments": {
                    "type": "object",
                    "properties": {
                        "name": filter, "code": filter, "notes": filter, "owner": filter,
                    },
                    "required": [],
                    "additionalProperties": false,
                },
            },
            "required": ["arguments"],
            "additionalProperties": false,
        });
        let shared = AiSharedTypePlanSchema::compile(&inline, &std::collections::BTreeMap::new())
            .expect("shared types");
        assert_eq!(shared.types().len(), 1);
        let description = || AiCapabilityDescription {
            loaded_reference: "a".repeat(64),
            capability_id: AiToolId::parse("records.lookup").expect("id"),
            capability_kind: AiCapabilityKind::GeneratedQuery,
            capability_fingerprint: "b".repeat(64),
            operations: Vec::new(),
            contract: json!({"version": AI_CAPABILITY_BROKER_VERSION}),
        };

        let attached = description()
            .with_shared_type_plan_schema(&shared, 64 * 1_024)
            .expect("shared plan schema")
            .into_model_result();
        assert_eq!(attached["planSchemaAvailable"], true);
        assert_eq!(attached["planSchema"], shared.to_model_schema());
        assert!(
            attached["planSchemaTypeReferences"]
                .as_str()
                .expect("reference note")
                .contains("planSchema.types")
        );
        let shared_bytes = serde_json::to_vec(&attached).expect("shared").len();
        let inline_bytes = serde_json::to_vec(
            &description()
                .with_plan_schema(inline.clone(), 64 * 1_024)
                .expect("inline plan schema")
                .into_model_result(),
        )
        .expect("inline")
        .len();
        assert!(shared_bytes < inline_bytes);

        // A bound the inline schema exceeds can still admit the shared one.
        let bound = u32::try_from((shared_bytes + inline_bytes) / 2).expect("bound");
        assert_eq!(
            description()
                .with_plan_schema(inline, bound)
                .expect("oversized inline schema")
                .into_model_result()["planSchemaAvailable"],
            false
        );
        assert_eq!(
            description()
                .with_shared_type_plan_schema(&shared, bound)
                .expect("shared schema within the bound")
                .into_model_result()["planSchemaAvailable"],
            true
        );

        // A schema with nothing to share is attached unchanged, without a note.
        let plain = json!({"type": "object", "properties": {}, "additionalProperties": false});
        let unshared = AiSharedTypePlanSchema::compile(&plain, &std::collections::BTreeMap::new())
            .expect("no shared types");
        let attached = description()
            .with_shared_type_plan_schema(&unshared, 64 * 1_024)
            .expect("plain plan schema")
            .into_model_result();
        assert_eq!(attached["planSchema"], plain);
        assert!(attached.get("planSchemaTypeReferences").is_none());
    }

    /// Denies the listed root capability IDs and allows everything else.
    struct RootAuthority {
        denied: RwLock<BTreeSet<String>>,
        policy_fingerprint: RwLock<String>,
    }

    #[async_trait]
    impl AiCapabilityAuthorityPolicy for RootAuthority {
        async fn authorize(
            &self,
            _principal: &ResolvedPrincipal,
            _owning_index: &AiCapabilityIndex,
            entry: &AiCapabilityIndexEntry,
            _run: &AiCapabilityRunBinding,
        ) -> Result<AiCapabilityAuthorityDecision, AiError> {
            Ok(AiCapabilityAuthorityDecision {
                allowed: !self
                    .denied
                    .read()
                    .map_err(|_| AiError::PersistenceFailed)?
                    .contains(entry.id.as_str()),
                policy_fingerprint: self
                    .policy_fingerprint
                    .read()
                    .map_err(|_| AiError::PersistenceFailed)?
                    .clone(),
            })
        }
    }

    struct EntityBrokerFixture {
        broker: AiCapabilityDiscoveryBroker,
        principal: PrincipalReference,
        run: AiCapabilityRunBinding,
        authority: Arc<RootAuthority>,
        index: Arc<CurrentIndex>,
        session: AiCapabilityBrokerSession,
    }

    impl EntityBrokerFixture {
        fn new() -> Self {
            let principal = principal();
            let reference = principal.reference();
            let authority = Arc::new(RootAuthority {
                denied: RwLock::new(BTreeSet::new()),
                policy_fingerprint: RwLock::new("current-policy-v1".to_owned()),
            });
            let index = Arc::new(CurrentIndex(RwLock::new(two_root_entity_index(
                "target-policy-v1",
            ))));
            let broker = AiCapabilityDiscoveryBroker::new(
                Arc::new(Resolver(principal)),
                index.clone(),
                authority.clone(),
                Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
                Duration::seconds(30),
            )
            .expect("broker");
            Self {
                broker,
                principal: reference,
                run: run_binding(),
                authority,
                index,
                session: DiscoveryCacheFixture::session(),
            }
        }

        fn root_id(&self, operation: &str) -> AiToolId {
            let index = self.index.0.read().expect("index").clone();
            let entity = index
                .entries()
                .find(|entry| entry.kind == AiCapabilityKind::GeneratedEntity)
                .expect("entity entry")
                .clone();
            index
                .operation_entry(&entity.id, operation)
                .expect("root entry")
                .id
                .clone()
        }

        async fn discover(&self) -> serde_json::Value {
            self.broker
                .dispatch_discover(
                    &self.principal,
                    &self.run,
                    &self.session,
                    &json!({"text": "reviewed application record", "maximumResults": 4}),
                )
                .await
                .expect("discovery")
        }

        async fn describe(&self) -> Result<AiCapabilityDescription, AiError> {
            let discovered = self.discover().await;
            let candidate = &discovered["candidates"][0];
            self.broker
                .dispatch_describe(
                    &self.principal,
                    &self.run,
                    &self.session,
                    &json!({
                        "capabilityId": candidate["capabilityId"],
                        "candidateFingerprint": candidate["candidateFingerprint"],
                    }),
                )
                .await
        }

        async fn execute(
            &self,
            arguments: serde_json::Value,
        ) -> Result<AiCapabilityExecution, AiError> {
            self.broker
                .authorize_broker_execution(&self.principal, &self.run, &self.session, &arguments)
                .await
        }
    }

    fn assert_input_error(error: &AiError, expected: &str) {
        assert!(
            matches!(error, AiError::InvalidInput(message) if message == expected),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn one_entity_capability_covers_its_roots_with_per_root_authority() {
        let fixture = EntityBrokerFixture::new();
        let discovered = fixture.discover().await;
        assert_eq!(discovered["version"], 2);
        let candidates = discovered["candidates"].as_array().expect("candidates");
        assert_eq!(
            candidates.len(),
            1,
            "one candidate per entity: {discovered}"
        );
        let candidate = &candidates[0];
        assert_eq!(candidate["kind"], "generated_entity");
        assert_eq!(candidate["entity"], "Record");
        assert_eq!(
            candidate["operations"],
            json!([
                {"operation": "GeneratedRecord", "shape": "custom"},
                {"operation": "RecordBySubject", "shape": "custom"}
            ])
        );

        let described = fixture.describe().await.expect("describe");
        assert_eq!(
            described.capability_kind(),
            AiCapabilityKind::GeneratedEntity
        );
        let operations = described
            .operations()
            .iter()
            .map(|operation| {
                (
                    operation.operation_name().to_owned(),
                    operation.capability_id().clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            operations,
            [
                (
                    "GeneratedRecord".to_owned(),
                    fixture.root_id("GeneratedRecord")
                ),
                (
                    "RecordBySubject".to_owned(),
                    fixture.root_id("RecordBySubject")
                ),
            ]
        );
        let contract = described.contract();
        assert_eq!(contract["version"], 2);
        assert!(contract.get("planSchema").is_none());
        assert_eq!(contract["operations"][1]["operation"], "RecordBySubject");
        let loaded_reference = described.loaded_reference().to_owned();

        // Execute resolves the exact root; the plan compiles against it.
        let execution = fixture
            .execute(json!({
                "loadedReference": loaded_reference,
                "operation": "RecordBySubject",
                "arguments": [{"name": "subject", "value": "quarterly"}],
                "selections": ["recordId"]
            }))
            .await
            .expect("execute one operation");
        assert_eq!(
            execution.capability_id(),
            &fixture.root_id("RecordBySubject")
        );
        assert_eq!(
            execution.capability_kind(),
            AiCapabilityKind::GeneratedQuery
        );
        assert_eq!(execution.plan()["arguments"]["subject"], "quarterly");

        // Several operations: the call must name one the description returned.
        for operation in [None, Some("SomethingElse")] {
            let mut arguments =
                json!({"loadedReference": loaded_reference, "selections": ["recordId"]});
            if let Some(operation) = operation {
                arguments["operation"] = json!(operation);
            }
            let error = fixture
                .execute(arguments)
                .await
                .expect_err("operation is required");
            assert_input_error(&error, AI_CAPABILITY_OPERATION_INVALID);
            assert_eq!(
                crate::classify_safe_application_tool_error(&error),
                Some(crate::AiApplicationToolFailureCode::InvalidArguments)
            );
        }

        // describe_type is fenced by the same loaded reference and operation.
        let request = fixture
            .broker
            .dispatch_describe_type(
                &fixture.principal,
                &fixture.run,
                &fixture.session,
                &json!({"loadedReference": loaded_reference, "operation": "GeneratedRecord", "type": "select"}),
            )
            .await
            .expect("describe type");
        assert_eq!(
            request.operation().capability_id(),
            &fixture.root_id("GeneratedRecord")
        );
        assert_eq!(request.type_name(), "select");
        assert_eq!(
            (request.offset(), request.limit()),
            (0, crate::AI_CAPABILITY_TYPE_PAGE_DEFAULT)
        );
        assert_eq!(fixture.session.amplification().describe_type_calls, 1);

        // Host policy decides each root. A root denied at describe is absent
        // from the description and unreachable; one revoked afterwards is
        // refused at execute.
        fixture
            .authority
            .denied
            .write()
            .expect("policy")
            .insert(fixture.root_id("GeneratedRecord").as_str().to_owned());
        let narrowed = fixture.describe().await.expect("narrowed describe");
        assert_eq!(
            narrowed
                .operations()
                .iter()
                .map(AiCapabilityDescribedOperation::operation_name)
                .collect::<Vec<_>>(),
            ["RecordBySubject"]
        );
        assert_eq!(
            narrowed.contract()["operations"].as_array().map(Vec::len),
            Some(1)
        );
        let only = fixture
            .execute(
                json!({"loadedReference": narrowed.loaded_reference(), "selections": ["recordId"]}),
            )
            .await
            .expect("the single admitted operation needs no name");
        assert_eq!(only.capability_id(), &fixture.root_id("RecordBySubject"));
        let denied = fixture
            .execute(json!({
                "loadedReference": narrowed.loaded_reference(),
                "operation": "GeneratedRecord",
                "selections": ["recordId"]
            }))
            .await
            .expect_err("a root denied at describe is not loaded");
        assert_input_error(&denied, AI_CAPABILITY_OPERATION_INVALID);
        assert!(matches!(
            fixture
                .execute(json!({
                    "loadedReference": loaded_reference,
                    "operation": "GeneratedRecord",
                    "selections": ["recordId"]
                }))
                .await,
            Err(AiError::Forbidden)
        ));
        // With every root denied the entity cannot be loaded at all.
        fixture
            .authority
            .denied
            .write()
            .expect("policy")
            .insert(fixture.root_id("RecordBySubject").as_str().to_owned());
        assert!(matches!(fixture.describe().await, Err(AiError::Forbidden)));
    }

    #[tokio::test]
    async fn broker_v1_selections_and_bindings_are_refused_and_v2_works() {
        let fixture = EntityBrokerFixture::new();
        let index = fixture.index.0.read().expect("index").clone();
        // A version 1 discovery returned the root itself, with the root
        // entry's fingerprint. Version 2 never returns it, so a retained
        // transcript that replays it holds no selection.
        let root = index
            .entry(&fixture.root_id("GeneratedRecord"))
            .expect("root entry")
            .clone();
        let v1_candidate = json!({
            "capabilityId": root.id.as_str(),
            "candidateFingerprint": root.fingerprint,
        });
        fixture.discover().await;
        let error = fixture
            .broker
            .dispatch_describe(
                &fixture.principal,
                &fixture.run,
                &fixture.session,
                &v1_candidate,
            )
            .await
            .expect_err("a version 1 candidate is not held");
        assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
        assert_eq!(
            crate::classify_safe_application_tool_error(&error),
            Some(crate::AiApplicationToolFailureCode::SelectionStale)
        );
        for call in [
            json!({"loadedReference": "e".repeat(64), "selections": ["recordId"]}),
            json!({"loadedReference": "e".repeat(64), "type": "select"}),
        ] {
            let error = if call.get("type").is_some() {
                fixture
                    .broker
                    .dispatch_describe_type(
                        &fixture.principal,
                        &fixture.run,
                        &fixture.session,
                        &call,
                    )
                    .await
                    .expect_err("an unheld reference is refused")
            } else {
                fixture
                    .execute(call)
                    .await
                    .expect_err("an unheld reference is refused")
            };
            assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
        }

        // A retained fixed-broker binding minted under version 1 differs from
        // every version 2 binding, so the provider session rebinds once.
        let fields = (
            BTreeSet::<String>::new(),
            "provider-projection-v1",
            "model-test",
            ModelReasoningEffort::Unspecified,
            "f".repeat(64),
        );
        let v1_binding = hash_json(&json!({
            "version": 2,
            "delivery_mode": AiCapabilityDeliveryMode::FixedBroker,
            "broker_version": 1,
            "static_bootstrap_tool_fingerprints": fields.0,
            "provider_projection_version": fields.1,
            "model": fields.2,
            "reasoning_effort": fields.3,
            "registration_identity": fields.4,
        }));
        let v2_binding = AiProviderCapabilitySessionBinding::new(
            AiCapabilityDeliveryMode::FixedBroker,
            "a".repeat(64),
            fields.0.clone(),
            fields.1,
            fields.2,
            fields.3,
            fields.4.clone(),
        )
        .expect("binding");
        assert_ne!(v2_binding.fingerprint(), v1_binding);

        // The version 1 surface was three definitions; a turn offering it is
        // not the crate-owned version 2 surface and is refused.
        let definitions = capability_broker_definitions();
        assert_eq!(
            definitions
                .iter()
                .map(|definition| definition.tool_id.as_str())
                .collect::<Vec<_>>(),
            [
                AI_CAPABILITY_DISCOVER_TOOL_ID,
                AI_CAPABILITY_DESCRIBE_TOOL_ID,
                AI_CAPABILITY_DESCRIBE_TYPE_TOOL_ID,
                AI_CAPABILITY_EXECUTE_TOOL_ID,
            ]
        );
        for definition in &definitions {
            let v1_fingerprint = hash_json(&json!({
                "version": 2,
                "tool_id": definition.tool_id,
                "broker_version": 1,
                "parameters": definition.parameters,
            }));
            assert_ne!(definition.fingerprint, v1_fingerprint);
        }
        let capabilities = ProviderCapabilities {
            custom_tools: true,
            capability_delivery_modes: BTreeSet::from([AiCapabilityDeliveryMode::FixedBroker]),
            ..ProviderCapabilities::default()
        };
        let turn = AiCapabilityDeliveryTurn::select(
            &capabilities,
            &"a".repeat(64),
            Vec::new(),
            true,
            v2_binding,
            Arc::new(fixture.broker.clone()),
            DiscoveryCacheFixture::session(),
        )
        .expect("version 2 turn");
        let v1_surface = definitions
            .iter()
            .filter(|definition| definition.tool_id != AI_CAPABILITY_DESCRIBE_TYPE_TOOL_ID)
            .cloned()
            .collect::<Vec<_>>();
        assert!(!turn.matches_offered_tools(&v1_surface));
        let mut sorted = definitions.clone();
        sorted.sort_by(|left, right| left.tool_id.cmp(&right.tool_id));
        assert!(turn.matches_offered_tools(&sorted));

        // Version 2 discover, describe, describe_type and execute work.
        let described = fixture.describe().await.expect("describe");
        let page = fixture
            .broker
            .dispatch_describe_type(
                &fixture.principal,
                &fixture.run,
                &fixture.session,
                &json!({"loadedReference": described.loaded_reference(), "operation": "GeneratedRecord", "type": "select", "offset": 0, "limit": 8}),
            )
            .await
            .expect("describe type");
        assert_eq!(page.limit(), 8);
        fixture
            .execute(json!({
                "loadedReference": described.loaded_reference(),
                "operation": "GeneratedRecord",
                "arguments": [{"name": "recordId", "value": "record-1"}],
                "selections": ["recordId"]
            }))
            .await
            .expect("execute");
        // After a schema change, the held selection is capability_stale.
        *fixture.index.0.write().expect("index") = generated_index_with_schema(
            "generated-read-application",
            "generated-read",
            "target-policy-v1",
            Some("note"),
            true,
        );
        for error in [
            fixture
                .broker
                .dispatch_describe_type(
                    &fixture.principal,
                    &fixture.run,
                    &fixture.session,
                    &json!({"loadedReference": described.loaded_reference(), "operation": "GeneratedRecord", "type": "select"}),
                )
                .await
                .expect_err("stale"),
            fixture
                .execute(json!({
                    "loadedReference": described.loaded_reference(),
                    "operation": "GeneratedRecord",
                    "selections": ["recordId"]
                }))
                .await
                .expect_err("stale"),
        ] {
            assert_input_error(&error, AI_CAPABILITY_STALE);
            assert_eq!(
                crate::classify_safe_application_tool_error(&error),
                Some(crate::AiApplicationToolFailureCode::CapabilityStale)
            );
        }
    }

    #[test]
    fn operation_signatures_fit_the_bound_or_are_withheld_without_truncation() {
        let description = || AiCapabilityDescription {
            loaded_reference: "a".repeat(64),
            capability_id: AiToolId::parse("records.entity.record").expect("id"),
            capability_kind: AiCapabilityKind::GeneratedEntity,
            capability_fingerprint: "b".repeat(64),
            operations: vec![AiCapabilityDescribedOperation {
                operation_name: "Records".to_owned(),
                capability_id: AiToolId::parse("records.query.records").expect("id"),
                capability_kind: AiCapabilityKind::GeneratedQuery,
                capability_fingerprint: "c".repeat(64),
            }],
            contract: json!({"version": 2, "operations": [{"operation": "Records"}]}),
        };
        let summary = json!({
            "operation": "Records",
            "signature": "Records(where?: RecordWhereInput) -> select",
            "planSchema": {"padding": "x".repeat(4_096)},
        });
        let with_schema = description()
            .with_operation_signatures(vec![summary.clone()], 64 * 1_024)
            .expect("fits");
        assert!(
            with_schema.contract()["operations"][0]
                .get("planSchema")
                .is_some()
        );
        let without_schema = description()
            .with_operation_signatures(vec![summary.clone()], 1_024)
            .expect("signature fits without the inline schema");
        assert_eq!(
            without_schema.contract()["operations"][0]["signature"],
            "Records(where?: RecordWhereInput) -> select"
        );
        assert!(
            without_schema.contract()["operations"][0]
                .get("planSchema")
                .is_none()
        );
        let mut long = summary;
        long["signature"] = json!("y".repeat(4_096));
        let withheld = description()
            .with_operation_signatures(vec![long], 1_024)
            .expect("contract fits without signatures");
        assert_eq!(withheld.contract()["signaturesAvailable"], false);
        assert!(
            withheld.contract()["operations"][0]
                .get("signature")
                .is_none()
        );
        assert!(
            description()
                .with_operation_signatures(Vec::new(), 1_024)
                .is_err(),
            "one summary per operation"
        );
    }
}
