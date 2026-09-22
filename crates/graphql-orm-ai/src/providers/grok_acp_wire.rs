//! Strict Grok ACP wire process over deployment-owned bounded pipes.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex as SyncMutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::grok_acp::{AiGrokAcpSdkBroker, AiGrokAcpSdkInbound, AiGrokAcpUsage};
use super::grok_acp_provider::{AiGrokAcpRegistration, AiGrokAcpRunProcess};
use crate::{
    AiProviderSessionCursor, ProviderDynamicToolResponder, ProviderError, ProviderEvent,
    ProviderEventStream,
};

const FRAME: usize = 16 * 1024 * 1024;
const TOTAL: usize = 64 * 1024 * 1024;
const SERVER: &str = "graphql-orm-ai-broker";
const FIXED_TITLE: &str = "Authorized capability session";
fn rejected() -> ProviderError {
    ProviderError::Classified(crate::AiProviderFailureCategory::ProtocolViolation)
}

/// Deployment-owned bounded newline transport. No protocol logic or tool
/// execution belongs here. Never log frames or stderr. Both methods must be
/// cancellation-safe; writes must serialize concurrent cancellation frames.
#[async_trait]
pub trait AiGrokAcpWireTransport: Send + Sync {
    /// Writes one complete newline-terminated frame with a bounded deadline.
    ///
    /// # Errors
    /// Returns a sanitized error for timeout, process exit or write failure.
    async fn write_frame(&self, frame: Vec<u8>) -> Result<(), ProviderError>;
    /// Reads one newline-terminated frame, limiting allocation to 16 MiB before
    /// reading it. EOF, partial lines and discarded-stderr overflow fail closed.
    ///
    /// # Errors
    /// Rejects timeouts, incomplete/oversized lines and transport failures.
    async fn read_frame(&self) -> Result<Vec<u8>, ProviderError>;
}

struct State {
    serial: u64,
    initialized: bool,
    prompted: bool,
    bytes: usize,
    frames: usize,
    skills_reloads: u8,
    broker: AiGrokAcpSdkBroker,
}

/// Upstream-owned ACP state machine using only a trusted bounded wire transport.
/// This performs protocol normalization, never launches a process or executes
/// application tools. Construction does not authorize provider admission.
pub struct AiGrokAcpWireProcess {
    registration: Arc<AiGrokAcpRegistration>,
    transport: Arc<dyn AiGrokAcpWireTransport>,
    cwd: String,
    state: Arc<Mutex<State>>,
    session: Arc<SyncMutex<Option<String>>>,
}

