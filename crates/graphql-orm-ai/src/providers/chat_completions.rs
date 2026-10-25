//! Explicitly profiled OpenAI Chat Completions-compatible adapter.
//!
//! Many self-hosted and local model servers (Ollama, LM Studio, vLLM,
//! llama.cpp server, LocalAI and gateways) expose only
//! `POST /v1/chat/completions` with `chat.completion.chunk` server-sent
//! events. Every turn is a stateless replay of the bounded conversation; the
//! endpoint never retains provider state for the engine.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::header::CONTENT_TYPE;
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use url::Url;

use super::sse::SseDecoder;
use crate::{
    AiOpenAiCompatibleProfileView, AiProvider, AiProviderEndpointPolicy, AiProviderKindInput,
    AiProviderProfileView, AiSecretStore, ModelContinuation, ModelContinuationMode,
    ModelConversationMessage, ModelInputBlock, ModelRequest, ModelToolDefinition,
    ProviderCapabilities, ProviderError, ProviderEvent, ProviderEventStream, ProviderKind,
    ProviderRequestContext, SecretRef,
};

const MAXIMUM_SSE_EVENT_BYTES: usize = 2 * 1024 * 1024;
const MAXIMUM_STREAM_EVENTS: usize = 65_536;
const MAXIMUM_TOOL_CALLS: usize = 64;
const MAXIMUM_TOOL_ARGUMENT_BYTES: usize = 16 * 1024 * 1024;
const MAXIMUM_VISIBLE_TEXT_BYTES: usize = 64 * 1024 * 1024;
const MAXIMUM_SCHEMA_BYTES: usize = 256 * 1024;
const MAXIMUM_CALL_ID_BYTES: usize = 128;

/// Immutable capabilities declared for one Chat Completions endpoint.
///
/// These flags are deployment-reviewed assertions, never probed from the
/// endpoint or model. Streaming text is always required. Attachments,
/// provider built-ins, background work and provider-retained continuation are
/// structurally unavailable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatCompletionsCapabilities {
    /// Permit strict function tools; tool turns use stateless replay.
    pub custom_tools: bool,
    /// Permit more than one tool call in one response.
    pub parallel_tool_calls: bool,
    /// Permit JSON-schema structured output (`response_format`).
    pub structured_output: bool,
}

impl ChatCompletionsCapabilities {
    fn validate(&self) -> Result<(), ProviderError> {
        if self.parallel_tool_calls && !self.custom_tools {
            return Err(ProviderError::InvalidConfiguration(
                "parallel Chat Completions tools require custom-tool capability".to_owned(),
            ));
        }
        Ok(())
    }

    fn provider_capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            capability_delivery_modes: if self.custom_tools {
                [
                    crate::AiCapabilityDeliveryMode::EagerExact,
                    crate::AiCapabilityDeliveryMode::FixedBroker,
                ]
                .into_iter()
                .collect()
            } else {
                Default::default()
            },
            streaming: true,
            custom_tools: self.custom_tools,
            parallel_tool_calls: self.parallel_tool_calls,
            structured_output: self.structured_output,
            stateless_continuation: self.custom_tools,
            ..ProviderCapabilities::default()
        }
    }
}

impl From<&AiOpenAiCompatibleProfileView> for ChatCompletionsCapabilities {
    fn from(profile: &AiOpenAiCompatibleProfileView) -> Self {
        Self {
            custom_tools: profile.custom_tools,
            parallel_tool_calls: profile.parallel_tool_calls,
            structured_output: profile.structured_output,
        }
    }
}

/// Deployment-owned immutable Chat Completions profile.
///
/// The exact profile ID, normalized endpoint and retention label are bound to
/// every inference and tool-result egress manifest. A credential is optional,
/// because local servers often need none; its plaintext is resolved only
/// immediately before transport.
#[derive(Clone)]
pub struct ChatCompletionsProviderConfig {
    /// Stable GraphQL-managed provider-profile ID.
    pub profile_id: String,
    /// Exact deployment-configured `/chat/completions` endpoint.
    pub endpoint: String,
    /// Optional secret-store reference for a Bearer credential.
    pub credential: Option<SecretRef>,
    /// Exact provider retention label expected in every egress manifest.
    pub retention: String,
    /// Overall HTTP request and stream timeout.
    pub timeout: Duration,
    /// Reviewed endpoint capabilities.
    pub capabilities: ChatCompletionsCapabilities,
}

impl ChatCompletionsProviderConfig {
    /// Creates a text-only profile.
    pub fn new(
        profile_id: impl Into<String>,
        endpoint: impl Into<String>,
        credential: Option<SecretRef>,
        retention: impl Into<String>,
    ) -> Self {
        Self {
            profile_id: profile_id.into(),
            endpoint: endpoint.into(),
            credential,
            retention: retention.into(),
            timeout: Duration::from_secs(120),
            capabilities: ChatCompletionsCapabilities::default(),
        }
    }

    /// Creates a transport profile from one redacted GraphQL-managed view and
    /// its separately loaded secret reference, if the profile has a credential.
    ///
    /// The view does not prove endpoint authorization, credential availability
    /// or egress permission; [`ChatCompletionsProvider::new`] still validates
    /// the endpoint and every request needs exact egress and budget proofs.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidConfiguration`] unless the view is an
    /// enabled, fully configured `chat_completions` profile with an endpoint,
    /// and `credential` is present exactly when the profile has one.
    pub fn from_profile(
        profile: &AiProviderProfileView,
        credential: Option<SecretRef>,
    ) -> Result<Self, ProviderError> {
        let invalid = || {
            ProviderError::InvalidConfiguration(
                "provider profile is not an enabled Chat Completions route".to_owned(),
            )
        };
        let contract = profile.openai_compatible.as_ref().ok_or_else(invalid)?;
        let endpoint = profile.base_url.clone().ok_or_else(invalid)?;
        if profile.provider_kind != AiProviderKindInput::ChatCompletions.as_str()
            || !profile.enabled
            || contract.provider_retained_continuation
            || profile.credential_configured != credential.is_some()
        {
            return Err(invalid());
        }
        let mut config = Self::new(
            profile.id.to_string(),
            endpoint,
            credential,
            contract.retention.clone(),
        );
        config.capabilities = contract.into();
        Ok(config)
    }
}

