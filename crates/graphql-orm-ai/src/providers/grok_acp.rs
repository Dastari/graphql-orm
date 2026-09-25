//! Narrow Grok ACP wire primitives, deliberately not an executable provider.
//!
//! SDK MCP requests must flow to the existing coordinator-owned dynamic-tool
//! responder. This module owns no process, credential, session cursor or tool
//! executor and grants no provider admission or application authority.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    ModelToolDefinition, ProviderDynamicToolCall, ProviderDynamicToolResult, ProviderError,
};

const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_DEFINITIONS: usize = 64;
pub(super) const MAX_SDK_CALLS: usize = 4096;

fn rejected() -> ProviderError {
    ProviderError::Classified(crate::AiProviderFailureCategory::ProtocolViolation)
}

fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 200 && !value.chars().any(char::is_control)
}

fn rpc_id(value: &Value) -> bool {
    value.as_u64().is_some() || value.as_str().is_some_and(identifier)
}

/// One admitted SDK MCP request. Tool calls still require the ordinary
/// coordinator's authorization, budgets, persistence and disclosure checks.
///
/// This type intentionally has no `Debug`: serialized responses may contain
/// protected tool metadata. Never log raw inbound or outbound frames.
pub enum AiGrokAcpSdkInbound {
    /// Bounded protocol-only response to `initialize` or `tools/list`.
    Response(Vec<u8>),
    /// Exact schema-validated application-tool request; never execute locally.
    ToolCall(ProviderDynamicToolCall),
    /// Offered call with schema-invalid arguments; only durable rejection is permitted.
    InvalidToolCall(crate::ProviderInvalidDynamicToolCall),
}

struct Pending {
    outer_id: Value,
    inner_id: Value,
    tool_id: String,
}

/// Frozen, bounded SDK MCP reverse-channel broker for one provider prompt.
///
/// This is a codec, not an [`crate::AiProvider`] or sandbox proof. The host
/// must bind its `response_id` and definitions to an authorized exact request,
/// install only this SDK server, and terminate transport after any error.
/// It admits no general MCP transport, discovery, roots, sampling or shell.
/// Definitions are frozen for the codec lifetime. Completed calls cannot be
/// replayed through the same codec; durable recovery remains the coordinator's
/// responsibility and must never reconstruct this codec to replay callbacks.
pub struct AiGrokAcpSdkBroker {
    server_id: String,
    response_id: String,
    tools: BTreeMap<String, ModelToolDefinition>,
    seen_outer: BTreeSet<String>,
    seen_inner: BTreeSet<String>,
    pending: BTreeMap<String, Pending>,
    maximum_calls: usize,
    maximum_frame_bytes: usize,
    maximum_total_bytes: usize,
    total_bytes: usize,
    calls: usize,
    initialized: bool,
    discovery_rejected: bool,
    poisoned: bool,
}

impl AiGrokAcpSdkBroker {
    /// Constructs an immutable protocol broker with hard request/result bounds.
    /// `maximum_total_bytes` includes both directions and protocol metadata.
    ///
    /// # Errors
    /// Rejects malformed identities, duplicate or invalid definitions, more
    /// than 64 definitions or 4096 callbacks, frames over 16 MiB, or total
    /// bytes over 64 MiB.
    pub fn new(
        server_id: String,
        response_id: String,
        tools: Vec<ModelToolDefinition>,
        maximum_calls: usize,
        maximum_frame_bytes: usize,
        maximum_total_bytes: usize,
    ) -> Result<Self, ProviderError> {
        if !identifier(&server_id)
            || !identifier(&response_id)
            || tools.is_empty()
            || tools.len() > MAX_DEFINITIONS
            || !(1..=MAX_SDK_CALLS).contains(&maximum_calls)
            || !(256..=MAX_FRAME_BYTES).contains(&maximum_frame_bytes)
            || maximum_total_bytes < maximum_frame_bytes
            || maximum_total_bytes > 64 * 1024 * 1024
        {
            return Err(ProviderError::InvalidRequest);
        }
        let mut frozen = BTreeMap::new();
        let mut ids = BTreeSet::new();
        for tool in tools {
            tool.validate()?;
            if !ids.insert(tool.tool_id.clone())
                || frozen.insert(tool.provider_name.clone(), tool).is_some()
            {
                return Err(ProviderError::InvalidRequest);
            }
        }
        Ok(Self {
            server_id,
            response_id,
            tools: frozen,
            seen_outer: BTreeSet::new(),
            seen_inner: BTreeSet::new(),
            pending: BTreeMap::new(),
            maximum_calls,
            maximum_frame_bytes,
            maximum_total_bytes,
            total_bytes: 0,
            calls: 0,
            initialized: false,
            discovery_rejected: false,
            poisoned: false,
        })
    }