impl AiGrokAcpWireProcess {
    /// Creates a protocol actor for one isolated process and empty absolute cwd.
    /// The factory must independently enforce the registration and sandbox.
    ///
    /// # Errors
    /// Rejects relative/control-bearing cwd or invalid frozen broker metadata.
    pub fn new(
        registration: Arc<AiGrokAcpRegistration>,
        transport: Arc<dyn AiGrokAcpWireTransport>,
        cwd: String,
    ) -> Result<Self, ProviderError> {
        if registration.retained_namespace().is_none()
            || !std::path::Path::new(&cwd).is_absolute()
            || cwd.len() > 4096
            || cwd.chars().any(char::is_control)
        {
            return Err(ProviderError::InvalidRequest);
        }
        let broker = AiGrokAcpSdkBroker::new(
            SERVER.into(),
            "bootstrap".into(),
            registration.tools().to_vec(),
            super::grok_acp::MAX_SDK_CALLS,
            FRAME,
            TOTAL,
        )?;
        Ok(Self {
            registration,
            transport,
            cwd,
            state: Arc::new(Mutex::new(State {
                serial: 0,
                initialized: false,
                prompted: false,
                bytes: 0,
                frames: 0,
                skills_reloads: 0,
                broker,
            })),
            session: Arc::new(SyncMutex::new(None)),
        })
    }
    fn profile(&self) -> Value {
        json!({
            "name":"graphql-orm-ai","description":"Fixed authorized capability broker",
            "toolConfig":{"tools":[{"id":"GrokBuild:search_tool"},{"id":"GrokBuild:use_tool"}]},
            "injectDefaultTools":false,"discoverSkills":false,"agentsMd":false,
            "skills":[],"disallowedTools":["Agent"],"maxTurns":self.registration.maximum_model_calls(),
        })
    }
    fn session_params(&self) -> Value {
        json!({"cwd":self.cwd,"mcpServers":[],"_meta":{
            "x.ai/mcp/servers":[{"name":"capabilities","serverId":SERVER}],
            "agentProfile":self.profile(),"systemPromptOverride":self.registration.bootstrap(),"yoloMode":true,
        }})
    }
    async fn send(
        transport: &dyn AiGrokAcpWireTransport,
        value: Value,
    ) -> Result<(), ProviderError> {
        let mut bytes = serde_json::to_vec(&value).map_err(|_| rejected())?;
        bytes.push(b'\n');
        if bytes.len() > FRAME {
            return Err(rejected());
        }
        transport.write_frame(bytes).await
    }
    async fn read(
        transport: &dyn AiGrokAcpWireTransport,
        state: &mut State,
    ) -> Result<(Vec<u8>, Value), ProviderError> {
        let bytes = transport.read_frame().await?;
        state.bytes = state.bytes.checked_add(bytes.len()).ok_or_else(rejected)?;
        state.frames += 1;
        if bytes.len() > FRAME
            || state.bytes > TOTAL
            || state.frames > 65_536
            || !bytes.ends_with(b"\n")
        {
            return Err(rejected());
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| rejected())?;
        if value["jsonrpc"] != "2.0" {
            return Err(rejected());
        }
        Ok((bytes, value))
    }
    fn next_id(state: &mut State) -> Result<u64, ProviderError> {
        state.serial = state.serial.checked_add(1).ok_or_else(rejected)?;
        Ok(state.serial)
    }
    fn option_matches(response: &Value, id: &str, value: &str) -> bool {
        response["configOptions"].as_array().is_some_and(|options| {
            options
                .iter()
                .filter(|option| option["id"] == id && option["currentValue"] == value)
                .count()
                == 1
        })
    }
    fn session_id(value: &Value) -> Result<String, ProviderError> {
        let id = value.as_str().ok_or_else(rejected)?;
        if id.is_empty() || id.len() > 200 || id.chars().any(char::is_control) {
            return Err(rejected());
        }
        Ok(id.into())
    }
    fn internal_reload(value: &Value, state: &mut State) -> Result<bool, ProviderError> {
        if value["id"] != "skills-reload" {
            return Ok(false);
        }
        if value.as_object().is_none_or(|v| v.len() != 3)
            || value["result"].as_object().is_none_or(|v| v.len() != 1)
            || value["result"]["result"]
                .as_object()
                .is_none_or(|v| v.len() != 1)
            || value["result"]["result"]["reloaded"]
                .as_u64()
                .is_none_or(|n| n > 1)
            || state.skills_reloads >= 16
        {
            return Err(rejected());
        }
        state.skills_reloads += 1;
        Ok(true)
    }
    fn validate_resumed(&self, response: &Value) -> Result<(), ProviderError> {
        if response["models"]["currentModelId"] != self.registration.model()
            || !Self::option_matches(response, "model", self.registration.model())
            || !Self::option_matches(
                response,
                "reasoning_effort",
                self.registration.reasoning_effort().as_str(),
            )
        {
            return Err(rejected());
        }
        Ok(())
    }
    async fn rpc(
        &self,
        state: &mut State,
        method: &str,
        params: Value,
    ) -> Result<Value, ProviderError> {
        let id = Self::next_id(state)?;
        Self::send(
            self.transport.as_ref(),
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
        )
        .await?;
        loop {
            let (bytes, value) = Self::read(self.transport.as_ref(), state).await?;
            if value.get("method").is_none() {
                if Self::internal_reload(&value, state)? {
                    continue;
                }
                if value["id"] != id || value.get("error").is_some() || !value["result"].is_object()
                {
                    return Err(rejected());
                }
                return Ok(value["result"].clone());
            }
            if value["method"] == "_x.ai/mcp/sdk_call" {
                match state.broker.accept(&bytes)? {
                    AiGrokAcpSdkInbound::Response(response) => {
                        self.transport.write_frame(response).await?
                    }
                    AiGrokAcpSdkInbound::ToolCall(_) => return Err(rejected()),
                }
            } else {
                if value.get("id").is_some() {
                    return Err(rejected());
                }
                Self::notification(&value, None, true, &mut BTreeSet::new())?;
            }
        }
    }
    async fn initialize(&self, state: &mut State) -> Result<(), ProviderError> {
        if state.initialized {
            return Ok(());
        }
        let response=self.rpc(state,"initialize",json!({"protocolVersion":1,
            "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},
            "clientInfo":{"name":"graphql-orm-ai","version":"1"}})).await?;
        if response["protocolVersion"] != 1
            || response["_meta"]["x.ai/mcp/sdk"] != true
            || response["agentCapabilities"]["loadSession"] != true
            || !response["authMethods"]
                .as_array()
                .is_some_and(|methods| methods.iter().any(|m| m["id"] == "cached_token"))
        {
            return Err(ProviderError::Unsupported);
        }
        self.rpc(
            state,
            "authenticate",
            json!({"methodId":"cached_token","_meta":{"headless":true}}),
        )
        .await?;
        state.initialized = true;
        Ok(())
    }
    fn notification(
        value: &Value,
        session: Option<&str>,
        history: bool,
        tool_ids: &mut BTreeSet<String>,
    ) -> Result<Option<String>, ProviderError> {
        let method = value["method"].as_str().ok_or_else(rejected)?;
        if value.get("id").is_some() {
            return Err(rejected());
        }
        if let (Some(expected), Some(actual)) = (session, value["params"]["sessionId"].as_str())
            && expected != actual
        {
            return Err(rejected());
        }
        if matches!(method, "session/update" | "_x.ai/session_notification")
            && session.is_some()
            && value["params"]["sessionId"].as_str() != session
        {
            return Err(rejected());
        }
        match method {
            "session/update" => {
                let update = &value["params"]["update"];
                match update["sessionUpdate"].as_str() {
                    Some("agent_message_chunk") => {
                        if history {
                            return Ok(None);
                        }
                        if update["content"]["type"] != "text" {
                            return Err(rejected());
                        }
                        Ok(Some(
                            update["content"]["text"]
                                .as_str()
                                .ok_or_else(rejected)?
                                .into(),
                        ))
                    }
                    Some(
                        "agent_thought_chunk"
                        | "available_commands_update"
                        | "session_info_update"
                        | "current_mode_update",
                    ) => Ok(None),
                    Some("config_option_update") if history => Ok(None),
                    Some("tool_call") => {
                        if history {
                            return Ok(None);
                        }
                        let name = update["_meta"]["x.ai/tool"]["name"]
                            .as_str()
                            .ok_or_else(rejected)?;
                        if !matches!(name, "search_tool" | "use_tool")
                            || update["_meta"]["x.ai/tool"]["namespace"] != "grok_build"
                            || tool_ids.len() >= 8192
                        {
                            return Err(rejected());
                        }
                        let id = Self::session_id(&update["toolCallId"])?;
                        if !tool_ids.insert(id) {
                            return Err(rejected());
                        }
                        Ok(None)
                    }
                    Some("tool_call_update") => {
                        if !history
                            && !tool_ids
                                .contains(update["toolCallId"].as_str().ok_or_else(rejected)?)
                        {
                            return Err(rejected());
                        }
                        Ok(None)
                    }
                    _ => Err(rejected()),
                }
            }
            "_x.ai/session_notification" => {
                match value["params"]["update"]["sessionUpdate"].as_str() {
                    Some("session_summary_generated") => {
                        if value["params"]["_meta"]["x.ai/titleIsManual"] != true
                            || value["params"]["update"]["session_summary"] != FIXED_TITLE
                        {
                            return Err(rejected());
                        }
                        Ok(None)
                    }
                    Some("background_tasks") => {
                        let update = &value["params"]["update"];
                        if !update["tasks"].as_array().is_some_and(Vec::is_empty)
                            || update["truncated"] == true
                        {
                            return Err(rejected());
                        }
                        Ok(None)
                    }
                    Some(
                        "usage"
                        | "model_changed"
                        | "response_completed"
                        | "turn_completed"
                        | "pending_interaction"
                        | "interaction_resolved"
                        | "prompt_complete"
                        | "session_title"
                        | "session_title_updated"
                        | "token_usage"
                        | "tool_call_delta_chunk",
                    ) => Ok(None),
                    // Retry, compaction, unaccounted side inference and unknown
                    // execution events cannot quietly extend a bounded prompt.
                    _ => Err(rejected()),
                }
            }
            "_x.ai/mcp/servers_updated"
            | "_x.ai/session/setup"
            | "_x.ai/mcp/init_progress"
            | "_x.ai/mcp_initialized"
            | "_x.ai/sessions/changed"
            | "_x.ai/queue/changed"
            | "_x.ai/mcp/server_status"
            | "_x.ai/models/update"
            | "_x.ai/settings/update"
            | "_x.ai/announcements/update"
            | "_x.ai/session/prompt_complete" => Ok(None),
            _ => Err(rejected()),
        }
    }
}