impl std::fmt::Debug for ChatCompletionsProviderConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChatCompletionsProviderConfig")
            .field("profile_id", &self.profile_id)
            .field("endpoint", &"<deployment-authorized>")
            .field(
                "credential",
                &self.credential.as_ref().map(|_| "<secret-reference>"),
            )
            .field("retention", &self.retention)
            .field("timeout", &self.timeout)
            .field("capabilities", &self.capabilities)
            .finish()
    }
}

/// Chat Completions provider with immutable endpoint and profile bounds.
///
/// The adapter never probes capabilities, never accepts a runtime URL, follows
/// no redirects and never echoes response bodies in errors.
pub struct ChatCompletionsProvider {
    config: ChatCompletionsProviderConfig,
    secrets: Arc<dyn AiSecretStore>,
    client: reqwest::Client,
    endpoint: Url,
    capabilities: ProviderCapabilities,
}

impl std::fmt::Debug for ChatCompletionsProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChatCompletionsProvider")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl ChatCompletionsProvider {
    /// Builds an adapter only after deployment authorization of the exact
    /// normalized endpoint.
    ///
    /// The endpoint policy must enforce permitted hosts, ports, network zones
    /// and TLS requirements; string authorization alone cannot prevent DNS
    /// rebinding.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidConfiguration`] for an unsafe or denied
    /// endpoint, invalid profile, retention, capability or timeout data, or an
    /// HTTP client construction failure.
    pub fn new(
        config: ChatCompletionsProviderConfig,
        endpoint_policy: Arc<dyn AiProviderEndpointPolicy>,
        secrets: Arc<dyn AiSecretStore>,
    ) -> Result<Self, ProviderError> {
        validate_config(&config)?;
        let endpoint = super::endpoint::normalized_provider_endpoint(&config.endpoint)?;
        if !endpoint_policy
            .authorize_endpoint(AiProviderKindInput::ChatCompletions, endpoint.as_str())
        {
            return Err(ProviderError::InvalidConfiguration(
                "Chat Completions endpoint was not authorized".to_owned(),
            ));
        }
        let client = super::http::client_builder()?
            .redirect(reqwest::redirect::Policy::none())
            .timeout(config.timeout)
            .build()
            .map_err(|_| {
                ProviderError::InvalidConfiguration(
                    "Chat Completions HTTP client could not be constructed".to_owned(),
                )
            })?;
        let capabilities = config.capabilities.provider_capabilities();
        Ok(Self {
            config,
            secrets,
            client,
            endpoint,
            capabilities,
        })
    }

    fn request_body(&self, request: &ModelRequest) -> Result<Value, ProviderError> {
        request.validate()?;
        if request.input.is_empty()
            || request.maximum_output_tokens.is_none()
            || request.tools.iter().any(|tool| !tool.strict)
        {
            return Err(ProviderError::InvalidRequest);
        }
        if !request.builtin_tools.is_empty()
            || request.reasoning_summary.maximum_bytes().is_some()
            || request.tools.iter().any(|tool| tool.defer_loading)
            || (!request.tools.is_empty() && !self.config.capabilities.custom_tools)
            || (request.output_schema.is_some() && !self.config.capabilities.structured_output)
            || request
                .input
                .iter()
                .any(|block| matches!(block, ModelInputBlock::Attachment { .. }))
        {
            return Err(ProviderError::Unsupported);
        }
        if request.continuation_mode == ModelContinuationMode::ProviderRetained
            && (request.continuation.is_some()
                || !request.tools.is_empty()
                || request
                    .input
                    .iter()
                    .any(|block| matches!(block, ModelInputBlock::ToolResult { .. })))
        {
            return Err(ProviderError::Unsupported);
        }
        let definitions = request
            .tools
            .iter()
            .map(|tool| (tool.tool_id.as_str(), tool))
            .collect::<BTreeMap<_, _>>();
        let instructions = match &request.continuation {
            Some(ModelContinuation::StatelessConversation { instructions, .. }) => instructions,
            Some(ModelContinuation::ProviderResponse { .. }) => {
                return Err(ProviderError::Unsupported);
            }
            None => &request.instructions,
        };
        let mut messages = Vec::new();
        if !instructions.is_empty() {
            messages.push(json!({"role": "system", "content": instructions.join("\n\n")}));
        }
        match &request.continuation {
            None => messages.push(user_message(&request.input)?),
            Some(ModelContinuation::StatelessConversation {
                messages: history, ..
            }) => {
                for message in history {
                    messages.push(conversation_message(message, &definitions)?);
                }
                let Some(ModelConversationMessage::Assistant { tool_calls, .. }) = history.last()
                else {
                    return Err(ProviderError::InvalidRequest);
                };
                if tool_calls.len() != request.input.len() {
                    return Err(ProviderError::InvalidRequest);
                }
                for (call, input) in tool_calls.iter().zip(&request.input) {
                    let ModelInputBlock::ToolResult {
                        call_id,
                        tool_id,
                        output,
                    } = input
                    else {
                        return Err(ProviderError::InvalidRequest);
                    };
                    if call_id != &call.call_id || tool_id != &call.tool_id {
                        return Err(ProviderError::InvalidRequest);
                    }
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "content": output.to_string()
                    }));
                }
            }
            Some(ModelContinuation::ProviderResponse { .. }) => {
                return Err(ProviderError::Unsupported);
            }
        }
        let maximum_output_tokens = request.maximum_output_tokens.unwrap_or_default();
        let mut body = json!({
            "model": request.model,
            "messages": messages,
            "stream": true,
            "stream_options": {"include_usage": true},
            "max_tokens": maximum_output_tokens
        });
        if !request.tools.is_empty() {
            body["tools"] = Value::Array(request.tools.iter().map(tool_definition).collect());
            body["parallel_tool_calls"] = Value::Bool(self.config.capabilities.parallel_tool_calls);
        }
        if let Some(schema) = &request.output_schema {
            if !schema.is_object()
                || serde_json::to_vec(schema)
                    .map_or(true, |encoded| encoded.len() > MAXIMUM_SCHEMA_BYTES)
            {
                return Err(ProviderError::InvalidRequest);
            }
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {"name": "response", "schema": schema, "strict": true}
            });
        }
        Ok(body)
    }
}

