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
/// Frozen broker tool identifier for browsing the current capabilities by
/// hierarchy: namespace, capability, operation (broker version 3).
pub const AI_CAPABILITY_BROWSE_TOOL_ID: &str = "graphql.capabilities.browse";

/// Current broker protocol version.
///
/// Version 2 adds the `describe_type` operation, returns compact typed
/// signatures from `describe` instead of the plan schema, discovers one
/// entity capability per entity, and takes the entity's `operation` at
/// `describe_type` and `execute`. Version 3 adds the `browse` operation and
/// continuation handles on `execute`. The version is hashed into the fixed
/// broker definitions and every fixed-broker session binding, so a session
/// bound under an earlier version rebinds once.
pub const AI_CAPABILITY_BROKER_VERSION: u16 = 3;

/// Most continuation handles one broker session retains. The oldest handle
/// is dropped first; a dropped handle is a stale selection.
pub const AI_CAPABILITY_MAXIMUM_CONTINUATIONS: usize = 32;

/// Most executions one continuation chain admits, including its first page.
///
/// The budget is shared by every handle of the chain: reusing a handle or
/// redeeming handles concurrently draws on it too, so this bounds the total
/// work of a chain, not only its depth. Once it is spent, no new handle is
/// issued and every retained handle of the chain is a stale selection.
pub const AI_CAPABILITY_MAXIMUM_CONTINUATION_PAGES: u16 = 256;

/// Response version of client-deferred discovery, whose contract broker
/// version 3 leaves unchanged.
pub const AI_CAPABILITY_CLIENT_DEFERRED_DISCOVERY_VERSION: u16 = 2;

/// Largest page of one `browse` call, and its default.
pub const AI_CAPABILITY_BROWSE_PAGE_MAXIMUM: u16 = 32;

/// Most policy decisions discover or browse sends to
/// [`AiCapabilityAuthorityPolicy::authorize_discovery`] in one call.
/// A host's [`AiCapabilityDiscoveryBroker::with_maximum_discovery_authorizations`]
/// lowers it. An entity with more grouped roots is decided alone, split
/// across calls, and admitted only once every part is decided.
pub const AI_CAPABILITY_BROWSE_POLICY_BATCH: usize = 256;

/// Largest `browse` offset: the most entries an index set can hold, so every
/// `nextOffset` a browse returns is an accepted offset.
pub const AI_CAPABILITY_BROWSE_MAXIMUM_OFFSET: u32 = 1_048_576;

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
    offering: Option<AiCapabilityOfferingSnapshot>,
}

#[derive(Debug)]
struct AiCapabilityOfferingState {
    tools: Vec<ModelToolDefinition>,
    static_fingerprints: BTreeSet<String>,
    generation: u64,
    frozen_plans: usize,
}

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
        "Search the current reviewed capabilities. A generated_entity candidate covers every read operation of one entity; its operations list names them. A generated_mutation candidate is a change this run may request; describe reports whether it needs approval. A generated_subscription candidate waits for one matching event. Describe a candidate with its capabilityId and candidateReference.",
        None,
        json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "maxLength": 1024},
                "namespace": {"type": ["string", "null"], "maxLength": 256},
                "kind": {
                    "type": ["string", "null"],
                    "enum": ["generated_query", "generated_entity", "reviewed_static", "generated_mutation", "generated_subscription", null],
                    "description": "generated_query: every generated read; generated_entity: entity capabilities only; reviewed_static: reviewed tools only; generated_mutation: generated mutations only; generated_subscription: generated event waits only."
                },
                "entityOrClass": {"type": ["string", "null"], "maxLength": 256},
                "maximumResults": {"type": "integer", "minimum": 1, "maximum": 32}
            },
            "required": ["text", "maximumResults"],
            "additionalProperties": false
        }),
    )
}

/// Fingerprint of the frozen fixed-broker execute definition of the current
/// broker version: the only provider-facing identity a broker-routed
/// tool-call row may carry.
pub(crate) fn fixed_broker_execute_fingerprint() -> &'static str {
    static FINGERPRINT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FINGERPRINT.get_or_init(|| {
        fixed_broker_definitions()
            .into_iter()
            .find(|definition| definition.tool_id == AI_CAPABILITY_EXECUTE_TOOL_ID)
            .map(|definition| definition.fingerprint)
            .unwrap_or_default()
    })
}

fn fixed_broker_definitions() -> Vec<ModelToolDefinition> {
    let index_fingerprint = None;
    let discover = fixed_discovery_definition();
    let browse = broker_definition(
        AI_CAPABILITY_BROWSE_TOOL_ID,
        "graphql_capabilities_browse",
        "Browse the current reviewed capabilities by hierarchy when a search is ambiguous. level namespaces lists the logical namespaces; capabilities lists the capabilities of one namespace, optionally of one entity; operations lists the operations of one entity capability. Every listed capability can be described with its capabilityId and candidateReference. Page with offset and nextOffset.",
        index_fingerprint,
        json!({
            "type": "object",
            "properties": {
                "level": {"type": "string", "enum": ["namespaces", "capabilities", "operations"]},
                "namespace": {
                    "type": ["string", "null"], "maxLength": 256,
                    "description": "Namespace from level namespaces. Required for capabilities and operations."
                },
                "entity": {
                    "type": ["string", "null"], "maxLength": 256,
                    "description": "Entity name. Optional for capabilities; required for operations."
                },
                "offset": {"type": ["integer", "null"], "minimum": 0, "maximum": AI_CAPABILITY_BROWSE_MAXIMUM_OFFSET},
                "limit": {"type": ["integer", "null"], "minimum": 1, "maximum": AI_CAPABILITY_BROWSE_PAGE_MAXIMUM}
            },
            "required": ["level"],
            "additionalProperties": false
        }),
    );
    let describe = broker_definition(
        AI_CAPABILITY_DESCRIBE_TOOL_ID,
        "graphql_capabilities_describe",
        "Load one exact current capability. Returns a loadedReference and one compact typed signature per operation; expand any named type with graphql_capabilities_describe_type.",
        index_fingerprint,
        json!({
            "type": "object",
            "properties": {
                "capabilityId": {"type": "string", "maxLength": 200},
                "candidateReference": {
                    "type": "string", "minLength": 64, "maxLength": 64,
                    "description": "Exact candidateReference of a discover or browse candidate returned in this run."
                }
            },
            "required": ["capabilityId", "candidateReference"],
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
        "Execute one operation of a previously loaded exact capability. Use only argument and selection paths its signature and expanded types admit: flatten argument leaves into name/value entries with dotted paths, and omit optional fields you do not need. When a result carries a continuation handle, fetch the next page with loadedReference, operation and continuation only. A mutation whose description requires approval is not run by this call: it waits for that approval. A subscription waits for one event: send observation within its described bounds, with an optional condition on a selected field.",
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
                    "description": "Optional root arguments of the signature: one name/value entry per supplied scalar leaf, named by its dotted public path, for example where.status.eq.",
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
                    "description": "One or more exact dotted scalar paths from the select type and its select.<path> expansions. Required except with continuation, and empty for a capability with a fixed result.",
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
                },
                "continuation": {
                    "type": ["string", "null"], "minLength": 64, "maxLength": 64,
                    "description": "Continuation handle from the previous page of this loadedReference and operation. With it, send no arguments, selections, relationship options or maximumItems: the server repeats the same query for the next page."
                },
                "observation": {
                    "type": "object",
                    "properties": {
                        "timeoutSeconds": {"type": "integer", "minimum": 1},
                        "maximumEvents": {"type": "integer", "minimum": 1},
                        "condition": {
                            "type": "object",
                            "properties": {
                                "field": {"type": "string"},
                                "operator": {"type": "string"},
                                "value": {"type": ["string", "integer", "number", "boolean", "null"]}
                            },
                            "required": ["field", "operator", "value"],
                            "additionalProperties": false
                        }
                    },
                    "required": ["timeoutSeconds", "maximumEvents"],
                    "additionalProperties": false
                }
            },
            "required": ["loadedReference"],
            "additionalProperties": false
        }),
    );
    vec![discover, browse, describe, describe_type, execute]
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
    /// returns. Each [`AiCapabilityKind::GeneratedEntity`] match is followed
    /// by the [`AiCapabilityKind::GeneratedQuery`] entries of the roots it
    /// groups; the broker returns the entity only with the roots this batch
    /// admits, lists only those roots, and ranks it on its own metadata and
    /// those roots alone. In per-root (client-deferred) discovery, the entity
    /// entry of a grouped root precedes that root's first match, and the root
    /// is returned only when both are admitted. A restricted principal therefore receives its permitted
    /// candidates even when unpermitted entries rank above them. When every
    /// policy call succeeds, the amount of policy work depends on the query
    /// and catalogue, never on which candidates the principal may use.
    ///
    /// One search can therefore evaluate up to the index set's
    /// `maximum_entries` matches (16,384 by default), plus the grouped roots
    /// of each matching entity. Bound it with
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

/// A host-owned, monotonically increasing version of everything that can
/// revoke a brokered capability: host capability policy and the broker's
/// current index.
///
/// Publish each revoking policy or index change together with a strictly
/// larger version, for example by updating both under the same lock. If
/// publication spans separate writes, return `None` throughout the update,
/// then publish a larger version once the new state is visible. Readers of
/// policy and index must observe updates in the same order as this version.
/// Never reuse a version.
///
/// Reads are synchronous. Admission still checks current policy and index
/// for each wait. See
/// [`crate::OrmAiSubscriptionWaitService::admit_subscription_results`].
pub trait AiCapabilityRevocationVersion: Send + Sync {
    /// The stable current version, or `None` while a change is in progress.
    fn current_version(&self) -> Option<u64>;
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
    /// The model-facing `loadedReference`, fixed at load and kept when a
    /// hidden-only catalogue change refreshes the binding.
    reference: String,
    /// [`visible_fingerprint`] of the capability as this principal was shown
    /// it at load.
    visible_fingerprint: String,
    principal_reference_fingerprint: String,
    capability_id: AiToolId,
    capability_kind: AiCapabilityKind,
    capability_fingerprint: String,
    /// The entry's reviewed approval rule at load. A session decides from it
    /// whether its bound rules reach a loaded generated mutation before any
    /// other check, so a refused mutation is an unknown reference to it.
    approval: AiApprovalRule,
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
    /// For a root an entity capability groups, the host-policy fingerprint
    /// of the entity entry, which is decided at load and again at execute.
    entity_policy_fingerprint: Option<String>,
}

/// One operation of a loaded entity capability, as admitted at load.
#[derive(Clone, Debug)]
struct AiLoadedCapabilityOperation {
    operation_name: String,
    capability_id: AiToolId,
    capability_fingerprint: String,
}

struct LoadedOperationSnapshot {
    indexes: Arc<AiCapabilityIndexSet>,
    loaded: AiLoadedCapabilityBinding,
}

/// The exact executable root a broker call resolved from a loaded binding.
struct LoadedTarget {
    capability_id: AiToolId,
    capability_kind: AiCapabilityKind,
    capability_fingerprint: String,
    /// Model-facing contract of the resolved root, which a continued page
    /// must share with its first page.
    contract_fingerprint: String,
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
        self.reference.clone()
    }
}

#[derive(Clone)]
struct CachedCapabilityDiscovery {
    /// The issued search, with the fence of the run its references were
    /// issued to. A cached search answers only that principal, session,
    /// run, attempt and provider session.
    issued: IssuedSearch,
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
    revocation_version: Option<Arc<dyn AiCapabilityRevocationVersion>>,
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
            revocation_version: None,
        })
    }

    /// Supplies the host's revocation version of this broker's policy and
    /// index ([`AiCapabilityRevocationVersion`]).
    ///
    /// Required for brokered subscriptions: the durable-wait service refuses
    /// to register a broker-routed wait through a broker without it, and its
    /// final admission fails closed. Other broker operations do not use it.
    #[must_use]
    pub fn with_revocation_version(
        mut self,
        version: Arc<dyn AiCapabilityRevocationVersion>,
    ) -> Self {
        self.revocation_version = Some(version);
        self
    }

    pub(crate) fn has_revocation_version(&self) -> bool {
        self.revocation_version.is_some()
    }

    /// The host's current revocation version, when one is configured.
    pub(crate) fn revocation_version(&self) -> Option<u64> {
        self.revocation_version
            .as_ref()
            .and_then(|version| version.current_version())
    }

    /// The fingerprint of the current index set for `run`, read
    /// synchronously.
    pub(crate) fn current_index_set_fingerprint(
        &self,
        run: &AiCapabilityRunBinding,
    ) -> Result<String, AiError> {
        Ok(self
            .current_indexes
            .current_index_set(run)?
            .fingerprint()
            .to_owned())
    }

    /// Enables bounded process-local retention of discovery results beyond a
    /// run's own [`AiCapabilityDeliveryLimits::maximum_retained_searches`].
    ///
    /// A discover candidate is described by an opaque `candidateReference`
    /// bound to the principal reference, AI session, run, attempt and provider
    /// session that received it, so a cache entry answers only that exact
    /// run, including through a fresh or cloned [`AiCapabilityBrokerSession`].
    /// A later run, or another attempt of the same run, discovers again: a
    /// reference never crosses runs. Only admitted discovery candidates are
    /// retained, server-side; describe still checks the current index,
    /// rehydrates the principal and authorizes the candidate, then issues a
    /// new short-lived binding. Execution bindings, permissions, browse
    /// results and tool results are never cached here. Cloned brokers share
    /// this cache; replacing the broker starts it empty. Entries expire after
    /// an absolute TTL and the oldest searches are evicted at capacity. The
    /// cache is disabled unless this builder is called.
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

    /// Caps the authorized view of one discovery query and the size of each
    /// host policy batch. Unset by default: no view is refused and policy
    /// batches hold at most [`AI_CAPABILITY_BROWSE_POLICY_BATCH`] decisions.
    ///
    /// Every ranked match is decided, in rank order, in policy batches of at
    /// most `maximum` decisions (and never more than
    /// [`AI_CAPABILITY_BROWSE_POLICY_BATCH`]); an entity whose grouped roots
    /// exceed a batch is decided alone, split across policy calls of at most
    /// that size. A query whose authorized view (the
    /// admitted matches with their admitted grouped roots and, in per-root
    /// discovery, each admitted entity) exceeds `maximum` is refused with
    /// [`AiError::InvalidInput`] carrying
    /// [`AI_CAPABILITY_DISCOVERY_TOO_BROAD`]. Denied matches never count
    /// towards it, so the refusal depends only on what the principal may
    /// see. The guarantee covers the response, not timing: policy work grows
    /// with the hidden matches decided. The ceiling is applied before
    /// truncation, so a restricted principal still receives every permitted
    /// candidate of an admitted query.
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

    fn cache_discovery(&self, issued: &IssuedSearch) {
        let Some(cache) = &self.discovery_cache else {
            return;
        };
        let now = self.clock.now();
        let mut entries = cache.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|entry| entry.expires_at > now);
        entries.push_back(CachedCapabilityDiscovery {
            issued: issued.clone(),
            expires_at: now + cache.ttl,
        });
        while entries.len() > cache.maximum_searches {
            entries.pop_front();
        }
    }

    /// The cached snapshot that issued `reference` for `capability_id` under
    /// exactly `fence`, with the reference's visible fingerprint.
    fn cached_discovery(
        &self,
        fence: &str,
        capability_id: &AiToolId,
        reference: &str,
    ) -> Option<(Arc<AiCapabilityIndexSetSearchResult>, String)> {
        let cache = self.discovery_cache.as_ref()?;
        let now = self.clock.now();
        let mut entries = cache.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|entry| entry.expires_at > now);
        entries
            .iter()
            .rev()
            .find_map(|entry| entry.issued.resolve(capability_id, reference, fence))
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
        self.search_admitted(principal_reference, run, query, false, |_, _| true)
            .await
            .map(|(result, _)| result)
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
        admit: impl Fn(&AiCapabilityIndex, &AiCapabilityIndexEntry) -> bool,
    ) -> Result<(AiCapabilityIndexSetSearchResult, Vec<String>), AiError> {
        let principal = self.rehydrate(principal_reference).await?;
        let indexes = self.current_indexes.current_index_set(run)?;
        let mut ranked = if per_root {
            indexes.ranked_operation_matches(query)?
        } else {
            indexes.ranked_matches(query)?
        };
        ranked.retain(|candidate| admit(candidate.index(), candidate.entry()));
        let kept = Box::pin(self.admit_ranked(&principal, run, &ranked, per_root)).await?;
        // Admission preserves the initial order. Whole-entry removals
        // affect neither other entries' fixed scores nor their ID ties.
        // Only a retained entity with a denied root can change its match
        // or rank. Per-root ranks already depend on the root alone.
        let needs_rerank = !per_root
            && kept.iter().any(|(candidate, admitted)| {
                candidate
                    .entry()
                    .operations
                    .iter()
                    .any(|member| !admitted.contains(&member.capability_id))
            });
        let order = if needs_rerank {
            indexes.rank_admitted(query, &kept)?
        } else {
            (0..kept.len()).collect::<Vec<_>>()
        };
        let (candidates, visible) = order
            .into_iter()
            .take(usize::from(query.maximum_results))
            .map(|position| {
                let (candidate, admitted) = &kept[position];
                (
                    candidate.admitted_candidate(admitted),
                    visible_fingerprint(candidate.entry(), admitted),
                )
            })
            .unzip();
        Ok((
            AiCapabilityIndexSetSearchResult {
                index_set_fingerprint: indexes.fingerprint().to_owned(),
                candidates,
            },
            visible,
        ))
    }

    /// Applies host policy to every ranked match, in rank order, in batches
    /// of at most [`AI_CAPABILITY_BROWSE_POLICY_BATCH`] decisions, or the
    /// host's [`Self::with_maximum_discovery_authorizations`] when it is
    /// lower. An entity whose grouped roots exceed a batch is decided alone,
    /// split across policy calls, before it is admitted.
    ///
    /// The configured authorization ceiling applies to the authorized view
    /// alone: the admitted matches with their admitted grouped roots (and,
    /// per root, their admitted entities). Denied matches never count
    /// towards it, so whether a query is refused as
    /// [`AI_CAPABILITY_DISCOVERY_TOO_BROAD`] depends only on what the
    /// principal may see. Hidden matches change how much policy work is
    /// done, never the response.
    async fn admit_ranked<'a>(
        &self,
        principal: &ResolvedPrincipal,
        run: &AiCapabilityRunBinding,
        ranked: &[AiCapabilityRankedMatch<'a>],
        per_root: bool,
    ) -> Result<Vec<(AiCapabilityRankedMatch<'a>, BTreeSet<AiToolId>)>, AiError> {
        let batch_size = self.policy_batch_size();
        let mut kept = Vec::new();
        let mut position = 0;
        while position < ranked.len() {
            let mut chunk = Vec::new();
            let mut decisions = 0_usize;
            let mut entities = BTreeSet::new();
            while let Some(candidate) = ranked.get(position) {
                let size = 1 + if per_root {
                    candidate.entity().map_or(0, |entity| {
                        usize::from(!entities.contains(&entity.entry().id))
                    })
                } else {
                    candidate.entry().operations.len()
                };
                if !chunk.is_empty() && decisions + size > batch_size {
                    break;
                }
                if per_root && let Some(entity) = candidate.entity() {
                    entities.insert(entity.entry().id.clone());
                }
                chunk.push(*candidate);
                decisions += size;
                position += 1;
            }
            kept.extend(Box::pin(self.admit_matches(principal, run, &chunk, per_root)).await?);
            if self
                .maximum_discovery_authorizations
                .is_some_and(|maximum| authorized_view_size(&kept, per_root) > maximum)
            {
                return Err(AiError::InvalidInput(
                    AI_CAPABILITY_DISCOVERY_TOO_BROAD.to_owned(),
                ));
            }
        }
        Ok(kept)
    }

    /// Most decisions one host policy call receives: at most
    /// [`AI_CAPABILITY_BROWSE_POLICY_BATCH`], or the host's
    /// [`Self::with_maximum_discovery_authorizations`] when it is lower.
    fn policy_batch_size(&self) -> usize {
        self.maximum_discovery_authorizations
            .unwrap_or(AI_CAPABILITY_BROWSE_POLICY_BATCH)
            .clamp(1, AI_CAPABILITY_BROWSE_POLICY_BATCH)
    }

    /// Applies host policy to every match, with the entries each depends
    /// on, in calls of at most [`Self::policy_batch_size`] decisions, and
    /// keeps the admitted matches in order, each with the grouped roots
    /// policy admitted.
    ///
    /// The batch size depends only on the matches, never on a decision.
    async fn admit_matches<'a>(
        &self,
        principal: &ResolvedPrincipal,
        run: &AiCapabilityRunBinding,
        ranked: &[AiCapabilityRankedMatch<'a>],
        per_root: bool,
    ) -> Result<Vec<(AiCapabilityRankedMatch<'a>, BTreeSet<AiToolId>)>, AiError> {
        // One policy batch decides every match together with the entries it
        // depends on: an entity entry's grouped roots follow it, and in
        // per-root ranking a root's entity entry precedes its first root.
        let mut batch = Vec::new();
        let mut slots = Vec::with_capacity(ranked.len());
        let mut entity_slots = BTreeMap::<AiToolId, usize>::new();
        for candidate in ranked {
            let entity_at = if per_root {
                candidate.entity().map(|entity| {
                    *entity_slots
                        .entry(entity.entry().id.clone())
                        .or_insert_with(|| {
                            batch.push(entity);
                            batch.len() - 1
                        })
                })
            } else {
                None
            };
            batch.push(*candidate);
            let at = batch.len() - 1;
            let root_start = batch.len();
            if !per_root {
                batch.extend(candidate.iter_grouped_roots());
            }
            slots.push((at, root_start..batch.len(), entity_at));
        }
        // No policy call decides more than one batch: an entity whose
        // grouped roots exceed it is split across calls, and the decisions
        // are combined before the entity is admitted.
        let mut decisions = Vec::with_capacity(batch.len());
        for part in batch.chunks(self.policy_batch_size()) {
            let decided = self
                .authority
                .authorize_discovery(principal, part, run)
                .await?;
            if decided.len() != part.len()
                || part
                    .iter()
                    .zip(&decided)
                    .any(|(candidate, decision)| decision.capability_id != candidate.entry().id)
            {
                return Err(AiError::Forbidden);
            }
            decisions.extend(decided);
        }
        // An entity is kept only with its admitted roots, and a root only
        // with its admitted entity. Nothing of a denied root is returned.
        let mut kept = Vec::with_capacity(slots.len());
        for (at, roots, entity_at) in slots {
            if !decisions[at].allowed || entity_at.is_some_and(|entity| !decisions[entity].allowed)
            {
                continue;
            }
            let admitted = roots
                .filter(|root| decisions[*root].allowed)
                .map(|root| batch[root].entry().id.clone())
                .collect::<BTreeSet<_>>();
            if batch[at].entry().kind == AiCapabilityKind::GeneratedEntity && admitted.is_empty() {
                continue;
            }
            kept.push((batch[at], admitted));
        }
        Ok(kept)
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
        self.load_entry(&principal, run, &indexes, index, entry)
            .await
    }

    /// Loads the current `entry` of `indexes` for `principal` after fresh
    /// host policy for the entry, its entity and each grouped root. The
    /// caller has already proven that the run holds the selection.
    async fn load_entry(
        &self,
        principal: &ResolvedPrincipal,
        run: &AiCapabilityRunBinding,
        indexes: &AiCapabilityIndexSet,
        index: &AiCapabilityIndex,
        entry: &AiCapabilityIndexEntry,
    ) -> Result<AiLoadedCapabilityBinding, AiError> {
        let decision = self
            .authority
            .authorize(principal, index, entry, run)
            .await?;
        if !decision.allowed || !valid_binding_value(&decision.policy_fingerprint) {
            return Err(AiError::Forbidden);
        }
        // A root an entity groups is loadable only while host policy also
        // admits its entity, as at discovery.
        let entity_policy_fingerprint = match index.entity_entry_of(&entry.id) {
            Some(entity) => {
                let entity_decision = self
                    .authority
                    .authorize(principal, index, entity, run)
                    .await?;
                if !entity_decision.allowed
                    || !valid_binding_value(&entity_decision.policy_fingerprint)
                {
                    return Err(AiError::Forbidden);
                }
                Some(entity_decision.policy_fingerprint)
            }
            None => None,
        };
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
                    .authorize(principal, index, root, run)
                    .await?;
                if member_decision.allowed
                    && valid_binding_value(&member_decision.policy_fingerprint)
                {
                    operations.push(AiLoadedCapabilityOperation {
                        operation_name: member.operation_name.clone(),
                        capability_id: root.id.clone(),
                        capability_fingerprint: root.capability_fingerprint.clone(),
                    });
                }
            }
            if operations.is_empty() {
                return Err(AiError::Forbidden);
            }
        }
        let nonce = Uuid::new_v4();
        let admitted = operations
            .iter()
            .map(|operation| operation.capability_id.clone())
            .collect::<BTreeSet<_>>();
        Ok(AiLoadedCapabilityBinding {
            reference: hash_json(&json!({
                "version": 2,
                "nonce": nonce,
                "capability": entry.id,
                "run": run.run_id,
                "attempt": run.attempt_id,
            })),
            visible_fingerprint: visible_fingerprint(entry, &admitted),
            operations,
            entity_policy_fingerprint,
            nonce,
            principal_reference_fingerprint: principal_reference_fingerprint(principal.reference()),
            capability_id: entry.id.clone(),
            capability_kind: entry.kind,
            capability_fingerprint: entry.capability_fingerprint.clone(),
            approval: entry.approval,
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
        self.verify_loaded_fence(principal_reference, run, loaded)?;
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
            // A grouped root also needs its entity admitted, with the policy
            // fingerprint recorded at load.
            match (
                index.entity_entry_of(&entry.id),
                &loaded.entity_policy_fingerprint,
            ) {
                (None, None) => {}
                (Some(entity), Some(recorded)) => {
                    let entity_decision = self
                        .authority
                        .authorize(&principal, index, entity, run)
                        .await?;
                    if !entity_decision.allowed {
                        return Err(AiError::Forbidden);
                    }
                    if &entity_decision.policy_fingerprint != recorded {
                        return Err(stale_capability());
                    }
                }
                _ => return Err(stale_capability()),
            }
        }
        Ok((principal, indexes))
    }

    /// Proves that `loaded` belongs to this principal reference and run and
    /// has not expired.
    fn verify_loaded_fence(
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
        Ok(())
    }

    /// Read the index for every well-formed reference, including unknown
    /// references, before looking up session state. No hidden reference can
    /// cause an extra call to a host index or principal source.
    fn loaded_operation_snapshot(
        &self,
        run: &AiCapabilityRunBinding,
        session: &AiCapabilityBrokerSession,
        reference: &str,
    ) -> Result<LoadedOperationSnapshot, AiError> {
        let indexes = self
            .current_indexes
            .current_index_set(run)
            .map_err(|_| stale_selection())?;
        let loaded = session
            .loaded(reference)
            .filter(|loaded| {
                self.clock.now() <= loaded.expires_at && session.reaches_loaded_kind(loaded)
            })
            .ok_or_else(stale_selection)?;
        Ok(LoadedOperationSnapshot { indexes, loaded })
    }

    /// Resolves and authorizes the root one broker call addresses.
    ///
    /// An entity capability needs `operation` unless exactly one of its
    /// operations was loaded; any other capability has one operation, its
    /// own, and `operation` may only name it.
    ///
    /// Every call, whether or not anything has changed, loads the capability
    /// again from the current index under current policy: the entry, its
    /// entity and each grouped root are authorized, so the chosen root is
    /// never reached on the entity's decision alone. A denied entry, entity
    /// or chosen root is [`AiError::Forbidden`]. Then what this principal is
    /// now shown ([`visible_fingerprint`], including every admitted sibling
    /// root) must equal what it was shown at load, or the call is
    /// [`AI_CAPABILITY_STALE`]. The outcome therefore depends only on what the
    /// principal can see, never on whether state hidden from it drifted. The
    /// session's binding is replaced by the current one, under the same
    /// `loadedReference` and expiry, and the call proceeds against the exact
    /// current capability.
    async fn authorize_loaded_operation(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        session: &AiCapabilityBrokerSession,
        snapshot: &LoadedOperationSnapshot,
        operation: Option<&str>,
    ) -> Result<LoadedTarget, AiError> {
        let LoadedOperationSnapshot { indexes, loaded } = snapshot;
        let (index, entry) = indexes
            .entry(&loaded.capability_id)
            .ok_or_else(stale_selection)?;
        if !session.brokers_entry(index, entry) {
            return Err(stale_selection());
        }
        self.verify_loaded_fence(principal_reference, run, loaded)?;
        if entry.kind != loaded.capability_kind {
            return Err(AiError::Forbidden);
        }
        let principal = self.rehydrate(principal_reference).await?;
        let mut current = self
            .load_entry(&principal, run, indexes, index, entry)
            .await?;
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
        let target = match selected {
            Some(selected) => {
                // The chosen root itself must still be admitted.
                let admitted = current
                    .operations
                    .iter()
                    .find(|candidate| candidate.operation_name == selected.operation_name)
                    .ok_or(AiError::Forbidden)?;
                let root = index
                    .entry(&admitted.capability_id)
                    .ok_or_else(stale_capability)?;
                if root.id != selected.capability_id
                    || root.kind != AiCapabilityKind::GeneratedQuery
                {
                    return Err(stale_capability());
                }
                LoadedTarget {
                    capability_id: root.id.clone(),
                    capability_kind: root.kind,
                    capability_fingerprint: root.capability_fingerprint.clone(),
                    contract_fingerprint: root.contract_fingerprint.clone(),
                    operation_name: root.operation_name.clone(),
                }
            }
            None => {
                if operation.is_some_and(|name| name != entry.operation_name) {
                    return Err(invalid_operation());
                }
                LoadedTarget {
                    capability_id: entry.id.clone(),
                    capability_kind: entry.kind,
                    capability_fingerprint: entry.capability_fingerprint.clone(),
                    contract_fingerprint: entry.contract_fingerprint.clone(),
                    operation_name: entry.operation_name.clone(),
                }
            }
        };
        if current.visible_fingerprint != loaded.visible_fingerprint {
            return Err(stale_capability());
        }
        current.nonce = loaded.nonce;
        current.reference.clone_from(&loaded.reference);
        current.expires_at = loaded.expires_at;
        session.replace_loaded(current);
        Ok(target)
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

    /// Re-runs this broker's own admission of one generated subscription a
    /// durable wait was registered through, for the run that registered it.
    ///
    /// The wait service calls this at every claim, delivery and resumption
    /// of a broker-routed wait, after the run's broker session is gone. It
    /// rehydrates the principal, requires the current index for `run` to
    /// still hold the exact subscription entry (same identity, kind and
    /// capability fingerprint, from an index compiled in process from a
    /// typed subscription catalogue), and requires current host capability
    /// policy to still admit that entry for `run`. It authorizes nothing by
    /// itself.
    ///
    /// # Errors
    ///
    /// Returns [`AI_CAPABILITY_STALE`] when the entry was removed, replaced
    /// or lost its in-process proof, [`AiError::Forbidden`] when host policy
    /// no longer admits it, [`AiError::ReauthorizationFailed`] when the
    /// principal cannot be rehydrated, and propagates an index failure.
    pub(crate) async fn readmit_brokered_subscription(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        capability_id: &AiToolId,
        capability_fingerprint: &str,
    ) -> Result<String, AiError> {
        let principal = self.rehydrate(principal_reference).await?;
        let indexes = self.current_indexes.current_index_set(run)?;
        let (index, entry) =
            current_subscription_entry(&indexes, capability_id, capability_fingerprint)?;
        let decision = self
            .authority
            .authorize(&principal, index, entry, run)
            .await?;
        if !decision.allowed || !valid_binding_value(&decision.policy_fingerprint) {
            return Err(AiError::Forbidden);
        }
        // A subscription an entity groups would be admitted only with its
        // entity, as at discovery.
        if let Some(entity) = index.entity_entry_of(&entry.id) {
            let decision = self
                .authority
                .authorize(&principal, index, entity, run)
                .await?;
            if !decision.allowed || !valid_binding_value(&decision.policy_fingerprint) {
                return Err(AiError::Forbidden);
            }
        }
        // Host policy awaited: the entry it admitted must still be current.
        // The index is read again after the last await, so an entry removed
        // or replaced while policy was decided is stale.
        let indexes = self.current_indexes.current_index_set(run)?;
        current_subscription_entry(&indexes, capability_id, capability_fingerprint)?;
        // The index set this admission was finally decided against, so a
        // caller can prove later that it is still current.
        Ok(indexes.fingerprint().to_owned())
    }
}

