//! Immutable, admitted session routing choices. These are not execution authority.

use agql_auth::AuthPrincipal;
use async_graphql::{Enum, InputObject};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{AiError, AiProviderSessionDescriptor, AiScope, ModelReasoningEffort, ProviderKind};

/// GraphQL representation of a provider family, independent of host routing profiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Enum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_items = "PascalCase"))]
pub enum AiSessionProviderKind {
    /// OpenAI API.
    OpenAi,
    /// Anthropic API.
    Anthropic,
    /// xAI API.
    Xai,
    /// Ollama API.
    Ollama,
    /// Explicit OpenAI-compatible API.
    OpenAiCompatible,
    /// Deployment-registered installed harness.
    LocalHarness,
}

impl From<ProviderKind> for AiSessionProviderKind {
    fn from(value: ProviderKind) -> Self {
        match value {
            ProviderKind::OpenAi => Self::OpenAi,
            ProviderKind::Anthropic => Self::Anthropic,
            ProviderKind::Xai => Self::Xai,
            ProviderKind::Ollama => Self::Ollama,
            ProviderKind::OpenAiCompatible => Self::OpenAiCompatible,
            ProviderKind::LocalHarness => Self::LocalHarness,
        }
    }
}

impl From<AiSessionProviderKind> for ProviderKind {
    fn from(value: AiSessionProviderKind) -> Self {
        match value {
            AiSessionProviderKind::OpenAi => Self::OpenAi,
            AiSessionProviderKind::Anthropic => Self::Anthropic,
            AiSessionProviderKind::Xai => Self::Xai,
            AiSessionProviderKind::Ollama => Self::Ollama,
            AiSessionProviderKind::OpenAiCompatible => Self::OpenAiCompatible,
            AiSessionProviderKind::LocalHarness => Self::LocalHarness,
        }
    }
}

/// Untrusted requested session choice. Only the configured host resolver admits it.
#[derive(Clone, Debug, InputObject)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct AiSessionExecutionSelectionInput {
    /// Provider family.
    pub provider: AiSessionProviderKind,
    /// Host routing profile, not necessarily a retained-provider descriptor profile.
    pub profile_id: String,
    /// Exact discovered and admitted model.
    pub model: String,
    /// Explicit effort; unspecified defaults cannot be persisted.
    pub reasoning_effort: ModelReasoningEffort,
}

/// Validated immutable routing choice, never a provider, budget or tool permit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiSessionExecutionSelection {
    provider: AiSessionProviderKind,
    profile_id: Box<str>,
    model: Box<str>,
    reasoning_effort: ModelReasoningEffort,
}

#[cfg(not(feature = "graphql-case-pascal"))]
#[async_graphql::Object]
impl AiSessionExecutionSelection {
    #[graphql(name = "provider")]
    async fn gql_provider(&self) -> AiSessionProviderKind {
        self.provider
    }
    #[graphql(name = "profileId")]
    async fn gql_profile_id(&self) -> &str {
        self.profile_id()
    }
    #[graphql(name = "model")]
    async fn gql_model(&self) -> &str {
        self.model()
    }
    #[graphql(name = "reasoningEffort")]
    async fn gql_reasoning_effort(&self) -> ModelReasoningEffort {
        self.reasoning_effort()
    }
}

#[cfg(feature = "graphql-case-pascal")]
#[async_graphql::Object]
impl AiSessionExecutionSelection {
    #[graphql(name = "Provider")]
    async fn gql_provider(&self) -> AiSessionProviderKind {
        self.provider
    }
    #[graphql(name = "ProfileId")]
    async fn gql_profile_id(&self) -> &str {
        self.profile_id()
    }
    #[graphql(name = "Model")]
    async fn gql_model(&self) -> &str {
        self.model()
    }
    #[graphql(name = "ReasoningEffort")]
    async fn gql_reasoning_effort(&self) -> ModelReasoningEffort {
        self.reasoning_effort()
    }
}