#[async_trait]
impl AiProvider for ChatCompletionsProvider {
    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::ChatCompletions
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn prepare_dispatch(
        &self,
        request: &ModelRequest,
        context: &ProviderRequestContext,
    ) -> Result<(), ProviderError> {
        self.authorize(request, context)?;
        self.request_body(request)?;
        if let Some(credential) = &self.config.credential {
            self.secrets
                .resolve(credential)
                .await
                .map_err(|_| ProviderError::CredentialUnavailable)?;
        }
        Ok(())
    }

    async fn stream(
        &self,
        request: ModelRequest,
        context: ProviderRequestContext,
    ) -> Result<ProviderEventStream, ProviderError> {
        self.authorize(&request, &context)?;
        let body = self.request_body(&request)?;
        let mut post = self.client.post(self.endpoint.clone()).json(&body);
        if let Some(credential) = &self.config.credential {
            let secret = self
                .secrets
                .resolve(credential)
                .await
                .map_err(|_| ProviderError::CredentialUnavailable)?;
            post = post.bearer_auth(secret.expose_secret());
        }
        let response = post.send().await.map_err(|_| ProviderError::Unavailable)?;
        if let Some(error) = http_error(response.status()) {
            return Err(error);
        }
        if response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_none_or(|value| !value.to_ascii_lowercase().starts_with("text/event-stream"))
        {
            return Err(ProviderError::Rejected);
        }
        let offered_tools = request
            .tools
            .iter()
            .map(|tool| (tool.provider_name.clone(), tool.tool_id.clone()))
            .collect();
        let mut normalizer = ChunkNormalizer::new(
            request.model,
            offered_tools,
            self.config.capabilities.parallel_tool_calls,
        );
        let mut bytes = response.bytes_stream();
        let output = async_stream::try_stream! {
            let mut decoder = SseDecoder::new(MAXIMUM_SSE_EVENT_BYTES);
            while let Some(chunk) = bytes.next().await {
                let chunk = chunk.map_err(|_| ProviderError::Unavailable)?;
                for payload in decoder.push(&chunk)? {
                    let value: Value = serde_json::from_str(&payload)
                        .map_err(|_| ProviderError::Rejected)?;
                    for event in normalizer.normalize(&value)? {
                        yield event;
                    }
                }
            }
            decoder.finish()?;
            normalizer.finish()?;
        };
        Ok(Box::pin(output))
    }
}

impl ChatCompletionsProvider {
    fn authorize(
        &self,
        request: &ModelRequest,
        context: &ProviderRequestContext,
    ) -> Result<(), ProviderError> {
        context.validate_request(&ProviderKind::ChatCompletions, request)?;
        if !context.permits_profile_destination_retention(
            &ProviderKind::ChatCompletions,
            request,
            &self.config.profile_id,
            self.endpoint.as_str(),
            &self.config.retention,
        ) {
            return Err(ProviderError::EgressDenied);
        }
        Ok(())
    }
}

fn http_error(status: reqwest::StatusCode) -> Option<ProviderError> {
    if status.is_success() {
        None
    } else if status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
    {
        Some(ProviderError::CredentialUnavailable)
    } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        Some(ProviderError::RateLimited)
    } else if status.is_server_error() {
        Some(ProviderError::Unavailable)
    } else {
        Some(ProviderError::Rejected)
    }
}

fn tool_definition(tool: &ModelToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.provider_name,
            "description": tool.description,
            "parameters": tool.parameters,
            "strict": tool.strict
        }
    })
}

fn text_content(input: &[ModelInputBlock]) -> Result<String, ProviderError> {
    input
        .iter()
        .map(|block| match block {
            ModelInputBlock::Text { text } => Ok(text.clone()),
            ModelInputBlock::Json { value } => Ok(value.to_string()),
            ModelInputBlock::Attachment { .. } => Err(ProviderError::Unsupported),
            ModelInputBlock::ToolResult { .. } => Err(ProviderError::InvalidRequest),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("\n\n"))
}

fn user_message(input: &[ModelInputBlock]) -> Result<Value, ProviderError> {
    Ok(json!({"role": "user", "content": text_content(input)?}))
}

fn conversation_message(
    message: &ModelConversationMessage,
    definitions: &BTreeMap<&str, &ModelToolDefinition>,
) -> Result<Value, ProviderError> {
    match message {
        ModelConversationMessage::User { content } => user_message(content),
        ModelConversationMessage::Assistant {
            content,
            tool_calls,
        } => {
            let mut message = json!({"role": "assistant", "content": content});
            if !tool_calls.is_empty() {
                let calls = tool_calls
                    .iter()
                    .map(|call| {
                        let definition = definitions
                            .get(call.tool_id.as_str())
                            .ok_or(ProviderError::InvalidRequest)?;
                        if definition.provider_name != call.provider_name
                            || definition.fingerprint != call.tool_fingerprint
                        {
                            return Err(ProviderError::InvalidRequest);
                        }
                        Ok(json!({
                            "id": call.call_id,
                            "type": "function",
                            "function": {
                                "name": call.provider_name,
                                "arguments": call.arguments.to_string()
                            }
                        }))
                    })
                    .collect::<Result<Vec<_>, ProviderError>>()?;
                message["tool_calls"] = Value::Array(calls);
            }
            Ok(message)
        }
        ModelConversationMessage::Tool {
            call_id,
            tool_id,
            provider_name,
            output,
        } => {
            let definition = definitions
                .get(tool_id.as_str())
                .ok_or(ProviderError::InvalidRequest)?;
            if definition.provider_name != *provider_name {
                return Err(ProviderError::InvalidRequest);
            }
            Ok(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": output.to_string()
            }))
        }
    }
}