/// The current index entry of one brokered generated subscription: present,
/// still a subscription with exactly `capability_fingerprint`, and from an
/// index compiled in process from a typed subscription catalogue.
fn current_subscription_entry<'a>(
    indexes: &'a AiCapabilityIndexSet,
    capability_id: &AiToolId,
    capability_fingerprint: &str,
) -> Result<(&'a AiCapabilityIndex, &'a AiCapabilityIndexEntry), AiError> {
    let (index, entry) = indexes.entry(capability_id).ok_or_else(stale_capability)?;
    if entry.kind != AiCapabilityKind::GeneratedSubscription
        || entry.capability_fingerprint != capability_fingerprint
        || !index.subscriptions_checked_against_finished_schema()
    {
        return Err(stale_capability());
    }
    Ok((index, entry))
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
/// The five operations are the complete crate-owned broker surface. Resolving
/// an operation from a tool identifier is discovery, not authorization: every
/// dispatch still rehydrates the principal, reapplies current host policy and,
/// for [`Self::Execute`] only, passes through ordinary resolver authorization.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiCapabilityBrokerOperation {
    /// Bounded authority-neutral search over the current compact index.
    Discover,
    /// Bounded hierarchical listing of the current compact index.
    Browse,
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
            AI_CAPABILITY_BROWSE_TOOL_ID => Some(Self::Browse),
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
            Self::Browse => AI_CAPABILITY_BROWSE_TOOL_ID,
            Self::Describe => AI_CAPABILITY_DESCRIBE_TOOL_ID,
            Self::DescribeType => AI_CAPABILITY_DESCRIBE_TYPE_TOOL_ID,
            Self::Execute => AI_CAPABILITY_EXECUTE_TOOL_ID,
        }
    }

    /// Whether this operation can reach an application resolver.
    ///
    /// Only [`Self::Execute`] can. Discovery, browse, describe and
    /// describe-type return bounded authority-neutral metadata and grant no
    /// execution authority.
    pub const fn reaches_resolver(self) -> bool {
        matches!(self, Self::Execute)
    }
}

/// Returns the five frozen fixed-broker definitions: discover, browse,
/// describe, describe-type and execute.
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
    /// Accepted browse calls.
    #[serde(default)]
    pub browse_calls: u32,
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
            .saturating_add(self.browse_calls)
            .saturating_add(self.describe_calls)
            .saturating_add(self.describe_type_calls)
            .saturating_add(self.execute_calls)
    }
}

#[derive(Debug, Default)]
struct BrokerSessionState {
    /// Retained discover and browse results, each with the references it
    /// issued.
    searches: VecDeque<IssuedSearch>,
    loaded: VecDeque<(String, AiLoadedCapabilityBinding)>,
    continuations: VecDeque<(String, BrokerContinuation)>,
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
    brokered_mutations: bool,
    /// Hierarchical rules a classified coordinator bound for brokered
    /// mutations. Without them no mutation is discoverable or executable.
    ///
    /// A value, not shared state: binding returns a new session for the
    /// classified turn alone, so a clone taken before binding, or one a
    /// read-only plan holds, never inherits another plan's rules.
    mutation_rules: Option<Arc<crate::AiResolvedRuleSet>>,
    brokered_subscriptions: bool,
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
            brokered_mutations: false,
            mutation_rules: None,
            brokered_subscriptions: false,
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

    /// Lets this run's fixed broker also discover, describe and resolve
    /// generated mutations.
    ///
    /// Opting in grants nothing by itself. A mutation is reachable only when
    /// a classified supervised coordinator that can route consequential
    /// calls binds its hierarchical rules to this session, and only while
    /// those rules admit the mutation's exact capability fingerprint at its
    /// reviewed classification: an approval-required mutation as a one-shot
    /// supervised write, an automatic one as an approval-free autonomous
    /// write. A prohibited mutation is never offered. The read-only
    /// coordinator binds no rules, so through it no mutation is ever
    /// discovered, described or executed.
    ///
    /// Execute only resolves a mutation: it returns the exact capability
    /// and closed plan, and the coordinator classifies, rule-checks,
    /// approves and runs that resolved mutation exactly as it would the
    /// direct definition, with every ordinary resolver authorization check.
    #[must_use]
    pub fn with_brokered_mutations(mut self) -> Self {
        self.brokered_mutations = true;
        self
    }

    /// Whether this session lets its fixed broker reach generated mutations.
    pub const fn brokers_mutations(&self) -> bool {
        self.brokered_mutations
    }

    /// Returns this session bound to the classified coordinator's current
    /// rules for brokered mutations. The returned value alone carries them:
    /// this session, and every clone taken from it, stays unbound. Binding a
    /// session already bound to rules with a different fingerprint is a
    /// conflict. A session that does not broker mutations is returned
    /// unchanged.
    pub(crate) fn bind_mutation_rules(
        &self,
        rules: &crate::AiResolvedRuleSet,
    ) -> Result<Self, AiError> {
        if !self.brokered_mutations {
            return Ok(self.clone());
        }
        if self
            .mutation_rules
            .as_ref()
            .is_some_and(|current| current.fingerprint() != rules.fingerprint())
        {
            return Err(AiError::Conflict);
        }
        let mut bound = self.clone();
        bound.mutation_rules = Some(Arc::new(rules.clone()));
        Ok(bound)
    }

    /// Fingerprint of the rules bound for brokered mutations, if any.
    pub(crate) fn mutation_rule_fingerprint(&self) -> Option<&str> {
        self.mutation_rules
            .as_ref()
            .map(|rules| rules.fingerprint())
    }

    /// Whether the bound rules admit this generated mutation entry at its
    /// reviewed classification.
    fn brokers_mutation_entry(
        &self,
        index: &AiCapabilityIndex,
        entry: &AiCapabilityIndexEntry,
    ) -> bool {
        entry.kind == AiCapabilityKind::GeneratedMutation
            && index.mutations_checked_against_finished_schema()
            && self.brokers_mutation(&entry.capability_fingerprint, entry.approval)
    }

    /// Whether the bound rules admit a generated mutation with this exact
    /// capability fingerprint at the reviewed classification `approval`.
    fn brokers_mutation(&self, capability_fingerprint: &str, approval: AiApprovalRule) -> bool {
        if !self.brokered_mutations {
            return false;
        }
        let Some(binding) = brokered_mutation_binding(approval) else {
            return false;
        };
        self.mutation_rules.as_ref().is_some_and(|rules| {
            rules.constrain_tool(capability_fingerprint, binding.0, binding.1) == Some(binding.1)
        })
    }

    /// Lets this run's fixed broker also discover, describe and resolve
    /// generated replayable subscriptions as durable waits.
    ///
    /// Opt in only when the host routes a broker execute that resolves to a
    /// subscription to
    /// [`crate::OrmAiSubscriptionWaitService::register_wait`], with this
    /// delivery attached to the registration context
    /// ([`crate::AiSubscriptionWaitRegistrationContext::with_capability_delivery`]).
    /// Use [`Self::holds_loaded_subscription`] to recognize such a call.
    /// Both coordinators refuse it on their read path with the fixed
    /// correction [`AI_CAPABILITY_SUBSCRIPTION_UNROUTED`], so nothing waits
    /// or runs there.
    ///
    /// Opting in grants nothing by itself. Only subscriptions of an index
    /// compiled in process from a typed subscription catalogue are admitted
    /// ([`AiCapabilityIndex::subscriptions_checked_against_finished_schema`]);
    /// that compiler refuses any root that takes a `@transientInput`
    /// argument. Host capability policy still decides each entry at
    /// discover, browse, describe and execute. Execute only resolves a
    /// subscription to its exact capability and closed plan. The wait
    /// service classifies the **resolved** subscription by its own
    /// fingerprint, applies current rules, principal, session, target and
    /// source authorization, and rechecks them at every claim and adoption.
    #[must_use]
    pub fn with_brokered_subscriptions(mut self) -> Self {
        self.brokered_subscriptions = true;
        self
    }

    /// Whether this session lets its fixed broker reach generated
    /// subscriptions.
    pub const fn brokers_subscriptions(&self) -> bool {
        self.brokered_subscriptions
    }

    /// Whether this generated subscription entry is admitted: the session
    /// opted in and the index was compiled in process from a typed
    /// subscription catalogue.
    fn brokers_subscription_entry(
        &self,
        index: &AiCapabilityIndex,
        entry: &AiCapabilityIndexEntry,
    ) -> bool {
        entry.kind == AiCapabilityKind::GeneratedSubscription
            && self.brokered_subscriptions
            && index.subscriptions_checked_against_finished_schema()
    }

    /// Whether `arguments` name a loaded reference this session holds for a
    /// generated subscription.
    ///
    /// This is a routing hint only: a host uses it to send a broker execute
    /// to [`crate::OrmAiSubscriptionWaitService::register_wait`] instead of
    /// the read path. It authorizes nothing, and registration repeats every
    /// check.
    pub fn holds_loaded_subscription(&self, arguments: &serde_json::Value) -> bool {
        self.brokered_subscriptions
            && arguments
                .get("loadedReference")
                .and_then(serde_json::Value::as_str)
                .and_then(|reference| self.loaded(reference))
                .is_some_and(|loaded| {
                    loaded.capability_kind == AiCapabilityKind::GeneratedSubscription
                })
    }

    /// Whether this run's broker reaches a held candidate, decided from the
    /// candidate alone, before any current-index lookup. Describe applies
    /// it first, so a candidate this run may not reach is the same stale
    /// selection whether or not the catalogue has since changed.
    fn brokers_candidate(&self, candidate: &crate::AiCapabilitySearchCandidate) -> bool {
        match candidate.kind {
            AiCapabilityKind::GeneratedEntity | AiCapabilityKind::GeneratedQuery => true,
            AiCapabilityKind::ReviewedStatic => self
                .brokered_static_tools
                .contains(&candidate.capability_fingerprint),
            AiCapabilityKind::GeneratedMutation => {
                self.brokers_mutation(&candidate.capability_fingerprint, candidate.approval)
            }
            AiCapabilityKind::GeneratedSubscription => self.brokered_subscriptions,
        }
    }

    /// Whether `arguments` name a loaded reference this session holds for a
    /// generated mutation. This is a routing hint only: it authorizes
    /// nothing, and the execute it routes repeats every check.
    pub(crate) fn holds_loaded_mutation(&self, arguments: &serde_json::Value) -> bool {
        arguments
            .get("loadedReference")
            .and_then(serde_json::Value::as_str)
            .and_then(|reference| self.loaded(reference))
            .is_some_and(|loaded| loaded.capability_kind == AiCapabilityKind::GeneratedMutation)
    }

    /// Whether this session's own configuration reaches a loaded binding: a
    /// generated read, a brokered static tool of this exact fingerprint, a
    /// mutation its bound rules admit at the reviewed approval it was loaded
    /// with, or a subscription when the session brokers subscriptions.
    /// Current index, policy and rules are checked again before execution.
    fn reaches_loaded_kind(&self, loaded: &AiLoadedCapabilityBinding) -> bool {
        match loaded.capability_kind {
            AiCapabilityKind::GeneratedEntity | AiCapabilityKind::GeneratedQuery => true,
            AiCapabilityKind::ReviewedStatic => self
                .brokered_static_tools
                .contains(&loaded.capability_fingerprint),
            AiCapabilityKind::GeneratedMutation => {
                self.brokers_mutation(&loaded.capability_fingerprint, loaded.approval)
            }
            AiCapabilityKind::GeneratedSubscription => self.brokered_subscriptions,
        }
    }

    /// Whether this run's broker reaches the entry at all: a generated read,
    /// a brokered static tool, an admitted generated mutation or an admitted
    /// generated subscription.
    fn brokers_entry(&self, index: &AiCapabilityIndex, entry: &AiCapabilityIndexEntry) -> bool {
        matches!(
            entry.kind,
            AiCapabilityKind::GeneratedEntity | AiCapabilityKind::GeneratedQuery
        ) || self.brokers_static_entry(entry)
            || self.brokers_mutation_entry(index, entry)
            || self.brokers_subscription_entry(index, entry)
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

    fn record_search(&self, issued: IssuedSearch) {
        let mut state = self.state();
        state.amplification.discover_calls = state.amplification.discover_calls.saturating_add(1);
        self.retain_search(&mut state, issued);
    }

    /// Retains one issued search with its references. Evicting a search
    /// evicts its references with it, so a reference resolves only while
    /// the exact snapshot it was issued with is retained.
    fn retain_search(&self, state: &mut BrokerSessionState, issued: IssuedSearch) {
        state.searches.push_back(issued);
        while state.searches.len() > usize::from(self.limits.maximum_retained_searches) {
            state.searches.pop_front();
        }
    }

    /// The exact snapshot, and the visible fingerprint, behind one opaque
    /// candidate reference that this session issued for `capability_id`
    /// under exactly `fence`.
    fn issued_reference(
        &self,
        capability_id: &AiToolId,
        reference: &str,
        fence: &str,
    ) -> Option<(Arc<AiCapabilityIndexSetSearchResult>, String)> {
        self.state()
            .searches
            .iter()
            .rev()
            .find_map(|issued| issued.resolve(capability_id, reference, fence))
    }

    /// Replaces the binding held under `binding`'s reference, after a
    /// hidden-only refresh.
    fn replace_loaded(&self, binding: AiLoadedCapabilityBinding) {
        let mut state = self.state();
        if let Some((_, held)) = state
            .loaded
            .iter_mut()
            .find(|(reference, _)| reference == &binding.reference)
        {
            *held = binding;
        }
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

    fn record_deferred_search(
        &self,
        result: AiCapabilityIndexSetSearchResult,
        loaded: Vec<(String, AiLoadedCapabilityBinding)>,
    ) {
        let mut state = self.state();
        state.amplification.discover_calls = state.amplification.discover_calls.saturating_add(1);
        // Client-deferred discovery issues no references; its candidates
        // are loaded at once.
        self.retain_search(
            &mut state,
            IssuedSearch {
                fence: String::new(),
                search: Arc::new(result),
                references: Vec::new(),
            },
        );
        state.loaded = loaded.into();
        state.deferred_installation_pending = true;
    }

    fn deferred_installation_pending(&self) -> bool {
        self.state().deferred_installation_pending
    }

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

    fn record_browse(&self, issued: Option<IssuedSearch>) {
        let mut state = self.state();
        state.amplification.browse_calls = state.amplification.browse_calls.saturating_add(1);
        if let Some(issued) = issued {
            self.retain_search(&mut state, issued);
        }
    }

    fn record_continuation(&self, handle: String, continuation: BrokerContinuation) {
        let mut state = self.state();
        state.continuations.push_back((handle, continuation));
        while state.continuations.len() > AI_CAPABILITY_MAXIMUM_CONTINUATIONS {
            state.continuations.pop_front();
        }
    }

    fn continuation(&self, handle: &str) -> Option<BrokerContinuation> {
        self.state()
            .continuations
            .iter()
            .find(|(existing, _)| existing == handle)
            .map(|(_, continuation)| continuation.clone())
    }

    /// Continuation handles still retained for this run, at most
    /// [`AI_CAPABILITY_MAXIMUM_CONTINUATIONS`].
    pub fn continuation_count(&self) -> usize {
        self.state().continuations.len()
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
    #[serde(default)]
    candidate_reference: Option<String>,
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
    #[serde(default)]
    selections: Vec<String>,
    #[serde(default)]
    relationship_arguments: Vec<BrokerRelationshipArguments>,
    #[serde(default)]
    relationship_maximum_items: Vec<BrokerRelationshipMaximumItems>,
    #[serde(default)]
    maximum_items: Option<u32>,
    #[serde(default)]
    continuation: Option<String>,
    #[serde(default)]
    observation: Option<BrokerObservation>,
}

/// Bounded observation of one brokered subscription wait. The subscription
/// compiler checks every value against the exact capability.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrokerObservation {
    timeout_seconds: u32,
    maximum_events: u32,
    #[serde(default)]
    condition: Option<BrokerObservationCondition>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrokerObservationCondition {
    field: String,
    operator: String,
    value: serde_json::Value,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum BrowseLevel {
    Namespaces,
    Capabilities,
    Operations,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrokerBrowseArguments {
    level: BrowseLevel,
    #[serde(default)]
    namespace: Option<String>,
    #[serde(default)]
    entity: Option<String>,
    #[serde(default)]
    offset: Option<u32>,
    #[serde(default)]
    limit: Option<u16>,
}

/// The page of a browse that lists nothing: identical for an unknown
/// filter, a filter policy denies and a browse over the work bound.
fn empty_browse_page(level: BrowseLevel) -> serde_json::Value {
    match level {
        BrowseLevel::Namespaces => json!({
            "version": AI_CAPABILITY_BROKER_VERSION,
            "level": "namespaces",
            "nextOffset": null,
            "namespaces": [],
        }),
        BrowseLevel::Capabilities => json!({
            "version": AI_CAPABILITY_BROKER_VERSION,
            "level": "capabilities",
            "nextOffset": null,
            "capabilities": [],
        }),
        BrowseLevel::Operations => json!({
            "version": AI_CAPABILITY_BROKER_VERSION,
            "level": "operations",
            "nextOffset": null,
            "capability": null,
            "operations": [],
        }),
    }
}

/// Model-facing fixed-broker candidate of discover or browse: the
/// admitted candidate's metadata with an opaque `candidateReference`, never
/// an index or entry fingerprint, and the entity's operation list only where
/// `with_operations` asks for it.
fn fixed_candidate_value(
    candidate: &crate::AiCapabilitySearchCandidate,
    reference: &CandidateReference,
    with_operations: bool,
) -> serde_json::Value {
    let mut value = candidate_value(candidate);
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "candidateReference".to_owned(),
            json!(reference.reference.as_str()),
        );
        if !with_operations {
            object.remove("operations");
        }
    }
    value
}

/// One opaque candidate reference a discover or browse call issued: the
/// model-facing reference, the capability it names and the
/// [`visible_fingerprint`] of that candidate as it was shown.
#[derive(Clone, Debug)]
struct CandidateReference {
    reference: String,
    capability_id: AiToolId,
    visible_fingerprint: String,
}

impl CandidateReference {
    /// A fresh random reference to `candidate`, shown as
    /// `visible_fingerprint` describes. The reference hashes no index, entry
    /// or policy fingerprint, so nothing about entries the principal may not
    /// see enters it.
    fn issue(candidate: &crate::AiCapabilitySearchCandidate, visible_fingerprint: String) -> Self {
        Self {
            reference: hash_json(&json!({
                "version": 1,
                "nonce": Uuid::new_v4(),
                "capability": candidate.id,
            })),
            capability_id: candidate.id.clone(),
            visible_fingerprint,
        }
    }
}

/// One discover or browse result a run was given: the exact search snapshot
/// the model was shown, the fence of the run it was issued to, and the
/// references it issued. A reference resolves only against this snapshot,
/// and only while it is retained.
#[derive(Clone, Debug)]
struct IssuedSearch {
    fence: String,
    search: Arc<AiCapabilityIndexSetSearchResult>,
    references: Vec<CandidateReference>,
}

impl IssuedSearch {
    fn resolve(
        &self,
        capability_id: &AiToolId,
        reference: &str,
        fence: &str,
    ) -> Option<(Arc<AiCapabilityIndexSetSearchResult>, String)> {
        if self.fence != fence {
            return None;
        }
        self.references
            .iter()
            .find(|issued| issued.reference == reference && &issued.capability_id == capability_id)
            .map(|issued| (Arc::clone(&self.search), issued.visible_fingerprint.clone()))
    }
}

/// Fence of one candidate reference: the principal reference and the
/// session, run, attempt and provider session it was issued to. A reference
/// presented under any other binding, through a reused or cloned session or
/// the discovery cache, resolves nothing.
fn candidate_reference_fence(
    principal_reference: &PrincipalReference,
    run: &AiCapabilityRunBinding,
) -> String {
    hash_json(&json!({
        "version": 1,
        "principal": principal_reference_fingerprint(principal_reference),
        "session": run.session_id,
        "run": run.run_id,
        "attempt": run.attempt_id,
        "provider": run.provider_kind,
        "providerSession": run.provider_session_fingerprint,
    }))
}

fn valid_browse_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

/// Server-held state of one continued page: the exact query of its first
/// page and the resolver cursor of the next one. Never model-visible, and
/// its `Debug` output redacts the cursor.
#[derive(Clone)]
struct BrokerContinuation {
    loaded_reference: String,
    capability_id: AiToolId,
    /// Model-facing contract of the first page's root.
    contract_fingerprint: String,
    operation_name: String,
    plan: serde_json::Value,
    cursor: String,
    page: u16,
    expires_at: OffsetDateTime,
    chain: ContinuationChain,
}

impl std::fmt::Debug for BrokerContinuation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BrokerContinuation")
            .field("loaded_reference", &self.loaded_reference)
            .field("capability_id", &self.capability_id)
            .field("operation_name", &self.operation_name)
            .field("cursor", &REDACTED_CURSOR)
            .field("page", &self.page)
            .field("expires_at", &self.expires_at)
            .field("chain", &self.chain)
            .finish_non_exhaustive()
    }
}

/// Placeholder `Debug` output for a resolver keyset cursor.
const REDACTED_CURSOR: &str = "<redacted cursor>";

/// Executions one continuation chain has used, shared by every handle of the
/// chain so reused or concurrent handles draw on one budget of
/// [`AI_CAPABILITY_MAXIMUM_CONTINUATION_PAGES`] executions.
#[derive(Clone, Debug)]
struct ContinuationChain(Arc<std::sync::atomic::AtomicU16>);

impl ContinuationChain {
    fn first_page() -> Self {
        Self(Arc::new(std::sync::atomic::AtomicU16::new(1)))
    }