impl AiSessionExecutionSelection {
    /// Creates a structurally validated choice. Host admission is still required.
    ///
    /// # Errors
    /// Returns invalid input for empty, excessive or control-bearing identifiers
    /// or an unspecified effort.
    pub fn new(
        provider: ProviderKind,
        profile_id: impl Into<String>,
        model: impl Into<String>,
        reasoning_effort: ModelReasoningEffort,
    ) -> Result<Self, AiError> {
        let value = Self {
            provider: provider.into(),
            profile_id: String::into_boxed_str(profile_id.into()),
            model: String::into_boxed_str(model.into()),
            reasoning_effort,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), AiError> {
        if [self.profile_id.as_ref(), self.model.as_ref()]
            .into_iter()
            .any(|v| {
                v.is_empty() || v.len() > 200 || v.trim() != v || v.chars().any(char::is_control)
            })
            || self.reasoning_effort == ModelReasoningEffort::Unspecified
        {
            return Err(AiError::InvalidInput(
                "invalid session execution selection".to_owned(),
            ));
        }
        Ok(())
    }

    /// Provider family.
    pub fn provider_kind(&self) -> ProviderKind {
        self.provider.into()
    }
    /// Host routing profile identifier.
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }
    /// Exact model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }
    /// Frozen explicit effort.
    pub const fn reasoning_effort(&self) -> ModelReasoningEffort {
        self.reasoning_effort
    }
    /// Canonical identity for bounded host runtime caches, not authority.
    pub fn fingerprint(&self) -> String {
        hex::encode(Sha256::digest(format!(
            "ai-session-execution-v1\0{}\0{}\0{}\0{}",
            self.provider_kind().as_str(),
            self.profile_id,
            self.model,
            self.reasoning_effort.as_str()
        )))
    }
    /// Returns the exact requested fields for re-admission without default fallback.
    pub fn as_input(&self) -> AiSessionExecutionSelectionInput {
        AiSessionExecutionSelectionInput {
            provider: self.provider,
            profile_id: self.profile_id.to_string(),
            model: self.model.to_string(),
            reasoning_effort: self.reasoning_effort,
        }
    }
    #[cfg(any(feature = "sqlite", feature = "postgres", test))]
    pub(crate) fn encode(&self) -> Result<String, AiError> {
        self.validate()?;
        serde_json::to_string(&(1u8, self)).map_err(|_| AiError::PersistenceFailed)
    }
    #[cfg(any(feature = "sqlite", feature = "postgres", test))]
    pub(crate) fn decode(value: &str) -> Result<Self, AiError> {
        if value.len() > 2_048 {
            return Err(AiError::PersistenceFailed);
        }
        let (version, selection): (u8, Self) =
            serde_json::from_str(value).map_err(|_| AiError::PersistenceFailed)?;
        if version != 1 || selection.validate().is_err() {
            return Err(AiError::PersistenceFailed);
        }
        Ok(selection)
    }
}

/// Host-owned catalogue/current-user admission for every session creation path.
/// A missing request must resolve an explicit reviewed default or fail; a supplied
/// request must never silently fall back. Re-admission must preserve exact identity.
#[async_trait]
pub trait AiSessionExecutionSelectionResolver: Send + Sync {
    /// Admits an exact current-user choice. Discovery alone is not admission.
    ///
    /// # Errors
    /// Returns unavailable or forbidden when this exact choice cannot be used.
    async fn resolve(
        &self,
        principal: &AuthPrincipal,
        scope: &AiScope,
        requested: Option<&AiSessionExecutionSelectionInput>,
    ) -> Result<AiSessionExecutionSelection, AiError>;

    /// Supplies an independently verified descriptor for explicit legacy pinning.
    /// It must describe the selected route/model/effort, never today's fallback.
    ///
    /// # Errors
    /// Defaults to unavailable. The service independently compares durable
    /// binding and historical explicit effort evidence before pinning.
    async fn legacy_descriptor(
        &self,
        _principal: &AuthPrincipal,
        _scope: &AiScope,
        _selection: &AiSessionExecutionSelection,
        _session_id: crate::AiSessionId,
        _observed: &AiProviderSessionDescriptor,
    ) -> Result<AiProviderSessionDescriptor, AiError> {
        Err(AiError::SessionExecutionUnavailable)
    }
}

/// Explicit owner request to bind a proven idle legacy session, never to reroute it.
#[derive(Clone, Debug, InputObject)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct PinAiSessionExecutionSelectionInput {
    /// Exact owned legacy session.
    pub session_id: uuid::Uuid,
    /// Requested choice requiring exact legacy descriptor and effort evidence.
    pub execution_selection: AiSessionExecutionSelectionInput,
}