#[async_trait]
impl AiGrokAcpRunProcess for AiGrokAcpWireProcess {
    async fn create_empty_session(&self) -> Result<AiProviderSessionCursor, ProviderError> {
        let mut state = self.state.lock().await;
        if self.session.lock().map_err(|_| rejected())?.is_some() {
            return Err(rejected());
        }
        self.initialize(&mut state).await?;
        let response = self
            .rpc(&mut state, "session/new", self.session_params())
            .await?;
        let session = Self::session_id(&response["sessionId"])?;
        *self.session.lock().map_err(|_| rejected())? = Some(session.clone());
        let selected = self
            .rpc(
                &mut state,
                "session/set_config_option",
                json!({"sessionId":session,"configId":"model","value":self.registration.model()}),
            )
            .await?;
        if !Self::option_matches(&selected, "model", self.registration.model()) {
            return Err(rejected());
        }
        let effort = self.registration.reasoning_effort();
        if effort != crate::ModelReasoningEffort::Unspecified {
            let selected=self.rpc(&mut state,"session/set_config_option",json!({"sessionId":session,"configId":"reasoning_effort","value":effort.as_str()})).await?;
            if !Self::option_matches(&selected, "reasoning_effort", effort.as_str()) {
                return Err(rejected());
            }
        }
        let renamed = self
            .rpc(
                &mut state,
                "_x.ai/session/rename",
                json!({"sessionId":session,"cwd":self.cwd,"title":FIXED_TITLE}),
            )
            .await?;
        if renamed != json!({"success":true}) {
            return Err(rejected());
        }
        let closed = self
            .rpc(&mut state, "session/close", json!({"sessionId":session}))
            .await?;
        if closed
            .as_object()
            .is_none_or(|object| object.keys().any(|key| key != "_meta"))
            || closed.get("_meta").is_some_and(|meta| !meta.is_object())
        {
            return Err(rejected());
        }
        state.broker = AiGrokAcpSdkBroker::new(
            SERVER.into(),
            "bootstrap".into(),
            self.registration.tools().to_vec(),
            super::grok_acp::MAX_SDK_CALLS,
            FRAME,
            TOTAL,
        )?;
        let mut params = self.session_params();
        params["sessionId"] = json!(session);
        let resumed = self.rpc(&mut state, "session/resume", params).await?;
        self.validate_resumed(&resumed)?;
        AiProviderSessionCursor::new(self.registration.cursor_kind(), session)
            .map_err(|_| rejected())
    }
    async fn resume_session(&self, cursor: &AiProviderSessionCursor) -> Result<(), ProviderError> {
        if cursor.kind() != self.registration.cursor_kind() {
            return Err(rejected());
        }
        let mut state = self.state.lock().await;
        if self.session.lock().map_err(|_| rejected())?.is_some() {
            return Err(rejected());
        }
        self.initialize(&mut state).await?;
        let mut params = self.session_params();
        params["sessionId"] = json!(cursor.expose_to_provider_adapter());
        let response = self.rpc(&mut state, "session/resume", params).await?;
        self.validate_resumed(&response)?;
        *self.session.lock().map_err(|_| rejected())? =
            Some(cursor.expose_to_provider_adapter().into());
        Ok(())
    }
    async fn prompt(
        &self,
        input: Vec<String>,
        responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        let mut state = self.state.clone().lock_owned().await;
        if state.prompted {
            return Err(rejected());
        }
        state.prompted = true;
        let session = self
            .session
            .lock()
            .map_err(|_| rejected())?
            .clone()
            .ok_or_else(rejected)?;
        let id = Self::next_id(&mut state)?;
        let response_id = format!("grok-{}", uuid::Uuid::new_v4());
        state.broker.bind_prompt(response_id.clone())?;
        let prompt: Vec<_> = input
            .into_iter()
            .map(|text| json!({"type":"text","text":text}))
            .collect();
        Self::send(self.transport.as_ref(),json!({"jsonrpc":"2.0","id":id,"method":"session/prompt","params":{"sessionId":session,"prompt":prompt}})).await?;
        let transport = self.transport.clone();
        let registration = self.registration.clone();
        Ok(Box::pin(async_stream::try_stream! {
            yield ProviderEvent::ResponseStarted{response_id:Some(response_id.clone())};
            let mut tool_ids=BTreeSet::new();
            loop {
                let (bytes,value)=Self::read(transport.as_ref(),&mut state).await?;
                if value.get("method").is_none(){
                    if Self::internal_reload(&value,&mut state)? {continue;}
                    if value["id"]!=id||value.get("error").is_some(){Err(rejected())?;}
                    let result=&value["result"];
                    if state.broker.has_pending_calls(){Err(rejected())?;}
                    let category=result["_meta"]["cancellationCategory"].as_str();
                    if result["stopReason"]=="cancelled" && result["_meta"].get("usage").is_none() {
                        if category==Some("max_turns_reached") {Err(ProviderError::BudgetDenied)?;} else {Err(ProviderError::Cancelled)?;}
                    }
                    let usage=AiGrokAcpUsage::decode(&result["_meta"]["usage"],registration.usage_model(),super::grok_acp::MAX_USAGE_TOKENS,super::grok_acp::MAX_USAGE_TOKENS,u64::from(registration.maximum_model_calls()))?;
                    yield ProviderEvent::Usage{input_tokens:usage.input_tokens,output_tokens:usage.output_tokens,cached_input_tokens:usage.cached_input_tokens};
                    if category==Some("max_turns_reached") {Err(ProviderError::BudgetDenied)?;}
                    if result["stopReason"]=="cancelled" {Err(ProviderError::Cancelled)?;}
                    if result["stopReason"]!="end_turn"||result["_meta"].get("cancellationCategory").is_some()||result["_meta"].get("completionKind").is_some(){Err(rejected())?;}
                    yield ProviderEvent::ResponseCompleted{response_id:Some(response_id)};break;
                }
                if value["method"]=="_x.ai/mcp/sdk_call" {
                    let response=match state.broker.accept(&bytes)? {
                        AiGrokAcpSdkInbound::Response(response)=>response,
                        AiGrokAcpSdkInbound::ToolCall(call)=>{
                            let result=responder.respond(call).await?;
                            state.broker.tool_response(&result)?
                        }
                    };
                    transport.write_frame(response).await?;
                } else {
                    if value["method"]=="session/update" && value["params"]["update"]["sessionUpdate"]=="config_option_update" {
                        let update=&value["params"]["update"];
                        if value["params"]["sessionId"]!=session || value.get("id").is_some() || !Self::option_matches(update,"model",registration.model()) || !Self::option_matches(update,"reasoning_effort",registration.reasoning_effort().as_str()) {Err(rejected())?;}
                        continue;
                    }
                    if value["method"]=="_x.ai/session_notification" && value["params"]["update"]["sessionUpdate"]=="model_changed" {
                        let update=&value["params"]["update"];
                        if update["model_id"]!=registration.model() || update["reasoning_effort"]!=registration.reasoning_effort().as_str() {Err(rejected())?;}
                    }
                    if let Some(text)=Self::notification(&value,Some(&session),false,&mut tool_ids)? { yield ProviderEvent::TextDelta{text}; }
                }
            }
        }))
    }
    async fn cancel(&self) -> Result<(), ProviderError> {
        let session = self
            .session
            .lock()
            .map_err(|_| rejected())?
            .clone()
            .ok_or_else(rejected)?;
        Self::send(
            self.transport.as_ref(),
            json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}}),
        )
        .await
    }
    async fn delete_session(&self, cursor: &AiProviderSessionCursor) -> Result<(), ProviderError> {
        if cursor.kind() != self.registration.cursor_kind() {
            return Err(rejected());
        }
        let mut state = self.state.lock().await;
        self.initialize(&mut state).await?;
        let response = self
            .rpc(
                &mut state,
                "_x.ai/session/delete",
                json!({"sessionId":cursor.expose_to_provider_adapter(),"cwd":self.cwd}),
            )
            .await?;
        if response["success"] != true {
            return Err(rejected());
        }
        *self.session.lock().map_err(|_| rejected())? = None;
        Ok(())
    }
}