    fn exhausted(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
            >= AI_CAPABILITY_MAXIMUM_CONTINUATION_PAGES
    }

    /// Reserves one execution, or refuses once the chain's budget is spent.
    fn reserve(&self) -> bool {
        self.0
            .fetch_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |used| (used < AI_CAPABILITY_MAXIMUM_CONTINUATION_PAGES).then_some(used + 1),
            )
            .is_ok()
    }

    /// Returns one reserved execution that never reached the executor.
    fn release(&self) {
        let _ = self.0.fetch_update(
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
            |used| used.checked_sub(1),
        );
    }
}

/// Server-held state of one continued page an execution carries: the
/// cursor, its page number, the handle and its expiry, and whether the
/// execution's reserved budget was returned on refusal.
#[derive(Clone)]
struct ContinuedPage {
    cursor: String,
    page: u16,
    handle: String,
    expires_at: OffsetDateTime,
    released: Arc<std::sync::atomic::AtomicBool>,
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

/// Safe [`AiError::InvalidInput`] message for a broker execute that resolved
/// to a generated mutation on a path that cannot route consequential calls.
///
/// The model-facing code is `invalid_arguments` with a fixed correction:
/// nothing was executed and this run cannot request the change. Only a
/// classified supervised coordinator routes brokered mutations.
pub const AI_CAPABILITY_CONSEQUENTIAL_UNROUTED: &str =
    "broker capability requires a classified coordinator";

/// Safe [`AiError::InvalidInput`] message for a broker execute that resolved
/// to a generated subscription on a path that does not register durable
/// waits.
///
/// The model-facing code is `invalid_arguments` with a fixed correction:
/// nothing waited or ran. A brokered subscription becomes a wait only when
/// the host routes the call to
/// [`crate::OrmAiSubscriptionWaitService::register_wait`]
/// ([`AiCapabilityBrokerSession::holds_loaded_subscription`]); both
/// coordinators' read paths refuse it.
pub const AI_CAPABILITY_SUBSCRIPTION_UNROUTED: &str =
    "broker capability requires a durable subscription wait";

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
/// The reviewed classification a generated mutation is admitted at: an
/// approval-required mutation is a one-shot supervised write and an
/// automatic one an approval-free autonomous write. A prohibited mutation,
/// or any other approval rule, has none.
pub(crate) fn brokered_mutation_binding(
    approval: AiApprovalRule,
) -> Option<(crate::ToolMaturity, AiApprovalRule)> {
    match approval {
        AiApprovalRule::OneShot => Some((
            crate::ToolMaturity::SupervisedWrite,
            AiApprovalRule::OneShot,
        )),
        AiApprovalRule::None => Some((crate::ToolMaturity::AutonomousWrite, AiApprovalRule::None)),
        _ => None,
    }
}

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

/// Size of an authorized discovery view: each admitted match with its
/// admitted grouped roots and, per root, each distinct admitted entity.
fn authorized_view_size(
    kept: &[(AiCapabilityRankedMatch<'_>, BTreeSet<AiToolId>)],
    per_root: bool,
) -> usize {
    let entities = if per_root {
        kept.iter()
            .filter_map(|(candidate, _)| candidate.entity().map(|entity| entity.entry().id.clone()))
            .collect::<BTreeSet<_>>()
            .len()
    } else {
        0
    };
    kept.iter()
        .map(|(_, admitted)| 1 + admitted.len())
        .sum::<usize>()
        + entities
}

/// Fingerprint of what one principal is shown of `entry`: its model-facing
/// metadata and contract fingerprint and, for an entity, exactly the
/// `admitted` roots with theirs.
///
/// It covers nothing host policy hides and no catalogue-wide fingerprint, so
/// an edit to a root the principal is denied, to another capability or to
/// another target leaves it unchanged. It decides only whether a held
/// selection is reported as [`AI_CAPABILITY_STALE`]; execution always binds
/// the exact current capability.
fn visible_fingerprint(entry: &AiCapabilityIndexEntry, admitted: &BTreeSet<AiToolId>) -> String {
    let entity = entry.kind == AiCapabilityKind::GeneratedEntity;
    let operations = entry
        .operations
        .iter()
        .filter(|member| admitted.contains(&member.capability_id))
        .map(|member| {
            json!({
                "id": member.capability_id.as_str(),
                "operation": member.operation_name,
                "shape": member.operation_shape,
                "description": member.description,
                "resultDescription": member.result_description,
                "resultRecordCost": member.result_record_cost,
                "risk": member.risk,
                "approval": member.approval,
                "contract": member.contract_fingerprint,
            })
        })
        .collect::<Vec<_>>();
    hash_json(&json!({
        "version": 1,
        "id": entry.id.as_str(),
        "kind": entry.kind,
        "name": entry.name,
        "description": entry.description,
        "namespace": entry.namespace,
        "entity": entry.entity_name,
        "operation": entry.operation_name,
        "shape": entry.operation_shape,
        "scalarFields": entry.scalar_fields,
        "relationships": entry.relationships,
        "aggregates": entry.aggregate_features,
        "resultClassification": entry.result_classification,
        // An entity's own risk, result summaries and contract combine all
        // of its roots, hidden ones included; its admitted roots stand in.
        "resultDescription": (!entity).then_some(&entry.result_description),
        "resultRecordCost": (!entity).then_some(entry.result_record_cost),
        "risk": (!entity).then_some(entry.risk),
        "approval": (!entity).then_some(entry.approval),
        "contract": (!entity).then_some(&entry.contract_fingerprint),
        "operations": operations,
    }))
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
///
/// Its `Debug` output redacts the server-held cursor of a continued page.
#[derive(Clone)]
pub struct AiCapabilityExecution {
    capability_id: AiToolId,
    capability_kind: AiCapabilityKind,
    capability_fingerprint: String,
    /// Model-facing contract of the resolved root.
    contract_fingerprint: String,
    audit_reference: String,
    plan: serde_json::Value,
    operation_name: String,
    loaded_expires_at: OffsetDateTime,
    /// Budget of the continuation chain this execution belongs to, created
    /// with the first page; `None` for a reviewed static tool.
    chain: Option<ContinuationChain>,
    /// Server-held state of a continued page.
    continued: Option<ContinuedPage>,
}

impl std::fmt::Debug for AiCapabilityExecution {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiCapabilityExecution")
            .field("capability_id", &self.capability_id)
            .field("capability_kind", &self.capability_kind)
            .field("capability_fingerprint", &self.capability_fingerprint)
            .field("audit_reference", &self.audit_reference)
            .field("plan", &self.plan)
            .field("operation_name", &self.operation_name)
            .field("loaded_expires_at", &self.loaded_expires_at)
            .field(
                "continued",
                &self
                    .continued
                    .as_ref()
                    .map(|continued| (REDACTED_CURSOR, continued.page)),
            )
            .finish()
    }
}

/// One model-facing continuation handle of a continued result.
///
/// The handle is an opaque random reference to server-held state in one
/// [`AiCapabilityBrokerSession`]: the exact query of the first page and the
/// resolver cursor of the next one. It carries no cursor, plan or identity
/// and grants nothing by itself. Each continued page rehydrates the
/// principal, re-proves the loaded binding against the current index and
/// host policy, and runs through ordinary resolver authorization and row
/// policy, exactly as a first page does.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiCapabilityContinuation {
    handle: String,
    expires_in_seconds: i64,
}

impl AiCapabilityContinuation {
    /// Opaque handle the model passes back as `continuation`.
    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// Seconds until the handle and its loaded binding expire.
    pub const fn expires_in_seconds(&self) -> i64 {
        self.expires_in_seconds
    }

    /// Model-facing value of this continuation.
    pub fn to_model_value(&self) -> serde_json::Value {
        json!({
            "handle": self.handle,
            "expiresInSeconds": self.expires_in_seconds,
        })
    }
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

    /// Public name of the executed root.
    pub fn operation_name(&self) -> &str {
        &self.operation_name
    }

    /// Server-held keyset cursor of the page this execution continues, or
    /// `None` for a first page.
    ///
    /// The cursor is resolver state and must never reach the model. Pass it
    /// to [`crate::AiRuntime::execute_query_capability_page`].
    pub fn continuation_cursor(&self) -> Option<&str> {
        self.continued
            .as_ref()
            .map(|continued| continued.cursor.as_str())
    }

    /// Whether [`AiCapabilityDiscoveryBroker::continuation_admission`]
    /// refused this continued page before it reached the executor.
    pub fn continuation_refused(&self) -> bool {
        self.continued.as_ref().is_some_and(|continued| {
            continued
                .released
                .load(std::sync::atomic::Ordering::Acquire)
        })
    }

    /// One-based page number of this execution in its continuation chain.
    pub fn page_number(&self) -> u16 {
        self.continued
            .as_ref()
            .map_or(1, |continued| continued.page)
    }

    /// Whether this execution resolves a consequential capability: a
    /// generated mutation.
    ///
    /// A consequential resolution is never executed by the broker. The
    /// classified coordinator classifies and rule-checks the resolved
    /// capability, then requests approval or runs it exactly as it would the
    /// direct definition. Every other coordinator refuses it.
    pub const fn is_consequential(&self) -> bool {
        matches!(self.capability_kind, AiCapabilityKind::GeneratedMutation)
    }

    /// Whether this execution resolves a generated subscription: a durable
    /// wait.
    ///
    /// A subscription resolution is never executed by the broker. Only
    /// [`crate::OrmAiSubscriptionWaitService::register_wait`] turns it into
    /// a wait, after classifying the resolved subscription by its own
    /// fingerprint. Both coordinators' read paths refuse it.
    pub const fn is_subscription_wait(&self) -> bool {
        matches!(
            self.capability_kind,
            AiCapabilityKind::GeneratedSubscription
        )
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

/// Model-facing effect of executing one described generated mutation:
/// `automatic`, or `approval_required` when nothing runs until a person
/// approves the exact request. A generated subscription's effect is `wait`:
/// execute parks the run until one bounded outcome. Reads carry no effect.
fn capability_effect(entry: &AiCapabilityIndexEntry) -> Option<&'static str> {
    match (entry.kind, entry.approval) {
        (AiCapabilityKind::GeneratedMutation, AiApprovalRule::None) => Some("automatic"),
        (AiCapabilityKind::GeneratedMutation, _) => Some("approval_required"),
        (AiCapabilityKind::GeneratedSubscription, _) => Some("wait"),
        _ => None,
    }
}

/// Model-facing usage note of every broker version 3 description.
const DESCRIBE_USAGE: &str = "Each operation has a typed signature. Expand a named type from inputTypes, select or select.<path> with graphql_capabilities_describe_type (loadedReference, operation, type). Execute with loadedReference, operation, arguments as dotted leaf paths (for example where.status.eq) and selections as dotted scalar paths. For an ungrouped COUNT on an aggregate operation, omit the grouping argument and use no selections. An operation marked continuation returns a continuation handle while more rows exist; execute again with loadedReference, operation and continuation only to read the next page.";

fn planning_contract(
    entry: &AiCapabilityIndexEntry,
    loaded: &AiLoadedCapabilityBinding,
    loaded_reference: &str,
    expires_in_seconds: i64,
) -> serde_json::Value {
    // Entity-level shapes and bounds count only the roots this principal
    // may use, never a denied one.
    let admitted = loaded
        .operations
        .iter()
        .map(|operation| operation.capability_id.clone())
        .collect::<BTreeSet<_>>();
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
    let mut contract = json!({
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
        "resultDescription": entry.admitted_result_description(&admitted),
        "resultRecordCost": entry.admitted_result_record_cost(&admitted),
        "risk": entry.risk,
        "approval": entry.approval,
        "aggregates": entry.aggregate_features,
        "usage": DESCRIBE_USAGE,
        "operations": operations,
    });
    if let (Some(effect), Some(object)) = (capability_effect(entry), contract.as_object_mut()) {
        object.insert("effect".to_owned(), json!(effect));
    }
    contract
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
    /// it grants no load or execution authority. It covers generated reads,
    /// the reviewed static tools this run brokers and, only when the session
    /// brokers mutations and the classified coordinator's bound rules admit
    /// them ([`AiCapabilityBrokerSession::with_brokered_mutations`]),
    /// generated mutations, and, only when the session brokers subscriptions
    /// ([`AiCapabilityBrokerSession::with_brokered_subscriptions`]),
    /// generated subscriptions of an index compiled in process. Every
    /// returned candidate has already passed current host policy for the
    /// current principal.
    ///
    /// The result carries no index or entry fingerprint: those hash entries
    /// host policy may hide from this principal. Each candidate instead has a
    /// fresh opaque `candidateReference` that describe accepts only from the
    /// run it was issued to.
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
        let admit: fn(
            &AiCapabilityBrokerSession,
            &AiCapabilityIndex,
            &AiCapabilityIndexEntry,
        ) -> bool = match parsed.kind {
            Some(AiCapabilityKind::GeneratedQuery) => |_, _, entry| {
                matches!(
                    entry.kind,
                    AiCapabilityKind::GeneratedEntity | AiCapabilityKind::GeneratedQuery
                )
            },
            Some(AiCapabilityKind::GeneratedEntity) => {
                |_, _, entry| entry.kind == AiCapabilityKind::GeneratedEntity
            }
            Some(AiCapabilityKind::ReviewedStatic) if brokers_static => {
                |session, _, entry| session.brokers_static_entry(entry)
            }
            Some(AiCapabilityKind::GeneratedMutation) if session.brokers_mutations() => {
                AiCapabilityBrokerSession::brokers_mutation_entry
            }
            Some(AiCapabilityKind::GeneratedSubscription) if session.brokers_subscriptions() => {
                AiCapabilityBrokerSession::brokers_subscription_entry
            }
            Some(_) => return Err(malformed_broker_arguments()),
            None => AiCapabilityBrokerSession::brokers_entry,
        };
        let query = AiCapabilitySearchQuery {
            text: parsed.text,
            namespace: parsed.namespace,
            kind: None,
            entity_or_operation: parsed.entity_or_class,
            maximum_results: parsed.maximum_results,
        };
        let (result, visible) = self
            .search_admitted(principal_reference, run, &query, false, |index, entry| {
                admit(session, index, entry)
            })
            .await?;
        // Each candidate carries a fresh opaque reference bound to this
        // principal and run, and to this exact snapshot. The index-set and
        // entry fingerprints hash entries host policy hides from this
        // principal, so they stay server-side.
        let fence = candidate_reference_fence(principal_reference, run);
        let references = result
            .candidates
            .iter()
            .zip(visible)
            .map(|(candidate, visible)| CandidateReference::issue(candidate, visible))
            .collect::<Vec<_>>();
        let value = json!({
            "version": AI_CAPABILITY_BROKER_VERSION,
            "candidates": result
                .candidates
                .iter()
                .zip(&references)
                .map(|(candidate, reference)| fixed_candidate_value(candidate, reference, true))
                .collect::<Vec<_>>(),
        });
        let issued = IssuedSearch {
            fence,
            search: Arc::new(result),
            references,
        };
        self.cache_discovery(&issued);
        session.record_search(issued);
        Ok(value)
    }

    /// Dispatches one frozen `graphql.capabilities.browse` call.
    ///
    /// Browsing lists the current top-level capabilities by hierarchy:
    /// `namespaces` lists the logical namespaces, `capabilities` the
    /// capabilities of one namespace (optionally of one entity), and
    /// `operations` the operations of one entity capability. It covers the
    /// capabilities discovery covers: generated reads and the reviewed static
    /// tools this run brokers.
    ///
    /// Host policy decides every listed entry, with each entity's grouped
    /// roots, in one batch before anything is counted or paged, exactly as
    /// for discovery: a namespace, capability or operation appears only when
    /// current policy admits it, and an entity only with its admitted roots.
    /// Policy is decided in batches bounded by
    /// [`Self::with_maximum_discovery_authorizations`], and browsing stops
    /// once the requested page is decided, so hidden entries change how much
    /// policy work is done, never the response. Listed capabilities are
    /// retained, with their references, like discovery candidates, so
    /// describe accepts them;
    /// browsing grants no load or execution authority.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for malformed arguments or
    /// [`AI_CAPABILITY_DISCOVERY_TOO_BROAD`], and fails closed on
    /// rehydration, index, or policy failure.
    pub async fn dispatch_browse(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        session: &AiCapabilityBrokerSession,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, AiError> {
        let parsed: BrokerBrowseArguments =
            serde_json::from_value(arguments.clone()).map_err(|_| malformed_broker_arguments())?;
        let (namespace, entity) = match parsed.level {
            BrowseLevel::Namespaces if parsed.namespace.is_none() && parsed.entity.is_none() => {
                (None, None)
            }
            BrowseLevel::Capabilities if parsed.namespace.is_some() => {
                (parsed.namespace.as_deref(), parsed.entity.as_deref())
            }
            BrowseLevel::Operations if parsed.namespace.is_some() && parsed.entity.is_some() => {
                (parsed.namespace.as_deref(), parsed.entity.as_deref())
            }
            _ => return Err(malformed_broker_arguments()),
        };
        let offset = parsed.offset.unwrap_or(0);
        let limit = parsed.limit.unwrap_or(AI_CAPABILITY_BROWSE_PAGE_MAXIMUM);
        if [namespace, entity]
            .into_iter()
            .flatten()
            .any(|name| !valid_browse_name(name))
            || offset > AI_CAPABILITY_BROWSE_MAXIMUM_OFFSET
            || !(1..=AI_CAPABILITY_BROWSE_PAGE_MAXIMUM).contains(&limit)
        {
            return Err(malformed_broker_arguments());
        }
        let principal = self.rehydrate(principal_reference).await?;
        let indexes = self.current_indexes.current_index_set(run)?;
        let mut listed = indexes.browse_matches(namespace, entity)?;
        listed.retain(|candidate| {
            let entry = candidate.entry();
            match parsed.level {
                // Operations belong to an entity capability alone.
                BrowseLevel::Operations => entry.kind == AiCapabilityKind::GeneratedEntity,
                _ => session.brokers_entry(candidate.index(), entry),
            }
        });
        // Policy decides the listed entries in order, in batches of at most
        // `batch_size` decisions, and stops once the requested page is
        // decided: the first `offset + limit + 1` admitted results. What is
        // returned is therefore exactly the admitted prefix, whatever hidden
        // entries the listing holds. Hidden entries change only how much
        // policy work is done, never the response.
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let wanted = start.saturating_add(usize::from(limit)).saturating_add(1);
        let batch_size = self.policy_batch_size();
        let mut kept = Vec::new();
        let mut found_namespaces = BTreeSet::<&str>::new();
        let mut position = 0;
        loop {
            let decided = match parsed.level {
                BrowseLevel::Namespaces => found_namespaces.len() >= wanted,
                BrowseLevel::Capabilities => kept.len() >= wanted,
                // Two admitted entities of one name are ambiguous.
                BrowseLevel::Operations => kept.len() >= 2,
            };
            if decided || position >= listed.len() {
                break;
            }
            let mut chunk = Vec::new();
            let mut decisions = 0_usize;
            while let Some(candidate) = listed.get(position) {
                // A namespace already listed needs no further decisions.
                if parsed.level == BrowseLevel::Namespaces
                    && found_namespaces.contains(candidate.entry().namespace.as_str())
                {
                    position += 1;
                    continue;
                }
                let size = 1 + candidate.entry().operations.len();
                if !chunk.is_empty() && decisions + size > batch_size {
                    break;
                }
                chunk.push(*candidate);
                decisions += size;
                position += 1;
            }
            if chunk.is_empty() {
                break;
            }
            for admitted in Box::pin(self.admit_matches(&principal, run, &chunk, false)).await? {
                if parsed.level == BrowseLevel::Namespaces {
                    found_namespaces.insert(admitted.0.entry().namespace.as_str());
                }
                kept.push(admitted);
            }
        }
        let page = |total: usize| {
            let end = start.saturating_add(usize::from(limit)).min(total);
            let next = (end < total).then_some(end);
            (start.min(total)..end, next)
        };
        let mut value = empty_browse_page(parsed.level);
        let object = value
            .as_object_mut()
            .ok_or_else(|| AiError::InvalidConfiguration("browse result is invalid".to_owned()))?;
        // Listed capabilities carry an opaque per-run reference in place of
        // their entry fingerprint; describe resolves it server-side, only
        // for the same principal and run.
        let fence = candidate_reference_fence(principal_reference, run);
        let mut references = Vec::new();
        let mut reference_for = |candidate: &crate::AiCapabilitySearchCandidate,
                                 entry: &AiCapabilityIndexEntry,
                                 admitted: &BTreeSet<AiToolId>| {
            let reference =
                CandidateReference::issue(candidate, visible_fingerprint(entry, admitted));
            references.push(reference.clone());
            reference
        };
        let mut retained = Vec::new();
        match parsed.level {
            BrowseLevel::Namespaces => {
                // Listed in namespace order: each namespace with at least one
                // admitted capability, without a count, which would need
                // every entry decided.
                let namespaces = found_namespaces.into_iter().collect::<Vec<_>>();
                let (range, next) = page(namespaces.len());
                object.insert("nextOffset".to_owned(), json!(next));
                object.insert(
                    "namespaces".to_owned(),
                    namespaces[range]
                        .iter()
                        .map(|namespace| json!({"namespace": namespace}))
                        .collect(),
                );
            }
            BrowseLevel::Capabilities => {
                let (range, next) = page(kept.len());
                let mut items = Vec::new();
                for (candidate, admitted) in &kept[range] {
                    let summary = candidate.admitted_candidate(admitted);
                    items.push(fixed_candidate_value(
                        &summary,
                        &reference_for(&summary, candidate.entry(), admitted),
                        true,
                    ));
                    retained.push(summary);
                }
                object.insert("nextOffset".to_owned(), json!(next));
                object.insert("capabilities".to_owned(), items.into());
            }
            BrowseLevel::Operations => {
                let (candidate, admitted) = match kept.as_slice() {
                    // An unknown entity and one policy denies look the same.
                    [] => {
                        session.record_browse(None);
                        return Ok(value);
                    }
                    [only] => only,
                    _ => return Err(malformed_broker_arguments()),
                };
                let operations = candidate
                    .entry()
                    .operations
                    .iter()
                    .filter(|member| admitted.contains(&member.capability_id))
                    .collect::<Vec<_>>();
                let (range, next) = page(operations.len());
                let summary = candidate.admitted_candidate(admitted);
                // The header carries no operation list: the page below is
                // the only one, and it honours the requested limit.
                object.insert(
                    "capability".to_owned(),
                    fixed_candidate_value(
                        &summary,
                        &reference_for(&summary, candidate.entry(), admitted),
                        false,
                    ),
                );
                object.insert("nextOffset".to_owned(), json!(next));
                object.insert(
                    "operations".to_owned(),
                    operations[range]
                        .iter()
                        .map(|member| {
                            json!({
                                "operation": member.operation_name,
                                "shape": member.operation_shape,
                                "description": member.description,
                                "resultDescription": member.result_description,
                            })
                        })
                        .collect(),
                );
                retained.push(summary);
            }
        }
        let issued = (!retained.is_empty()).then(|| IssuedSearch {
            fence,
            search: Arc::new(AiCapabilityIndexSetSearchResult {
                index_set_fingerprint: indexes.fingerprint().to_owned(),
                candidates: retained,
            }),
            references,
        });
        session.record_browse(issued);
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
        let (result, _) = self
            .search_admitted(principal_reference, run, &query, true, |_, _| true)
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
        // Client-deferred delivery keeps its version 2 response: its
        // definition, session binding and disclosure contract are unchanged
        // by broker version 3.
        let value = json!({
            "version": AI_CAPABILITY_CLIENT_DEFERRED_DISCOVERY_VERSION,
            "indexFingerprint": result.index_set_fingerprint,
            "candidates": result
                .candidates
                .iter()
                .map(|candidate| {
                    let mut value = candidate_value(candidate);
                    if let Some(object) = value.as_object_mut() {
                        object.insert(
                            "candidateFingerprint".to_owned(),
                            json!(candidate.entry_fingerprint),
                        );
                    }
                    value
                })
                .collect::<Vec<_>>(),
        });
        session.record_deferred_search(result, loaded);
        Ok(value)
    }

    /// Dispatches one frozen `graphql.capabilities.describe` call.
    ///
    /// The model names the candidate by the opaque `candidateReference` a
    /// discover or browse call of this run returned. The reference resolves
    /// only for the principal reference, AI session, run, attempt and
    /// provider session it was issued to, from this session or, when enabled,
    /// the unexpired discovery cache, and only against the exact search
    /// snapshot it was issued with, while that search is retained. An unknown
    /// identifier, an unknown or evicted reference, one issued to another run
    /// and an earlier contract's `candidateFingerprint` are all reported as
    /// one bounded retryable [`AI_CAPABILITY_SELECTION_STALE`], so describe
    /// cannot be used to probe for capabilities the current principal cannot
    /// see.
    ///
    /// A held candidate whose server-side fingerprints have moved is loaded
    /// from the current index under current policy. When what this principal
    /// is shown of it is unchanged, the change lies only in state it cannot
    /// see and the description is returned; otherwise the call is the
    /// distinct retryable [`AI_CAPABILITY_STALE`], telling the model to
    /// discover and describe again.
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
        // Earlier broker contracts named a discover candidate by its entry
        // fingerprint. A replayed call holds no selection, whatever the value
        // of that member or any other: it is answered before the arguments
        // are parsed, and the model discovers again.
        if arguments
            .as_object()
            .is_some_and(|object| object.contains_key("candidateFingerprint"))
        {
            return Err(stale_selection());
        }
        let parsed: BrokerDescribeArguments =
            serde_json::from_value(arguments.clone()).map_err(|_| malformed_broker_arguments())?;
        let reference = parsed
            .candidate_reference
            .filter(|reference| crate::valid_sha256(reference))
            .ok_or_else(malformed_broker_arguments)?;
        let capability_id = AiToolId::parse(parsed.capability_id).map_err(|_| stale_selection())?;
        // A discovered or browsed candidate names an opaque reference that
        // only the run it was issued to resolves, from its own session or
        // the discovery cache, to the exact snapshot it was issued with.
        let fence = candidate_reference_fence(principal_reference, run);
        let (search, shown) = session
            .issued_reference(&capability_id, &reference, &fence)
            .or_else(|| self.cached_discovery(&fence, &capability_id, &reference))
            .ok_or_else(stale_selection)?;
        let candidate = search
            .candidates
            .iter()
            .find(|candidate| candidate.id == capability_id)
            .ok_or_else(stale_selection)?;
        // Admission is decided from the held candidate first: a candidate
        // this run's rules or brokered tools do not reach is a stale
        // selection, never a distinguishable catalogue-drift outcome.
        if !session.brokers_candidate(candidate) {
            return Err(stale_selection());
        }
        let principal = self.rehydrate(principal_reference).await?;
        let indexes = self.current_indexes.current_index_set(run)?;
        // The run holds this exact candidate, so reporting catalogue drift
        // discloses nothing the principal was not already shown.
        let (index, entry) = indexes.entry(&capability_id).ok_or_else(stale_capability)?;
        // A static tool is loadable only while this run brokers its exact
        // fingerprint. A cached discovery from a run that did, or any other
        // capability family, is the same stale selection as an unknown ID.
        if !session.brokers_entry(index, entry) {
            return Err(stale_selection());
        }
        if candidate.kind != entry.kind {
            return Err(AiError::Forbidden);
        }
        let loaded = self
            .load_entry(&principal, run, &indexes, index, entry)
            .await?;
        // What this principal is shown now, including every admitted
        // sibling root, must be what discovery showed it, whether or not
        // state hidden from it has drifted since: the candidate is loaded
        // from the current index, and any change it can see is stale.
        if loaded.visible_fingerprint != shown {
            return Err(stale_capability());
        }
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
        let snapshot = self.loaded_operation_snapshot(run, session, &parsed.loaded_reference)?;
        let loaded = &snapshot.loaded;
        let target = self
            .authorize_loaded_operation(
                principal_reference,
                run,
                session,
                &snapshot,
                parsed.operation.as_deref(),
            )
            .await?;
        let indexes = &snapshot.indexes;
        let field_descriptions = indexes
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

    /// Retains the next page of one executed continuable page and returns its
    /// model-facing handle.
    ///
    /// Call this after the page has executed through
    /// [`crate::AiRuntime::execute_query_capability_page`], with the page info
    /// it returned. Returns `None`, and retains nothing, when there is no
    /// further page, the end cursor is absent or oversized, the chain has
    /// reached [`AI_CAPABILITY_MAXIMUM_CONTINUATION_PAGES`], the loaded binding
    /// has expired, or the execution is not a generated query root.
    ///
    /// The retained state is bound to this session, to the loaded reference
    /// the execution used (and so to its principal, run, attempt, provider
    /// session and every fingerprint proven at load), and to the exact root
    /// and plan of the first page. It expires with the loaded binding. At
    /// most [`AI_CAPABILITY_MAXIMUM_CONTINUATIONS`] handles are retained.
    pub fn record_continuation(
        &self,
        session: &AiCapabilityBrokerSession,
        execution: &AiCapabilityExecution,
        page_info: &crate::AiGraphqlQueryPageInfo,
    ) -> Option<AiCapabilityContinuation> {
        if execution.capability_kind != AiCapabilityKind::GeneratedQuery {
            return None;
        }
        let cursor = page_info.next_cursor()?;
        if cursor.len() > crate::AI_GRAPHQL_QUERY_MAXIMUM_CURSOR_BYTES {
            return None;
        }
        let page = execution.page_number().checked_add(1)?;
        let now = self.clock.now();
        // Every handle issued from one first-page execution, however often
        // it is recorded, shares that execution's chain.
        let chain = execution.chain.clone()?;
        if page > AI_CAPABILITY_MAXIMUM_CONTINUATION_PAGES
            || chain.exhausted()
            || now >= execution.loaded_expires_at
        {
            return None;
        }
        let handle = hash_json(&json!({
            "version": 1,
            "nonce": Uuid::new_v4(),
            "loaded": execution.audit_reference,
            "page": page,
        }));
        session.record_continuation(
            handle.clone(),
            BrokerContinuation {
                loaded_reference: execution.audit_reference.clone(),
                capability_id: execution.capability_id.clone(),
                contract_fingerprint: execution.contract_fingerprint.clone(),
                operation_name: execution.operation_name.clone(),
                plan: execution.plan.clone(),
                cursor: cursor.to_owned(),
                page,
                expires_at: execution.loaded_expires_at,
                chain,
            },
        );
        Some(AiCapabilityContinuation {
            handle,
            expires_in_seconds: (execution.loaded_expires_at - now).whole_seconds().max(0),
        })
    }

    /// Dispatches one frozen `graphql.capabilities.execute` call up to, but
    /// not including, the resolver.
    ///
    /// A generated read, a brokered reviewed static tool and, for a session
    /// [`AiCapabilityBrokerSession::with_brokered_mutations`] whose bound
    /// rules admit it, a generated mutation resolve here. A mutation is only
    /// resolved ([`AiCapabilityExecution::is_consequential`]): the classified
    /// coordinator classifies, approves and runs it. For a session
    /// [`AiCapabilityBrokerSession::with_brokered_subscriptions`], a generated
    /// subscription of an index compiled in process from a typed catalogue
    /// resolves here too, with its observation, and only resolves
    /// ([`AiCapabilityExecution::is_subscription_wait`]): the wait service
    /// registers it.
    /// The returned closed plan contains public names and finite typed values
    /// only and is compiled by the authoritative schema-derived compiler.
    ///
    /// With `continuation`, the call names a handle
    /// [`Self::record_continuation`] returned for this loaded reference and
    /// must carry no other plan field. The retained plan of the first page is
    /// returned with the server-held cursor
    /// ([`AiCapabilityExecution::continuation_cursor`]), after the same
    /// loaded-binding, index and host-policy checks as a first page. An
    /// unknown, expired or foreign handle is a stale selection.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] for malformed arguments, for an
    /// unknown loaded reference or an expired binding
    /// ([`AI_CAPABILITY_SELECTION_STALE`]), and for a loaded capability of
    /// which something this principal is shown has changed
    /// ([`AI_CAPABILITY_STALE`]). Drift only in state the principal cannot see
    /// refreshes the binding under the same reference instead. Returns [`AiError::Forbidden`] for a non-read
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
        if !crate::valid_sha256(&parsed.loaded_reference)
            || parsed
                .continuation
                .as_ref()
                .is_some_and(|handle| !crate::valid_sha256(handle))
        {
            return Err(malformed_broker_arguments());
        }
        let snapshot = self.loaded_operation_snapshot(run, session, &parsed.loaded_reference)?;
        let loaded = &snapshot.loaded;
        if let Some(handle) = parsed.continuation.clone() {
            return Box::pin(self.authorize_continuation(
                principal_reference,
                run,
                session,
                &snapshot,
                parsed,
                &handle,
            ))
            .await;
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
            AiCapabilityKind::GeneratedMutation if session.brokers_mutations() => false,
            AiCapabilityKind::GeneratedSubscription if session.brokers_subscriptions() => false,
            _ => return Err(AiError::Forbidden),
        };
        let target = self
            .authorize_loaded_operation(
                principal_reference,
                run,
                session,
                &snapshot,
                parsed.operation.as_deref(),
            )
            .await?;
        // Call shape is validated only once the session is proven to still
        // reach the capability, so a held reference it no longer reaches
        // answers as an unknown one whatever the call carries.
        // Only a subscription takes an observation, and it always does.
        let subscription = loaded.capability_kind == AiCapabilityKind::GeneratedSubscription;
        if subscription != parsed.observation.is_some()
            || (subscription && parsed.maximum_items.is_some())
            || parsed
                .operation
                .as_ref()
                .is_some_and(|operation| !valid_graphql_name(operation))
        {
            return Err(malformed_broker_arguments());
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
                contract_fingerprint: target.contract_fingerprint,
                audit_reference: loaded.audit_reference(),
                plan: plan_arguments,
                operation_name: target.operation_name,
                loaded_expires_at: loaded.expires_at,
                chain: None,
                continued: None,
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
        if let Some(observation) = parsed.observation {
            if observation.timeout_seconds == 0 || observation.maximum_events == 0 {
                return Err(malformed_broker_arguments());
            }
            plan.insert(
                "timeoutSeconds".to_owned(),
                json!(observation.timeout_seconds),
            );
            plan.insert(
                "maximumEvents".to_owned(),
                json!(observation.maximum_events),
            );
            if let Some(condition) = observation.condition {
                plan.insert(
                    "condition".to_owned(),
                    json!({
                        "field": condition.field,
                        "operator": condition.operator,
                        "value": condition.value,
                    }),
                );
            }
        }
        // A subscription resolves to one wait and is never continued.
        let chain = (!subscription).then(ContinuationChain::first_page);
        session.record_execute();
        Ok(AiCapabilityExecution {
            capability_id: target.capability_id,
            capability_kind: target.capability_kind,
            capability_fingerprint: target.capability_fingerprint,
            contract_fingerprint: target.contract_fingerprint,
            audit_reference: loaded.audit_reference(),
            plan: serde_json::Value::Object(plan),
            operation_name: target.operation_name,
            loaded_expires_at: loaded.expires_at,
            chain,
            continued: None,
        })
    }

    /// Resolves one continued page: the retained plan and cursor of a handle
    /// this session holds for this exact loaded reference, after the same
    /// checks as a first page.
    async fn authorize_continuation(
        &self,
        principal_reference: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        session: &AiCapabilityBrokerSession,
        snapshot: &LoadedOperationSnapshot,
        parsed: BrokerExecuteArguments,
        handle: &str,
    ) -> Result<AiCapabilityExecution, AiError> {
        let loaded = &snapshot.loaded;
        // The retained query is the whole plan: nothing else may vary. The
        // shape is enforced only after the session is proven to still reach
        // the capability, so a held reference it no longer reaches answers
        // as an unknown one whatever the call carries.
        let operation_valid = parsed
            .operation
            .as_ref()
            .is_none_or(|operation| valid_graphql_name(operation));
        let shape_valid = parsed.arguments.is_empty()
            && parsed.observation.is_none()
            && parsed.selections.is_empty()
            && parsed.relationship_arguments.is_empty()
            && parsed.relationship_maximum_items.is_empty()
            && parsed.maximum_items.is_none()
            && operation_valid;
        let requested_operation = parsed.operation.as_deref().filter(|_| operation_valid);
        if !matches!(
            loaded.capability_kind,
            AiCapabilityKind::GeneratedQuery | AiCapabilityKind::GeneratedEntity
        ) {
            // A capability that is never continued is first checked against
            // the current index and policy exactly as an ordinary execute,
            // so a reference this session no longer reaches answers as it
            // would there (an unknown reference). Only a capability it does
            // reach learns that it cannot be continued.
            Box::pin(self.authorize_loaded_operation(
                principal_reference,
                run,
                session,
                snapshot,
                requested_operation,
            ))
            .await?;
            return Err(AiError::Forbidden);
        }
        // A handle of another loaded reference or session is the same stale
        // selection as an unknown one.
        let loaded_reference = loaded.audit_reference();
        let continuation = session
            .continuation(handle)
            .filter(|continuation| continuation.loaded_reference == loaded_reference)
            .ok_or_else(stale_selection)?;
        if self.clock.now() >= continuation.expires_at {
            return Err(stale_selection());
        }
        // Reserve the execution first, so concurrent redemptions of one
        // chain cannot overspend it, and return it on any refusal below.
        // Every handle of a chain draws on one budget.
        if !continuation.chain.reserve() {
            return Err(stale_selection());
        }
        let checked = async {
            let operation = (loaded.capability_kind == AiCapabilityKind::GeneratedEntity)
                .then_some(continuation.operation_name.as_str());
            let target = self
                .authorize_loaded_operation(principal_reference, run, session, snapshot, operation)
                .await?;
            // The first page's plan stays valid while the root's
            // model-facing contract is unchanged; the page executes
            // against the exact current capability.
            if target.capability_id != continuation.capability_id
                || target.contract_fingerprint != continuation.contract_fingerprint
                || target.capability_kind != AiCapabilityKind::GeneratedQuery
            {
                return Err(stale_capability());
            }
            // The policy checks awaited: the handle must still be retained
            // and unexpired now.
            if !self.continuation_retained(session, handle, continuation.expires_at) {
                return Err(stale_selection());
            }
            // Reached: now the call itself must be a bare continuation of
            // this handle's operation.
            if !shape_valid {
                return Err(malformed_broker_arguments());
            }
            if requested_operation.is_some_and(|operation| operation != continuation.operation_name)
            {
                return Err(invalid_operation());
            }
            Ok(target)
        }
        .await;
        let target = match checked {
            Ok(target) => target,
            Err(error) => {
                continuation.chain.release();
                return Err(error);
            }
        };
        session.record_execute();
        Ok(AiCapabilityExecution {
            capability_id: target.capability_id,
            capability_kind: target.capability_kind,
            capability_fingerprint: target.capability_fingerprint,
            contract_fingerprint: target.contract_fingerprint,
            audit_reference: loaded_reference,
            plan: continuation.plan,
            operation_name: target.operation_name,
            loaded_expires_at: loaded.expires_at,
            chain: Some(continuation.chain),
            continued: Some(ContinuedPage {
                cursor: continuation.cursor,
                page: continuation.page,
                handle: handle.to_owned(),
                expires_at: continuation.expires_at,
                released: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }),
        })
    }

    fn continuation_retained(
        &self,
        session: &AiCapabilityBrokerSession,
        handle: &str,
        expires_at: OffsetDateTime,
    ) -> bool {
        session.continuation(handle).is_some() && self.clock.now() < expires_at
    }

    /// Final admission of one execution, for the executor to call after
    /// host tool policy and the request context are built, immediately
    /// before the resolver runs.
    ///
    /// A first page is always admitted. A continued page is admitted only
    /// while its handle is still retained by `session` and unexpired; on
    /// refusal its reserved chain execution is returned and
    /// [`AiCapabilityExecution::continuation_refused`] reports it, so the
    /// caller answers with [`AI_CAPABILITY_SELECTION_STALE`].
    pub fn continuation_admission<'a>(
        &'a self,
        session: &'a AiCapabilityBrokerSession,
        execution: &'a AiCapabilityExecution,
    ) -> impl Fn() -> bool + Send + Sync + 'a {
        move || {
            let Some(continued) = &execution.continued else {
                return true;
            };
            if self.continuation_retained(session, &continued.handle, continued.expires_at) {
                return true;
            }
            if !continued
                .released
                .swap(true, std::sync::atomic::Ordering::AcqRel)
                && let Some(chain) = &execution.chain
            {
                chain.release();
            }
            false
        }
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
            offering: Some(AiCapabilityOfferingSnapshot {
                state: self.tools.clone(),
                generation: state.generation,
            }),
        }
    }

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