fn validate_config(config: &ChatCompletionsProviderConfig) -> Result<(), ProviderError> {
    let reference = |value: &str| {
        !value.is_empty()
            && value.len() <= 200
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
    };
    if !reference(&config.profile_id)
        || !reference(&config.retention)
        || config.timeout.is_zero()
        || config.timeout > Duration::from_secs(600)
    {
        return Err(ProviderError::InvalidConfiguration(
            "invalid Chat Completions profile binding".to_owned(),
        ));
    }
    config.capabilities.validate()
}

struct CallState {
    call_id: String,
    arguments: String,
}

/// Normalizes `chat.completion.chunk` events into provider events.
struct ChunkNormalizer {
    expected_model: String,
    offered_tools: BTreeMap<String, String>,
    parallel_tool_calls: bool,
    calls: BTreeMap<u64, CallState>,
    started: bool,
    finished: bool,
    completed: bool,
    visible_bytes: usize,
    wire_events: usize,
}

impl ChunkNormalizer {
    fn new(
        expected_model: String,
        offered_tools: BTreeMap<String, String>,
        parallel_tool_calls: bool,
    ) -> Self {
        Self {
            expected_model,
            offered_tools,
            parallel_tool_calls,
            calls: BTreeMap::new(),
            started: false,
            finished: false,
            completed: false,
            visible_bytes: 0,
            wire_events: 0,
        }
    }