/// Lease-fenced reader used by a host's single global claim/concurrency loop.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
#[async_trait]
pub trait AiRunExecutionSelectionReader: Send + Sync {
    /// Reads the enqueue-time snapshot after fresh owner authorization.
    ///
    /// # Errors
    /// Returns conflict for a stale lease, unbound for a legacy run, and the
    /// ordinary authorization/persistence errors. Never returns a new default.
    async fn selection_for_run(
        &self,
        lease: &crate::AiRunLease,
    ) -> Result<AiSessionExecutionSelection, AiError>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub(crate) async fn resolve_exact(
    resolver: &dyn AiSessionExecutionSelectionResolver,
    principal: &AuthPrincipal,
    scope: &AiScope,
    requested: Option<&AiSessionExecutionSelectionInput>,
) -> Result<AiSessionExecutionSelection, AiError> {
    let expected = requested
        .map(|v| {
            AiSessionExecutionSelection::new(
                v.provider.into(),
                v.profile_id.clone(),
                v.model.clone(),
                v.reasoning_effort,
            )
        })
        .transpose()?;
    let selection = resolver.resolve(principal, scope, requested).await?;
    selection.validate()?;
    if expected.as_ref().is_some_and(|v| v != &selection) {
        return Err(AiError::SessionExecutionUnavailable);
    }
    Ok(selection)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshots_are_versioned_validated_and_identity_complete() {
        let baseline = AiSessionExecutionSelection::new(
            ProviderKind::LocalHarness,
            "grok_acp",
            "newly-discovered-model",
            ModelReasoningEffort::Ultra,
        )
        .unwrap();
        assert_eq!(
            AiSessionExecutionSelection::decode(&baseline.encode().unwrap()).unwrap(),
            baseline
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&baseline.encode().unwrap()).unwrap(),
            serde_json::json!([1,{"provider":"local_harness","profile_id":"grok_acp","model":"newly-discovered-model","reasoning_effort":"ultra"}])
        );
        for other in [
            AiSessionExecutionSelection::new(
                ProviderKind::OpenAi,
                "grok_acp",
                "newly-discovered-model",
                ModelReasoningEffort::Ultra,
            )
            .unwrap(),
            AiSessionExecutionSelection::new(
                ProviderKind::LocalHarness,
                "codex_app_server",
                "newly-discovered-model",
                ModelReasoningEffort::Ultra,
            )
            .unwrap(),
            AiSessionExecutionSelection::new(
                ProviderKind::LocalHarness,
                "grok_acp",
                "other-model",
                ModelReasoningEffort::Ultra,
            )
            .unwrap(),
            AiSessionExecutionSelection::new(
                ProviderKind::LocalHarness,
                "grok_acp",
                "newly-discovered-model",
                ModelReasoningEffort::High,
            )
            .unwrap(),
        ] {
            assert_ne!(baseline.fingerprint(), other.fingerprint());
        }
        assert!(
            AiSessionExecutionSelection::decode(
                &baseline.encode().unwrap().replacen("[1,", "[2,", 1)
            )
            .is_err()
        );
        assert!(
            AiSessionExecutionSelection::decode(
                &baseline.encode().unwrap().replace("ultra", "unspecified")
            )
            .is_err()
        );
        assert!(
            AiSessionExecutionSelection::new(
                ProviderKind::LocalHarness,
                "bad\0profile",
                "model",
                ModelReasoningEffort::High
            )
            .is_err()
        );
    }
    #[test]
    fn ultra_requires_exact_model_profile_admission() {
        let unsupported = crate::ModelReasoningEffortProfile::new(
            "model",
            [ModelReasoningEffort::High],
            ModelReasoningEffort::High,
        )
        .unwrap();
        assert!(
            !unsupported
                .supported()
                .contains(&ModelReasoningEffort::Ultra)
        );
        let supported = crate::ModelReasoningEffortProfile::new(
            "model",
            [
                ModelReasoningEffort::None,
                ModelReasoningEffort::Low,
                ModelReasoningEffort::Medium,
                ModelReasoningEffort::High,
                ModelReasoningEffort::XHigh,
                ModelReasoningEffort::Max,
                ModelReasoningEffort::Ultra,
            ],
            ModelReasoningEffort::Ultra,
        )
        .unwrap();
        assert_eq!(supported.supported().len(), 7);
        assert_eq!(ModelReasoningEffort::Ultra.wire_value(), Some("ultra"));
    }
}