#[cfg(all(test, any(feature = "sqlite", feature = "postgres")))]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::collections::VecDeque;
    struct Wire {
        reads: SyncMutex<VecDeque<Value>>,
        writes: SyncMutex<Vec<Value>>,
    }
    #[async_trait]
    impl AiGrokAcpWireTransport for Wire {
        async fn write_frame(&self, frame: Vec<u8>) -> Result<(), ProviderError> {
            self.writes
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&frame).unwrap());
            Ok(())
        }
        async fn read_frame(&self) -> Result<Vec<u8>, ProviderError> {
            let value = self
                .reads
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(rejected)?;
            let mut bytes = serde_json::to_vec(&value).unwrap();
            bytes.push(b'\n');
            Ok(bytes)
        }
    }
    struct NoTools;
    #[async_trait]
    impl ProviderDynamicToolResponder for NoTools {
        async fn respond(
            &self,
            _: crate::ProviderDynamicToolCall,
        ) -> Result<crate::ProviderDynamicToolResult, ProviderError> {
            panic!("history or unoffered tool executed")
        }
    }
    fn response(id: u64, result: Value) -> Value {
        json!({"jsonrpc":"2.0","id":id,"result":result})
    }
    fn initialization() -> Vec<Value> {
        vec![
            response(
                1,
                json!({"protocolVersion":1,"_meta":{"x.ai/mcp/sdk":true},"agentCapabilities":{"loadSession":true},"authMethods":[{"id":"cached_token"}]}),
            ),
            response(2, json!({})),
        ]
    }
    fn config() -> Value {
        json!({"models":{"currentModelId":"grok-4.7"},"configOptions":[{"id":"model","currentValue":"grok-4.7"},{"id":"reasoning_effort","currentValue":"low"}]})
    }
    fn fixture(reads: Vec<Value>) -> (AiGrokAcpWireProcess, Arc<Wire>) {
        let wire = Arc::new(Wire {
            reads: SyncMutex::new(reads.into()),
            writes: SyncMutex::new(vec![]),
        });
        let process = AiGrokAcpWireProcess::new(
            Arc::new(super::super::grok_acp_provider::tests::registration()),
            wire.clone(),
            "/private/empty".into(),
        )
        .unwrap();
        (process, wire)
    }
    fn usage() -> Value {
        json!({"inputTokens":100,"outputTokens":20,"totalTokens":120,"cachedReadTokens":10,"cacheCreationTokens":0,"reasoningTokens":5,"modelCalls":1,"numTurns":1,"modelUsage":{"grok-4.7-build":{"inputTokens":100,"outputTokens":20,"totalTokens":120,"cachedReadTokens":10,"cacheCreationTokens":0,"reasoningTokens":5,"modelCalls":1}}})
    }
    fn update(kind: &str, text: &str) -> Value {
        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session-1","update":{"sessionUpdate":kind,"content":{"type":"text","text":text}}}})
    }
    fn new_session_reads() -> Vec<Value> {
        let mut reads = initialization();
        reads.extend([
            response(3, json!({"sessionId":"session-1"})),
            response(4, config()),
            response(5, config()),
            response(6, json!({"success":true})),
            response(7, json!({})),
            response(8, config()),
        ]);
        reads
    }
    #[tokio::test]
    async fn frozen_profile_scalar_selection_stream_and_usage() {
        let mut reads = new_session_reads();
        reads.extend([
            update("agent_thought_chunk", "private reasoning"),
            update("agent_message_chunk", "hello"),
            response(
                9,
                json!({"stopReason":"end_turn","_meta":{"usage":usage()}}),
            ),
        ]);
        let (process, wire) = fixture(reads);
        process.create_empty_session().await.unwrap();
        let mut stream = process
            .prompt(vec!["synthetic".into()], Arc::new(NoTools))
            .await
            .unwrap();
        assert!(matches!(
            stream.next().await.unwrap().unwrap(),
            ProviderEvent::ResponseStarted { .. }
        ));
        assert!(
            matches!(stream.next().await.unwrap().unwrap(),ProviderEvent::TextDelta{text} if text=="hello")
        );
        assert!(matches!(
            stream.next().await.unwrap().unwrap(),
            ProviderEvent::Usage {
                input_tokens: 100,
                output_tokens: 20,
                cached_input_tokens: 10
            }
        ));
        assert!(matches!(
            stream.next().await.unwrap().unwrap(),
            ProviderEvent::ResponseCompleted { .. }
        ));
        assert!(stream.next().await.is_none());
        let writes = wire.writes.lock().unwrap();
        assert_eq!(writes[1]["params"]["methodId"], "cached_token");
        assert_eq!(
            writes[2]["params"]["_meta"]["agentProfile"]["injectDefaultTools"],
            false
        );
        assert_eq!(
            writes[2]["params"]["_meta"]["agentProfile"]["toolConfig"]["tools"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(writes[3]["params"]["value"], "grok-4.7");
        assert_eq!(writes[4]["params"]["value"], "low");
    }
    #[tokio::test]
    async fn cancellation_without_usage_never_completes_or_invents_zero() {
        let mut reads = new_session_reads();
        reads.push(response(9, json!({"stopReason":"cancelled"})));
        let (process, wire) = fixture(reads);
        process.create_empty_session().await.unwrap();
        let mut stream = process
            .prompt(vec!["synthetic".into()], Arc::new(NoTools))
            .await
            .unwrap();
        process.cancel().await.unwrap();
        stream.next().await.unwrap().unwrap();
        assert!(matches!(
            stream.next().await.unwrap(),
            Err(ProviderError::Cancelled)
        ));
        assert!(stream.next().await.is_none());
        assert_eq!(
            wire.writes.lock().unwrap().last().unwrap()["method"],
            "session/cancel"
        );
    }
    #[tokio::test]
    async fn restart_resume_discards_history_and_delete_requires_authoritative_success() {
        let mut reads = initialization();
        reads.extend([
            update("agent_message_chunk", "completed earlier"),
            response(3, config()),
            response(4, json!({"success":true})),
            response(5, json!({"success":false})),
        ]);
        let (process, wire) = fixture(reads);
        let cursor =
            AiProviderSessionCursor::new(process.registration.cursor_kind(), "session-1").unwrap();
        process.resume_session(&cursor).await.unwrap();
        process.delete_session(&cursor).await.unwrap();
        assert!(process.delete_session(&cursor).await.is_err());
        assert_eq!(wire.writes.lock().unwrap()[2]["method"], "session/resume");
    }
    #[tokio::test]
    async fn moved_retained_namespace_rejects_resume_and_delete_before_transport() {
        let (process, wire) = fixture(vec![]);
        let moved = process
            .registration
            .as_ref()
            .clone()
            .with_retained_namespace("e".repeat(64))
            .unwrap();
        let cursor = AiProviderSessionCursor::new(moved.cursor_kind(), "session-1").unwrap();
        assert!(process.resume_session(&cursor).await.is_err());
        assert!(process.delete_session(&cursor).await.is_err());
        assert!(wire.writes.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn missing_usage_wrong_correlation_and_resume_callback_fail_closed() {
        for terminal in [
            response(9, json!({"stopReason":"end_turn"})),
            response(
                99,
                json!({"stopReason":"end_turn","_meta":{"usage":usage()}}),
            ),
        ] {
            let mut reads = new_session_reads();
            reads.push(terminal);
            let (process, _) = fixture(reads);
            process.create_empty_session().await.unwrap();
            let mut stream = process
                .prompt(vec!["synthetic".into()], Arc::new(NoTools))
                .await
                .unwrap();
            stream.next().await.unwrap().unwrap();
            assert!(stream.next().await.unwrap().is_err());
        }
        let mut reads = initialization();
        reads.push(
            json!({"jsonrpc":"2.0","id":100,"method":"session/request_permission","params":{}}),
        );
        let (process, _) = fixture(reads);
        let cursor =
            AiProviderSessionCursor::new(process.registration.cursor_kind(), "session-1").unwrap();
        assert!(process.resume_session(&cursor).await.is_err());
    }
    #[test]
    fn native_tools_background_work_and_compaction_fail_closed() {
        for value in [
            json!({"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call","toolCallId":"a","title":"shell"}}}),
            json!({"jsonrpc":"2.0","method":"_x.ai/session_notification","params":{"update":{"sessionUpdate":"background_tasks","tasks":[{"id":"a"}]}}}),
            json!({"jsonrpc":"2.0","method":"_x.ai/session_notification","params":{"update":{"sessionUpdate":"compaction_started"}}}),
        ] {
            assert!(
                AiGrokAcpWireProcess::notification(&value, None, false, &mut BTreeSet::new())
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn complete_usage_above_reservation_estimate_is_retained() {
        let mut actual = usage();
        actual["inputTokens"] = json!(29390);
        actual["outputTokens"] = json!(3000);
        actual["totalTokens"] = json!(32390);
        actual["modelUsage"]["grok-4.7-build"]["inputTokens"] = json!(29390);
        actual["modelUsage"]["grok-4.7-build"]["outputTokens"] = json!(3000);
        actual["modelUsage"]["grok-4.7-build"]["totalTokens"] = json!(32390);
        let mut reads = new_session_reads();
        reads.push(response(
            9,
            json!({"stopReason":"end_turn","_meta":{"usage":actual}}),
        ));
        let (process, _) = fixture(reads);
        process.create_empty_session().await.unwrap();
        let mut stream = process
            .prompt(vec!["synthetic".into()], Arc::new(NoTools))
            .await
            .unwrap();
        stream.next().await.unwrap().unwrap();
        assert!(matches!(
            stream.next().await.unwrap().unwrap(),
            ProviderEvent::Usage {
                input_tokens: 29390,
                output_tokens: 3000,
                ..
            }
        ));
        assert!(matches!(
            stream.next().await.unwrap().unwrap(),
            ProviderEvent::ResponseCompleted { .. }
        ));
    }
    #[tokio::test]
    async fn only_exact_internal_reload_and_manual_fixed_title_are_accepted() {
        let (process, _) = fixture(vec![]);
        let mut state = process.state.lock().await;
        assert!(
            AiGrokAcpWireProcess::internal_reload(
                &json!({"jsonrpc":"2.0","id":"skills-reload","result":{"result":{"reloaded":1}}}),
                &mut state
            )
            .unwrap()
        );
        assert!(
            AiGrokAcpWireProcess::internal_reload(
                &json!({"jsonrpc":"2.0","id":"skills-reload","result":{"result":{"reloaded":2}}}),
                &mut state
            )
            .is_err()
        );
        let mut manual = json!({"jsonrpc":"2.0","method":"_x.ai/session_notification","params":{"sessionId":"session-1","update":{"sessionUpdate":"session_summary_generated","session_summary":FIXED_TITLE},"_meta":{"x.ai/titleIsManual":true}}});
        assert!(
            AiGrokAcpWireProcess::notification(
                &manual,
                Some("session-1"),
                true,
                &mut BTreeSet::new()
            )
            .is_ok()
        );
        manual["params"]["_meta"]["x.ai/titleIsManual"] = json!(false);
        assert!(
            AiGrokAcpWireProcess::notification(
                &manual,
                Some("session-1"),
                true,
                &mut BTreeSet::new()
            )
            .is_err()
        );
    }
    #[cfg(unix)]
    struct LiveWire {
        read: Mutex<(
            tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
            Vec<u8>,
        )>,
        write: Mutex<tokio::net::unix::OwnedWriteHalf>,
    }
    #[cfg(unix)]
    #[async_trait]
    impl AiGrokAcpWireTransport for LiveWire {
        async fn write_frame(&self, frame: Vec<u8>) -> Result<(), ProviderError> {
            use tokio::io::AsyncWriteExt;
            self.write
                .lock()
                .await
                .write_all(&frame)
                .await
                .map_err(|_| rejected())
        }
        async fn read_frame(&self) -> Result<Vec<u8>, ProviderError> {
            use tokio::io::AsyncBufReadExt;
            let mut read = self.read.lock().await;
            loop {
                let (reader, pending) = &mut *read;
                let available = reader.fill_buf().await.map_err(|_| rejected())?;
                if available.is_empty() {
                    return Err(rejected());
                }
                let count = available
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(available.len(), |n| n + 1);
                if pending.len() + count > FRAME {
                    return Err(rejected());
                }
                pending.extend_from_slice(&available[..count]);
                reader.consume(count);
                if pending.ends_with(b"\n") {
                    return Ok(std::mem::take(pending));
                }
            }
        }
    }
    #[cfg(unix)]
    async fn live_process(registration: Arc<AiGrokAcpRegistration>) -> AiGrokAcpWireProcess {
        let socket =
            std::env::var("GROK_ACP_TEST_SOCKET").expect("explicit isolated host socket required");
        let cwd = std::env::var("GROK_ACP_TEST_CWD").expect("explicit isolated cwd required");
        let stream = tokio::net::UnixStream::connect(socket).await.unwrap();
        let (read, write) = stream.into_split();
        AiGrokAcpWireProcess::new(
            registration,
            Arc::new(LiveWire {
                read: Mutex::new((tokio::io::BufReader::new(read), vec![])),
                write: Mutex::new(write),
            }),
            cwd,
        )
        .unwrap()
    }
    #[cfg(unix)]
    struct SyntheticBroker(SyncMutex<Vec<String>>);
    #[cfg(unix)]
    #[async_trait]
    impl ProviderDynamicToolResponder for SyntheticBroker {
        async fn respond(
            &self,
            call: crate::ProviderDynamicToolCall,
        ) -> Result<crate::ProviderDynamicToolResult, ProviderError> {
            let mut calls = self.0.lock().unwrap();
            let expected = [
                "graphql_capabilities_discover",
                "graphql_capabilities_describe",
                "graphql_capabilities_execute",
            ];
            if calls.len() >= expected.len() || call.provider_name() != expected[calls.len()] {
                return Err(rejected());
            }
            let result = match calls.len() {
                0 => json!({"capability":"probe.sample","description":"Synthetic count only"}),
                1 => {
                    if call.arguments() != &json!({"capability":"probe.sample"}) {
                        return Err(rejected());
                    }
                    json!({"reference":"probe-run-1","description":"Returns synthetic count"})
                }
                _ => {
                    if call.arguments() != &json!({"reference":"probe-run-1"}) {
                        return Err(rejected());
                    }
                    json!({"count":7})
                }
            };
            calls.push(call.provider_name().into());
            crate::ProviderDynamicToolResult::new(&call, result)
        }
    }
    #[cfg(unix)]
    fn live_registration() -> Arc<AiGrokAcpRegistration> {
        use crate::{ModelReasoningEffort, ModelReasoningEffortProfile, ModelToolDefinition};
        let tools=[("discover","query"),("describe","capability"),("execute","reference")].into_iter().map(|(name,key)|ModelToolDefinition{
            tool_id:format!("capabilities.{name}"),provider_name:format!("graphql_capabilities_{name}"),fingerprint:hex::encode(sha2::Sha256::digest(name.as_bytes())),description:format!("{name} synthetic probe capability"),parameters:json!({"type":"object","properties":{key:{"type":"string"}},"required":[key],"additionalProperties":false}),strict:true,defer_loading:false,
        }).collect();
        use sha2::Digest;
        Arc::new(AiGrokAcpRegistration::new("grok-live-synthetic".into(),"grok-4.7".into(),"92c997dfd109c0672d40d5ae6fbd15835d53ffaf12cf9ea124d22aaef3ff23fc".into(),"1.0.40".into(),"explicit-isolated-test-socket".into(),ModelReasoningEffortProfile::new("grok-4.7",[ModelReasoningEffort::Low],ModelReasoningEffort::Low).unwrap(),ModelReasoningEffort::Low,"Use only the provided authorized synthetic capability tools. Never use other tools.".into(),tools,16_384,2_048,8).unwrap().with_usage_model("grok-4.7-build".into()).unwrap().with_retained_namespace("d".repeat(64)).unwrap())
    }
    /// Requires an explicitly supplied, isolated, authenticated host pipe proxy.
    /// Uses only synthetic tool data; emits counts/status, never native payloads.
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "requires explicit isolated Grok ACP host socket and existing managed login"]
    async fn live_grok_acp_synthetic_broker_restart_resume_and_delete() {
        let registration = live_registration();
        let process = live_process(registration.clone()).await;
        let cursor = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            process.create_empty_session(),
        )
        .await
        .unwrap()
        .expect("empty-session preflight");
        let responder = Arc::new(SyntheticBroker(SyncMutex::new(vec![])));
        let mut stream=process.prompt(vec!["Use graphql_capabilities_discover with query synthetic, then graphql_capabilities_describe with its returned capability, then graphql_capabilities_execute with its returned reference, each exactly once. Report only the resulting count.".into()],responder.clone()).await.expect("prompt dispatch");
        let mut complete = false;
        let mut metered = false;
        let mut text = false;
        while let Some(event) =
            tokio::time::timeout(std::time::Duration::from_secs(120), stream.next())
                .await
                .unwrap()
        {
            match event.expect("synthetic streaming protocol") {
                ProviderEvent::TextDelta { text: delta } => text |= !delta.is_empty(),
                ProviderEvent::Usage {
                    input_tokens,
                    output_tokens,
                    ..
                } => {
                    assert!(input_tokens > 0 && output_tokens > 0);
                    metered = true;
                }
                ProviderEvent::ResponseCompleted { .. } => complete = true,
                _ => {}
            }
        }
        assert!(complete && metered && text);
        assert_eq!(responder.0.lock().unwrap().len(), 3);
        drop(stream);
        drop(process);
        let resumed = live_process(registration.clone()).await;
        tokio::time::timeout(
            std::time::Duration::from_secs(60),
            resumed.resume_session(&cursor),
        )
        .await
        .unwrap()
        .expect("restart retained resume");
        assert_eq!(responder.0.lock().unwrap().len(), 3);
        let mut followup=resumed.prompt(vec!["Without calling any tools, report only the synthetic count returned in our previous turn.".into()],Arc::new(NoTools)).await.unwrap();
        let mut answer = String::new();
        let mut completed = false;
        while let Some(event) =
            tokio::time::timeout(std::time::Duration::from_secs(60), followup.next())
                .await
                .unwrap()
        {
            match event.unwrap() {
                ProviderEvent::TextDelta { text } => answer.push_str(&text),
                ProviderEvent::ResponseCompleted { .. } => completed = true,
                _ => {}
            }
        }
        assert!(completed && answer.contains('7'));
        assert_eq!(responder.0.lock().unwrap().len(), 3);
        drop(followup);
        resumed
            .delete_session(&cursor)
            .await
            .expect("delete retained session");
        resumed
            .delete_session(&cursor)
            .await
            .expect("idempotent absence");
        drop(resumed);
        let absent = live_process(registration).await;
        assert!(absent.resume_session(&cursor).await.is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "requires explicit isolated Grok ACP host socket and existing managed login"]
    async fn live_grok_acp_stop_after_visible_text_is_uncertain_and_deletable() {
        let process = live_process(live_registration()).await;
        let cursor = process.create_empty_session().await.unwrap();
        let mut stream=process.prompt(vec!["Do not call tools. Write all integers from 1 to 100000 in order, separated by spaces. Continue until complete.".into()],Arc::new(NoTools)).await.unwrap();
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(60), stream.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
            {
                ProviderEvent::TextDelta { .. } => break,
                ProviderEvent::ResponseCompleted { .. } => panic!("finished before Stop"),
                _ => {}
            }
        }
        process.cancel().await.unwrap();
        let mut cancelled = false;
        while let Some(event) =
            tokio::time::timeout(std::time::Duration::from_secs(15), stream.next())
                .await
                .unwrap()
        {
            match event {
                Err(ProviderError::Cancelled) => {
                    cancelled = true;
                    break;
                }
                Ok(ProviderEvent::ResponseCompleted { .. }) => panic!("Stop reported completed"),
                Ok(_) => {}
                Err(_) => panic!("Stop protocol failure"),
            }
        }
        assert!(cancelled);
        drop(stream);
        process.delete_session(&cursor).await.unwrap();
    }
}