    pub(super) fn bind_prompt(&mut self, response_id: String) -> Result<(), ProviderError> {
        if !identifier(&response_id) || self.has_pending_calls() || self.calls != 0 || self.poisoned
        {
            return Err(rejected());
        }
        self.response_id = response_id;
        Ok(())
    }

    fn account(&mut self, bytes: usize) -> Result<(), ProviderError> {
        self.total_bytes = self.total_bytes.checked_add(bytes).ok_or_else(rejected)?;
        if bytes > self.maximum_frame_bytes || self.total_bytes > self.maximum_total_bytes {
            return Err(rejected());
        }
        Ok(())
    }

    /// Accepts one complete `_x.ai/mcp/sdk_call` JSON-RPC frame.
    ///
    /// # Errors
    /// Permanently poisons this codec on malformed, duplicate, unknown,
    /// unoffered or over-budget input. Raw provider payloads never enter errors.
    pub fn accept(&mut self, frame: &[u8]) -> Result<AiGrokAcpSdkInbound, ProviderError> {
        if self.poisoned {
            return Err(rejected());
        }
        let result = self.accept_inner(frame);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn accept_inner(&mut self, frame: &[u8]) -> Result<AiGrokAcpSdkInbound, ProviderError> {
        self.account(frame.len())?;
        let outer: Value = serde_json::from_slice(frame).map_err(|_| rejected())?;
        let inner = &outer["params"]["message"];
        if outer["jsonrpc"] != "2.0"
            || outer["method"] != "_x.ai/mcp/sdk_call"
            || outer.get("result").is_some()
            || outer.get("error").is_some()
            || outer["params"]["serverId"] != self.server_id
            || !rpc_id(&outer["id"])
            || inner["jsonrpc"] != "2.0"
            || !rpc_id(&inner["id"])
            || inner.get("result").is_some()
            || inner.get("error").is_some()
            || self.seen_outer.len() >= self.maximum_calls + 3
            || !self.seen_outer.insert(outer["id"].to_string())
            || (inner["method"] != "server/discover"
                && !self.seen_inner.insert(inner["id"].to_string()))
        {
            return Err(rejected());
        }
        match inner["method"].as_str() {
            Some("server/discover") if !self.initialized && !self.discovery_rejected => {
                self.discovery_rejected = true;
                let mut response = serde_json::to_vec(&json!({
                    "jsonrpc":"2.0", "id":outer["id"],
                    "result":{"jsonrpc":"2.0","id":inner["id"],
                        "error":{"code":-32601,"message":"unsupported method"}}
                }))
                .map_err(|_| rejected())?;
                response.push(b'\n');
                self.account(response.len())?;
                Ok(AiGrokAcpSdkInbound::Response(response))
            }
            Some("initialize") if !self.initialized => {
                let version = inner["params"]["protocolVersion"]
                    .as_str()
                    .ok_or_else(rejected)?;
                if !matches!(
                    version,
                    "2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25"
                ) {
                    return Err(rejected());
                }
                self.initialized = true;
                let response = self.response(
                    &outer["id"],
                    &inner["id"],
                    json!({
                        "protocolVersion":version,"capabilities":{"tools":{"listChanged":false}},
                        "serverInfo":{"name":"graphql-orm-ai-fixed-broker","version":"1"}
                    }),
                )?;
                Ok(AiGrokAcpSdkInbound::Response(response))
            }
            Some("tools/list") if self.initialized => {
                if inner["params"].get("cursor").is_some() {
                    return Err(rejected());
                }
                let tools: Vec<_> = self
                    .tools
                    .values()
                    .map(|tool| {
                        json!({
                            "name":tool.provider_name,"description":tool.description,
                            "inputSchema":tool.parameters
                        })
                    })
                    .collect();
                let response = self.response(&outer["id"], &inner["id"], json!({"tools":tools}))?;
                Ok(AiGrokAcpSdkInbound::Response(response))
            }
            Some("tools/call") if self.initialized && self.calls < self.maximum_calls => {
                let name = inner["params"]["name"].as_str().ok_or_else(rejected)?;
                let tool = self.tools.get(name).ok_or_else(rejected)?;
                let mut identity = Sha256::new();
                identity.update(b"graphql-orm-ai/grok-sdk-call/v1\0");
                for value in [&self.response_id, &self.server_id, &tool.fingerprint] {
                    identity.update((value.len() as u64).to_be_bytes());
                    identity.update(value.as_bytes());
                }
                identity.update((self.calls as u64).to_be_bytes());
                let call_id = format!("grok-sdk-{}", hex::encode(identity.finalize()));
                let arguments = inner["params"]
                    .get("arguments")
                    .cloned()
                    .ok_or_else(rejected)?;
                let validator =
                    jsonschema::validator_for(&tool.parameters).map_err(|_| rejected())?;
                let inbound = if !arguments.is_object() || !validator.is_valid(&arguments) {
                    AiGrokAcpSdkInbound::InvalidToolCall(
                        crate::ProviderInvalidDynamicToolCall::from_definition(
                            &self.response_id,
                            &call_id,
                            tool,
                            arguments,
                        )?,
                    )
                } else {
                    AiGrokAcpSdkInbound::ToolCall(ProviderDynamicToolCall::from_definition(
                        &self.response_id,
                        &call_id,
                        tool,
                        arguments,
                    )?)
                };
                self.pending.insert(
                    call_id,
                    Pending {
                        outer_id: outer["id"].clone(),
                        inner_id: inner["id"].clone(),
                        tool_id: tool.tool_id.clone(),
                    },
                );
                self.calls += 1;
                Ok(inbound)
            }
            _ => Err(rejected()),
        }
    }

    fn response(
        &mut self,
        outer_id: &Value,
        inner_id: &Value,
        result: Value,
    ) -> Result<Vec<u8>, ProviderError> {
        let mut encoded = serde_json::to_vec(&json!({
            "jsonrpc":"2.0","id":outer_id,
            "result":{"jsonrpc":"2.0","id":inner_id,"result":result}
        }))
        .map_err(|_| rejected())?;
        encoded.push(b'\n');
        self.account(encoded.len())?;
        Ok(encoded)
    }

    /// Encodes only the coordinator's separately authorized result, exactly
    /// once, for its pending call. Never accepts arbitrary model-visible JSON.
    ///
    /// # Errors
    /// Permanently poisons the codec on result swaps, replay or output limits.
    pub fn tool_response(
        &mut self,
        result: &ProviderDynamicToolResult,
    ) -> Result<Vec<u8>, ProviderError> {
        if self.poisoned {
            return Err(rejected());
        }
        let response =
            (|| {
                let pending = self.pending.remove(result.call_id()).ok_or_else(rejected)?;
                if pending.tool_id != result.tool_id() {
                    return Err(rejected());
                }
                self.response(&pending.outer_id, &pending.inner_id, json!({
                "content":[{"type":"text","text":result.output().to_string()}],"isError":false
            }))
            })();
        if response.is_err() {
            self.poisoned = true;
        }
        response
    }

    /// Whether every admitted callback has a successfully encoded result.
    /// This is not durable persistence or cancellation-settlement proof.
    pub fn has_pending_calls(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// Complete prompt-level token usage decoded from Grok ACP `_meta.usage`.
/// This reports provider counters, not billing authority or budget settlement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiGrokAcpUsage {
    /// Full input including cache reads and creation; do not add cache twice.
    pub input_tokens: u64,
    /// Provider-reported output including reasoning.
    pub output_tokens: u64,
    /// Cache-read subset of input.
    pub cached_input_tokens: u64,
    /// Main-agent model rounds in this prompt.
    pub model_calls: u64,
}

/// Protocol/storage sanity bound, deliberately independent of request estimates.
pub(super) const MAX_USAGE_TOKENS: u64 = 1_000_000_000_000;

impl AiGrokAcpUsage {
    /// Decodes exact prompt usage, rejecting missing/incomplete counters,
    /// model drift, inconsistent sums and protocol token/round sanity overruns.
    /// Token bounds are protocol validation bounds, never reservation estimates.
    /// The caller must pass the nested `usage`, never last-call `_meta` fields.
    ///
    /// # Errors
    /// Returns a content-free protocol error. No missing counter becomes zero.
    pub fn decode(
        usage: &Value,
        model: &str,
        maximum_input_tokens: u64,
        maximum_output_tokens: u64,
        maximum_model_calls: u64,
    ) -> Result<Self, ProviderError> {
        fn counter(value: &Value, key: &str) -> Result<u64, ProviderError> {
            value[key].as_u64().ok_or_else(rejected)
        }
        if !usage.is_object()
            || usage
                .get("usageIsIncomplete")
                .is_some_and(|v| v != &Value::Bool(false))
        {
            return Err(ProviderError::Classified(
                crate::AiProviderFailureCategory::UsageIncomplete,
            ));
        }
        let rejected = || ProviderError::Classified(crate::AiProviderFailureCategory::UsageInvalid);
        let counter = |value: &Value, key: &str| counter(value, key).map_err(|_| rejected());
        let input_tokens = counter(usage, "inputTokens")?;
        let output_tokens = counter(usage, "outputTokens")?;
        let cached_input_tokens = counter(usage, "cachedReadTokens")?;
        let creation = counter(usage, "cacheCreationTokens")?;
        let reasoning = counter(usage, "reasoningTokens")?;
        let model_calls = counter(usage, "modelCalls")?;
        if model_calls > maximum_model_calls {
            return Err(ProviderError::Classified(
                crate::AiProviderFailureCategory::ExecutionLimit,
            ));
        }
        let models = usage["modelUsage"].as_object().ok_or_else(rejected)?;
        let row = models.get(model).ok_or_else(rejected)?;
        if models.len() != 1
            || input_tokens > maximum_input_tokens
            || output_tokens > maximum_output_tokens
            || model_calls == 0
            || model_calls > maximum_model_calls
            || counter(usage, "numTurns")? != model_calls
            || cached_input_tokens
                .checked_add(creation)
                .is_none_or(|n| n > input_tokens)
            || reasoning > output_tokens
            || input_tokens.checked_add(output_tokens) != Some(counter(usage, "totalTokens")?)
        {
            return Err(rejected());
        }
        for key in [
            "inputTokens",
            "outputTokens",
            "cachedReadTokens",
            "cacheCreationTokens",
            "reasoningTokens",
            "totalTokens",
            "modelCalls",
        ] {
            if counter(row, key)? != counter(usage, key)? {
                return Err(rejected());
            }
        }
        Ok(Self {
            input_tokens,
            output_tokens,
            cached_input_tokens,
            model_calls,
        })
    }
}

#[cfg(all(test, any(feature = "sqlite", feature = "postgres")))]
mod tests {
    use super::*;

    fn definition() -> ModelToolDefinition {
        ModelToolDefinition {
            tool_id: "capabilities.discover".to_owned(),
            provider_name: "discover".to_owned(),
            fingerprint: "reviewed-definition-v1".to_owned(),
            description: "Discover authorized read capabilities".to_owned(),
            parameters: json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}),
            strict: true,
            defer_loading: false,
        }
    }