    fn normalize(&mut self, chunk: &Value) -> Result<Vec<ProviderEvent>, ProviderError> {
        self.wire_events = self
            .wire_events
            .checked_add(1)
            .filter(|count| *count <= MAXIMUM_STREAM_EVENTS)
            .ok_or(ProviderError::Rejected)?;
        if self.completed
            || chunk.get("error").is_some()
            || chunk.get("model").and_then(Value::as_str) != Some(self.expected_model.as_str())
            || chunk
                .get("object")
                .is_some_and(|object| object.as_str() != Some("chat.completion.chunk"))
        {
            return Err(ProviderError::Rejected);
        }
        let mut events = Vec::new();
        if !self.started {
            self.started = true;
            events.push(ProviderEvent::ResponseStarted { response_id: None });
        }
        let choices = match chunk.get("choices") {
            None | Some(Value::Null) => &[][..],
            Some(Value::Array(choices)) => choices.as_slice(),
            Some(_) => return Err(ProviderError::Rejected),
        };
        for choice in choices {
            if choice.get("index").and_then(Value::as_u64) != Some(0) || self.finished {
                return Err(ProviderError::Rejected);
            }
            let delta = choice.get("delta").unwrap_or(&Value::Null);
            match delta.get("content") {
                None | Some(Value::Null) => {}
                Some(Value::String(text)) => {
                    if !text.is_empty() {
                        self.visible_bytes = self
                            .visible_bytes
                            .checked_add(text.len())
                            .filter(|bytes| *bytes <= MAXIMUM_VISIBLE_TEXT_BYTES)
                            .ok_or(ProviderError::Rejected)?;
                        events.push(ProviderEvent::TextDelta { text: text.clone() });
                    }
                }
                Some(_) => return Err(ProviderError::Rejected),
            }
            match delta.get("tool_calls") {
                None | Some(Value::Null) => {}
                Some(Value::Array(calls)) => {
                    for call in calls {
                        self.tool_call_fragment(call, &mut events)?;
                    }
                }
                Some(_) => return Err(ProviderError::Rejected),
            }
            match choice.get("finish_reason") {
                None | Some(Value::Null) => {}
                Some(Value::String(reason)) => {
                    // Only a complete answer or a complete tool request ends a
                    // turn; truncated or filtered output is refused. Servers
                    // differ on `stop` versus `tool_calls` after a tool call.
                    if !matches!(reason.as_str(), "stop" | "tool_calls") {
                        return Err(ProviderError::Rejected);
                    }
                    self.finished = true;
                    for state in std::mem::take(&mut self.calls).into_values() {
                        let arguments: Value = serde_json::from_str(&state.arguments)
                            .ok()
                            .filter(Value::is_object)
                            .ok_or(ProviderError::Rejected)?;
                        events.push(ProviderEvent::ToolCallCompleted {
                            call_id: state.call_id,
                            arguments,
                        });
                    }
                }
                Some(_) => return Err(ProviderError::Rejected),
            }
        }
        match chunk.get("usage") {
            None | Some(Value::Null) => {}
            Some(usage) => {
                if !self.finished {
                    return Err(ProviderError::Rejected);
                }
                let input_tokens = usage
                    .get("prompt_tokens")
                    .and_then(Value::as_u64)
                    .ok_or(ProviderError::Rejected)?;
                let output_tokens = usage
                    .get("completion_tokens")
                    .and_then(Value::as_u64)
                    .ok_or(ProviderError::Rejected)?;
                let cached_input_tokens = usage
                    .get("prompt_tokens_details")
                    .and_then(|details| details.get("cached_tokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                if cached_input_tokens > input_tokens {
                    return Err(ProviderError::Rejected);
                }
                self.completed = true;
                events.push(ProviderEvent::Usage {
                    input_tokens,
                    output_tokens,
                    cached_input_tokens,
                });
                events.push(ProviderEvent::ResponseCompleted { response_id: None });
            }
        }
        Ok(events)
    }

    fn tool_call_fragment(
        &mut self,
        call: &Value,
        events: &mut Vec<ProviderEvent>,
    ) -> Result<(), ProviderError> {
        let index = call
            .get("index")
            .and_then(Value::as_u64)
            .ok_or(ProviderError::Rejected)?;
        let function = call.get("function").unwrap_or(&Value::Null);
        if !self.calls.contains_key(&index) {
            if self.calls.len() >= MAXIMUM_TOOL_CALLS
                || (!self.parallel_tool_calls && !self.calls.is_empty())
            {
                return Err(ProviderError::Rejected);
            }
            let provider_name = function
                .get("name")
                .and_then(Value::as_str)
                .ok_or(ProviderError::Rejected)?;
            let tool_id = self
                .offered_tools
                .get(provider_name)
                .ok_or(ProviderError::Rejected)?
                .clone();
            // Some local servers omit call IDs; the ID is opaque and only
            // pairs the call with its replayed result, so one is generated.
            let call_id = match call.get("id") {
                None | Some(Value::Null) => format!("chat-{}", uuid::Uuid::new_v4().simple()),
                Some(Value::String(id))
                    if !id.is_empty()
                        && id.len() <= MAXIMUM_CALL_ID_BYTES
                        && id.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                        }) =>
                {
                    id.clone()
                }
                Some(_) => return Err(ProviderError::Rejected),
            };
            if self.calls.values().any(|state| state.call_id == call_id) {
                return Err(ProviderError::Rejected);
            }
            events.push(ProviderEvent::ToolCallStarted {
                call_id: call_id.clone(),
                tool_id,
            });
            self.calls.insert(
                index,
                CallState {
                    call_id,
                    arguments: String::new(),
                },
            );
        } else if function
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| !name.is_empty())
        {
            // A later fragment cannot rename an open call.
            return Err(ProviderError::Rejected);
        }
        match function.get("arguments") {
            None | Some(Value::Null) => {}
            Some(Value::String(fragment)) => {
                if !fragment.is_empty() {
                    let state = self.calls.get_mut(&index).ok_or(ProviderError::Rejected)?;
                    if state.arguments.len() + fragment.len() > MAXIMUM_TOOL_ARGUMENT_BYTES {
                        return Err(ProviderError::Rejected);
                    }
                    state.arguments.push_str(fragment);
                    events.push(ProviderEvent::ToolArgumentsDelta {
                        call_id: state.call_id.clone(),
                        delta: fragment.clone(),
                    });
                }
            }
            Some(_) => return Err(ProviderError::Rejected),
        }
        Ok(())
    }

    fn finish(&self) -> Result<(), ProviderError> {
        if !self.finished {
            return Err(ProviderError::Unavailable);
        }
        if !self.completed {
            // Settlement requires authoritative usage; an endpoint that never
            // reports it fails every turn instead of being estimated.
            tracing::warn!(
                target: "graphql_orm_ai::provider",
                "Chat Completions stream ended without usage; turn refused"
            );
            return Err(ProviderError::Rejected);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use futures::TryStreamExt;
    use secrecy::SecretString;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    use super::*;
    use crate::{
        AiBudgetAmounts, AiBudgetReservation, AiBudgetReservationId, AiDataSourceRef,
        AiDestinationTrust, AiEgressCapability, AiEgressDecision, AiEgressManifest, AiRunId,
        AiScope, AiSessionId, AiSourceTrust, DataClassification, ModelConversationToolCall,
        SecretError,
    };

    struct Policy(String, bool);

    impl AiProviderEndpointPolicy for Policy {
        fn authorize_endpoint(&self, kind: AiProviderKindInput, normalized_url: &str) -> bool {
            kind == AiProviderKindInput::ChatCompletions && self.1 && normalized_url == self.0
        }
    }

    struct Secrets(SecretRef);

    #[async_trait]
    impl AiSecretStore for Secrets {
        async fn resolve(&self, reference: &SecretRef) -> Result<SecretString, SecretError> {
            if reference == &self.0 {
                Ok(SecretString::from("synthetic-chat-key".to_owned()))
            } else {
                Err(SecretError::Unavailable)
            }
        }
        async fn put(
            &self,
            _: Option<&SecretRef>,
            _: SecretString,
        ) -> Result<SecretRef, SecretError> {
            Err(SecretError::ReadOnly)
        }
        async fn delete(&self, _: &SecretRef) -> Result<(), SecretError> {
            Err(SecretError::ReadOnly)
        }
    }

    fn reference() -> SecretRef {
        SecretRef::parse("chat/test").expect("reference should parse")
    }

    fn tool() -> ModelToolDefinition {
        ModelToolDefinition {
            tool_id: "records.read".to_owned(),
            provider_name: "records_read".to_owned(),
            fingerprint: "reviewed-fingerprint".to_owned(),
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

    fn request() -> ModelRequest {
        ModelRequest {
            model: "local-model".to_owned(),
            instructions: vec!["Be brief.".to_owned()],
            input: vec![ModelInputBlock::Text {
                text: "hello".to_owned(),
            }],
            continuation: None,
            continuation_mode: ModelContinuationMode::ProviderRetained,
            tools: Vec::new(),
            builtin_tools: Vec::new(),
            maximum_builtin_tool_calls: None,
            reasoning_summary: crate::ModelReasoningSummaryRequest::Disabled,
            reasoning_effort: crate::ModelReasoningEffort::Unspecified,
            output_schema: None,
            maximum_output_tokens: Some(32),
        }
    }

    fn manifest(
        request: &ModelRequest,
        destination: &str,
        session_id: AiSessionId,
        run_id: AiRunId,
        capability: AiEgressCapability,
    ) -> AiEgressManifest {
        AiEgressManifest {
            provider_profile_id: "chat-profile".to_owned(),
            provider_kind: ProviderKind::ChatCompletions.as_str().to_owned(),
            model: request.model.clone(),
            destination: destination.to_owned(),
            destination_trust: AiDestinationTrust::ExternalProcessor,
            capability,
            scope: AiScope::new("project", "chat-test"),
            session_id: Some(session_id),
            run_id: Some(run_id),
            sources: vec![AiDataSourceRef {
                kind: "message".to_owned(),
                reference: "synthetic".to_owned(),
                classification: DataClassification::Public,
                trust: AiSourceTrust::UserProvided,
            }],
            estimated_bytes: request.conservative_egress_bytes(),
            estimated_tokens: 100,
            attachment_count: 0,
            purpose: "test".to_owned(),
            retention: "local-none".to_owned(),
            residency: None,
            policy_version: "test".to_owned(),
            consent_reference: None,
        }
    }

    fn context(
        request: &ModelRequest,
        destination: &str,
        tool_results: usize,
    ) -> ProviderRequestContext {
        let session_id = AiSessionId::new();
        let run_id = AiRunId::new();
        let attempt_id = uuid::Uuid::new_v4();
        let inference = manifest(
            request,
            destination,
            session_id,
            run_id,
            AiEgressCapability::ModelInference,
        );
        let proof = AiEgressDecision::allow(&inference, "test", "test-user")
            .authorize(&inference)
            .expect("manifest should authorize");
        let budget = AiBudgetReservation::new_reserved(
            AiBudgetReservationId::new(),
            run_id,
            attempt_id,
            1,
            ProviderKind::ChatCompletions,
            &request.model,
            "test-pricing-v1",
            AiBudgetAmounts {
                input_tokens: 1_000,
                output_tokens: 1_000,
                runs: 1,
                ..AiBudgetAmounts::default()
            },
            time::OffsetDateTime::now_utc() + time::Duration::hours(1),
        )
        .expect("budget should validate")
        .authorize_provider_call(
            run_id,
            attempt_id,
            1,
            &ProviderKind::ChatCompletions,
            &request.model,
            32,
            0,
            time::OffsetDateTime::now_utc(),
        )
        .expect("budget should authorize");
        let mut context =
            ProviderRequestContext::new(session_id, run_id, "test", budget, inference, proof)
                .expect("context should validate");
        for index in 0..tool_results {
            let mut transfer = manifest(
                request,
                destination,
                session_id,
                run_id,
                AiEgressCapability::ToolResult,
            );
            transfer.sources = vec![AiDataSourceRef {
                kind: "application_tool_result".to_owned(),
                reference: uuid::Uuid::new_v4().to_string(),
                classification: DataClassification::Internal,
                trust: AiSourceTrust::ResolverResult,
            }];
            transfer.purpose = format!("test-tool-result-{index}");
            let transfer_proof = AiEgressDecision::allow(&transfer, "test", "test-user")
                .authorize(&transfer)
                .expect("tool manifest should authorize");
            context = context
                .with_authorized_transfer(transfer, transfer_proof)
                .expect("tool transfer should bind");
        }
        context
    }

    /// One-request loopback server returning `status` and an SSE `body`.
    async fn serve(
        status: &'static str,
        body: String,
    ) -> (
        String,
        oneshot::Receiver<String>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener should bind");
        let address = listener.local_addr().expect("listener should have address");
        let endpoint = format!("http://{address}/v1/chat/completions");
        let (request_tx, request_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("request should connect");
            let mut request = Vec::new();
            let mut buffer = vec![0_u8; 64 * 1024];
            loop {
                let count = socket.read(&mut buffer).await.expect("request should read");
                request.extend_from_slice(&buffer[..count]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some(split) = text.find("\r\n\r\n") {
                    let length = text[..split]
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if request.len() >= split + 4 + length || count == 0 {
                        break;
                    }
                }
            }
            let _ = request_tx.send(String::from_utf8(request).expect("request should be UTF-8"));
            let headers = format!(
                "HTTP/1.1 {status}\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            socket
                .write_all(headers.as_bytes())
                .await
                .expect("headers should write");
            socket
                .write_all(body.as_bytes())
                .await
                .expect("body should write");
        });
        (endpoint, request_rx, task)
    }

    fn sse(chunks: &[Value]) -> String {
        let mut body = chunks
            .iter()
            .map(|chunk| format!("data: {chunk}\n\n"))
            .collect::<String>();
        body.push_str("data: [DONE]\n\n");
        body
    }

    fn chunk(delta: Value, finish_reason: Option<&str>) -> Value {
        json!({
            "object": "chat.completion.chunk",
            "model": "local-model",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}]
        })
    }

    fn usage(prompt: u64, completion: u64) -> Value {
        json!({
            "object": "chat.completion.chunk",
            "model": "local-model",
            "choices": [],
            "usage": {"prompt_tokens": prompt, "completion_tokens": completion}
        })
    }

    fn chat_provider(
        endpoint: &str,
        credential: Option<SecretRef>,
        tools: bool,
        parallel: bool,
    ) -> ChatCompletionsProvider {
        let mut config =
            ChatCompletionsProviderConfig::new("chat-profile", endpoint, credential, "local-none");
        config.capabilities.custom_tools = tools;
        config.capabilities.parallel_tool_calls = parallel;
        ChatCompletionsProvider::new(
            config,
            Arc::new(Policy(endpoint.to_owned(), true)),
            Arc::new(Secrets(reference())),
        )
        .expect("chat provider should build")
    }

    fn body_json(raw: &str) -> Value {
        let (_, body) = raw
            .split_once("\r\n\r\n")
            .expect("request should have a body");
        serde_json::from_str(body).expect("request body should be JSON")
    }

    #[tokio::test]
    async fn text_stream_normalizes_and_reports_usage_without_a_credential() {
        let (endpoint, request_rx, server) = serve(
            "200 OK",
            sse(&[
                chunk(json!({"role": "assistant", "content": "hel"}), None),
                chunk(json!({"content": "lo"}), None),
                chunk(json!({}), Some("stop")),
                usage(7, 2),
            ]),
        )
        .await;
        let provider = chat_provider(&endpoint, None, false, false);
        let model_request = request();
        let events = provider
            .stream(model_request.clone(), context(&model_request, &endpoint, 0))
            .await
            .expect("stream should start")
            .try_collect::<Vec<_>>()
            .await
            .expect("stream should normalize");
        assert_eq!(
            events,
            vec![
                ProviderEvent::ResponseStarted { response_id: None },
                ProviderEvent::TextDelta {
                    text: "hel".to_owned()
                },
                ProviderEvent::TextDelta {
                    text: "lo".to_owned()
                },
                ProviderEvent::Usage {
                    input_tokens: 7,
                    output_tokens: 2,
                    cached_input_tokens: 0
                },
                ProviderEvent::ResponseCompleted { response_id: None },
            ]
        );
        let raw = request_rx.await.expect("request should be captured");
        assert!(!raw.to_ascii_lowercase().contains("authorization:"));
        let body = body_json(&raw);
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["max_tokens"], 32);
        assert_eq!(
            body["messages"],
            json!([
                {"role": "system", "content": "Be brief."},
                {"role": "user", "content": "hello"}
            ])
        );
        server.await.expect("server should finish");
    }

    #[tokio::test]
    async fn fragmented_tool_call_assembles_and_replays_statelessly() {
        let (endpoint, request_rx, server) = serve(
            "200 OK",
            sse(&[
                chunk(
                    json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": "records_read", "arguments": ""}}]}),
                    None,
                ),
                chunk(json!({"tool_calls": [{"index": 0, "function": {"arguments": "{\"id\":"}}]}), None),
                chunk(json!({"tool_calls": [{"index": 0, "function": {"arguments": "\"7\"}"}}]}), None),
                chunk(json!({}), Some("tool_calls")),
                usage(9, 4),
            ]),
        )
        .await;
        let provider = chat_provider(&endpoint, Some(reference()), true, false);
        assert!(provider.capabilities().stateless_continuation);
        let mut first = request();
        first.continuation_mode = ModelContinuationMode::StatelessReplay;
        first.tools = vec![tool()];
        let events = provider
            .stream(first.clone(), context(&first, &endpoint, 0))
            .await
            .expect("stream should start")
            .try_collect::<Vec<_>>()
            .await
            .expect("stream should normalize");
        assert!(events.contains(&ProviderEvent::ToolCallStarted {
            call_id: "call_1".to_owned(),
            tool_id: "records.read".to_owned()
        }));
        assert!(events.contains(&ProviderEvent::ToolCallCompleted {
            call_id: "call_1".to_owned(),
            arguments: json!({"id": "7"})
        }));
        let raw = request_rx.await.expect("request should be captured");
        assert!(
            raw.to_ascii_lowercase()
                .contains("authorization: bearer synthetic-chat-key")
        );
        let body = body_json(&raw);
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(body["tools"][0]["function"]["name"], "records_read");
        assert_eq!(body["tools"][0]["function"]["strict"], true);
        server.await.expect("server should finish");

        // Second turn: history and the tool result are replayed.
        let (endpoint, request_rx, server) = serve(
            "200 OK",
            sse(&[
                chunk(json!({"content": "Record 7."}), Some("stop")),
                usage(20, 3),
            ]),
        )
        .await;
        let provider = chat_provider(&endpoint, Some(reference()), true, false);
        let mut second = request();
        second.instructions = Vec::new();
        second.continuation_mode = ModelContinuationMode::StatelessReplay;
        second.tools = vec![tool()];
        second.continuation = Some(ModelContinuation::StatelessConversation {
            instructions: vec!["Be brief.".to_owned()],
            messages: vec![
                ModelConversationMessage::User {
                    content: vec![ModelInputBlock::Text {
                        text: "hello".to_owned(),
                    }],
                },
                ModelConversationMessage::Assistant {
                    content: String::new(),
                    tool_calls: vec![ModelConversationToolCall {
                        call_id: "call_1".to_owned(),
                        tool_id: "records.read".to_owned(),
                        provider_name: "records_read".to_owned(),
                        tool_fingerprint: "reviewed-fingerprint".to_owned(),
                        arguments: json!({"id": "7"}),
                    }],
                },
            ],
        });
        second.input = vec![ModelInputBlock::ToolResult {
            call_id: "call_1".to_owned(),
            tool_id: "records.read".to_owned(),
            output: json!({"record": 7}),
        }];
        provider
            .stream(second.clone(), context(&second, &endpoint, 1))
            .await
            .expect("continuation should start")
            .try_collect::<Vec<_>>()
            .await
            .expect("continuation should normalize");
        let body = body_json(&request_rx.await.expect("request should be captured"));
        assert_eq!(
            body["messages"],
            json!([
                {"role": "system", "content": "Be brief."},
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": "", "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "records_read", "arguments": "{\"id\":\"7\"}"}}
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "{\"record\":7}"}
            ])
        );
        server.await.expect("server should finish");
    }

    #[tokio::test]
    async fn parallel_calls_require_the_declared_capability() {
        let two_calls = sse(&[
            chunk(
                json!({"tool_calls": [
                    {"index": 0, "id": "call_a", "function": {"name": "records_read", "arguments": "{\"id\":\"1\"}"}},
                    {"index": 1, "id": "call_b", "function": {"name": "records_read", "arguments": "{\"id\":\"2\"}"}}
                ]}),
                None,
            ),
            chunk(json!({}), Some("tool_calls")),
            usage(5, 5),
        ]);
        for parallel in [true, false] {
            let (endpoint, _request_rx, server) = serve("200 OK", two_calls.clone()).await;
            let provider = chat_provider(&endpoint, None, true, parallel);
            let mut model_request = request();
            model_request.continuation_mode = ModelContinuationMode::StatelessReplay;
            model_request.tools = vec![tool()];
            let result = provider
                .stream(model_request.clone(), context(&model_request, &endpoint, 0))
                .await
                .expect("stream should start")
                .try_collect::<Vec<_>>()
                .await;
            if parallel {
                let events = result.expect("declared parallel calls should normalize");
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| matches!(event, ProviderEvent::ToolCallCompleted { .. }))
                        .count(),
                    2
                );
            } else {
                assert!(matches!(result, Err(ProviderError::Rejected)));
            }
            server.abort();
        }
    }

    #[tokio::test]
    async fn missing_usage_truncation_and_http_failures_fail_closed() {
        // No usage chunk: the turn is refused rather than estimated.
        let (endpoint, _rx, server_task) = serve(
            "200 OK",
            sse(&[chunk(json!({"content": "hi"}), Some("stop"))]),
        )
        .await;
        let provider = chat_provider(&endpoint, None, false, false);
        let model_request = request();
        assert!(matches!(
            provider
                .stream(model_request.clone(), context(&model_request, &endpoint, 0))
                .await
                .expect("stream should start")
                .try_collect::<Vec<_>>()
                .await,
            Err(ProviderError::Rejected)
        ));
        server_task.await.expect("server should finish");

        // Truncated output is not a completed answer.
        let (endpoint, _rx, server_task) = serve(
            "200 OK",
            sse(&[chunk(json!({"content": "hi"}), Some("length")), usage(1, 1)]),
        )
        .await;
        let provider = chat_provider(&endpoint, None, false, false);
        assert!(matches!(
            provider
                .stream(model_request.clone(), context(&model_request, &endpoint, 0))
                .await
                .expect("stream should start")
                .try_collect::<Vec<_>>()
                .await,
            Err(ProviderError::Rejected)
        ));
        server_task.await.expect("server should finish");

        // Authentication and rate limiting map like the Responses adapter.
        for (status, expected) in [
            ("401 Unauthorized", ProviderError::CredentialUnavailable),
            ("429 Too Many Requests", ProviderError::RateLimited),
            ("503 Service Unavailable", ProviderError::Unavailable),
        ] {
            let (endpoint, _rx, server_task) = serve(
                status,
                "{\"error\":{\"message\":\"secret detail\"}}".to_owned(),
            )
            .await;
            let provider = chat_provider(&endpoint, Some(reference()), false, false);
            let Err(error) = provider
                .stream(model_request.clone(), context(&model_request, &endpoint, 0))
                .await
            else {
                panic!("HTTP failure should be refused");
            };
            assert_eq!(
                std::mem::discriminant(&error),
                std::mem::discriminant(&expected)
            );
            assert!(!format!("{error:?}").contains("secret detail"));
            server_task.await.expect("server should finish");
        }
    }

    #[tokio::test]
    async fn endpoint_policy_egress_binding_and_profile_contract_fail_closed() {
        let endpoint = "http://127.0.0.1:1/v1/chat/completions";
        let mut config =
            ChatCompletionsProviderConfig::new("chat-profile", endpoint, None, "local-none");
        assert!(matches!(
            ChatCompletionsProvider::new(
                config.clone(),
                Arc::new(Policy(endpoint.to_owned(), false)),
                Arc::new(Secrets(reference()))
            ),
            Err(ProviderError::InvalidConfiguration(_))
        ));
        config.capabilities.parallel_tool_calls = true;
        assert!(matches!(
            ChatCompletionsProvider::new(
                config,
                Arc::new(Policy(endpoint.to_owned(), true)),
                Arc::new(Secrets(reference()))
            ),
            Err(ProviderError::InvalidConfiguration(_))
        ));

        // A manifest bound to another destination is refused before transport.
        let provider = chat_provider(endpoint, None, false, false);
        let model_request = request();
        assert!(matches!(
            provider
                .stream(
                    model_request.clone(),
                    context(
                        &model_request,
                        "http://swapped.invalid/v1/chat/completions",
                        0
                    )
                )
                .await,
            Err(ProviderError::EgressDenied)
        ));
        let debug = format!("{provider:?}");
        assert!(!debug.contains("127.0.0.1"));

        let profile = AiProviderProfileView {
            id: uuid::Uuid::new_v4(),
            scope_kind: "project".to_owned(),
            scope_id: "chat-test".to_owned(),
            tenant_id: None,
            provider_kind: AiProviderKindInput::ChatCompletions.as_str().to_owned(),
            display_name: "Local".to_owned(),
            base_url: Some(endpoint.to_owned()),
            openai_compatible: Some(AiOpenAiCompatibleProfileView {
                retention: "local-none".to_owned(),
                custom_tools: true,
                parallel_tool_calls: false,
                structured_output: false,
                provider_retained_continuation: false,
            }),
            credential_configured: false,
            enabled: true,
            row_version: 1,
            updated_at: 1,
        };
        let config = ChatCompletionsProviderConfig::from_profile(&profile, None)
            .expect("credential-free profile should construct");
        assert!(config.capabilities.custom_tools && config.credential.is_none());
        assert!(ChatCompletionsProviderConfig::from_profile(&profile, Some(reference())).is_err());
        let mut retained = profile;
        retained
            .openai_compatible
            .as_mut()
            .expect("contract")
            .provider_retained_continuation = true;
        assert!(ChatCompletionsProviderConfig::from_profile(&retained, None).is_err());
    }
}
