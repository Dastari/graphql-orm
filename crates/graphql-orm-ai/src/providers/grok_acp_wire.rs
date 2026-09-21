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

const FRAME: usize = 1024 * 1024;
const TOTAL: usize = 64 * FRAME;
const SERVER: &str = "graphql-orm-ai-broker";
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
    /// Reads one newline-terminated frame, limiting allocation to 1 MiB before
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
        if !std::path::Path::new(&cwd).is_absolute()
            || cwd.len() > 4096
            || cwd.chars().any(char::is_control)
        {
            return Err(ProviderError::InvalidRequest);
        }
        let broker = AiGrokAcpSdkBroker::new(
            SERVER.into(),
            "bootstrap".into(),
            registration.tools().to_vec(),
            64,
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
            || state.frames > 8192
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
                            || tool_ids.len() >= 256
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
        AiProviderSessionCursor::new("grok.acp.session.v1", session).map_err(|_| rejected())
    }
    async fn resume_session(&self, cursor: &AiProviderSessionCursor) -> Result<(), ProviderError> {
        if cursor.kind() != "grok.acp.session.v1" {
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
        if response["models"]["currentModelId"] != self.registration.model()
            || !Self::option_matches(&response, "model", self.registration.model())
            || (self.registration.reasoning_effort() != crate::ModelReasoningEffort::Unspecified
                && !Self::option_matches(
                    &response,
                    "reasoning_effort",
                    self.registration.reasoning_effort().as_str(),
                ))
        {
            return Err(rejected());
        }
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
                    if value["id"]!=id||value.get("error").is_some(){Err(rejected())?;}
                    let result=&value["result"];
                    if state.broker.has_pending_calls(){Err(rejected())?;}
                    let category=result["_meta"]["cancellationCategory"].as_str();
                    if result["stopReason"]=="cancelled" && result["_meta"].get("usage").is_none() {
                        if category==Some("max_turns_reached") {Err(ProviderError::BudgetDenied)?;} else {Err(ProviderError::Cancelled)?;}
                    }
                    let usage=AiGrokAcpUsage::decode(&result["_meta"]["usage"],registration.usage_model(),registration.maximum_input_tokens(),registration.maximum_output_tokens(),u64::from(registration.maximum_model_calls()))?;
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
        if cursor.kind() != "grok.acp.session.v1" {
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
                6,
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
        reads.push(response(6, json!({"stopReason":"cancelled"})));
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
        let cursor = AiProviderSessionCursor::new("grok.acp.session.v1", "session-1").unwrap();
        process.resume_session(&cursor).await.unwrap();
        process.delete_session(&cursor).await.unwrap();
        assert!(process.delete_session(&cursor).await.is_err());
        assert_eq!(wire.writes.lock().unwrap()[2]["method"], "session/resume");
    }
    #[tokio::test]
    async fn missing_usage_wrong_correlation_and_resume_callback_fail_closed() {
        for terminal in [
            response(6, json!({"stopReason":"end_turn"})),
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
        let cursor = AiProviderSessionCursor::new("grok.acp.session.v1", "session-1").unwrap();
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
}