    /// Returns this delivery with its broker session bound to the classified
    /// coordinator's rules for brokered mutations
    /// ([`AiCapabilityBrokerSession::bind_mutation_rules`]). Only the
    /// returned delivery carries them; the turn it was cloned from, and any
    /// other clone, stays closed to mutations.
    pub(crate) fn with_mutation_rules(
        mut self,
        rules: &crate::AiResolvedRuleSet,
    ) -> Result<Self, AiError> {
        self.session = self.session.bind_mutation_rules(rules)?;
        Ok(self)
    }

    /// Exact crate-authored definition of one broker operation in this
    /// turn's delivery mode, or `None` when the mode does not offer it.
    ///
    /// The fixed broker's definitions are catalogue-independent; the
    /// client-deferred discovery definition stays bound to the index.
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
        generated_index_with_roots(
            target_id,
            profile_id,
            target_policy,
            added_field,
            second_root.then_some("Read one reviewed application record by its subject heading."),
        )
    }

    /// [`generated_index_with_schema`] whose `RecordBySubject` root, when
    /// present, carries `second_root` as its description.
    fn generated_index_with_roots(
        target_id: &str,
        profile_id: &str,
        target_policy: &str,
        added_field: Option<&str>,
        second_root: Option<&str>,
    ) -> Arc<AiCapabilityIndex> {
        let added_sdl = added_field
            .map(|field| format!(", {field}: String!"))
            .unwrap_or_default();
        let second_root_sdl = if second_root.is_some() {
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
        let second_operation = second_root.map(|description| {
            GraphqlSemanticOperationDescriptor::custom(
                GraphqlOperationKind::Query,
                "RecordBySubject",
                description,
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
        assert_eq!(fixed.tools().len(), 5);
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
                .all(|tool| serde_json::to_vec(tool).is_ok_and(|bytes| bytes.len() < 4_096)),
            "{:?}",
            fixed
                .tools()
                .iter()
                .map(|tool| (
                    tool.tool_id.clone(),
                    serde_json::to_vec(tool).unwrap().len()
                ))
                .collect::<Vec<_>>()
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
        assert_eq!(surface.tools().len(), 6);
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
        assert_eq!(delivery.current_tools().len(), 5);
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
    async fn discovery_authorization_bound_counts_only_the_authorized_view() {
        let index = large_static_index(200);
        let principal_reference = principal().reference();
        // One permitted entry among 200 matches: the hidden matches are
        // decided in policy batches of the bound, never counted against it.
        let authority = Arc::new(PermittedSet::new(["catalogue.record_0199"]));
        let unbounded = batch_broker(&index, authority.clone())
            .search(&principal_reference, &run_binding(), &catalogue_query(8))
            .await
            .expect("unbounded search");
        for bound in [1, 7, 199, 200] {
            let bounded = batch_broker(&index, authority.clone())
                .with_maximum_discovery_authorizations(bound)
                .expect("bound");
            let result = bounded
                .search(&principal_reference, &run_binding(), &catalogue_query(8))
                .await
                .expect("bounded search");
            assert_eq!(result.candidates, unbounded.candidates, "bound {bound}");
        }
        assert_eq!(unbounded.candidates.len(), 1);
        assert_eq!(unbounded.candidates[0].id.as_str(), "catalogue.record_0199");

        // The bound refuses a query whose admitted view exceeds it, whatever
        // the hidden matches.
        let permitted = (0..8)
            .map(|position| format!("catalogue.record_{position:04}"))
            .collect::<Vec<_>>();
        let visible = Arc::new(PermittedSet::new(permitted.iter().map(String::as_str)));
        for size in [8, 200] {
            let broad = batch_broker(&large_static_index(size), visible.clone())
                .with_maximum_discovery_authorizations(7)
                .expect("bound");
            assert!(
                matches!(
                    broad
                        .search(&principal_reference, &run_binding(), &catalogue_query(8))
                        .await,
                    Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_DISCOVERY_TOO_BROAD
                ),
                "{size} entries"
            );
            let admitted = batch_broker(&large_static_index(size), visible.clone())
                .with_maximum_discovery_authorizations(8)
                .expect("bound")
                .search(&principal_reference, &run_binding(), &catalogue_query(8))
                .await
                .expect("admitted view within the bound");
            assert_eq!(admitted.candidates.len(), 8, "{size} entries");
        }

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
        assert_eq!(parsed.continuation, None);
        // A continued page names only its loaded reference and handle.
        let continued = json!({
            "loadedReference": "b".repeat(64),
            "continuation": "c".repeat(64),
        });
        assert!(validator.is_valid(&continued));
        let parsed: BrokerExecuteArguments =
            serde_json::from_value(continued).expect("continued execute wrapper should decode");
        assert!(parsed.selections.is_empty());
        assert_eq!(
            parsed.continuation.as_deref(),
            Some("c".repeat(64).as_str())
        );
        assert!(!validator.is_valid(&json!({
            "loadedReference": "b".repeat(64),
            "continuation": "short",
        })));

        assert_eq!(execute.parameters["required"], json!(["loadedReference"]));
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
                "candidateReference": result["candidates"][0]["candidateReference"]
            })
        }

        fn session() -> AiCapabilityBrokerSession {
            AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
                .expect("run state")
        }
    }

    #[tokio::test]
    async fn discovery_cache_answers_only_the_run_its_references_were_issued_to() {
        let mut fixture = DiscoveryCacheFixture::new(4);
        fixture.principal.expires_at = Some(fixture.clock.now() + Duration::minutes(75));
        let candidate = fixture.discover(&fixture.run).await;
        // A fresh session of the issuing run resolves the reference from the
        // cache.
        let first_session = DiscoveryCacheFixture::session();
        let first = fixture
            .broker
            .dispatch_describe(&fixture.principal, &fixture.run, &first_session, &candidate)
            .await
            .expect("cached candidate describes in the issuing run");
        let mut changed_deadline = fixture.principal.clone();
        changed_deadline.expires_at = Some(fixture.clock.now() + Duration::minutes(90));
        assert!(matches!(fixture.broker.authorize_broker_execution(
            &changed_deadline, &fixture.run, &first_session,
            &json!({"loadedReference": first.loaded_reference(), "selections": ["records.id"]}),
        ).await, Err(AiError::Forbidden)));

        // No later run, and no other attempt of this run, resolves it, so a
        // reference never crosses runs through the cache.
        fixture.clock.advance_seconds(3_600);
        fixture.principal.expires_at = Some(fixture.clock.now() + Duration::minutes(75));
        let mut next_run = fixture.run.clone();
        next_run.run_id = AiRunId(Uuid::from_u128(200));
        next_run.attempt_id = Uuid::from_u128(201);
        let mut next_attempt = fixture.run.clone();
        next_attempt.attempt_id = Uuid::from_u128(202);
        let mut next_provider_session = fixture.run.clone();
        next_provider_session.provider_session_fingerprint = "provider-session-v2".to_owned();
        for run in [&next_run, &next_attempt, &next_provider_session] {
            let session = DiscoveryCacheFixture::session();
            let error = fixture
                .broker
                .clone()
                .dispatch_describe(&fixture.principal, run, &session, &candidate)
                .await
                .expect_err("another run holds no selection");
            assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
            assert_eq!(session.amplification().describe_calls, 0);
        }

        // The next run discovers again and loads with fresh fences.
        let rediscovered = fixture.discover(&next_run).await;
        assert_ne!(
            rediscovered["candidateReference"], candidate["candidateReference"],
            "every discovery issues fresh references"
        );
        let next_session = DiscoveryCacheFixture::session();
        let next = fixture
            .broker
            .dispatch_describe(&fixture.principal, &next_run, &next_session, &rediscovered)
            .await
            .expect("rediscovered candidate describes");
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
                    &rediscovered,
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
        forged["candidateReference"] = json!("f".repeat(64));
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

        // A changed host-policy fingerprint that still admits the same view
        // shows the principal nothing new: the binding is refreshed and the
        // call proceeds. A denial is never reported as drift.
        *fixture
            .authority
            .policy_fingerprint
            .write()
            .expect("policy") = "current-policy-v2".to_owned();
        fixture
            .broker
            .authorize_broker_execution(&fixture.principal, &fixture.run, &session, &execute)
            .await
            .expect("a changed policy fingerprint refreshes the loaded binding");
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
        forged["candidateReference"] = json!("f".repeat(64));
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
        // A target-policy change shows the principal nothing new: the
        // cached candidate loads from the current index.
        *fixture.index.0.write().expect("index") = generated_index("target-policy-v2");
        fixture
            .broker
            .dispatch_describe(
                &fixture.principal,
                &fixture.run,
                &DiscoveryCacheFixture::session(),
                &candidate,
            )
            .await
            .expect("target-policy change refreshes");
        // A schema change it can see is drift.
        *fixture.index.0.write().expect("index") = schema_changed_index();
        let error = fixture
            .broker
            .dispatch_describe(
                &fixture.principal,
                &fixture.run,
                &DiscoveryCacheFixture::session(),
                &candidate,
            )
            .await
            .expect_err("visible drift");
        assert_input_error(&error, AI_CAPABILITY_STALE);
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
        let evicted = fixture.discover(&fixture.run).await;
        let mut other_run = fixture.run.clone();
        other_run.session_id = AiSessionId(Uuid::from_u128(999));
        let newest = fixture.discover(&other_run).await;
        assert!(matches!(
            fixture
                .broker
                .dispatch_describe(
                    &fixture.principal,
                    &fixture.run,
                    &DiscoveryCacheFixture::session(),
                    &evicted,
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
                &newest,
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
                    &newest,
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
                    "candidateReference": candidate["candidateReference"]
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
        // A session that opts into neither keeps mutations and
        // subscriptions outside the broker.
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
            "candidateReference": candidate["candidateReference"],
        });
        // A candidate this run did not broker is a stale selection, even when
        // its search result and reference are at hand.
        default.record_search(session.issued_search_holding(&static_entry.id));
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
        // tool does not execute, and is indistinguishable from an unknown
        // reference.
        let revoked = AiCapabilityBrokerSession {
            inner: Arc::clone(&session.inner),
            limits: session.limits,
            brokered_static_tools: Arc::new(BTreeSet::new()),
            inline_describe_schema: false,
            brokered_mutations: false,
            mutation_rules: None,
            brokered_subscriptions: false,
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
            Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_SELECTION_STALE
        ));
        describe_type_matches_unknown(
            &broker,
            &principal,
            &run,
            &revoked,
            &loaded_reference,
            "excluded static tool",
        )
        .await;
    }

    /// Asserts that `describe_type` of a held reference answers exactly as
    /// for an unknown reference, with `selection_stale`.
    async fn describe_type_matches_unknown(
        broker: &AiCapabilityDiscoveryBroker,
        principal: &PrincipalReference,
        run: &AiCapabilityRunBinding,
        session: &AiCapabilityBrokerSession,
        held: &str,
        label: &str,
    ) {
        let unknown = "f".repeat(64);
        assert_ne!(held, unknown);
        let call = |reference: &str| json!({"loadedReference": reference, "type": "select"});
        let held = broker
            .dispatch_describe_type(principal, run, session, &call(held))
            .await
            .err();
        let unknown = broker
            .dispatch_describe_type(principal, run, session, &call(&unknown))
            .await
            .err();
        assert!(
            matches!(&held, Some(AiError::InvalidInput(message)) if message == AI_CAPABILITY_SELECTION_STALE),
            "{label}: {held:?}"
        );
        assert_eq!(format!("{held:?}"), format!("{unknown:?}"), "{label}");
    }

    /// One automatic `CreateRecord` and one approval-required
    /// `ArchiveRecord` mutation of the `Record` entity.
    fn mutation_index(target_policy: &str) -> Arc<AiCapabilityIndex> {
        compile_mutation_index(target_policy, false).expect("mutation capability index")
    }

    fn compile_mutation_index(
        target_policy: &str,
        transient_argument: bool,
    ) -> Result<Arc<AiCapabilityIndex>, AiError> {
        let sdl = "schema { query: Query, mutation: Mutation }
            type Query { health: Boolean! }
            type Mutation { CreateRecord(input: RecordInput!): Record!, ArchiveRecord(recordId: ID!): Record! }
            input RecordInput { subject: String! }
            type Record { recordId: ID!, subject: String! }";
        let marked_sdl;
        let sdl = if transient_argument {
            marked_sdl = format!(
                "directive @transientInput on ARGUMENT_DEFINITION\n{}",
                sdl.replace("input: RecordInput!", "input: RecordInput! @transientInput")
            );
            marked_sdl.as_str()
        } else {
            sdl
        };
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
            fields: vec![scalar("recordId", "ID"), scalar("subject", "String")].into_boxed_slice(),
            label_fields: Vec::new(),
            record_version: None,
        };
        let record =
            GraphqlSemanticTypeRef::named("Record", GraphqlSemanticTypeKind::Object, false);
        let create = GraphqlSemanticOperationDescriptor::custom(
            GraphqlOperationKind::Mutation,
            "CreateRecord",
            "Create one reviewed application record.",
            vec![GraphqlSemanticArgumentDescriptor {
                graphql_name: "input".to_owned(),
                description: "Reviewed record input.".to_owned(),
                type_ref: GraphqlSemanticTypeRef::named(
                    "RecordInput",
                    GraphqlSemanticTypeKind::Object,
                    false,
                ),
            }],
            record.clone(),
            true,
        )
        .expect("create semantics")
        .with_ai_mutation_execution(graphql_orm::graphql::orm::AiMutationExecutionPolicy::Automatic)
        .expect("automatic classification");
        let archive = GraphqlSemanticOperationDescriptor::custom(
            GraphqlOperationKind::Mutation,
            "ArchiveRecord",
            "Archive one reviewed application record.",
            vec![GraphqlSemanticArgumentDescriptor {
                graphql_name: "recordId".to_owned(),
                description: "Reviewed record identifier.".to_owned(),
                type_ref: GraphqlSemanticTypeRef::named(
                    "ID",
                    GraphqlSemanticTypeKind::Scalar,
                    false,
                ),
            }],
            record,
            true,
        )
        .expect("archive semantics")
        .with_ai_mutation_execution(
            graphql_orm::graphql::orm::AiMutationExecutionPolicy::ApprovalRequired,
        )
        .expect("approval classification");
        let semantics = GraphqlSemanticCatalog::compose_with_custom(
            [entity],
            &GraphqlOperationCatalog::compose(std::iter::empty()),
            [create, archive],
        )
        .expect("mutation semantics");
        let target = crate::GraphqlExecutionTargetId::parse("generated-application")
            .expect("generated target");
        let catalogue = crate::AiGraphqlMutationCapabilityCatalog::compile_with_options(
            "generated",
            target.clone(),
            sdl,
            &semantics,
            crate::AiGraphqlQueryCapabilityLimits::default(),
            crate::AiGraphqlQueryCapabilityOptions::default().with_custom_input_disclosure(
                crate::AiCustomInputDisclosure::new()
                    .with_exportable_field("RecordInput", "subject")
                    .expect("custom input disclosure"),
            ),
        )?;
        AiCapabilityIndex::compile(
            target,
            catalogue.finished_schema_fingerprint(),
            &semantics,
            None,
            Some(&catalogue),
            None,
            [],
            target_policy,
            crate::AiCapabilityIndexLimits::default(),
        )
        .map(Arc::new)
    }

    fn mutation_rules(allowed: Option<BTreeSet<String>>) -> crate::AiResolvedRuleSet {
        let scope = crate::AiScope {
            kind: "workspace".to_owned(),
            id: "workspace-1".to_owned(),
            tenant_id: Some("tenant-1".to_owned()),
        };
        crate::AiResolvedRuleSet::new(
            scope.clone(),
            crate::AiRuleConstraints {
                enabled: true,
                maximum_classification: crate::DataClassification::Restricted,
                maximum_tool_maturity: crate::ToolMaturity::AutonomousWrite,
                approval_requirement: crate::AiRuleApprovalRequirement::DescriptorPolicy,
                allowed_tool_fingerprints: allowed,
                allowed_provider_kinds: None,
                allowed_provider_capabilities: None,
                allow_provider_retention: true,
                allow_byok: true,
                budget: crate::AiRuleBudgetCeilings::default(),
            },
            vec![crate::AiAppliedRuleLayer {
                scope,
                row_version: 1,
            }],
        )
        .expect("rules")
    }

    #[tokio::test]
    async fn broker_discovery_requires_mutations_checked_against_finished_sdl() {
        assert!(matches!(
            compile_mutation_index("target-policy-v1", true),
            Err(AiError::InvalidConfiguration(message)) if message.contains("transient")
        ));
        let principal = principal();
        let principal_reference = principal.reference();
        let index = mutation_index("target-policy-v1");
        // Transporting the index leaves only discovery metadata. It cannot
        // prove the original mutation compiler checked @transientInput.
        let decoded: AiCapabilityIndex =
            serde_json::from_value(serde_json::to_value(index.as_ref()).unwrap()).unwrap();
        let indexes = |index| Arc::new(AiCapabilityIndexSet::compile([index]).expect("index set"));
        let current = Arc::new(CurrentIndexes(RwLock::new(indexes(index))));
        let broker = AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal)),
            current.clone(),
            Arc::new(Authority {
                allowed: AtomicBool::new(true),
                policy_fingerprint: RwLock::new("current-policy-v1".to_owned()),
            }),
            Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
            Duration::seconds(30),
        )
        .expect("broker");
        let session = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
            .expect("session")
            .with_brokered_mutations()
            .bind_mutation_rules(&mutation_rules(None))
            .expect("bind rules");
        let run = run_binding();
        let search =
            json!({"text": "create record", "kind": "generated_mutation", "maximumResults": 8});

        let discovery = broker
            .dispatch_discover(&principal_reference, &run, &session, &search)
            .await
            .expect("discovery using the compiled index");
        assert!(candidate_kinds(&discovery).contains("generated_mutation"));

        *current.0.write().unwrap() = indexes(Arc::new(decoded));
        let discovery = broker
            .dispatch_discover(&principal_reference, &run, &session, &search)
            .await
            .expect("discovery using serialized metadata");
        assert!(discovery["candidates"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn generated_mutations_reach_the_broker_only_under_bound_rules_and_are_only_resolved() {
        let principal = principal();
        let principal_reference = principal.reference();
        let indexes = AiCapabilityIndexSet::compile([
            generated_index("target-policy-v1"),
            mutation_index("target-policy-v1"),
        ])
        .expect("index set");
        let entry = |operation: &str| {
            indexes
                .indexes()
                .flat_map(|index| index.entries())
                .find(|entry| {
                    entry.kind == AiCapabilityKind::GeneratedMutation
                        && entry.operation_name == operation
                })
                .expect("mutation entry")
                .clone()
        };
        let (create, archive) = (entry("CreateRecord"), entry("ArchiveRecord"));
        assert_eq!(create.approval, AiApprovalRule::None);
        assert_eq!(archive.approval, AiApprovalRule::OneShot);
        let authority = Arc::new(Authority {
            allowed: AtomicBool::new(true),
            policy_fingerprint: RwLock::new("current-policy-v1".to_owned()),
        });
        let current = Arc::new(CurrentIndexes(RwLock::new(Arc::new(indexes))));
        let broker = AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal)),
            current.clone(),
            authority.clone(),
            Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
            Duration::seconds(30),
        )
        .expect("broker");
        let run = run_binding();
        let search =
            json!({"text": "archive create reviewed application record", "maximumResults": 16});
        let mutation_search = json!({"text": "archive create record", "kind": "generated_mutation", "maximumResults": 8});

        // Default session: mutations stay outside the broker.
        let default =
            AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default()).expect("session");
        let discovery = broker
            .dispatch_discover(&principal_reference, &run, &default, &search)
            .await
            .expect("default discovery");
        assert!(!candidate_kinds(&discovery).contains("generated_mutation"));
        assert!(matches!(
            broker
                .dispatch_discover(&principal_reference, &run, &default, &mutation_search)
                .await,
            Err(AiError::InvalidInput(_))
        ));
        // Opted in but no coordinator bound rules (the read-only path):
        // nothing consequential is discoverable.
        let unbound = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
            .expect("session")
            .with_brokered_mutations();
        let discovery = broker
            .dispatch_discover(&principal_reference, &run, &unbound, &mutation_search)
            .await
            .expect("unbound discovery");
        assert!(discovery["candidates"].as_array().unwrap().is_empty());
        // Rules that admit only other fingerprints (for example the broker's
        // own) never admit a mutation.
        let narrow = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
            .expect("session")
            .with_brokered_mutations()
            .bind_mutation_rules(&mutation_rules(Some(BTreeSet::from([
                fixed_broker_definitions()[0].fingerprint.clone(),
            ]))))
            .expect("bind narrow rules");
        let discovery = broker
            .dispatch_discover(&principal_reference, &run, &narrow, &mutation_search)
            .await
            .expect("narrow discovery");
        assert!(discovery["candidates"].as_array().unwrap().is_empty());

        // Rules admitting the approval-required mutation alone. Binding
        // returns the bound session; the session it was taken from, and any
        // clone of it, stays closed.
        let opted = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
            .expect("session")
            .with_brokered_mutations();
        let unbound_clone = opted.clone();
        let rules = mutation_rules(Some(BTreeSet::from([archive
            .capability_fingerprint
            .clone()])));
        let session = opted.bind_mutation_rules(&rules).expect("bind rules");
        session
            .bind_mutation_rules(&rules)
            .expect("same rules rebind");
        assert!(matches!(
            session.bind_mutation_rules(&mutation_rules(None)),
            Err(AiError::Conflict)
        ));
        for closed in [&opted, &unbound_clone] {
            let discovery = broker
                .dispatch_discover(&principal_reference, &run, closed, &mutation_search)
                .await
                .expect("unbound clone discovery");
            assert!(
                discovery["candidates"].as_array().unwrap().is_empty(),
                "a clone never inherits another plan's rules"
            );
        }
        let discovery = broker
            .dispatch_discover(&principal_reference, &run, &session, &mutation_search)
            .await
            .expect("mutation discovery");
        let candidates = discovery["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 1, "{discovery}");
        assert_eq!(candidates[0]["capabilityId"], archive.id.as_str());
        assert_eq!(candidates[0]["kind"], "generated_mutation");
        // Unfiltered discovery covers reads and the admitted mutation.
        let mixed = broker
            .dispatch_discover(&principal_reference, &run, &session, &search)
            .await
            .expect("mixed discovery");
        assert!(candidate_kinds(&mixed).contains("generated_mutation"));
        assert!(
            !mixed.to_string().contains(create.id.as_str()),
            "a mutation the rules refuse is never listed"
        );
        // Browse lists the same admitted mutation.
        let browsed = broker
            .dispatch_browse(
                &principal_reference,
                &run,
                &session,
                &json!({"level": "capabilities", "namespace": archive.namespace}),
            )
            .await
            .expect("browse");
        assert!(
            browsed.to_string().contains(archive.id.as_str()),
            "{browsed}"
        );
        assert!(!browsed.to_string().contains(create.id.as_str()));

        let listed = browsed["capabilities"]
            .as_array()
            .expect("capabilities")
            .iter()
            .find(|candidate| candidate["capabilityId"] == archive.id.as_str())
            .expect("browsed mutation")
            .clone();
        let describe = json!({
            "capabilityId": archive.id.as_str(),
            "candidateReference": listed["candidateReference"],
        });
        // The same candidate in a session whose rules refuse it is stale,
        // even with its search result and reference at hand.
        narrow.record_search(session.issued_search_holding(&archive.id));
        assert!(matches!(
            broker
                .dispatch_describe(&principal_reference, &run, &narrow, &describe)
                .await,
            Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_SELECTION_STALE
        ));
        let description = broker
            .dispatch_describe(&principal_reference, &run, &session, &describe)
            .await
            .expect("mutation description");
        assert_eq!(
            description.capability_kind(),
            AiCapabilityKind::GeneratedMutation
        );
        assert_eq!(description.contract()["effect"], "approval_required");
        assert_eq!(
            description.contract()["approval"],
            json!(AiApprovalRule::OneShot)
        );
        assert_eq!(description.operations().len(), 1);
        let loaded_reference = description.loaded_reference().to_owned();
        assert!(session.holds_loaded_mutation(&json!({"loadedReference": loaded_reference})));
        assert!(!default.holds_loaded_mutation(&json!({"loadedReference": loaded_reference})));

        let execute = json!({
            "loadedReference": loaded_reference,
            "arguments": [{"name": "recordId", "value": "record-7"}],
            "selections": ["recordId", "subject"]
        });
        let execution = broker
            .authorize_broker_execution(&principal_reference, &run, &session, &execute)
            .await
            .expect("mutation resolution");
        assert!(execution.is_consequential());
        assert_eq!(execution.capability_id(), &archive.id);
        assert_eq!(
            execution.capability_fingerprint(),
            archive.capability_fingerprint
        );
        assert_eq!(
            execution.plan()["arguments"],
            json!({"recordId": "record-7"})
        );
        // A mutation is never continued.
        assert!(matches!(
            broker
                .authorize_broker_execution(
                    &principal_reference,
                    &run,
                    &session,
                    &json!({"loadedReference": loaded_reference, "continuation": "a".repeat(64)}),
                )
                .await,
            Err(AiError::Forbidden)
        ));
        // The loaded binding in a run whose rules no longer admit the
        // mutation does not resolve.
        let revoked_rules = AiCapabilityBrokerSession {
            inner: Arc::clone(&session.inner),
            limits: session.limits,
            brokered_static_tools: Arc::new(BTreeSet::new()),
            inline_describe_schema: false,
            brokered_mutations: true,
            mutation_rules: Some(Arc::new(mutation_rules(Some(BTreeSet::new())))),
            brokered_subscriptions: false,
        };
        // An unbound clone sharing this run's broker state still describes
        // no mutation.
        assert!(matches!(
            broker
                .dispatch_describe(&principal_reference, &run, &opted, &describe)
                .await,
            Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_SELECTION_STALE
        ));
        // A session whose rules do not admit the loaded mutation (revoked
        // rules, rules admitting only other fingerprints, no bound rules, or
        // no mutation brokering at all) answers exactly as for an unknown
        // reference, with or without a continuation.
        let unknown = "f".repeat(64);
        assert_ne!(unknown, loaded_reference);
        let calls = |reference: &str| {
            [
                json!({
                    "loadedReference": reference,
                    "arguments": [{"name": "recordId", "value": "record-7"}],
                    "selections": ["recordId", "subject"]
                }),
                json!({"loadedReference": reference, "continuation": "a".repeat(64)}),
            ]
        };
        // Every refusing session below shares this run's loaded state, so
        // each holds the reference.
        let narrow_shared = AiCapabilityBrokerSession {
            mutation_rules: Some(Arc::new(mutation_rules(Some(BTreeSet::from([
                fixed_broker_definitions()[0].fingerprint.clone(),
            ]))))),
            ..session.clone()
        };
        let default_shared = AiCapabilityBrokerSession {
            brokered_mutations: false,
            mutation_rules: None,
            ..session.clone()
        };
        assert!(Arc::ptr_eq(&opted.inner, &session.inner));
        for (label, refusing) in [
            ("revoked rules", &revoked_rules),
            ("narrow rules", &narrow_shared),
            ("unbound rules", &opted),
            ("default session", &default_shared),
        ] {
            assert!(refusing.loaded(&loaded_reference).is_some(), "{label}");
            describe_type_matches_unknown(
                &broker,
                &principal_reference,
                &run,
                refusing,
                &loaded_reference,
                label,
            )
            .await;
            for (held, unknown_call) in calls(&loaded_reference).into_iter().zip(calls(&unknown)) {
                let held = broker
                    .authorize_broker_execution(&principal_reference, &run, refusing, &held)
                    .await;
                let unknown = broker
                    .authorize_broker_execution(&principal_reference, &run, refusing, &unknown_call)
                    .await;
                assert!(
                    matches!(&held, Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_SELECTION_STALE),
                    "{label}: {held:?}"
                );
                assert_eq!(
                    format!("{:?}", held.err()),
                    format!("{:?}", unknown.err()),
                    "{label}"
                );
            }
        }
        // Host policy revoked between describe and execute refuses.
        authority.allowed.store(false, Ordering::SeqCst);
        assert!(matches!(
            broker
                .authorize_broker_execution(&principal_reference, &run, &session, &execute)
                .await,
            Err(AiError::Forbidden)
        ));
        authority.allowed.store(true, Ordering::SeqCst);

        // After a catalogue change, a held mutation candidate the turn's
        // rules refuse is the same stale selection as before; only a turn
        // that may reach it learns of the drift.
        *current.0.write().unwrap() = Arc::new(
            AiCapabilityIndexSet::compile([generated_index("target-policy-v1")])
                .expect("index set"),
        );
        assert!(matches!(
            broker
                .dispatch_describe(&principal_reference, &run, &narrow, &describe)
                .await,
            Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_SELECTION_STALE
        ));
        assert!(matches!(
            broker
                .dispatch_describe(&principal_reference, &run, &session, &describe)
                .await,
            Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_STALE
        ));
    }

    /// An index over one replayable `ParentChanged` subscription, compiled in
    /// process from its typed catalogue. With `transient_argument` the root's
    /// argument is marked `@transientInput`.
    fn compile_subscription_index(
        transient_argument: bool,
    ) -> Result<Arc<AiCapabilityIndex>, AiError> {
        let sdl = "schema { query: Query, subscription: Subscription }
            type Query { health: Boolean! }
            type Subscription { ParentChanged(id: ID): Parent! }
            type Parent { id: String!, name: String! }";
        let marked_sdl;
        let sdl = if transient_argument {
            marked_sdl = format!(
                "directive @transientInput on ARGUMENT_DEFINITION\n{}",
                sdl.replace("(id: ID)", "(id: ID @transientInput)")
            );
            marked_sdl.as_str()
        } else {
            sdl
        };
        let scalar = |name: &str| GraphqlSemanticFieldMetadata {
            field_name: name.to_owned(),
            description: format!("Reviewed public {name}."),
            type_ref: GraphqlSemanticTypeRef::named(
                "String",
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
            has_field_policy: true,
        };
        let entity = GraphqlEntitySemanticMetadata {
            generated_input_members: Vec::new(),
            entity_name: "Parent".to_owned(),
            description: "One reviewed parent event.".to_owned(),
            default_classification: GraphqlSemanticClassification::Internal,
            fields: vec![scalar("id"), scalar("name")].into_boxed_slice(),
            label_fields: Vec::new(),
            record_version: None,
        };
        let operation = GraphqlSemanticOperationDescriptor::custom(
            GraphqlOperationKind::Subscription,
            "ParentChanged",
            "Observe reviewed parent changes.",
            vec![GraphqlSemanticArgumentDescriptor {
                graphql_name: "id".to_owned(),
                description: "Optional parent identity.".to_owned(),
                type_ref: GraphqlSemanticTypeRef::named(
                    "ID",
                    GraphqlSemanticTypeKind::Scalar,
                    true,
                ),
            }],
            GraphqlSemanticTypeRef::named("Parent", GraphqlSemanticTypeKind::Object, false),
            true,
        )
        .expect("subscription semantics")
        .with_subscription_observation(
            graphql_orm::graphql::orm::GraphqlSubscriptionObservationDescriptor {
                replay_mode:
                    graphql_orm::graphql::orm::GraphqlSubscriptionReplayMode::ReplayThenLive,
                maximum_duration_seconds: Some(120),
                maximum_events: Some(20),
                condition_fields: vec![
                    graphql_orm::graphql::orm::GraphqlSubscriptionConditionField {
                        field_name: "id".to_owned(),
                        operators: vec![
                            graphql_orm::graphql::orm::GraphqlSubscriptionConditionOperator::Equal,
                        ],
                    },
                ],
            },
        )
        .expect("replayable subscription semantics");
        let semantics = GraphqlSemanticCatalog::compose_with_custom(
            [entity],
            &GraphqlOperationCatalog::compose(std::iter::empty()),
            [operation],
        )
        .expect("subscription semantic catalogue");
        let target =
            crate::GraphqlExecutionTargetId::parse("generated-events").expect("generated target");
        let catalogue = crate::AiGraphqlSubscriptionCapabilityCatalog::compile(
            "events",
            target.clone(),
            sdl,
            &semantics,
            crate::AiGraphqlSubscriptionCapabilityLimits::default(),
        )?;
        AiCapabilityIndex::compile(
            target,
            catalogue.finished_schema_fingerprint(),
            &semantics,
            None,
            None,
            Some(&catalogue),
            [],
            "target-policy-v1",
            crate::AiCapabilityIndexLimits::default(),
        )
        .map(Arc::new)
    }

    #[tokio::test]
    async fn generated_subscriptions_reach_the_broker_only_when_opted_in_and_are_only_resolved() {
        // The typed subscription compiler refuses a root that takes a
        // transient argument, so such a root is never in an index.
        assert!(matches!(
            compile_subscription_index(true),
            Err(AiError::InvalidConfiguration(message)) if message.contains("transient")
        ));
        let principal = principal();
        let principal_reference = principal.reference();
        let index = compile_subscription_index(false).expect("subscription index");
        assert!(index.subscriptions_checked_against_finished_schema());
        assert!(!index.mutations_checked_against_finished_schema());
        let entry = index
            .entries()
            .find(|entry| entry.kind == AiCapabilityKind::GeneratedSubscription)
            .expect("subscription entry")
            .clone();
        let decoded: AiCapabilityIndex =
            serde_json::from_value(serde_json::to_value(index.as_ref()).unwrap()).unwrap();
        assert!(!decoded.subscriptions_checked_against_finished_schema());
        let indexes = |index| Arc::new(AiCapabilityIndexSet::compile([index]).expect("index set"));
        let current = Arc::new(CurrentIndexes(RwLock::new(indexes(index.clone()))));
        let authority = Arc::new(Authority {
            allowed: AtomicBool::new(true),
            policy_fingerprint: RwLock::new("current-policy-v1".to_owned()),
        });
        let broker = AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal)),
            current.clone(),
            authority.clone(),
            Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
            Duration::seconds(30),
        )
        .expect("broker");
        let run = run_binding();
        let search = json!({"text": "parent changed", "maximumResults": 8});
        let subscription_search = json!({"text": "parent changed", "kind": "generated_subscription", "maximumResults": 8});

        // Default session: subscriptions stay outside the broker.
        let default =
            AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default()).expect("session");
        let discovery = broker
            .dispatch_discover(&principal_reference, &run, &default, &search)
            .await
            .expect("default discovery");
        assert!(!candidate_kinds(&discovery).contains("generated_subscription"));
        assert!(matches!(
            broker
                .dispatch_discover(&principal_reference, &run, &default, &subscription_search)
                .await,
            Err(AiError::InvalidInput(_))
        ));

        // Opted in: discover, browse and describe it.
        let session = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default())
            .expect("session")
            .with_brokered_subscriptions();
        assert!(session.brokers_subscriptions() && !default.brokers_subscriptions());
        let discovery = broker
            .dispatch_discover(&principal_reference, &run, &session, &subscription_search)
            .await
            .expect("subscription discovery");
        let candidates = discovery["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 1, "{discovery}");
        assert_eq!(candidates[0]["capabilityId"], entry.id.as_str());
        assert_eq!(candidates[0]["kind"], "generated_subscription");
        let mixed = broker
            .dispatch_discover(&principal_reference, &run, &session, &search)
            .await
            .expect("unfiltered discovery");
        assert!(candidate_kinds(&mixed).contains("generated_subscription"));
        let browsed = broker
            .dispatch_browse(
                &principal_reference,
                &run,
                &session,
                &json!({"level": "capabilities", "namespace": entry.namespace}),
            )
            .await
            .expect("browse");
        assert!(browsed.to_string().contains(entry.id.as_str()), "{browsed}");
        let browsed_default = broker
            .dispatch_browse(
                &principal_reference,
                &run,
                &default,
                &json!({"level": "capabilities", "namespace": entry.namespace}),
            )
            .await
            .expect("default browse");
        assert!(!browsed_default.to_string().contains(entry.id.as_str()));
        let describe = json!({
            "capabilityId": candidates[0]["capabilityId"],
            "candidateReference": candidates[0]["candidateReference"],
        });
        // A session that did not opt in cannot load the candidate even with
        // its reference at hand.
        default.record_search(session.issued_search_holding(&entry.id));
        assert!(matches!(
            broker
                .dispatch_describe(&principal_reference, &run, &default, &describe)
                .await,
            Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_SELECTION_STALE
        ));
        let description = broker
            .dispatch_describe(&principal_reference, &run, &session, &describe)
            .await
            .expect("subscription description");
        assert_eq!(
            description.capability_kind(),
            AiCapabilityKind::GeneratedSubscription
        );
        assert_eq!(description.contract()["effect"], "wait");
        let loaded_reference = description.loaded_reference().to_owned();
        let loaded = json!({"loadedReference": loaded_reference});
        assert!(session.holds_loaded_subscription(&loaded));
        assert!(!session.holds_loaded_mutation(&loaded));
        assert!(!default.holds_loaded_subscription(&loaded));

        // Execute only resolves, and only with an observation.
        let observation = json!({
            "timeoutSeconds": 60,
            "maximumEvents": 2,
            "condition": {"field": "id", "operator": "equal", "value": "wanted"}
        });
        let execute = json!({
            "loadedReference": loaded_reference,
            "arguments": [{"name": "id", "value": "wanted"}],
            "selections": ["id", "name"],
            "observation": observation,
        });
        for malformed in [
            json!({"loadedReference": loaded_reference, "selections": ["id"]}),
            json!({
                "loadedReference": loaded_reference,
                "selections": ["id"],
                "maximumItems": 5,
                "observation": observation,
            }),
            json!({
                "loadedReference": loaded_reference,
                "selections": ["id"],
                "observation": {"timeoutSeconds": 0, "maximumEvents": 2},
            }),
        ] {
            assert!(matches!(
                broker
                    .authorize_broker_execution(&principal_reference, &run, &session, &malformed)
                    .await,
                Err(AiError::InvalidInput(_))
            ));
        }
        let execution = broker
            .authorize_broker_execution(&principal_reference, &run, &session, &execute)
            .await
            .expect("subscription resolution");
        assert!(execution.is_subscription_wait());
        assert!(!execution.is_consequential());
        assert_eq!(execution.capability_id(), &entry.id);
        assert_eq!(
            execution.capability_fingerprint(),
            entry.capability_fingerprint
        );
        assert_eq!(
            execution.plan(),
            &json!({
                "arguments": {"id": "wanted"},
                "selections": ["id", "name"],
                "relationshipArguments": {},
                "relationshipMaximumItems": {},
                "timeoutSeconds": 60,
                "maximumEvents": 2,
                "condition": {"field": "id", "operator": "equal", "value": "wanted"},
            })
        );
        // A subscription resolution has no continuation chain, and a
        // continuation is never issued or redeemed for it.
        assert!(execution.chain.is_none());
        for continued in [
            json!({"loadedReference": loaded_reference, "continuation": "a".repeat(64)}),
            json!({
                "loadedReference": loaded_reference,
                "continuation": "a".repeat(64),
                "observation": observation,
            }),
        ] {
            assert!(
                broker
                    .authorize_broker_execution(&principal_reference, &run, &session, &continued)
                    .await
                    .is_err()
            );
        }
        // The loaded reference does not resolve for a session that did not
        // opt in, even when it shares this run's broker state.
        // A clone that did not opt in shares this run's loaded state, but a
        // subscription reference it holds is exactly an unknown reference to
        // it, with or without a continuation or an observation.
        let closed = AiCapabilityBrokerSession {
            brokered_subscriptions: false,
            ..session.clone()
        };
        let unknown = "f".repeat(64);
        assert_ne!(unknown, loaded_reference);
        for (reference, arguments) in [
            (&loaded_reference, execute.clone()),
            (
                &loaded_reference,
                json!({"loadedReference": loaded_reference, "continuation": "a".repeat(64)}),
            ),
            (
                &loaded_reference,
                json!({"loadedReference": loaded_reference, "selections": ["id"]}),
            ),
            (
                &unknown,
                json!({"loadedReference": unknown, "selections": ["id"], "observation": observation}),
            ),
        ] {
            let refused = broker
                .authorize_broker_execution(&principal_reference, &run, &closed, &arguments)
                .await;
            assert!(
                matches!(&refused, Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_SELECTION_STALE),
                "{reference}: {refused:?}"
            );
        }
        describe_type_matches_unknown(
            &broker,
            &principal_reference,
            &run,
            &closed,
            &loaded_reference,
            "subscription in a closed clone",
        )
        .await;
        // Host policy revoked between describe and execute refuses.
        authority.allowed.store(false, Ordering::SeqCst);
        assert!(matches!(
            broker
                .authorize_broker_execution(&principal_reference, &run, &session, &execute)
                .await,
            Err(AiError::Forbidden)
        ));
        let hidden = broker
            .dispatch_discover(&principal_reference, &run, &session, &subscription_search)
            .await
            .expect("discovery under revoked policy");
        assert!(hidden["candidates"].as_array().unwrap().is_empty());
        authority.allowed.store(true, Ordering::SeqCst);

        // Serialized index metadata cannot prove the subscription compiler
        // checked @transientInput: nothing is admitted from it.
        *current.0.write().unwrap() = indexes(Arc::new(decoded));
        let discovery = broker
            .dispatch_discover(&principal_reference, &run, &session, &subscription_search)
            .await
            .expect("discovery using serialized metadata");
        assert!(discovery["candidates"].as_array().unwrap().is_empty());
        // A held reference the session no longer reaches there is exactly an
        // unknown reference.
        let held = broker
            .authorize_broker_execution(&principal_reference, &run, &session, &execute)
            .await;
        let unknown = broker
            .authorize_broker_execution(
                &principal_reference,
                &run,
                &session,
                &json!({"loadedReference": "f".repeat(64), "selections": ["id"], "observation": observation}),
            )
            .await;
        assert!(
            matches!(&held, Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_SELECTION_STALE),
            "{held:?}"
        );
        assert_eq!(format!("{:?}", held.err()), format!("{:?}", unknown.err()));
        // A continuation of that reference consults the current index first
        // and answers the same, never a distinguishable refusal.
        let continued =
            |reference: &str| json!({"loadedReference": reference, "continuation": "a".repeat(64)});
        let held = broker
            .authorize_broker_execution(
                &principal_reference,
                &run,
                &session,
                &continued(&loaded_reference),
            )
            .await;
        let unknown = broker
            .authorize_broker_execution(
                &principal_reference,
                &run,
                &session,
                &continued(&"f".repeat(64)),
            )
            .await;
        assert!(
            matches!(&held, Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_SELECTION_STALE),
            "{held:?}"
        );
        assert_eq!(format!("{:?}", held.err()), format!("{:?}", unknown.err()));
        describe_type_matches_unknown(
            &broker,
            &principal_reference,
            &run,
            &session,
            &loaded_reference,
            "subscription whose index lost its proof",
        )
        .await;
        // Shape is validated only after reachability: a continuation that
        // also carries plan fields answers as an unknown reference.
        let malformed = |reference: &str| {
            json!({
                "loadedReference": reference,
                "continuation": "a".repeat(64),
                "arguments": [{"name": "id", "value": "wanted"}],
                "selections": ["id"],
                "observation": observation,
            })
        };
        let held = broker
            .authorize_broker_execution(
                &principal_reference,
                &run,
                &session,
                &malformed(&loaded_reference),
            )
            .await;
        let unknown = broker
            .authorize_broker_execution(
                &principal_reference,
                &run,
                &session,
                &malformed(&"f".repeat(64)),
            )
            .await;
        assert!(
            matches!(&held, Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_SELECTION_STALE),
            "{held:?}"
        );
        assert_eq!(format!("{:?}", held.err()), format!("{:?}", unknown.err()));
        // Reachability is decided before the principal is resolved: with a
        // principal that no longer resolves, the held reference still
        // answers exactly as an unknown one, never with its own
        // reauthorization outcome.
        let unresolvable = AiCapabilityDiscoveryBroker::new(
            Arc::new(UnresolvablePrincipal),
            current.clone(),
            authority.clone(),
            Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
            Duration::seconds(30),
        )
        .expect("broker");
        for call in [
            execute.clone(),
            json!({"loadedReference": loaded_reference, "continuation": "a".repeat(64)}),
        ] {
            let unknown_call = {
                let mut call = call.clone();
                call["loadedReference"] = json!("f".repeat(64));
                call
            };
            let held = unresolvable
                .authorize_broker_execution(&principal_reference, &run, &session, &call)
                .await;
            let unknown = unresolvable
                .authorize_broker_execution(&principal_reference, &run, &session, &unknown_call)
                .await;
            assert!(
                matches!(&held, Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_SELECTION_STALE),
                "{held:?}"
            );
            assert_eq!(format!("{:?}", held.err()), format!("{:?}", unknown.err()));
        }
        describe_type_matches_unknown(
            &unresolvable,
            &principal_reference,
            &run,
            &session,
            &loaded_reference,
            "unresolvable principal",
        )
        .await;

        let failing = Arc::new(FailingIndex::default());
        let broker_with_failed_index = AiCapabilityDiscoveryBroker::new(
            Arc::new(UnresolvablePrincipal),
            failing.clone(),
            authority,
            Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
            Duration::seconds(30),
        )
        .expect("broker");
        for reference in [&loaded_reference, &"f".repeat(64)] {
            for call in [
                json!({"loadedReference": reference, "operation": "noSuchOperation"}),
                json!({"loadedReference": reference, "continuation": "a".repeat(64)}),
            ] {
                let before = failing.0.load(Ordering::SeqCst);
                assert!(matches!(
                    broker_with_failed_index
                        .authorize_broker_execution(&principal_reference, &run, &session, &call)
                        .await,
                    Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_SELECTION_STALE
                ));
                assert_eq!(failing.0.load(Ordering::SeqCst), before + 1);
            }
            let before = failing.0.load(Ordering::SeqCst);
            assert!(matches!(
                broker_with_failed_index
                    .dispatch_describe_type(
                        &principal_reference, &run, &session,
                        &json!({"loadedReference": reference, "type": "Parent", "operation": "noSuchOperation"}),
                    )
                    .await,
                Err(AiError::InvalidInput(message)) if message == AI_CAPABILITY_SELECTION_STALE
            ));
            assert_eq!(failing.0.load(Ordering::SeqCst), before + 1);
        }
    }

    /// An index source that makes host calls observable and always fails.
    #[derive(Default)]
    struct FailingIndex(std::sync::atomic::AtomicUsize);

    impl AiCurrentCapabilityIndexSet for FailingIndex {
        fn current_index_set(
            &self,
            _run: &AiCapabilityRunBinding,
        ) -> Result<Arc<AiCapabilityIndexSet>, AiError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(AiError::PersistenceFailed)
        }
    }

    /// A principal resolver that no longer resolves anyone.
    struct UnresolvablePrincipal;

    #[async_trait]
    impl CurrentPrincipalResolver for UnresolvablePrincipal {
        async fn resolve(
            &self,
            _reference: &PrincipalReference,
        ) -> agql_auth::AuthResult<ResolvedPrincipal> {
            Err(agql_auth::AuthError::Forbidden)
        }
    }

    #[test]
    fn only_reviewed_automatic_and_approval_required_mutations_have_a_broker_binding() {
        assert_eq!(
            brokered_mutation_binding(AiApprovalRule::OneShot),
            Some((
                crate::ToolMaturity::SupervisedWrite,
                AiApprovalRule::OneShot
            ))
        );
        assert_eq!(
            brokered_mutation_binding(AiApprovalRule::None),
            Some((crate::ToolMaturity::AutonomousWrite, AiApprovalRule::None))
        );
        assert_eq!(brokered_mutation_binding(AiApprovalRule::Never), None);
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
        assert_eq!(delivery.current_tools().len(), 6);
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
        clock: FixedClock,
        principal: PrincipalReference,
        run: AiCapabilityRunBinding,
        authority: Arc<RootAuthority>,
        index: Arc<CurrentIndex>,
        session: AiCapabilityBrokerSession,
    }

    impl EntityBrokerFixture {
        fn new() -> Self {
            Self::with_principal(principal(), two_root_entity_index("target-policy-v1"))
        }

        fn with_principal(principal: AuthPrincipal, index: Arc<AiCapabilityIndex>) -> Self {
            let reference = principal.reference();
            let authority = Arc::new(RootAuthority {
                denied: RwLock::new(BTreeSet::new()),
                policy_fingerprint: RwLock::new("current-policy-v1".to_owned()),
            });
            let index = Arc::new(CurrentIndex(RwLock::new(index)));
            let clock = FixedClock::new(OffsetDateTime::UNIX_EPOCH);
            let broker = AiCapabilityDiscoveryBroker::new(
                Arc::new(Resolver(principal)),
                index.clone(),
                authority.clone(),
                Arc::new(clock.clone()),
                Duration::seconds(30),
            )
            .expect("broker");
            Self {
                broker,
                clock,
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
                        "candidateReference": candidate["candidateReference"],
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
        assert_eq!(discovered["version"], 3);
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
        assert_eq!(contract["version"], 3);
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
        // With every root denied the entity is neither discovered nor, from a
        // candidate the run already holds, loadable.
        let held = fixture.discover().await["candidates"][0].clone();
        fixture
            .authority
            .denied
            .write()
            .expect("policy")
            .insert(fixture.root_id("RecordBySubject").as_str().to_owned());
        assert_eq!(fixture.discover().await["candidates"], json!([]));
        assert!(matches!(
            fixture
                .broker
                .dispatch_describe(
                    &fixture.principal,
                    &fixture.run,
                    &fixture.session,
                    &json!({
                        "capabilityId": held["capabilityId"],
                        "candidateReference": held["candidateReference"],
                    }),
                )
                .await,
            Err(AiError::Forbidden)
        ));
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
                AI_CAPABILITY_BROWSE_TOOL_ID,
                AI_CAPABILITY_DESCRIBE_TOOL_ID,
                AI_CAPABILITY_DESCRIBE_TYPE_TOOL_ID,
                AI_CAPABILITY_EXECUTE_TOOL_ID,
            ]
        );
        for definition in &definitions {
            for earlier in [1, 2] {
                let earlier_fingerprint = hash_json(&json!({
                    "version": 2,
                    "tool_id": definition.tool_id,
                    "broker_version": earlier,
                    "parameters": definition.parameters,
                }));
                assert_ne!(definition.fingerprint, earlier_fingerprint);
            }
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

    impl EntityBrokerFixture {
        fn deny(&self, operation: &str) {
            let id = self.root_id(operation);
            self.authority
                .denied
                .write()
                .expect("policy")
                .insert(id.as_str().to_owned());
        }

        fn entity_id(&self) -> AiToolId {
            self.index
                .0
                .read()
                .expect("index")
                .entries()
                .find(|entry| entry.kind == AiCapabilityKind::GeneratedEntity)
                .expect("entity entry")
                .id
                .clone()
        }

        async fn discover_text(&self, text: &str) -> serde_json::Value {
            self.broker
                .dispatch_discover(
                    &self.principal,
                    &self.run,
                    &self.session,
                    &json!({"text": text, "maximumResults": 4}),
                )
                .await
                .expect("discovery")
        }
    }

    #[tokio::test]
    async fn search_matches_legacy_ranking_after_admission() {
        let first = two_root_entity_index("target-policy-v1");
        let second = generated_index_for("other-application", "other", "target-policy-v1");
        let indexes = Arc::new(AiCapabilityIndexSet::compile([first, second]).expect("index set"));
        let principal = principal();
        let reference = principal.reference();
        let run = run_binding();
        let all = indexes.browse_matches(None, None).expect("entries");
        let first_entity = all
            .iter()
            .find(|candidate| candidate.entry().operations.len() == 2)
            .expect("two-root entity");
        let denied_root = first_entity
            .operations()
            .into_iter()
            .find(|root| root.entry().operation_name == "RecordBySubject")
            .expect("root with a unique search term")
            .entry()
            .id
            .as_str();
        let denied_entity = first_entity.entry().id.as_str();
        let admitted_ids = all
            .iter()
            .copied()
            .flat_map(|candidate| std::iter::once(candidate).chain(candidate.operations()))
            .map(|candidate| candidate.entry().id.as_str())
            .collect::<BTreeSet<_>>();
        let current = Arc::new(CurrentIndexes(RwLock::new(indexes.clone())));
        for (text, entity_or_operation, maximum_results, match_count) in [
            ("record", None, 2, 2),
            ("heading", None, 1, 1),
            ("record", Some("RecordBySubject"), 1, 1),
            ("unmatchedword", None, 2, 0),
        ] {
            let query = AiCapabilitySearchQuery {
                text: text.to_owned(),
                namespace: None,
                kind: Some(AiCapabilityKind::GeneratedEntity),
                entity_or_operation: entity_or_operation.map(str::to_owned),
                maximum_results,
            };
            let ranked = indexes.ranked_matches(&query).expect("ranked matches");
            assert_eq!(ranked.len(), match_count);
            let expected_calls: usize = ranked
                .iter()
                .map(|candidate| 1 + candidate.operations().len())
                .sum();
            for scope in [
                admitted_ids.clone(),
                admitted_ids
                    .difference(&BTreeSet::from([denied_root]))
                    .copied()
                    .collect(),
                admitted_ids
                    .difference(&BTreeSet::from([denied_entity]))
                    .copied()
                    .collect(),
                BTreeSet::new(),
            ] {
                let admitted = ranked
                    .iter()
                    .filter_map(|&candidate| {
                        let roots = candidate
                            .operations()
                            .iter()
                            .filter(|root| scope.contains(root.entry().id.as_str()))
                            .map(|root| root.entry().id.clone())
                            .collect::<BTreeSet<_>>();
                        (scope.contains(candidate.entry().id.as_str()) && !roots.is_empty())
                            .then_some((candidate, roots))
                    })
                    .collect::<Vec<_>>();
                let legacy = indexes
                    .rank_admitted(&query, &admitted)
                    .expect("legacy ranking")
                    .into_iter()
                    .take(usize::from(maximum_results))
                    .map(|position| {
                        let (candidate, roots) = &admitted[position];
                        (
                            candidate.admitted_candidate(roots),
                            visible_fingerprint(candidate.entry(), roots),
                        )
                    })
                    .collect::<Vec<_>>();
                let authority = Arc::new(PermittedSet::new(scope));
                let broker = AiCapabilityDiscoveryBroker::new(
                    Arc::new(Resolver(principal.clone())),
                    current.clone(),
                    authority.clone(),
                    Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
                    Duration::seconds(30),
                )
                .expect("broker");
                let (result, visible) = broker
                    .search_admitted(&reference, &run, &query, false, |_, _| true)
                    .await
                    .expect("authorized search");
                assert_eq!(
                    result
                        .candidates
                        .into_iter()
                        .zip(visible)
                        .collect::<Vec<_>>(),
                    legacy
                );
                assert_eq!(authority.calls.load(Ordering::SeqCst), expected_calls);
            }
        }
    }

    #[derive(Clone, Copy)]
    enum LaterBatch {
        Aligned,
        MissingDecision,
        WrongId,
        PolicyError,
    }

    struct GroupedBatchAuthority {
        allowed: BTreeSet<AiToolId>,
        batches: Mutex<Vec<Vec<AiToolId>>>,
        later_batch: LaterBatch,
    }

    #[async_trait]
    impl AiCapabilityAuthorityPolicy for GroupedBatchAuthority {
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
            let mut batches = self.batches.lock().expect("batches");
            batches.push(matches.iter().map(|item| item.entry().id.clone()).collect());
            let mut decisions = matches
                .iter()
                .map(|item| {
                    AiCapabilityDiscoveryDecision::for_match(
                        item,
                        self.allowed.contains(&item.entry().id),
                    )
                })
                .collect::<Vec<_>>();
            if batches.len() == 2 {
                match self.later_batch {
                    LaterBatch::Aligned => {}
                    LaterBatch::MissingDecision => {
                        decisions.pop();
                    }
                    LaterBatch::WrongId => decisions.swap(0, 1),
                    LaterBatch::PolicyError => {
                        return Err(AiError::InvalidInput("policy unavailable".to_owned()));
                    }
                }
            }
            Ok(decisions)
        }
    }

    #[tokio::test]
    async fn grouped_discovery_preserves_policy_batches_before_top_k() {
        let indexes = Arc::new(
            AiCapabilityIndexSet::compile([
                two_root_entity_index("target-policy-v1"),
                generated_index_with_schema(
                    "other-application",
                    "other",
                    "target-policy-v1",
                    None,
                    true,
                ),
                generated_index_with_schema(
                    "third-application",
                    "third",
                    "target-policy-v1",
                    None,
                    true,
                ),
            ])
            .expect("index set"),
        );
        let query = AiCapabilitySearchQuery {
            text: "record".to_owned(),
            namespace: None,
            kind: Some(AiCapabilityKind::GeneratedEntity),
            entity_or_operation: None,
            maximum_results: 1,
        };
        let ranked = indexes.ranked_matches(&query).expect("ranking");
        assert_eq!(ranked.len(), 3);
        let grouped_ids = ranked
            .iter()
            .map(|candidate| {
                std::iter::once(candidate.entry().id.clone())
                    .chain(
                        candidate
                            .entry()
                            .operations
                            .iter()
                            .map(|root| root.capability_id.clone()),
                    )
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert!(grouped_ids.iter().all(|group| group.len() == 3));
        let expected_batches = [grouped_ids[..2].concat(), grouped_ids[2].clone()];
        let principal = principal();
        let reference = principal.reference();
        let run = run_binding();
        for (allowed_position, later_batch) in [
            (Some(2), LaterBatch::Aligned),
            (Some(1), LaterBatch::Aligned),
            (None, LaterBatch::Aligned),
            (Some(0), LaterBatch::MissingDecision),
            (Some(0), LaterBatch::WrongId),
            (Some(0), LaterBatch::PolicyError),
        ] {
            let allowed = allowed_position
                .map(|position: usize| {
                    BTreeSet::from([
                        grouped_ids[position][0].clone(),
                        grouped_ids[position][2].clone(),
                    ])
                })
                .unwrap_or_default();
            let authority = Arc::new(GroupedBatchAuthority {
                allowed,
                batches: Mutex::new(Vec::new()),
                later_batch,
            });
            let broker = AiCapabilityDiscoveryBroker::new(
                Arc::new(Resolver(principal.clone())),
                Arc::new(CurrentIndexes(RwLock::new(indexes.clone()))),
                authority.clone(),
                Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
                Duration::seconds(30),
            )
            .expect("broker")
            .with_maximum_discovery_authorizations(6)
            .expect("bound");

            let result = broker
                .search_admitted(&reference, &run, &query, false, |_, _| true)
                .await;
            assert_eq!(
                *authority.batches.lock().expect("batches"),
                expected_batches
            );
            match later_batch {
                LaterBatch::Aligned => {
                    let (result, visible) = result.expect("admitted");
                    if let Some(position) = allowed_position {
                        let candidate = ranked[position];
                        let roots = BTreeSet::from([grouped_ids[position][2].clone()]);
                        assert_eq!(result.candidates, [candidate.admitted_candidate(&roots)]);
                        assert_eq!(visible, [visible_fingerprint(candidate.entry(), &roots)]);
                    } else {
                        assert!(result.candidates.is_empty());
                        assert!(visible.is_empty());
                    }
                }
                LaterBatch::MissingDecision | LaterBatch::WrongId => {
                    assert!(matches!(result, Err(AiError::Forbidden)));
                }
                LaterBatch::PolicyError => {
                    assert_input_error(&result.expect_err("policy error"), "policy unavailable");
                }
            }
        }
    }

    #[tokio::test]
    async fn discover_lists_and_ranks_only_admitted_roots() {
        let fixture = EntityBrokerFixture::new();
        // `heading` appears only in RecordBySubject's description.
        let found = fixture.discover_text("subject heading").await;
        assert_eq!(found["candidates"].as_array().map(Vec::len), Some(1));
        let only_heading = fixture.discover_text("heading").await;
        assert_eq!(only_heading["candidates"].as_array().map(Vec::len), Some(1));

        fixture.deny("RecordBySubject");
        // The denied root is not listed.
        let discovered = fixture.discover_text("reviewed application record").await;
        assert_eq!(
            discovered["candidates"][0]["operations"],
            json!([{"operation": "GeneratedRecord", "shape": "custom"}]),
            "{discovered}"
        );
        // A term only the denied root's description holds no longer surfaces
        // the entity.
        let hidden = fixture.discover_text("heading").await;
        assert_eq!(hidden["candidates"], json!([]), "{hidden}");
        // Nor does the denied root's operation name.
        let by_name = fixture
            .broker
            .dispatch_discover(
                &fixture.principal,
                &fixture.run,
                &fixture.session,
                &json!({"text": "record", "entityOrClass": "RecordBySubject", "maximumResults": 4}),
            )
            .await
            .expect("discovery");
        assert_eq!(by_name["candidates"], json!([]), "{by_name}");

        // An entity whose roots are all denied is not listed at all.
        fixture.deny("GeneratedRecord");
        let none = fixture.discover_text("reviewed application record").await;
        assert_eq!(none["candidates"], json!([]), "{none}");

        // An entity the policy denies hides its admitted roots too.
        fixture.authority.denied.write().expect("policy").clear();
        let entity = fixture.entity_id();
        fixture
            .authority
            .denied
            .write()
            .expect("policy")
            .insert(entity.as_str().to_owned());
        let denied_entity = fixture.discover_text("reviewed application record").await;
        assert_eq!(denied_entity["candidates"], json!([]), "{denied_entity}");
    }

    #[tokio::test]
    async fn describe_shapes_and_cost_count_only_admitted_roots() {
        let fixture = EntityBrokerFixture::new();
        fixture.deny("RecordBySubject");
        let described = fixture.describe().await.expect("describe");
        let index = fixture.index.0.read().expect("index").clone();
        let entity = index
            .entries()
            .find(|entry| entry.kind == AiCapabilityKind::GeneratedEntity)
            .expect("entity")
            .clone();
        let admitted = BTreeSet::from([fixture.root_id("GeneratedRecord")]);
        let contract = described.contract();
        assert_eq!(
            contract["resultDescription"],
            entity.admitted_result_description(&admitted)
        );
        assert_eq!(
            contract["resultRecordCost"],
            json!(entity.admitted_result_record_cost(&admitted))
        );
        assert_eq!(contract["operations"].as_array().map(Vec::len), Some(1));
        assert!(!contract.to_string().contains("RecordBySubject"));
        assert!(!contract.to_string().contains("heading"));
    }

    #[tokio::test]
    async fn client_deferred_roots_need_their_entity_admitted() {
        let fixture = EntityBrokerFixture::new();
        let session =
            AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default()).expect("session");
        let discover = |session: AiCapabilityBrokerSession| {
            let fixture = &fixture;
            async move {
                fixture
                    .broker
                    .dispatch_client_deferred_discover(
                        &fixture.principal,
                        &fixture.run,
                        &session,
                        &json!({"text": "reviewed application record", "maximumResults": 4}),
                    )
                    .await
                    .map(|value| (value, session))
            }
        };
        let (allowed, session) = discover(session).await.expect("discover");
        let roots = allowed["candidates"].as_array().expect("roots");
        assert_eq!(roots.len(), 2, "{allowed}");
        assert!(roots.iter().all(|root| root["kind"] == "generated_query"));
        let loaded = session.loaded_bindings();
        assert_eq!(loaded.len(), 2);

        // Denying the entity hides its roots from per-root discovery and
        // refuses execution of a root already loaded.
        let entity = fixture.entity_id();
        fixture
            .authority
            .denied
            .write()
            .expect("policy")
            .insert(entity.as_str().to_owned());
        for binding in &loaded {
            assert!(matches!(
                fixture
                    .broker
                    .authorize_execution(&fixture.principal, &fixture.run, binding)
                    .await,
                Err(AiError::Forbidden)
            ));
        }
        let fresh =
            AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default()).expect("session");
        let (hidden, _) = discover(fresh).await.expect("discover");
        assert_eq!(hidden["candidates"], json!([]), "{hidden}");

        // A denied root is not discovered; its sibling still is.
        fixture.authority.denied.write().expect("policy").clear();
        fixture.deny("RecordBySubject");
        let fresh =
            AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits::default()).expect("session");
        let (narrowed, _) = discover(fresh).await.expect("discover");
        assert_eq!(narrowed["candidates"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            narrowed["candidates"][0]["capabilityId"],
            fixture.root_id("GeneratedRecord").as_str()
        );
    }

    impl EntityBrokerFixture {
        async fn browse(&self, arguments: serde_json::Value) -> Result<serde_json::Value, AiError> {
            self.broker
                .dispatch_browse(&self.principal, &self.run, &self.session, &arguments)
                .await
        }
    }

    #[tokio::test]
    async fn browse_lists_the_hierarchy_after_host_policy_and_feeds_describe() {
        let fixture = EntityBrokerFixture::new();
        let namespaces = fixture
            .browse(json!({"level": "namespaces"}))
            .await
            .expect("namespaces");
        assert_eq!(namespaces["version"], AI_CAPABILITY_BROKER_VERSION);
        // Browse keeps canonical fingerprints server-side.
        assert!(namespaces.get("indexFingerprint").is_none(), "{namespaces}");
        let listed = namespaces["namespaces"].as_array().expect("namespaces");
        assert_eq!(listed.len(), 1, "{namespaces}");
        // A namespace is listed without a count, which would need every
        // entry decided.
        assert_eq!(listed[0].as_object().map(|object| object.len()), Some(1));
        let namespace = listed[0]["namespace"]
            .as_str()
            .expect("namespace")
            .to_owned();

        let capabilities = fixture
            .browse(json!({"level": "capabilities", "namespace": namespace}))
            .await
            .expect("capabilities");
        let candidate = capabilities["capabilities"][0].clone();
        assert_eq!(candidate["kind"], "generated_entity", "{capabilities}");
        assert_eq!(candidate["entity"], "Record");
        assert_eq!(capabilities["nextOffset"], serde_json::Value::Null);
        assert!(capabilities.get("indexFingerprint").is_none());
        // An opaque reference replaces the entry fingerprint.
        let entity_fingerprint = fixture
            .index
            .0
            .read()
            .expect("index")
            .entries()
            .find(|entry| entry.kind == AiCapabilityKind::GeneratedEntity)
            .expect("entity")
            .fingerprint
            .clone();
        assert!(
            candidate.get("candidateFingerprint").is_none(),
            "{candidate}"
        );
        assert!(!capabilities.to_string().contains(&entity_fingerprint));
        assert!(crate::valid_sha256(
            candidate["candidateReference"].as_str().expect("reference")
        ));

        let operations = fixture
            .browse(json!({"level": "operations", "namespace": namespace, "entity": "Record"}))
            .await
            .expect("operations");
        assert_eq!(
            operations["capability"]["capabilityId"],
            candidate["capabilityId"]
        );
        // The header lists no operations: the page below is the only list.
        assert!(
            operations["capability"].get("operations").is_none(),
            "{operations}"
        );
        assert!(!operations.to_string().contains(&entity_fingerprint));
        let names = operations["operations"]
            .as_array()
            .expect("operations")
            .iter()
            .map(|operation| operation["operation"].clone())
            .collect::<Vec<_>>();
        assert_eq!(names, [json!("GeneratedRecord"), json!("RecordBySubject")]);
        assert!(operations.to_string().contains("subject heading"));

        // A browsed candidate is describable without discovery, by its
        // reference, which only this run resolves.
        let describe = |arguments: serde_json::Value, session: AiCapabilityBrokerSession| {
            let fixture = &fixture;
            async move {
                fixture
                    .broker
                    .dispatch_describe(&fixture.principal, &fixture.run, &session, &arguments)
                    .await
            }
        };
        let reference = candidate["candidateReference"].clone();
        let mut altered = reference.as_str().expect("reference").to_owned();
        let last = altered.pop().expect("reference");
        altered.push(if last == '0' { '1' } else { '0' });
        for (arguments, session) in [
            (
                json!({"capabilityId": candidate["capabilityId"], "candidateReference": altered}),
                fixture.session.clone(),
            ),
            (
                json!({"capabilityId": candidate["capabilityId"], "candidateReference": reference}),
                DiscoveryCacheFixture::session(),
            ),
        ] {
            let error = describe(arguments, session)
                .await
                .expect_err("unheld reference");
            assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
        }
        let error = describe(
            json!({
                "capabilityId": candidate["capabilityId"],
                "candidateReference": reference,
                "candidateFingerprint": entity_fingerprint,
            }),
            fixture.session.clone(),
        )
        .await
        .expect_err("both selectors");
        assert!(matches!(error, AiError::InvalidInput(_)), "{error:?}");
        let described = describe(
            json!({"capabilityId": candidate["capabilityId"], "candidateReference": reference}),
            fixture.session.clone(),
        )
        .await
        .expect("browsed candidate describes");
        assert_eq!(described.operations().len(), 2);
        assert_eq!(fixture.session.amplification().browse_calls, 3);
        assert_eq!(fixture.session.amplification().discover_calls, 0);

        // Paging is over admitted entries only.
        let paged = fixture
            .browse(json!({"level": "operations", "namespace": namespace, "entity": "Record", "limit": 1}))
            .await
            .expect("first operation page");
        assert_eq!(paged["operations"].as_array().map(Vec::len), Some(1));
        assert_eq!(paged["nextOffset"], 1);
        fixture.deny("RecordBySubject");
        let paged = fixture
            .browse(json!({"level": "operations", "namespace": namespace, "entity": "Record", "limit": 1}))
            .await
            .expect("first operation page");
        assert_eq!(paged["nextOffset"], serde_json::Value::Null, "{paged}");
        let denied_root = fixture
            .browse(json!({"level": "operations", "namespace": namespace, "entity": "Record"}))
            .await
            .expect("operations");
        assert!(
            !denied_root.to_string().contains("RecordBySubject"),
            "{denied_root}"
        );
        assert!(
            !denied_root.to_string().contains("subject heading"),
            "{denied_root}"
        );

        // With every root denied nothing of the entity is listed at any
        // level, and the response matches one for an unknown entity.
        fixture.deny("GeneratedRecord");
        let empty = fixture
            .browse(json!({"level": "namespaces"}))
            .await
            .expect("namespaces");
        assert_eq!(empty["namespaces"], json!([]), "{empty}");
        let none = fixture
            .browse(json!({"level": "capabilities", "namespace": namespace}))
            .await
            .expect("capabilities");
        assert_eq!(none["capabilities"], json!([]), "{none}");
        let denied = fixture
            .browse(json!({"level": "operations", "namespace": namespace, "entity": "Record"}))
            .await
            .expect("operations");
        let unknown = fixture
            .browse(json!({"level": "operations", "namespace": namespace, "entity": "Unknown"}))
            .await
            .expect("operations");
        assert_eq!(denied, unknown);
        assert_eq!(denied["capability"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn browse_refuses_malformed_levels_and_bounds() {
        let fixture = EntityBrokerFixture::new();
        for arguments in [
            json!({"level": "namespaces", "namespace": "generated-read"}),
            json!({"level": "namespaces", "entity": "Record"}),
            json!({"level": "capabilities"}),
            json!({"level": "operations", "namespace": "generated-read"}),
            json!({"level": "capabilities", "namespace": "generated-read", "limit": 0}),
            json!({"level": "capabilities", "namespace": "generated-read", "limit": 33}),
            json!({
                "level": "capabilities",
                "namespace": "generated-read",
                "offset": AI_CAPABILITY_BROWSE_MAXIMUM_OFFSET + 1,
            }),
            json!({"level": "capabilities", "namespace": " "}),
            json!({"level": "targets"}),
            json!({"level": "namespaces", "text": "record"}),
        ] {
            let error = fixture
                .browse(arguments.clone())
                .await
                .expect_err("malformed");
            assert!(
                matches!(error, AiError::InvalidInput(_)),
                "{arguments}: {error:?}"
            );
        }
        assert_eq!(fixture.session.amplification().browse_calls, 0);
    }

    #[tokio::test]
    async fn browse_over_a_small_policy_batch_still_answers_and_denied_matches_unknown() {
        let mut fixture = EntityBrokerFixture::new();
        let namespace = fixture
            .browse(json!({"level": "namespaces"}))
            .await
            .expect("namespaces")["namespaces"][0]["namespace"]
            .clone();
        // A deep offset that the largest index set can reach is accepted.
        let deep = fixture
            .browse(json!({
                "level": "capabilities",
                "namespace": namespace,
                "offset": AI_CAPABILITY_BROWSE_MAXIMUM_OFFSET,
            }))
            .await
            .expect("deep offset");
        assert_eq!(deep["capabilities"], json!([]));
        let without_references = |mut page: serde_json::Value| {
            for path in ["/capability", "/capabilities/0"] {
                if let Some(object) = page.pointer_mut(path).and_then(|v| v.as_object_mut()) {
                    object.remove("candidateReference");
                }
            }
            page
        };
        let unbounded = without_references(
            fixture
                .browse(json!({"level": "operations", "namespace": namespace, "entity": "Record"}))
                .await
                .expect("unbounded"),
        );
        // The entity and its two roots are three decisions: a smaller
        // policy batch decides them alone, with the same response.
        fixture.broker = fixture
            .broker
            .clone()
            .with_maximum_discovery_authorizations(2)
            .expect("bound");
        let bounded = without_references(
            fixture
                .browse(json!({"level": "operations", "namespace": namespace, "entity": "Record"}))
                .await
                .expect("bounded"),
        );
        assert_eq!(bounded, unbounded);
        // A denied entity answers exactly as an unknown one.
        fixture.deny("GeneratedRecord");
        fixture.deny("RecordBySubject");
        for level in ["capabilities", "operations"] {
            let denied = fixture
                .browse(json!({"level": level, "namespace": namespace, "entity": "Record"}))
                .await
                .expect("denied");
            let unknown = fixture
                .browse(json!({"level": level, "namespace": namespace, "entity": "Unknown"}))
                .await
                .expect("unknown");
            assert_eq!(denied, unknown, "{level}");
        }
    }

    /// Browses a 40-tool catalogue in which only every seventh tool is
    /// permitted, under one policy batch bound.
    async fn browse_mixed_catalogue(
        bound: Option<u32>,
        arguments: serde_json::Value,
    ) -> serde_json::Value {
        let index = large_static_index(40);
        let fingerprints = index
            .entries()
            .map(|entry| entry.capability_fingerprint.clone())
            .collect::<Vec<_>>();
        let permitted = index
            .entries()
            .enumerate()
            .filter(|(position, _)| position % 7 == 6)
            .map(|(_, entry)| entry.id.as_str().to_owned())
            .collect::<Vec<_>>();
        let principal = principal();
        let reference = principal.reference();
        let mut broker = AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal)),
            Arc::new(CurrentIndex(RwLock::new(index))),
            Arc::new(PermittedSet::new(permitted.iter().map(String::as_str))),
            Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
            Duration::seconds(30),
        )
        .expect("broker");
        if let Some(bound) = bound {
            broker = broker
                .with_maximum_discovery_authorizations(bound)
                .expect("bound");
        }
        let mut page = broker
            .dispatch_browse(
                &reference,
                &run_binding(),
                &brokering(&fingerprints),
                &arguments,
            )
            .await
            .expect("browse");
        for candidate in page["capabilities"].as_array_mut().into_iter().flatten() {
            candidate
                .as_object_mut()
                .expect("candidate")
                .remove("candidateReference");
        }
        page
    }

    #[tokio::test]
    async fn hidden_matches_never_change_what_browse_returns() {
        let namespaces = browse_mixed_catalogue(None, json!({"level": "namespaces"})).await;
        let namespace = namespaces["namespaces"][0]["namespace"].clone();
        let mut pages = Vec::new();
        for offset in [0, 2, 4] {
            let arguments = json!({
                "level": "capabilities",
                "namespace": namespace,
                "offset": offset,
                "limit": 2,
            });
            let unbounded = browse_mixed_catalogue(None, arguments.clone()).await;
            // However many hidden tools a policy batch can hold, the page
            // is the same admitted prefix with the same nextOffset.
            for bound in [1, 3, 64] {
                assert_eq!(
                    browse_mixed_catalogue(Some(bound), arguments.clone()).await,
                    unbounded,
                    "offset {offset}, bound {bound}"
                );
            }
            pages.push(unbounded);
        }
        let ids = pages
            .iter()
            .flat_map(|page| page["capabilities"].as_array().cloned().unwrap_or_default())
            .map(|candidate| candidate["capabilityId"].as_str().expect("id").to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [6, 13, 20, 27, 34]
                .map(|position| format!("catalogue.record_{position:04}"))
                .to_vec()
        );
        assert_eq!(pages[0]["nextOffset"], 2);
        assert_eq!(pages[1]["nextOffset"], 4);
        assert_eq!(pages[2]["nextOffset"], serde_json::Value::Null);
        for bound in [1, 3] {
            assert_eq!(
                browse_mixed_catalogue(Some(bound), json!({"level": "namespaces"})).await,
                namespaces
            );
        }
    }

    #[tokio::test]
    async fn browse_references_resolve_only_for_the_issuing_run() {
        let fixture = EntityBrokerFixture::new();
        let namespace = fixture
            .browse(json!({"level": "namespaces"}))
            .await
            .expect("namespaces")["namespaces"][0]["namespace"]
            .clone();
        let candidate = fixture
            .browse(json!({"level": "capabilities", "namespace": namespace}))
            .await
            .expect("capabilities")["capabilities"][0]
            .clone();
        let arguments = json!({
            "capabilityId": candidate["capabilityId"],
            "candidateReference": candidate["candidateReference"],
        });
        // The same (cloned) session under another run or attempt resolves
        // nothing.
        let mut other_run = fixture.run.clone();
        other_run.run_id = AiRunId::new();
        let mut other_attempt = fixture.run.clone();
        other_attempt.attempt_id = Uuid::new_v4();
        for run in [other_run, other_attempt] {
            let error = fixture
                .broker
                .dispatch_describe(
                    &fixture.principal,
                    &run,
                    &fixture.session.clone(),
                    &arguments,
                )
                .await
                .expect_err("another run");
            assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
        }
        fixture
            .broker
            .dispatch_describe(
                &fixture.principal,
                &fixture.run,
                &fixture.session,
                &arguments,
            )
            .await
            .expect("issuing run");
    }

    impl AiCapabilityBrokerSession {
        /// The newest retained search that holds `capability_id`, with the
        /// references it issued.
        fn issued_search_holding(&self, capability_id: &AiToolId) -> IssuedSearch {
            self.state()
                .searches
                .iter()
                .rev()
                .find(|issued| {
                    issued
                        .search
                        .candidates
                        .iter()
                        .any(|candidate| &candidate.id == capability_id)
                })
                .cloned()
                .expect("issued search")
        }
    }

    fn principal_named(user_id: &str) -> AuthPrincipal {
        let mut named = principal();
        if let AuthPrincipal::User(user) = &mut named {
            user.user_id = user_id.to_owned();
        }
        named
    }

    /// A model-facing broker response with each opaque per-call reference
    /// replaced by a placeholder, after checking that it is one and that the
    /// response carries no fingerprint member at all.
    fn without_references(mut value: serde_json::Value) -> serde_json::Value {
        fn walk(value: &mut serde_json::Value) {
            match value {
                serde_json::Value::Object(object) => {
                    for (key, member) in object.iter_mut() {
                        assert!(
                            !key.to_ascii_lowercase().contains("fingerprint"),
                            "model-facing {key}"
                        );
                        if key == "candidateReference" || key == "loadedReference" {
                            assert!(member.as_str().is_some_and(crate::valid_sha256), "{member}");
                            *member = json!("<reference>");
                        } else {
                            walk(member);
                        }
                    }
                }
                serde_json::Value::Array(items) => items.iter_mut().for_each(walk),
                _ => {}
            }
        }
        walk(&mut value);
        value
    }

    /// What one principal sees of the `Record` entity through discover and
    /// describe, when `RecordBySubject` carries `hidden_description` and,
    /// with `deny_hidden`, host policy denies it that root.
    async fn observe_record_entity(
        user_id: &str,
        hidden_description: &str,
        deny_hidden: bool,
    ) -> (serde_json::Value, serde_json::Value) {
        let fixture = EntityBrokerFixture::with_principal(
            principal_named(user_id),
            generated_index_with_roots(
                "generated-read-application",
                "generated-read",
                "target-policy-v1",
                None,
                Some(hidden_description),
            ),
        );
        if deny_hidden {
            fixture.deny("RecordBySubject");
        }
        let discovered = fixture.discover().await;
        let described = fixture
            .describe()
            .await
            .expect("describe")
            .into_model_result();
        (
            without_references(discovered),
            without_references(described),
        )
    }

    #[tokio::test]
    async fn discover_and_describe_never_vary_with_hidden_operations() {
        const HIDDEN: &str = "Read one reviewed application record by its subject heading.";
        const REVISED: &str =
            "Read one reviewed application record by its revised subject heading.";
        // `restricted` is denied RecordBySubject; `full` may use both roots.
        let restricted = observe_record_entity("restricted-user", HIDDEN, true).await;
        let revised = observe_record_entity("restricted-user", REVISED, true).await;
        // A change to a root host policy hides changes nothing this
        // principal receives: no index, entry or candidate fingerprint, and
        // no other artifact, follows it.
        assert_eq!(restricted, revised);
        for response in [&restricted.0, &restricted.1] {
            let text = response.to_string();
            assert!(!text.contains("RecordBySubject"), "{text}");
            assert!(!text.contains("heading"), "{text}");
        }

        // Two principals with different policy see the operation they share
        // identically, and the same candidate apart from the operation list.
        let full = observe_record_entity("full-user", HIDDEN, false).await;
        let shared = |described: &serde_json::Value| {
            described["operations"]
                .as_array()
                .expect("operations")
                .iter()
                .find(|operation| operation["operation"] == "GeneratedRecord")
                .cloned()
                .expect("shared operation")
        };
        assert_eq!(shared(&full.1), shared(&restricted.1));
        assert_eq!(full.1["operations"].as_array().map(Vec::len), Some(2));
        let mut full_candidate = full.0["candidates"][0].clone();
        full_candidate["operations"] = restricted.0["candidates"][0]["operations"].clone();
        assert_eq!(full_candidate, restricted.0["candidates"][0]);
        let mut full_discovery = full.0.clone();
        full_discovery["candidates"] = restricted.0["candidates"].clone();
        assert_eq!(full_discovery, restricted.0);
        for key in [
            "version",
            "capabilityId",
            "kind",
            "name",
            "description",
            "namespace",
            "entity",
            "risk",
            "approval",
            "usage",
        ] {
            assert_eq!(full.1[key], restricted.1[key], "{key}");
        }
    }

    #[tokio::test]
    async fn discover_references_resolve_only_for_the_issuing_run() {
        let fixture = EntityBrokerFixture::new();
        let discovered = fixture.discover().await;
        assert!(discovered.get("indexFingerprint").is_none(), "{discovered}");
        let candidate = discovered["candidates"][0].clone();
        assert!(candidate.get("candidateFingerprint").is_none());
        let arguments = json!({
            "capabilityId": candidate["capabilityId"],
            "candidateReference": candidate["candidateReference"],
        });
        // The same (cloned) session under another run, attempt, provider
        // session, AI session or principal resolves nothing.
        let mut other_run = fixture.run.clone();
        other_run.run_id = AiRunId::new();
        let mut other_attempt = fixture.run.clone();
        other_attempt.attempt_id = Uuid::new_v4();
        let mut other_provider_session = fixture.run.clone();
        other_provider_session.provider_session_fingerprint = "provider-session-v2".to_owned();
        let mut other_session = fixture.run.clone();
        other_session.session_id = AiSessionId::new();
        for run in [
            other_run,
            other_attempt,
            other_provider_session,
            other_session,
        ] {
            let error = fixture
                .broker
                .dispatch_describe(
                    &fixture.principal,
                    &run,
                    &fixture.session.clone(),
                    &arguments,
                )
                .await
                .expect_err("another run");
            assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
        }
        let error = fixture
            .broker
            .dispatch_describe(
                &principal_named("another-user").reference(),
                &fixture.run,
                &fixture.session,
                &arguments,
            )
            .await
            .expect_err("another principal");
        assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
        // A fresh session of the same run, without the discovery cache,
        // holds nothing either.
        let error = fixture
            .broker
            .dispatch_describe(
                &fixture.principal,
                &fixture.run,
                &DiscoveryCacheFixture::session(),
                &arguments,
            )
            .await
            .expect_err("fresh session");
        assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
        // The server-held entry fingerprint is no substitute: an earlier
        // contract's candidateFingerprint is an unheld selection.
        let entity = fixture.entity_id();
        let entry = fixture
            .index
            .0
            .read()
            .expect("index")
            .entries()
            .find(|entry| entry.id == entity)
            .expect("entity entry")
            .fingerprint
            .clone();
        let error = fixture
            .broker
            .dispatch_describe(
                &fixture.principal,
                &fixture.run,
                &fixture.session,
                &json!({"capabilityId": entity.as_str(), "candidateFingerprint": entry}),
            )
            .await
            .expect_err("entry fingerprint");
        assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
        // A reference named under another capability resolves nothing.
        let error = fixture
            .broker
            .dispatch_describe(
                &fixture.principal,
                &fixture.run,
                &fixture.session,
                &json!({
                    "capabilityId": fixture.root_id("GeneratedRecord").as_str(),
                    "candidateReference": candidate["candidateReference"],
                }),
            )
            .await
            .expect_err("another capability");
        assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
        let described = fixture
            .broker
            .dispatch_describe(
                &fixture.principal,
                &fixture.run,
                &fixture.session,
                &arguments,
            )
            .await
            .expect("issuing run");
        assert!(described.contract().get("candidateFingerprint").is_none());
        // Discovering again issues a fresh reference; both stay usable in
        // the issuing run.
        let again = fixture.discover().await["candidates"][0]["candidateReference"].clone();
        assert_ne!(again, candidate["candidateReference"]);
    }

    /// The `Record` entity with both roots, `RecordBySubject` described by
    /// `hidden_description` and, with `note`, one more `Record` field.
    fn record_roots_index(hidden_description: &str, note: bool) -> Arc<AiCapabilityIndex> {
        generated_index_with_roots(
            "generated-read-application",
            "generated-read",
            "target-policy-v1",
            note.then_some("note"),
            Some(hidden_description),
        )
    }

    const HIDDEN_ROOT: &str = "Read one reviewed application record by its subject heading.";
    const REVISED_HIDDEN_ROOT: &str =
        "Read one reviewed application record by its revised subject heading.";

    #[tokio::test]
    async fn hidden_only_revisions_keep_candidate_loaded_and_continuation_references() {
        let fixture = EntityBrokerFixture::with_principal(
            principal_named("restricted-user"),
            record_roots_index(HIDDEN_ROOT, false),
        );
        fixture.deny("RecordBySubject");
        let held = fixture.discover().await["candidates"][0].clone();
        let (described, first) = first_page(&fixture, "GeneratedRecord").await;
        let loaded = described.loaded_reference().to_owned();
        let continuation = fixture
            .broker
            .record_continuation(&fixture.session, &first, &more("cursor-1"))
            .expect("continuation");
        let before = fixture.index.0.read().expect("index").clone();

        // Only the root this principal is denied changes: every server-side
        // fingerprint moves, but nothing it was shown does.
        *fixture.index.0.write().expect("index") = record_roots_index(REVISED_HIDDEN_ROOT, false);
        let after = fixture.index.0.read().expect("index").clone();
        assert_ne!(before.fingerprint(), after.fingerprint());
        let root = fixture.root_id("GeneratedRecord");
        let old_root = before
            .entry(&root)
            .expect("root")
            .capability_fingerprint
            .clone();
        let new_root = after
            .entry(&root)
            .expect("root")
            .capability_fingerprint
            .clone();
        assert_ne!(old_root, new_root, "the catalogue-wide fingerprint moved");

        // The held candidate still describes, to the same view.
        let redescribed = fixture
            .broker
            .dispatch_describe(
                &fixture.principal,
                &fixture.run,
                &fixture.session,
                &json!({
                    "capabilityId": held["capabilityId"],
                    "candidateReference": held["candidateReference"],
                }),
            )
            .await
            .expect("held candidate after a hidden-only revision");
        assert_eq!(
            without_references(redescribed.into_model_result()),
            without_references(described.contract().clone())
        );
        // The loaded reference still expands types and executes, against
        // the exact current root.
        fixture
            .broker
            .dispatch_describe_type(
                &fixture.principal,
                &fixture.run,
                &fixture.session,
                &json!({"loadedReference": loaded, "operation": "GeneratedRecord", "type": "select"}),
            )
            .await
            .expect("describe_type after a hidden-only revision");
        let executed = fixture
            .execute(json!({
                "loadedReference": loaded,
                "operation": "GeneratedRecord",
                "arguments": [{"name": "recordId", "value": "record-1"}],
                "selections": ["subject"],
            }))
            .await
            .expect("execute after a hidden-only revision");
        assert_eq!(executed.capability_fingerprint(), new_root);
        assert_eq!(executed.audit_reference(), loaded);
        // The continuation of the first page continues against it too.
        let second = fixture
            .execute(json!({
                "loadedReference": loaded,
                "continuation": continuation.handle(),
            }))
            .await
            .expect("continuation after a hidden-only revision");
        assert_eq!(second.capability_fingerprint(), new_root);
        assert_eq!(second.plan(), first.plan());

        // A change this principal can see is still stale everywhere.
        let held = fixture.discover().await["candidates"][0].clone();
        *fixture.index.0.write().expect("index") = record_roots_index(REVISED_HIDDEN_ROOT, true);
        let stale = [
            fixture
                .broker
                .dispatch_describe(
                    &fixture.principal,
                    &fixture.run,
                    &fixture.session,
                    &json!({
                        "capabilityId": held["capabilityId"],
                        "candidateReference": held["candidateReference"],
                    }),
                )
                .await
                .map(|_| ())
                .expect_err("visible change at describe"),
            fixture
                .execute(json!({
                    "loadedReference": loaded,
                    "operation": "GeneratedRecord",
                    "arguments": [{"name": "recordId", "value": "record-1"}],
                    "selections": ["subject"],
                }))
                .await
                .map(|_| ())
                .expect_err("visible change at execute"),
        ];
        for error in &stale {
            assert_input_error(error, AI_CAPABILITY_STALE);
        }

        // The same revision is visible to a principal that may use the
        // revised root, so its loaded reference is stale.
        let full = EntityBrokerFixture::with_principal(
            principal_named("full-user"),
            record_roots_index(HIDDEN_ROOT, false),
        );
        let described = full.describe().await.expect("describe");
        *full.index.0.write().expect("index") = record_roots_index(REVISED_HIDDEN_ROOT, false);
        let error = full
            .execute(json!({
                "loadedReference": described.loaded_reference(),
                "operation": "GeneratedRecord",
                "arguments": [{"name": "recordId", "value": "record-1"}],
                "selections": ["subject"],
            }))
            .await
            .expect_err("visible revision");
        assert_input_error(&error, AI_CAPABILITY_STALE);
    }

    #[tokio::test]
    async fn references_resolve_only_against_their_issuing_snapshot() {
        // Eviction: a session retaining one search drops the first
        // search's references with it.
        let mut fixture = EntityBrokerFixture::new();
        fixture.session = AiCapabilityBrokerSession::new(AiCapabilityDeliveryLimits {
            maximum_retained_searches: 1,
            ..AiCapabilityDeliveryLimits::default()
        })
        .expect("session");
        let evicted = fixture.discover().await["candidates"][0].clone();
        let newest = fixture.discover().await["candidates"][0].clone();
        let describe = |candidate: &serde_json::Value| {
            json!({
                "capabilityId": candidate["capabilityId"],
                "candidateReference": candidate["candidateReference"],
            })
        };
        let error = fixture
            .broker
            .dispatch_describe(
                &fixture.principal,
                &fixture.run,
                &fixture.session,
                &describe(&evicted),
            )
            .await
            .expect_err("evicted issuing search");
        assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
        fixture
            .broker
            .dispatch_describe(
                &fixture.principal,
                &fixture.run,
                &fixture.session,
                &describe(&newest),
            )
            .await
            .expect("retained issuing search");
        // A browse retains its own snapshot and evicts the discovery.
        let namespace = newest["namespace"].clone();
        let browsed = fixture
            .browse(json!({"level": "capabilities", "namespace": namespace}))
            .await
            .expect("browse")["capabilities"][0]
            .clone();
        let error = fixture
            .broker
            .dispatch_describe(
                &fixture.principal,
                &fixture.run,
                &fixture.session,
                &describe(&newest),
            )
            .await
            .expect_err("evicted by browse");
        assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
        fixture
            .broker
            .dispatch_describe(
                &fixture.principal,
                &fixture.run,
                &fixture.session,
                &describe(&browsed),
            )
            .await
            .expect("browsed reference");

        // An unrelated target change: references and loaded handles of the
        // first target stay usable, before and after rediscovery, each
        // against its own snapshot.
        let first = generated_index_for("first-application", "first-read", "first-policy-v1");
        let second = generated_index_for("second-application", "second-read", "second-policy-v1");
        let first_id = first
            .entries()
            .find(|entry| entry.kind == AiCapabilityKind::GeneratedEntity)
            .expect("first entity")
            .id
            .clone();
        let indexes = Arc::new(CurrentIndexes(RwLock::new(Arc::new(
            AiCapabilityIndexSet::compile([Arc::clone(&first), second]).expect("set"),
        ))));
        let principal = principal();
        let principal_reference = principal.reference();
        let broker = AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal)),
            indexes.clone(),
            Arc::new(Authority {
                allowed: AtomicBool::new(true),
                policy_fingerprint: RwLock::new("current-policy-v1".to_owned()),
            }),
            Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
            Duration::seconds(30),
        )
        .expect("broker");
        let run = run_binding();
        let session = DiscoveryCacheFixture::session();
        let discover = || async {
            let found = broker
                .dispatch_discover(
                    &principal_reference,
                    &run,
                    &session,
                    &json!({"text": "reviewed record", "kind": "generated_entity", "maximumResults": 4}),
                )
                .await
                .expect("discover");
            found["candidates"]
                .as_array()
                .expect("candidates")
                .iter()
                .find(|candidate| candidate["capabilityId"] == first_id.as_str())
                .cloned()
                .expect("first-target candidate")
        };
        let held = discover().await;
        let loaded = broker
            .dispatch_describe(&principal_reference, &run, &session, &describe(&held))
            .await
            .expect("describe")
            .loaded_reference()
            .to_owned();
        *indexes.0.write().expect("indexes") = Arc::new(
            AiCapabilityIndexSet::compile([
                Arc::clone(&first),
                generated_index_with_schema(
                    "second-application",
                    "second-read",
                    "second-policy-v1",
                    Some("note"),
                    false,
                ),
            ])
            .expect("set"),
        );
        let rediscovered = discover().await;
        for candidate in [&held, &rediscovered] {
            broker
                .dispatch_describe(&principal_reference, &run, &session, &describe(candidate))
                .await
                .expect("first-target reference after an unrelated target change");
        }
        broker
            .authorize_broker_execution(
                &principal_reference,
                &run,
                &session,
                &json!({"loadedReference": loaded, "selections": ["recordId"]}),
            )
            .await
            .expect("first-target execution after an unrelated target change");
    }

    #[tokio::test]
    async fn sibling_policy_changes_give_the_same_outcome_with_or_without_hidden_drift() {
        let mut outcomes = Vec::new();
        for hidden_drift in [false, true] {
            let fixture = EntityBrokerFixture::new();
            let held = fixture.discover().await["candidates"][0].clone();
            let described = fixture.describe().await.expect("describe");
            let loaded = described.loaded_reference().to_owned();
            // A sibling of the operation used is revoked; optionally the
            // target policy changes too, which this principal cannot see.
            fixture.deny("RecordBySubject");
            if hidden_drift {
                *fixture.index.0.write().expect("index") =
                    two_root_entity_index("target-policy-v2");
            }
            let describe_type = fixture
                .broker
                .dispatch_describe_type(
                    &fixture.principal,
                    &fixture.run,
                    &fixture.session,
                    &json!({"loadedReference": loaded, "operation": "GeneratedRecord", "type": "select"}),
                )
                .await
                .map(|_| ());
            let execute = fixture
                .execute(json!({
                    "loadedReference": loaded,
                    "operation": "GeneratedRecord",
                    "selections": ["recordId"],
                }))
                .await
                .map(|_| ());
            let redescribe = fixture
                .broker
                .dispatch_describe(
                    &fixture.principal,
                    &fixture.run,
                    &fixture.session,
                    &json!({
                        "capabilityId": held["capabilityId"],
                        "candidateReference": held["candidateReference"],
                    }),
                )
                .await
                .map(|_| ());
            for outcome in [&describe_type, &execute, &redescribe] {
                let error = outcome.as_ref().expect_err("the admitted view changed");
                assert_input_error(error, AI_CAPABILITY_STALE);
            }
            outcomes.push(format!("{describe_type:?} {execute:?} {redescribe:?}"));
        }
        assert_eq!(outcomes[0], outcomes[1]);
    }

    /// Allows everything and records the largest policy batch it receives.
    struct PeakBatchAuthority(std::sync::atomic::AtomicUsize);

    #[async_trait]
    impl AiCapabilityAuthorityPolicy for PeakBatchAuthority {
        async fn authorize(
            &self,
            _principal: &ResolvedPrincipal,
            _owning_index: &AiCapabilityIndex,
            _entry: &AiCapabilityIndexEntry,
            _run: &AiCapabilityRunBinding,
        ) -> Result<AiCapabilityAuthorityDecision, AiError> {
            Ok(AiCapabilityAuthorityDecision {
                allowed: true,
                policy_fingerprint: "current-policy-v1".to_owned(),
            })
        }

        async fn authorize_discovery(
            &self,
            _principal: &ResolvedPrincipal,
            matches: &[AiCapabilityRankedMatch<'_>],
            _run: &AiCapabilityRunBinding,
        ) -> Result<Vec<AiCapabilityDiscoveryDecision>, AiError> {
            self.0.fetch_max(matches.len(), Ordering::SeqCst);
            Ok(matches
                .iter()
                .map(|candidate| AiCapabilityDiscoveryDecision::for_match(candidate, true))
                .collect())
        }
    }

    #[tokio::test]
    async fn an_entity_larger_than_a_policy_batch_is_split_across_calls() {
        // The entity and its two roots are three decisions; the bound is two.
        let authority = Arc::new(PeakBatchAuthority(std::sync::atomic::AtomicUsize::new(0)));
        let principal = principal();
        let principal_reference = principal.reference();
        let broker = |bound: u32| {
            AiCapabilityDiscoveryBroker::new(
                Arc::new(Resolver(principal.clone())),
                Arc::new(CurrentIndex(RwLock::new(two_root_entity_index(
                    "target-policy-v1",
                )))),
                authority.clone(),
                Arc::new(FixedClock::new(OffsetDateTime::UNIX_EPOCH)),
                Duration::seconds(30),
            )
            .expect("broker")
            .with_maximum_discovery_authorizations(bound)
            .expect("bound")
        };
        let run = run_binding();
        let session = DiscoveryCacheFixture::session();
        let namespace = broker(2)
            .dispatch_browse(
                &principal_reference,
                &run,
                &session,
                &json!({"level": "namespaces"}),
            )
            .await
            .expect("namespaces")["namespaces"][0]["namespace"]
            .clone();
        let listed = broker(2)
            .dispatch_browse(
                &principal_reference,
                &run,
                &session,
                &json!({"level": "capabilities", "namespace": namespace}),
            )
            .await
            .expect("capabilities");
        assert_eq!(
            listed["capabilities"][0]["operations"]
                .as_array()
                .map(Vec::len),
            Some(2),
            "{listed}"
        );
        assert!(authority.0.load(Ordering::SeqCst) <= 2);
        // Discover splits the same way; its view of three decisions then
        // exceeds a ceiling of two, and fits one of three.
        let search = json!({"text": "reviewed application record", "maximumResults": 4});
        let error = broker(2)
            .dispatch_discover(&principal_reference, &run, &session, &search)
            .await
            .expect_err("view above the ceiling");
        assert_input_error(&error, AI_CAPABILITY_DISCOVERY_TOO_BROAD);
        assert!(authority.0.load(Ordering::SeqCst) <= 2);
        let found = broker(3)
            .dispatch_discover(&principal_reference, &run, &session, &search)
            .await
            .expect("view within the ceiling");
        assert_eq!(
            found["candidates"][0]["operations"]
                .as_array()
                .map(Vec::len),
            Some(2)
        );
        assert!(authority.0.load(Ordering::SeqCst) <= 3);
    }

    #[tokio::test]
    async fn a_legacy_candidate_fingerprint_is_an_unheld_selection_before_parsing() {
        let fixture = EntityBrokerFixture::new();
        for arguments in [
            json!({"capabilityId": "x", "candidateFingerprint": null}),
            json!({"candidateFingerprint": "f".repeat(64), "candidateReference": "e".repeat(64), "capabilityId": "x"}),
            json!({"candidateFingerprint": 7, "capabilityId": 5, "unknown": true}),
            json!({"candidateFingerprint": "f".repeat(64)}),
        ] {
            let error = fixture
                .broker
                .dispatch_describe(
                    &fixture.principal,
                    &fixture.run,
                    &fixture.session,
                    &arguments,
                )
                .await
                .expect_err("legacy selection");
            assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
        }
        // Without the legacy member, malformed arguments stay malformed.
        let error = fixture
            .broker
            .dispatch_describe(
                &fixture.principal,
                &fixture.run,
                &fixture.session,
                &json!({"capabilityId": 5, "unknown": true}),
            )
            .await
            .expect_err("malformed");
        assert!(
            matches!(&error, AiError::InvalidInput(message) if message != AI_CAPABILITY_SELECTION_STALE),
            "{error:?}"
        );
    }

    /// Describes the entity and authorizes one first page of `operation`.
    async fn first_page(
        fixture: &EntityBrokerFixture,
        operation: &str,
    ) -> (AiCapabilityDescription, AiCapabilityExecution) {
        let described = fixture.describe().await.expect("describe");
        let execution = fixture
            .execute(json!({
                "loadedReference": described.loaded_reference(),
                "operation": operation,
                "arguments": [{"name": "recordId", "value": "record-1"}],
                "selections": ["subject"],
            }))
            .await
            .expect("first page");
        (described, execution)
    }

    fn more(cursor: &str) -> crate::AiGraphqlQueryPageInfo {
        crate::AiGraphqlQueryPageInfo {
            has_next_page: true,
            end_cursor: Some(cursor.to_owned()),
        }
    }

    #[tokio::test]
    async fn continuation_replays_the_exact_query_with_a_server_held_cursor() {
        let fixture = EntityBrokerFixture::new();
        let (described, first) = first_page(&fixture, "GeneratedRecord").await;
        assert_eq!(first.continuation_cursor(), None);
        assert_eq!(first.page_number(), 1);
        // No further page, or no cursor, retains nothing.
        for page_info in [
            crate::AiGraphqlQueryPageInfo {
                has_next_page: false,
                end_cursor: Some("cursor-1".to_owned()),
            },
            crate::AiGraphqlQueryPageInfo {
                has_next_page: true,
                end_cursor: None,
            },
        ] {
            assert!(
                fixture
                    .broker
                    .record_continuation(&fixture.session, &first, &page_info)
                    .is_none()
            );
        }
        assert_eq!(fixture.session.continuation_count(), 0);
        let continuation = fixture
            .broker
            .record_continuation(&fixture.session, &first, &more("cursor-1"))
            .expect("continuation");
        assert!(crate::valid_sha256(continuation.handle()));
        assert_eq!(continuation.expires_in_seconds(), 30);
        let model = continuation.to_model_value();
        assert!(!model.to_string().contains("cursor-1"), "{model}");

        let second = fixture
            .execute(json!({
                "loadedReference": described.loaded_reference(),
                "operation": "GeneratedRecord",
                "continuation": continuation.handle(),
            }))
            .await
            .expect("continued page");
        assert_eq!(second.continuation_cursor(), Some("cursor-1"));
        assert_eq!(second.page_number(), 2);
        assert_eq!(second.plan(), first.plan());
        assert_eq!(second.capability_id(), first.capability_id());
        assert_eq!(
            second.capability_fingerprint(),
            first.capability_fingerprint()
        );
        // The operation may be omitted: the handle names it.
        let omitted = fixture
            .execute(json!({
                "loadedReference": described.loaded_reference(),
                "continuation": continuation.handle(),
            }))
            .await
            .expect("continued page without operation");
        assert_eq!(omitted.plan(), first.plan());
        let third = fixture
            .broker
            .record_continuation(&fixture.session, &second, &more("cursor-2"))
            .expect("chained continuation");
        let third = fixture
            .execute(json!({
                "loadedReference": described.loaded_reference(),
                "continuation": third.handle(),
            }))
            .await
            .expect("third page");
        assert_eq!(third.page_number(), 3);
        assert_eq!(third.continuation_cursor(), Some("cursor-2"));
        assert_eq!(fixture.session.amplification().execute_calls, 4);
    }

    #[tokio::test]
    async fn continuation_handles_are_bound_tamper_evident_and_closed() {
        let fixture = EntityBrokerFixture::new();
        let (described, first) = first_page(&fixture, "GeneratedRecord").await;
        let continuation = fixture
            .broker
            .record_continuation(&fixture.session, &first, &more("cursor-1"))
            .expect("continuation");
        let loaded = described.loaded_reference().to_owned();
        let continue_with = |arguments: serde_json::Value| fixture.execute(arguments);

        // An unknown or altered handle is a stale selection.
        let mut altered = continuation.handle().to_owned();
        let last = altered.pop().expect("handle");
        altered.push(if last == '0' { '1' } else { '0' });
        let error = continue_with(json!({"loadedReference": loaded, "continuation": altered}))
            .await
            .expect_err("altered handle");
        assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
        let error =
            continue_with(json!({"loadedReference": loaded, "continuation": "not-a-handle"}))
                .await
                .expect_err("malformed handle");
        assert!(matches!(error, AiError::InvalidInput(_)), "{error:?}");

        // The retained query cannot be varied.
        for extra in [
            json!({"arguments": [{"name": "recordId", "value": "record-2"}]}),
            json!({"selections": ["recordId"]}),
            json!({"maximumItems": 5}),
            json!({"relationshipArguments": [{"path": "subject", "arguments": []}]}),
            json!({"relationshipMaximumItems": [{"path": "subject", "maximumItems": 1}]}),
        ] {
            let mut arguments =
                json!({"loadedReference": loaded, "continuation": continuation.handle()});
            for (key, value) in extra.as_object().expect("extra") {
                arguments[key] = value.clone();
            }
            let error = continue_with(arguments.clone())
                .await
                .expect_err("varied query");
            assert!(
                matches!(&error, AiError::InvalidInput(message) if message != AI_CAPABILITY_SELECTION_STALE),
                "{arguments}: {error:?}"
            );
        }
        let error = continue_with(json!({
            "loadedReference": loaded,
            "operation": "RecordBySubject",
            "continuation": continuation.handle(),
        }))
        .await
        .expect_err("other operation");
        assert_input_error(&error, AI_CAPABILITY_OPERATION_INVALID);

        // A handle belongs to the loaded reference that produced it: a new
        // description of the same capability cannot use it.
        let again = fixture.describe().await.expect("describe again");
        assert_ne!(again.loaded_reference(), loaded);
        let error = continue_with(json!({
            "loadedReference": again.loaded_reference(),
            "continuation": continuation.handle(),
        }))
        .await
        .expect_err("foreign loaded reference");
        assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);

        // Another session of the same principal and run holds nothing.
        let other_session = DiscoveryCacheFixture::session();
        let error = fixture
            .broker
            .authorize_broker_execution(
                &fixture.principal,
                &fixture.run,
                &other_session,
                &json!({"loadedReference": loaded, "continuation": continuation.handle()}),
            )
            .await
            .expect_err("other session");
        assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);

        // Another run attempt is refused by the loaded binding itself.
        let mut other_run = fixture.run.clone();
        other_run.attempt_id = Uuid::new_v4();
        let error = fixture
            .broker
            .authorize_broker_execution(
                &fixture.principal,
                &other_run,
                &fixture.session,
                &json!({"loadedReference": loaded, "continuation": continuation.handle()}),
            )
            .await
            .expect_err("other attempt");
        assert!(matches!(error, AiError::Forbidden), "{error:?}");
    }

    #[tokio::test]
    async fn continued_pages_reapply_current_policy_and_expire_with_the_binding() {
        let fixture = EntityBrokerFixture::new();
        let (described, first) = first_page(&fixture, "GeneratedRecord").await;
        let continuation = fixture
            .broker
            .record_continuation(&fixture.session, &first, &more("cursor-1"))
            .expect("continuation");
        let arguments = json!({
            "loadedReference": described.loaded_reference(),
            "continuation": continuation.handle(),
        });

        // Revoking the root after the first page refuses the next one.
        fixture.deny("GeneratedRecord");
        let error = fixture
            .execute(arguments.clone())
            .await
            .expect_err("revoked");
        assert!(matches!(error, AiError::Forbidden), "{error:?}");
        fixture.authority.denied.write().expect("policy").clear();
        fixture
            .execute(arguments.clone())
            .await
            .expect("readmitted");

        // A changed policy fingerprint that admits the same view refreshes
        // the binding, and the page runs.
        *fixture
            .authority
            .policy_fingerprint
            .write()
            .expect("policy") = "current-policy-v2".to_owned();
        fixture
            .execute(arguments.clone())
            .await
            .expect("refreshed under the new policy fingerprint");
        *fixture
            .authority
            .policy_fingerprint
            .write()
            .expect("policy") = "current-policy-v1".to_owned();

        // A schema change is drift too.
        *fixture.index.0.write().expect("index") = generated_index_with_schema(
            "generated-read-application",
            "generated-read",
            "target-policy-v1",
            Some("addedField"),
            true,
        );
        let error = fixture
            .execute(arguments.clone())
            .await
            .expect_err("schema drift");
        assert_input_error(&error, AI_CAPABILITY_STALE);
        *fixture.index.0.write().expect("index") = two_root_entity_index("target-policy-v1");
        fixture
            .execute(arguments.clone())
            .await
            .expect("index restored");

        // The handle lives no longer than its loaded binding, and an
        // expired binding records no new handle.
        // The exact expiry instant is already expired, as at issuance.
        fixture.clock.advance_seconds(30);
        let error = fixture.execute(arguments).await.expect_err("expired");
        assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
        assert!(
            fixture
                .broker
                .record_continuation(&fixture.session, &first, &more("cursor-9"))
                .is_none()
        );
    }

    #[tokio::test]
    async fn continuation_state_is_bounded_in_count_pages_and_size() {
        let fixture = EntityBrokerFixture::new();
        let (described, first) = first_page(&fixture, "GeneratedRecord").await;
        let oldest = fixture
            .broker
            .record_continuation(&fixture.session, &first, &more("cursor-0"))
            .expect("continuation");
        for page in 1..=AI_CAPABILITY_MAXIMUM_CONTINUATIONS {
            fixture
                .broker
                .record_continuation(&fixture.session, &first, &more(&format!("cursor-{page}")))
                .expect("continuation");
        }
        assert_eq!(
            fixture.session.continuation_count(),
            AI_CAPABILITY_MAXIMUM_CONTINUATIONS
        );
        let error = fixture
            .execute(json!({
                "loadedReference": described.loaded_reference(),
                "continuation": oldest.handle(),
            }))
            .await
            .expect_err("evicted");
        assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);

        // An oversized cursor ends the chain.
        let oversized = "c".repeat(crate::AI_GRAPHQL_QUERY_MAXIMUM_CURSOR_BYTES + 1);
        assert!(
            fixture
                .broker
                .record_continuation(&fixture.session, &first, &more(&oversized))
                .is_none()
        );

        // The chain stops at its page bound.
        let mut execution = first;
        while let Some(next) =
            fixture
                .broker
                .record_continuation(&fixture.session, &execution, &more("cursor"))
        {
            execution = fixture
                .execute(json!({
                    "loadedReference": described.loaded_reference(),
                    "continuation": next.handle(),
                }))
                .await
                .expect("continued page");
        }
        assert_eq!(
            execution.page_number(),
            AI_CAPABILITY_MAXIMUM_CONTINUATION_PAGES
        );
    }

    #[tokio::test]
    async fn every_handle_of_a_chain_draws_on_one_execution_budget() {
        let fixture = EntityBrokerFixture::new();
        let (described, first) = first_page(&fixture, "GeneratedRecord").await;
        let continuation = fixture
            .broker
            .record_continuation(&fixture.session, &first, &more("cursor-1"))
            .expect("continuation");
        let arguments = json!({
            "loadedReference": described.loaded_reference(),
            "continuation": continuation.handle(),
        });
        // Reusing one handle spends the chain's budget, which counts the
        // first page too.
        let mut redeemed = 0;
        let mut last = None;
        while let Ok(execution) = fixture.execute(arguments.clone()).await {
            redeemed += 1;
            last = Some(execution);
        }
        assert_eq!(redeemed, AI_CAPABILITY_MAXIMUM_CONTINUATION_PAGES - 1);
        let error = fixture.execute(arguments).await.expect_err("spent");
        assert_input_error(&error, AI_CAPABILITY_SELECTION_STALE);
        // A spent chain issues no new handle, from any of its pages.
        assert!(
            fixture
                .broker
                .record_continuation(&fixture.session, &last.expect("page"), &more("cursor-2"))
                .is_none()
        );
        // A new first page starts a new chain.
        let (_, fresh) = first_page(&fixture, "GeneratedRecord").await;
        assert!(
            fixture
                .broker
                .record_continuation(&fixture.session, &fresh, &more("cursor-3"))
                .is_some()
        );
    }

    #[tokio::test]
    async fn debug_output_never_carries_a_server_held_cursor() {
        let fixture = EntityBrokerFixture::new();
        let (described, first) = first_page(&fixture, "GeneratedRecord").await;
        let cursor = "server-held-cursor-7f3a";
        let continuation = fixture
            .broker
            .record_continuation(&fixture.session, &first, &more(cursor))
            .expect("continuation");
        let continued = fixture
            .execute(json!({
                "loadedReference": described.loaded_reference(),
                "continuation": continuation.handle(),
            }))
            .await
            .expect("continued page");
        assert_eq!(continued.continuation_cursor(), Some(cursor));
        for debug in [
            format!("{:?}", fixture.session),
            format!("{continued:?}"),
            format!("{:?}", more(cursor)),
        ] {
            assert!(!debug.contains(cursor), "{debug}");
        }
        assert!(format!("{continued:?}").contains("redacted"));
    }

    #[tokio::test]
    async fn executor_admission_refuses_an_evicted_or_expired_page_and_returns_its_budget() {
        let fixture = EntityBrokerFixture::new();
        let (described, first) = first_page(&fixture, "GeneratedRecord").await;
        // A first page is always admitted.
        assert!(fixture
            .broker
            .continuation_admission(&fixture.session, &first)(
        ));
        let continuation = fixture
            .broker
            .record_continuation(&fixture.session, &first, &more("cursor-1"))
            .expect("continuation");
        let arguments = json!({
            "loadedReference": described.loaded_reference(),
            "continuation": continuation.handle(),
        });
        let admitted = fixture
            .execute(arguments.clone())
            .await
            .expect("authorized");
        assert!(fixture
            .broker
            .continuation_admission(&fixture.session, &admitted)(
        ));
        assert!(!admitted.continuation_refused());

        // Evicted after authorization: refused at admission, once.
        let evicting = fixture
            .execute(arguments.clone())
            .await
            .expect("authorized");
        let chain_used = |execution: &AiCapabilityExecution| {
            execution
                .chain
                .as_ref()
                .expect("chain")
                .0
                .load(Ordering::SeqCst)
        };
        let before = chain_used(&evicting);
        for page in 0..AI_CAPABILITY_MAXIMUM_CONTINUATIONS {
            fixture
                .broker
                .record_continuation(&fixture.session, &first, &more(&format!("evict-{page}")))
                .expect("continuation");
        }
        let admission = fixture
            .broker
            .continuation_admission(&fixture.session, &evicting);
        assert!(!admission());
        assert!(!admission());
        assert!(evicting.continuation_refused());
        // The refused execution's reservation was returned, exactly once.
        assert_eq!(chain_used(&evicting), before - 1);

        // Expired after authorization: refused at admission too.
        let (described, first) = first_page(&fixture, "GeneratedRecord").await;
        let continuation = fixture
            .broker
            .record_continuation(&fixture.session, &first, &more("cursor-2"))
            .expect("continuation");
        let expiring = fixture
            .execute(json!({
                "loadedReference": described.loaded_reference(),
                "continuation": continuation.handle(),
            }))
            .await
            .expect("authorized");
        fixture.clock.advance_seconds(30);
        assert!(!fixture
            .broker
            .continuation_admission(&fixture.session, &expiring)(
        ));
        assert!(expiring.continuation_refused());
    }

    #[tokio::test]
    async fn recording_one_first_page_twice_shares_one_chain_budget() {
        let fixture = EntityBrokerFixture::new();
        let (described, first) = first_page(&fixture, "GeneratedRecord").await;
        let handles = (0..2)
            .map(|page| {
                fixture
                    .broker
                    .record_continuation(&fixture.session, &first, &more(&format!("cursor-{page}")))
                    .expect("continuation")
            })
            .collect::<Vec<_>>();
        let mut redeemed = 0;
        'redeem: loop {
            for handle in &handles {
                let execute = json!({
                    "loadedReference": described.loaded_reference(),
                    "continuation": handle.handle(),
                });
                if fixture.execute(execute).await.is_err() {
                    break 'redeem;
                }
                redeemed += 1;
            }
        }
        assert_eq!(redeemed, AI_CAPABILITY_MAXIMUM_CONTINUATION_PAGES - 1);
    }
}