    fn broker(maximum_calls: usize) -> AiGrokAcpSdkBroker {
        AiGrokAcpSdkBroker::new(
            "sdk-1".into(),
            "prompt-1".into(),
            vec![definition()],
            maximum_calls,
            4096,
            32768,
        )
        .unwrap()
    }

    fn request(id: u64, method: &str, params: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"method":"_x.ai/mcp/sdk_call","params":{
            "serverId":"sdk-1","message":{"jsonrpc":"2.0","id":id+100,"method":method,"params":params}
        }})).unwrap()
    }

    fn initialize(broker: &mut AiGrokAcpSdkBroker) {
        assert!(matches!(
            broker.accept(&request(
                1,
                "initialize",
                json!({"protocolVersion":"2025-03-26"})
            )),
            Ok(AiGrokAcpSdkInbound::Response(_))
        ));
    }

    #[test]
    fn callback_capacity_is_independent_of_definition_count_and_remains_bounded() {
        let mut broker = AiGrokAcpSdkBroker::new(
            "sdk-1".into(),
            "prompt-1".into(),
            vec![definition()],
            MAX_SDK_CALLS,
            4096,
            64 * 1024 * 1024,
        )
        .unwrap();
        initialize(&mut broker);
        broker.accept(&request(2, "tools/list", json!({}))).unwrap();
        for ordinal in 1..=MAX_SDK_CALLS {
            let frame = request(
                ordinal as u64 + 2,
                "tools/call",
                json!({"name":"discover","arguments":{"query":"fixture"}}),
            );
            let AiGrokAcpSdkInbound::ToolCall(call) = broker.accept(&frame).unwrap() else {
                panic!("callback {ordinal} must be admitted, including 65 and 1024");
            };
            let result = ProviderDynamicToolResult::new(&call, json!({"items":[]})).unwrap();
            broker.tool_response(&result).unwrap();
        }
        assert!(
            broker
                .accept(&request(
                    MAX_SDK_CALLS as u64 + 3,
                    "tools/call",
                    json!({"name":"discover","arguments":{"query":"fixture"}})
                ))
                .is_err()
        );
        assert!(
            AiGrokAcpSdkBroker::new(
                "sdk-1".into(),
                "prompt-1".into(),
                vec![definition()],
                MAX_SDK_CALLS + 1,
                4096,
                64 * 1024 * 1024
            )
            .is_err()
        );
    }

    #[test]
    fn observed_1040_sdk_handshake_falls_back_from_discovery_to_legacy_mcp() {
        let mut broker = broker(1);
        let AiGrokAcpSdkInbound::Response(response) = broker
            .accept(&request(0, "server/discover", json!({})))
            .unwrap()
        else {
            panic!("discovery")
        };
        let response: Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(response["result"]["error"]["code"], -32601);
        // The legacy MCP fallback restarts its inner ID counter after the
        // server/discover probe, while ACP outer correlation remains unique.
        let mut legacy: Value = serde_json::from_slice(&request(
            1,
            "initialize",
            json!({"protocolVersion":"2025-11-25"}),
        ))
        .unwrap();
        legacy["params"]["message"]["id"] = json!(100);
        assert!(matches!(
            broker.accept(&serde_json::to_vec(&legacy).unwrap()),
            Ok(AiGrokAcpSdkInbound::Response(_))
        ));
        assert!(matches!(
            broker.accept(&request(2, "tools/list", json!({}))),
            Ok(AiGrokAcpSdkInbound::Response(_))
        ));
        assert!(
            broker
                .accept(&request(3, "server/discover", json!({})))
                .is_err()
        );
    }

    #[test]
    fn exact_broker_bridge_uses_coordinator_result_and_rejects_replay() {
        let mut broker = broker(2);
        initialize(&mut broker);
        let AiGrokAcpSdkInbound::Response(list) =
            broker.accept(&request(2, "tools/list", json!({}))).unwrap()
        else {
            panic!("list")
        };
        let list: Value = serde_json::from_slice(&list).unwrap();
        assert_eq!(list["result"]["result"]["tools"][0]["name"], "discover");
        let frame = request(
            3,
            "tools/call",
            json!({"name":"discover","arguments":{"query":"fixture"}}),
        );
        let AiGrokAcpSdkInbound::ToolCall(call) = broker.accept(&frame).unwrap() else {
            panic!("call")
        };
        assert_eq!(call.tool_id(), "capabilities.discover");
        assert_eq!(call.tool_fingerprint(), "reviewed-definition-v1");
        assert_eq!(call.response_id(), "prompt-1");
        assert!(broker.has_pending_calls());
        let result = ProviderDynamicToolResult::new(&call, json!({"items":[]})).unwrap();
        let response: Value =
            serde_json::from_slice(&broker.tool_response(&result).unwrap()).unwrap();
        assert_eq!(response["id"], 3);
        assert_eq!(response["result"]["id"], 103);
        assert_eq!(
            response["result"]["result"]["content"][0]["text"],
            "{\"items\":[]}"
        );
        assert!(!broker.has_pending_calls());
        assert!(broker.accept(&frame).is_err());
        assert!(broker.accept(&request(4, "tools/list", json!({}))).is_err());
    }

    #[test]
    fn unknown_server_tools_and_preinitialize_requests_fail_closed() {
        let mut wrong_server: Value =
            serde_json::from_slice(&request(2, "tools/list", json!({}))).unwrap();
        wrong_server["params"]["serverId"] = json!("ambient-server");
        for frame in [
            serde_json::to_vec(&wrong_server).unwrap(),
            request(2, "sampling/createMessage", json!({})),
            request(
                2,
                "tools/call",
                json!({"name":"bash","arguments":{"query":"fixture"}}),
            ),
        ] {
            let mut broker = broker(2);
            initialize(&mut broker);
            assert!(broker.accept(&frame).is_err());
            assert!(broker.accept(&request(3, "tools/list", json!({}))).is_err());
        }
        assert!(
            broker(2)
                .accept(&request(
                    1,
                    "tools/call",
                    json!({"name":"discover","arguments":{"query":"fixture"}})
                ))
                .is_err()
        );
    }

    #[test]
    fn schema_invalid_sdk_call_can_be_corrected_without_restarting_the_broker() {
        let mut broker = broker(2);
        initialize(&mut broker);
        let frame = request(
            2,
            "tools/call",
            json!({"name":"discover","arguments":{"query":42}}),
        );
        let AiGrokAcpSdkInbound::InvalidToolCall(call) = broker.accept(&frame).unwrap() else {
            panic!("invalid arguments must never become an executable call")
        };
        let result = ProviderDynamicToolResult::persisted(
            call.call_id().to_owned(),
            call.tool_id().to_owned(),
            crate::AiApplicationToolFailureEnvelope::new(
                crate::AiApplicationToolFailureCode::InvalidArguments,
            )
            .to_json(),
        )
        .unwrap();
        let response: Value =
            serde_json::from_slice(&broker.tool_response(&result).unwrap()).unwrap();
        assert_eq!(response["result"]["id"], 102);
        assert!(!broker.has_pending_calls());
        assert!(matches!(
            broker.accept(&request(
                3,
                "tools/call",
                json!({"name":"discover","arguments":{"query":"fixed"}})
            )),
            Ok(AiGrokAcpSdkInbound::ToolCall(_))
        ));
        assert!(
            broker.accept(&frame).is_err(),
            "a rejected request cannot be replayed"
        );
    }

    #[test]
    fn call_and_wire_budgets_fail_closed() {
        let mut broker = broker(1);
        initialize(&mut broker);
        let args = json!({"name":"discover","arguments":{"query":"fixture"}});
        assert!(matches!(
            broker.accept(&request(2, "tools/call", args.clone())),
            Ok(AiGrokAcpSdkInbound::ToolCall(_))
        ));
        assert!(broker.accept(&request(3, "tools/call", args)).is_err());
        let mut bounded =
            AiGrokAcpSdkBroker::new("s".into(), "p".into(), vec![definition()], 1, 256, 256)
                .unwrap();
        assert!(bounded.accept(&vec![b' '; 257]).is_err());
    }

    #[test]
    fn response_swap_replay_and_result_overrun_fail_closed() {
        for mode in [0, 1, 2] {
            let mut broker = broker(1);
            initialize(&mut broker);
            let AiGrokAcpSdkInbound::ToolCall(call) = broker
                .accept(&request(
                    2,
                    "tools/call",
                    json!({"name":"discover","arguments":{"query":"fixture"}}),
                ))
                .unwrap()
            else {
                panic!("call")
            };
            let output = if mode == 2 {
                json!("x".repeat(4096))
            } else {
                json!({})
            };
            let result = ProviderDynamicToolResult::new(&call, output).unwrap();
            if mode == 0 {
                broker.pending.get_mut(call.call_id()).unwrap().tool_id = "wrong".into();
            } else if mode == 1 {
                assert!(broker.tool_response(&result).is_ok());
            }
            assert!(broker.tool_response(&result).is_err());
            assert!(broker.accept(&request(3, "tools/list", json!({}))).is_err());
        }
    }

    #[test]
    fn coordinator_results_cannot_cross_prompt_boundaries() {
        let mut first = broker(1);
        let mut second = AiGrokAcpSdkBroker::new(
            "sdk-1".into(),
            "other-prompt".into(),
            vec![definition()],
            1,
            4096,
            32768,
        )
        .unwrap();
        initialize(&mut first);
        initialize(&mut second);
        let frame = request(
            2,
            "tools/call",
            json!({"name":"discover","arguments":{"query":"fixture"}}),
        );
        let AiGrokAcpSdkInbound::ToolCall(first_call) = first.accept(&frame).unwrap() else {
            panic!("call")
        };
        let AiGrokAcpSdkInbound::ToolCall(second_call) = second.accept(&frame).unwrap() else {
            panic!("call")
        };
        assert_ne!(first_call.call_id(), second_call.call_id());
        let first_result = ProviderDynamicToolResult::new(&first_call, json!({})).unwrap();
        assert!(second.tool_response(&first_result).is_err());
        assert!(first.tool_response(&first_result).is_ok());
    }

    fn usage() -> Value {
        let row = json!({"inputTokens":100,"outputTokens":20,"totalTokens":120,"cachedReadTokens":30,"cacheCreationTokens":10,"reasoningTokens":5,"modelCalls":2});
        let mut all = row.clone();
        all["numTurns"] = json!(2);
        all["modelUsage"] = json!({"reviewed-model":row});
        all
    }

    #[test]
    fn extended_rounds_account_fully_and_failures_have_closed_categories() {
        use crate::AiProviderFailureCategory as Category;
        let mut value = usage();
        value["modelCalls"] = json!(256);
        value["numTurns"] = json!(256);
        value["modelUsage"]["reviewed-model"]["modelCalls"] = json!(256);
        assert_eq!(
            AiGrokAcpUsage::decode(&value, "reviewed-model", 100, 20, 256)
                .unwrap()
                .model_calls,
            256
        );
        assert_eq!(
            AiGrokAcpUsage::decode(&value, "reviewed-model", 100, 20, 16)
                .unwrap_err()
                .safe_category(),
            Category::ExecutionLimit
        );
        value["usageIsIncomplete"] = json!(true);
        assert_eq!(
            AiGrokAcpUsage::decode(&value, "reviewed-model", 100, 20, 256)
                .unwrap_err()
                .safe_category(),
            Category::UsageIncomplete
        );
        value["usageIsIncomplete"] = json!(false);
        value["totalTokens"] = json!(0);
        assert_eq!(
            AiGrokAcpUsage::decode(&value, "reviewed-model", 100, 20, 256)
                .unwrap_err()
                .safe_category(),
            Category::UsageInvalid
        );
    }

    #[test]
    fn prompt_usage_preserves_full_input_and_rejects_uncertainty() {
        let valid = usage();
        let decoded = AiGrokAcpUsage::decode(&valid, "reviewed-model", 100, 20, 2).unwrap();
        assert_eq!(decoded.input_tokens, 100);
        assert_eq!(decoded.cached_input_tokens, 30);
        assert!(AiGrokAcpUsage::decode(&valid, "wrong-model", 100, 20, 2).is_err());
        assert!(AiGrokAcpUsage::decode(&valid, "reviewed-model", 100, 19, 2).is_err());
        assert!(AiGrokAcpUsage::decode(&valid, "reviewed-model", 100, 20, 1).is_err());
        for (key, value) in [
            ("usageIsIncomplete", json!(true)),
            ("totalTokens", json!(121)),
            ("inputTokens", Value::Null),
            ("numTurns", json!(3)),
            ("cachedReadTokens", json!(101)),
            ("reasoningTokens", json!(21)),
        ] {
            let mut bad = valid.clone();
            bad[key] = value;
            assert!(AiGrokAcpUsage::decode(&bad, "reviewed-model", 100, 20, 2).is_err());
        }
        let mut drift = valid;
        drift["modelUsage"]["reviewed-model"]["outputTokens"] = json!(19);
        assert!(AiGrokAcpUsage::decode(&drift, "reviewed-model", 100, 20, 2).is_err());
    }
    #[test]
    fn compact_schema_batches_and_authorized_multimegabyte_results_fit_bounded_frames() {
        let mut first = definition();
        first.parameters["description"] = json!("s".repeat(600 * 1024));
        let mut second = first.clone();
        second.tool_id = "capabilities.describe".into();
        second.provider_name = "describe".into();
        let mut broker = AiGrokAcpSdkBroker::new(
            "sdk-1".into(),
            "prompt-1".into(),
            vec![first, second],
            4,
            16 * 1024 * 1024,
            64 * 1024 * 1024,
        )
        .unwrap();
        initialize(&mut broker);
        let AiGrokAcpSdkInbound::Response(list) =
            broker.accept(&request(2, "tools/list", json!({}))).unwrap()
        else {
            panic!("list");
        };
        assert!(list.len() > 1024 * 1024 && list.len() < 16 * 1024 * 1024);
        let AiGrokAcpSdkInbound::ToolCall(call) = broker
            .accept(&request(
                3,
                "tools/call",
                json!({"name":"discover","arguments":{"query":"fixture"}}),
            ))
            .unwrap()
        else {
            panic!("call");
        };
        let result =
            ProviderDynamicToolResult::new(&call, json!({"value":"x".repeat(4*1024*1024)}))
                .unwrap();
        let frame = broker.tool_response(&result).unwrap();
        assert!(frame.len() > 4 * 1024 * 1024 && frame.len() < 16 * 1024 * 1024);
        let AiGrokAcpSdkInbound::ToolCall(call) = broker
            .accept(&request(
                4,
                "tools/call",
                json!({"name":"discover","arguments":{"query":"fixture"}}),
            ))
            .unwrap()
        else {
            panic!("call");
        };
        let result =
            ProviderDynamicToolResult::new(&call, json!({"value":"x".repeat(16*1024*1024-20)}))
                .unwrap();
        assert!(broker.tool_response(&result).is_err());
        assert!(
            AiGrokAcpSdkBroker::new(
                "sdk-1".into(),
                "prompt-1".into(),
                vec![definition()],
                1,
                16 * 1024 * 1024 + 1,
                64 * 1024 * 1024
            )
            .is_err()
        );
    }
}
