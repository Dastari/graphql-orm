//! Built-in provider adapter implementations.

mod mock;

#[cfg(any(
    feature = "provider-openai-core",
    feature = "provider-anthropic-core",
    feature = "provider-xai-core",
    feature = "provider-ollama-core",
    feature = "provider-openai-compatible-core"
))]
mod http;

#[cfg(feature = "provider-anthropic-core")]
mod anthropic;

#[cfg(any(
    feature = "provider-openai-core",
    feature = "provider-xai-core",
    feature = "provider-openai-compatible-core"
))]
mod openai;

#[cfg(feature = "provider-openai-core")]
mod openai_webhooks;

#[cfg(feature = "provider-openai-compatible-core")]
mod openai_compatible;

#[cfg(feature = "provider-xai-core")]
mod xai;

#[cfg(feature = "provider-ollama-core")]
mod ollama;

#[cfg(feature = "provider-codex-app-server")]
mod codex_app_server;

#[cfg(all(test, feature = "provider-codex-app-server"))]
pub(crate) use codex_app_server::tests::canonical_dynamic_tool_catalog;

#[cfg(all(test, feature = "provider-openai-core"))]
pub(crate) use mock::MockBackgroundRetrievalFailure;
pub use mock::MockProvider;

#[cfg(feature = "provider-anthropic-core")]
pub use anthropic::{AnthropicProvider, AnthropicProviderConfig};

#[cfg(feature = "provider-openai-core")]
pub use openai::{OpenAiFileDeletionService, OpenAiProvider, OpenAiProviderConfig};

#[cfg(all(
    feature = "provider-openai-core",
    any(feature = "sqlite", feature = "postgres")
))]
pub(crate) use openai_webhooks::webhook_receipt_identity;
#[cfg(feature = "provider-openai-core")]
pub use openai_webhooks::{
    OpenAiVerifiedWebhookEvent, OpenAiWebhookEventKind, OpenAiWebhookHeaders,
    OpenAiWebhookVerifier, OpenAiWebhookVerifierLimits,
};

#[cfg(feature = "provider-openai-compatible-core")]
pub use openai_compatible::{
    OpenAiCompatibleCapabilities, OpenAiCompatibleProvider, OpenAiCompatibleProviderConfig,
};

#[cfg(feature = "provider-xai-core")]
pub use xai::{XAiProvider, XAiProviderConfig};

#[cfg(feature = "provider-ollama-core")]
pub use ollama::{OllamaProvider, OllamaProviderConfig};

#[cfg(feature = "provider-codex-app-server")]
pub use codex_app_server::{
    AI_CODEX_APP_SERVER_PROTOCOL_V2, AiCodexAppServerBootstrapInstructions,
    AiCodexAppServerInbound, AiCodexAppServerLaunchProfile, AiCodexAppServerLaunchedProcess,
    AiCodexAppServerModelToolMode, AiCodexAppServerProtocolActor, AiCodexAppServerProvider,
    AiCodexAppServerRegistration, AiCodexAppServerRunLimits, AiCodexAppServerRunPool,
    AiCodexAppServerRunProcess, AiCodexAppServerRunProcessFactory, AiCodexAppServerTurnInput,
    AiCodexAppServerWebSearchTurnConfig,
};

#[cfg(feature = "provider-grok-acp")]
mod grok_acp;
#[cfg(feature = "provider-grok-acp")]
pub use grok_acp::{AiGrokAcpSdkBroker, AiGrokAcpSdkInbound, AiGrokAcpUsage};

#[cfg(feature = "provider-grok-acp")]
mod grok_acp_provider;
#[cfg(feature = "provider-grok-acp")]
pub use grok_acp_provider::{
    AiGrokAcpDeletionService, AiGrokAcpLaunchedProcess, AiGrokAcpProcessFactory, AiGrokAcpProvider,
    AiGrokAcpRegistration, AiGrokAcpRunProcess,
};

#[cfg(feature = "provider-grok-acp")]
mod grok_acp_wire;
#[cfg(feature = "provider-grok-acp")]
pub use grok_acp_wire::{AiGrokAcpWireProcess, AiGrokAcpWireTransport};

#[cfg(all(
    test,
    feature = "provider-grok-acp",
    any(feature = "sqlite", feature = "postgres")
))]
pub(crate) use grok_acp_wire::tests::ExecutorWireFactory;
