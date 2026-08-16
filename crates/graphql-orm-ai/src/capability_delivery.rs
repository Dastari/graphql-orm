//! Provider-neutral capability delivery, loading, and run fencing.

use std::collections::BTreeSet;
use std::sync::Arc;

use agql_auth::{Clock, CurrentPrincipalResolver, PrincipalReference, ResolvedPrincipal};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    AiCapabilityIndex, AiCapabilityIndexEntry, AiCapabilityKind, AiCapabilitySearchQuery,
    AiCapabilitySearchResult, AiError, AiRunId, AiSessionId, AiToolId, ModelReasoningEffort,
    ModelToolDefinition, ProviderCapabilities, ProviderKind,
};

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
}

impl Default for AiCapabilityDeliveryLimits {
    fn default() -> Self {
        Self {
            maximum_eager_definitions: 16,
            maximum_eager_definition_bytes: 256 * 1024,
            maximum_deferred_definitions: 8,
        }
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
    /// for native deferred loading; fixed mode contains only the frozen broker.
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
    if loaded.is_empty()
        || loaded.len() != exact_definitions.len()
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
        "graphql.capabilities.discover",
        "graphql_capabilities_discover",
        "Search the current reviewed GraphQL capability index.",
        index_fingerprint,
        json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "maxLength": 1024},
                "namespace": {"type": ["string", "null"], "maxLength": 256},
                "kind": {"type": ["string", "null"], "enum": ["reviewed_static", "generated_query", "generated_mutation", "generated_subscription", null]},
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
        "graphql.capabilities.describe",
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
        "graphql.capabilities.execute",
        "graphql_capabilities_execute",
        "Execute one previously loaded exact capability using a closed public-name query plan.",
        index_fingerprint,
        json!({
            "type": "object",
            "properties": {
                "loadedReference": {"type": "string", "minLength": 64, "maxLength": 64},
                "arguments": {
                    "type": "array", "maxItems": 64,
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string", "maxLength": 256},
                            "value": {"type": ["string", "integer", "number", "boolean", "null"]}
                        },
                        "required": ["name", "value"],
                        "additionalProperties": false
                    }
                },
                "selections": {"type": "array", "minItems": 1, "maxItems": 256, "uniqueItems": true, "items": {"type": "string", "maxLength": 512}}
            },
            "required": ["loadedReference", "arguments", "selections"],
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
    if limits.maximum_eager_definitions == 0
        || limits.maximum_eager_definition_bytes == 0
        || limits.maximum_deferred_definitions == 0
    {
        return Err(AiError::InvalidConfiguration(
            "capability delivery limits are invalid".to_owned(),
        ));
    }
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
    } else if supported.contains(&AiCapabilityDeliveryMode::ClientDeferred) {
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
    /// Canonical compact-index fingerprint.
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

    /// Exact compact-index fingerprint.
    pub fn capability_index_fingerprint(&self) -> &str {
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
    current_index: Arc<dyn AiCurrentCapabilityIndex>,
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
        current_index: Arc<dyn AiCurrentCapabilityIndex>,
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
            current_index,
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
    ) -> Result<AiCapabilitySearchResult, AiError> {
        let principal = self.rehydrate(principal_reference).await?;
        let index = self.current_index.current_index(run)?;
        let mut result = index.search(query)?;
        let mut permitted = Vec::new();
        for candidate in result.candidates {
            let entry = index.entry(&candidate.id).ok_or(AiError::Forbidden)?;
            let decision = self.authority.authorize(&principal, entry, run).await?;
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
        search: &AiCapabilitySearchResult,
        capability_id: &AiToolId,
    ) -> Result<AiLoadedCapabilityBinding, AiError> {
        let principal = self.rehydrate(principal_reference).await?;
        let index = self.current_index.current_index(run)?;
        verify_search_binding(&index, search)?;
        let candidate = search
            .candidates
            .iter()
            .find(|candidate| &candidate.id == capability_id)
            .ok_or(AiError::Forbidden)?;
        let entry = index.entry(capability_id).ok_or(AiError::Forbidden)?;
        if candidate.kind != entry.kind
            || candidate.capability_fingerprint != entry.capability_fingerprint
            || candidate.entry_fingerprint != entry.fingerprint
        {
            return Err(AiError::Forbidden);
        }
        let decision = self.authority.authorize(&principal, entry, run).await?;
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
            || loaded.lease_generation != run.lease_generation
            || loaded.provider_kind != run.provider_kind
            || loaded.provider_session_fingerprint != run.provider_session_fingerprint
        {
            return Err(AiError::Forbidden);
        }
        let principal = self.rehydrate(principal_reference).await?;
        let index = self.current_index.current_index(run)?;
        let entry = index
            .entry(&loaded.capability_id)
            .ok_or(AiError::Forbidden)?;
        if loaded.capability_kind != entry.kind
            || loaded.capability_fingerprint != entry.capability_fingerprint
            || loaded.entry_fingerprint != entry.fingerprint
            || loaded.index_fingerprint != index.fingerprint()
            || loaded.schema_fingerprint != index.schema_fingerprint()
            || loaded.semantic_catalogue_fingerprint != index.semantic_catalogue_fingerprint()
            || loaded.target_policy_fingerprint != index.target_policy_fingerprint()
        {
            return Err(AiError::Forbidden);
        }
        let decision = self.authority.authorize(&principal, entry, run).await?;
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
    index: &AiCapabilityIndex,
    search: &AiCapabilitySearchResult,
) -> Result<(), AiError> {
    if search.index_fingerprint != index.fingerprint()
        || search.schema_fingerprint != index.schema_fingerprint()
        || search.semantic_catalogue_fingerprint != index.semantic_catalogue_fingerprint()
        || search.target_policy_fingerprint != index.target_policy_fingerprint()
    {
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

#[cfg(test)]
mod tests {
    use std::sync::{
        RwLock,
        atomic::{AtomicBool, Ordering},
    };

    use agql_auth::{AccessTokenMetadata, AuthPrincipal, AuthUser, FixedClock, SessionContext};
    use graphql_orm::graphql::orm::{
        GeneratedGraphqlOperationDescriptor, GraphqlOperationCatalog, GraphqlSemanticCatalog,
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

    struct Authority {
        allowed: AtomicBool,
        policy_fingerprint: RwLock<String>,
    }

    #[async_trait]
    impl AiCapabilityAuthorityPolicy for Authority {
        async fn authorize(
            &self,
            _principal: &ResolvedPrincipal,
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
        let broker = AiCapabilityDiscoveryBroker::new(
            Arc::new(Resolver(principal)),
            current_index.clone(),
            authority.clone(),
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

        let mut kind_substitution = search;
        kind_substitution.candidates[0].kind = AiCapabilityKind::GeneratedQuery;
        assert!(matches!(
            broker
                .load(&principal_reference, &run, &kind_substitution, &id)
                .await,
            Err(AiError::Forbidden)
        ));
    }
}
