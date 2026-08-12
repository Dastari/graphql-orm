//! Strict run-scoped Codex app-server process and retained-thread boundary.
//!
//! The default path reuses one bounded process while starting a fresh ephemeral
//! thread per call. A separately planned provider-session path may resume one
//! exact protected thread cursor. Experimental dynamic tools are disabled by
//! default; when a registration explicitly enables them, the adapter admits
//! only exact reviewed definitions and delegates execution back to the
//! ordinary coordinator. Every other server-initiated request remains
//! forbidden.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use agql_auth::Clock;
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::{
    AiProvider, AiProviderRunBinding, AiProviderRunCloseOutcome, AiProviderRunCloseReason,
    AiProviderRunInterruptOutcome, ModelContinuationMode, ModelInputBlock,
    ModelReasoningSummaryRequest, ModelRequest, ModelToolDefinition, ProviderCapabilities,
    ProviderDynamicToolCall, ProviderDynamicToolResponder, ProviderError, ProviderEventStream,
    ProviderKind, ProviderRequestContext,
};

const MAXIMUM_PROCESSES: usize = 4_096;
const MAXIMUM_TURNS_PER_RUN: u32 = 1_024;
const MAXIMUM_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAXIMUM_TEXT_BYTES: usize = 16 * 1024 * 1024;
const MAXIMUM_TEXT_BLOCKS: usize = 256;
const MAXIMUM_IDENTIFIER_BYTES: usize = 200;
const MAXIMUM_VERSION_BYTES: usize = 200;
const MAXIMUM_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const REMOTE_CONTROL_STATUS_CHANGED: &str = "remoteControl/status/changed";

/// Exact reviewed Codex app-server protocol contract supported by this
/// adapter.
pub const AI_CODEX_APP_SERVER_PROTOCOL_V2: &str = "app-server-v2";

/// Immutable, content-free identity of one reviewed Codex app-server
/// installation and provider profile.
///
/// A process factory must map this value to one fixed executable, argument
/// vector, cleared environment, working directory, and external sandbox. The
/// model and request cannot select any of those values. Construction proves
/// syntactic validity only; it is not executable-integrity or sandbox evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiCodexAppServerRegistration {
    provider_profile_id: String,
    logical_model: String,
    executable_sha256: String,
    executable_version: String,
    sandbox_profile: String,
    protocol_version: String,
    experimental_dynamic_tools: bool,
    identity: String,
}

impl AiCodexAppServerRegistration {
    /// Creates one immutable deployment registration identity.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidConfiguration`] for malformed or
    /// oversized identifiers, versions, or executable digest.
    pub fn new(
        provider_profile_id: impl Into<String>,
        logical_model: impl Into<String>,
        executable_sha256: impl Into<String>,
        executable_version: impl Into<String>,
        sandbox_profile: impl Into<String>,
        protocol_version: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        let provider_profile_id = provider_profile_id.into();
        let logical_model = logical_model.into();
        let executable_sha256 = executable_sha256.into();
        let executable_version = executable_version.into();
        let sandbox_profile = sandbox_profile.into();
        let protocol_version = protocol_version.into();
        if !valid_identifier(&provider_profile_id)
            || !valid_identifier(&logical_model)
            || !crate::valid_sha256(&executable_sha256)
            || !valid_version(&executable_version)
            || !valid_identifier(&sandbox_profile)
            || protocol_version != AI_CODEX_APP_SERVER_PROTOCOL_V2
        {
            return Err(ProviderError::InvalidConfiguration(
                "invalid Codex app-server registration".to_owned(),
            ));
        }
        let identity = registration_identity(
            &provider_profile_id,
            &logical_model,
            &executable_sha256,
            &executable_version,
            &sandbox_profile,
            &protocol_version,
            false,
        );
        Ok(Self {
            provider_profile_id,
            logical_model,
            executable_sha256,
            executable_version,
            sandbox_profile,
            protocol_version,
            experimental_dynamic_tools: false,
            identity,
        })
    }

    /// Enables the reviewed experimental native dynamic-tool protocol.
    ///
    /// This changes the immutable registration identity. It only permits the
    /// adapter to forward an exact provider request to a coordinator-owned
    /// responder; it grants no application-tool or resolver authority.
    #[must_use]
    pub fn with_experimental_dynamic_tools(mut self) -> Self {
        self.experimental_dynamic_tools = true;
        self.identity = registration_identity(
            &self.provider_profile_id,
            &self.logical_model,
            &self.executable_sha256,
            &self.executable_version,
            &self.sandbox_profile,
            &self.protocol_version,
            true,
        );
        self
    }

    /// Deployment-owned provider profile identifier.
    pub fn provider_profile_id(&self) -> &str {
        &self.provider_profile_id
    }

    /// Server-selected logical model.
    pub fn logical_model(&self) -> &str {
        &self.logical_model
    }

    /// Reviewed executable image digest.
    pub fn executable_sha256(&self) -> &str {
        &self.executable_sha256
    }

    /// Reviewed executable version.
    pub fn executable_version(&self) -> &str {
        &self.executable_version
    }

    /// Deployment-owned external sandbox profile.
    pub fn sandbox_profile(&self) -> &str {
        &self.sandbox_profile
    }

    /// Reviewed app-server protocol version.
    pub fn protocol_version(&self) -> &str {
        &self.protocol_version
    }

    /// Whether experimental app-server dynamic tools are enabled for this
    /// immutable registration.
    pub const fn experimental_dynamic_tools(&self) -> bool {
        self.experimental_dynamic_tools
    }

    /// Stable content-free registration identity used to prevent configuration
    /// swaps inside one claimed run.
    pub fn identity(&self) -> &str {
        &self.identity
    }
}

/// Bounded text-only input for one fresh app-server turn.
///
/// This type has no tool, URL, path, shell, environment, browser, image, MCP,
/// app, credential, or generic JSON-RPC field. It intentionally cannot model
/// coordinator-managed application tools.
#[derive(Clone, PartialEq)]
pub struct AiCodexAppServerTurnInput {
    model: String,
    instructions: Vec<String>,
    input: Vec<String>,
    tools: Vec<ModelToolDefinition>,
    maximum_output_tokens: u64,
}

impl std::fmt::Debug for AiCodexAppServerTurnInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiCodexAppServerTurnInput")
            .field("model", &self.model)
            .field("instructions", &"<protected>")
            .field("instruction_count", &self.instructions.len())
            .field("input", &"<protected>")
            .field("input_count", &self.input.len())
            .field("tool_count", &self.tools.len())
            .field("maximum_output_tokens", &self.maximum_output_tokens)
            .finish()
    }
}

impl AiCodexAppServerTurnInput {
    /// Creates one bounded text-only fresh turn.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidRequest`] for an invalid model, empty or
    /// oversized input, excessive instructions, NUL data, or a zero output
    /// ceiling.
    pub(crate) fn new(
        model: impl Into<String>,
        instructions: Vec<String>,
        input: Vec<String>,
        maximum_output_tokens: u64,
    ) -> Result<Self, ProviderError> {
        let turn = Self {
            model: model.into(),
            instructions,
            input,
            tools: Vec::new(),
            maximum_output_tokens,
        };
        turn.validate()?;
        Ok(turn)
    }

    fn validate(&self) -> Result<(), ProviderError> {
        let text_bytes = self
            .instructions
            .iter()
            .chain(&self.input)
            .try_fold(0_usize, |total, value| total.checked_add(value.len()))
            .ok_or(ProviderError::InvalidRequest)?;
        if !valid_identifier(&self.model)
            || self.instructions.len() > 32
            || self.input.is_empty()
            || self.input.len() > MAXIMUM_TEXT_BLOCKS
            || self.tools.len() > 128
            || text_bytes > MAXIMUM_TEXT_BYTES
            || self
                .instructions
                .iter()
                .chain(&self.input)
                .any(|value| value.contains('\0'))
            || self.maximum_output_tokens == 0
            || self.maximum_output_tokens > u64::from(u32::MAX)
        {
            return Err(ProviderError::InvalidRequest);
        }
        Ok(())
    }

    /// Exact server-selected model.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Trusted runtime instructions.
    pub fn instructions(&self) -> &[String] {
        &self.instructions
    }

    /// Bounded text input blocks.
    pub fn input(&self) -> &[String] {
        &self.input
    }

    /// Exact reviewed application-tool definitions for this turn.
    pub fn tools(&self) -> &[ModelToolDefinition] {
        &self.tools
    }

    /// Requested output-token ceiling.
    pub const fn maximum_output_tokens(&self) -> u64 {
        self.maximum_output_tokens
    }

    fn try_from_model_request(request: ModelRequest) -> Result<Self, ProviderError> {
        Self::try_from_tool_free_request(request, ModelContinuationMode::StatelessReplay)
    }

    fn try_from_retained_model_request(request: ModelRequest) -> Result<Self, ProviderError> {
        Self::try_from_tool_free_request(request, ModelContinuationMode::ProviderRetained)
    }

    fn try_from_tool_free_request(
        request: ModelRequest,
        expected_mode: ModelContinuationMode,
    ) -> Result<Self, ProviderError> {
        request.validate()?;
        if request.continuation.is_some()
            || request.continuation_mode != expected_mode
            || !request.tools.is_empty()
            || !request.builtin_tools.is_empty()
            || request.maximum_builtin_tool_calls.is_some()
            || request.reasoning_summary != ModelReasoningSummaryRequest::Disabled
            || request.output_schema.is_some()
        {
            return Err(ProviderError::Unsupported);
        }
        let input = request
            .input
            .into_iter()
            .map(|block| match block {
                ModelInputBlock::Text { text } => Ok(text),
                ModelInputBlock::Json { .. }
                | ModelInputBlock::ToolResult { .. }
                | ModelInputBlock::Attachment { .. } => Err(ProviderError::Unsupported),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(
            request.model,
            request.instructions,
            input,
            request
                .maximum_output_tokens
                .ok_or(ProviderError::InvalidRequest)?,
        )
    }

    fn try_from_dynamic_request(request: ModelRequest) -> Result<Self, ProviderError> {
        request.validate()?;
        if request.tools.is_empty()
            || request.continuation.is_some()
            || request.continuation_mode != ModelContinuationMode::ProviderRetained
            || !request.builtin_tools.is_empty()
            || request.maximum_builtin_tool_calls.is_some()
            || request.reasoning_summary != ModelReasoningSummaryRequest::Disabled
            || request.output_schema.is_some()
        {
            return Err(ProviderError::Unsupported);
        }
        let input = request
            .input
            .iter()
            .map(|block| match block {
                ModelInputBlock::Text { text } => Ok(text.clone()),
                ModelInputBlock::Json { value } => {
                    serde_json::to_string(value).map_err(|_| ProviderError::InvalidRequest)
                }
                ModelInputBlock::ToolResult { .. } | ModelInputBlock::Attachment { .. } => {
                    Err(ProviderError::Unsupported)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut turn = Self::new(
            request.model,
            request.instructions,
            input,
            request
                .maximum_output_tokens
                .ok_or(ProviderError::InvalidRequest)?,
        )?;
        turn.tools = request.tools;
        turn.validate()?;
        Ok(turn)
    }
}

/// Resource limits for the run-scoped app-server pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiCodexAppServerRunLimits {
    maximum_processes: usize,
    maximum_processes_per_owner: usize,
    maximum_turns_per_run: u32,
    startup_timeout: Duration,
    turn_timeout: Duration,
    interrupt_timeout: Duration,
    shutdown_timeout: Duration,
}

impl AiCodexAppServerRunLimits {
    /// Creates bounded process, turn, and lifecycle limits.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidConfiguration`] when a count is zero or
    /// above its hard maximum, or a timeout is zero or above one hour.
    pub fn new(
        maximum_processes: usize,
        maximum_turns_per_run: u32,
        startup_timeout: Duration,
        turn_timeout: Duration,
        interrupt_timeout: Duration,
        shutdown_timeout: Duration,
    ) -> Result<Self, ProviderError> {
        if !(1..=MAXIMUM_PROCESSES).contains(&maximum_processes)
            || !(1..=MAXIMUM_TURNS_PER_RUN).contains(&maximum_turns_per_run)
            || [
                startup_timeout,
                turn_timeout,
                interrupt_timeout,
                shutdown_timeout,
            ]
            .into_iter()
            .any(|timeout| timeout.is_zero() || timeout > MAXIMUM_TIMEOUT)
        {
            return Err(ProviderError::InvalidConfiguration(
                "invalid Codex app-server run limits".to_owned(),
            ));
        }
        Ok(Self {
            maximum_processes,
            maximum_processes_per_owner: maximum_processes.min(4),
            maximum_turns_per_run,
            startup_timeout,
            turn_timeout,
            interrupt_timeout,
            shutdown_timeout,
        })
    }

    /// Maximum simultaneously retained run processes.
    pub const fn maximum_processes(self) -> usize {
        self.maximum_processes
    }

    /// Sets the maximum simultaneously retained processes for one principal
    /// subject across its runs and sessions.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidConfiguration`] unless the value is in
    /// `1..=maximum_processes`.
    pub fn with_maximum_processes_per_owner(
        mut self,
        maximum_processes_per_owner: usize,
    ) -> Result<Self, ProviderError> {
        if !(1..=self.maximum_processes).contains(&maximum_processes_per_owner) {
            return Err(ProviderError::InvalidConfiguration(
                "invalid Codex app-server per-owner limit".to_owned(),
            ));
        }
        self.maximum_processes_per_owner = maximum_processes_per_owner;
        Ok(self)
    }

    /// Maximum simultaneously retained processes for one principal subject.
    pub const fn maximum_processes_per_owner(self) -> usize {
        self.maximum_processes_per_owner
    }

    /// Maximum fresh turns sent through one retained process.
    pub const fn maximum_turns_per_run(self) -> u32 {
        self.maximum_turns_per_run
    }

    /// Maximum process launch and protocol initialization duration.
    pub const fn startup_timeout(self) -> Duration {
        self.startup_timeout
    }

    /// Maximum wall-clock duration of one provider turn stream.
    pub const fn turn_timeout(self) -> Duration {
        self.turn_timeout
    }

    /// Maximum graceful interruption duration.
    pub const fn interrupt_timeout(self) -> Duration {
        self.interrupt_timeout
    }

    /// Maximum graceful shutdown duration before the process drop-kill
    /// fallback remains responsible for cleanup.
    pub const fn shutdown_timeout(self) -> Duration {
        self.shutdown_timeout
    }
}

impl Default for AiCodexAppServerRunLimits {
    fn default() -> Self {
        Self {
            maximum_processes: 32,
            maximum_processes_per_owner: 4,
            maximum_turns_per_run: 16,
            startup_timeout: Duration::from_secs(30),
            turn_timeout: Duration::from_secs(5 * 60),
            interrupt_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_secs(5),
        }
    }
}

/// Strict Codex app-server provider adapter.
///
/// The default registration is tool-free and starts fresh ephemeral threads.
/// An exact provider-session turn may instead resume a protected cursor, and
/// an explicit experimental registration may offer reviewed dynamic tools
/// whose execution remains coordinator-owned. Provider built-ins, stateless
/// conversation continuations, attachments, structured output, and raw
/// reasoning remain unavailable.
#[derive(Clone, Debug)]
pub struct AiCodexAppServerProvider {
    registration: Arc<AiCodexAppServerRegistration>,
    pool: AiCodexAppServerRunPool,
}

impl AiCodexAppServerProvider {
    /// Creates a provider bound to one immutable registration and process
    /// pool.
    pub fn new(
        registration: Arc<AiCodexAppServerRegistration>,
        pool: AiCodexAppServerRunPool,
    ) -> Self {
        Self { registration, pool }
    }

    /// Exact immutable deployment registration.
    pub fn registration(&self) -> &AiCodexAppServerRegistration {
        &self.registration
    }

    /// Creates the exact retained-thread cleanup adapter using the host's
    /// trusted clock.
    pub fn provider_session_deletion_service(
        &self,
        clock: Arc<dyn Clock>,
    ) -> AiCodexAppServerProviderSessionDeletionService {
        AiCodexAppServerProviderSessionDeletionService {
            registration: self.registration.clone(),
            pool: self.pool.clone(),
            clock,
        }
    }
}

/// Bounded deletion/absence adapter for retained Codex app-server threads.
#[derive(Clone)]
pub struct AiCodexAppServerProviderSessionDeletionService {
    registration: Arc<AiCodexAppServerRegistration>,
    pool: AiCodexAppServerRunPool,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for AiCodexAppServerProviderSessionDeletionService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiCodexAppServerProviderSessionDeletionService")
            .field("registration", &self.registration.identity())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl crate::AiProviderSessionDeletionService for AiCodexAppServerProviderSessionDeletionService {
    async fn delete_or_confirm_absent(
        &self,
        request: &crate::AiProviderSessionDeletionRequest,
    ) -> Result<crate::AiProviderSessionAbsenceProof, ProviderError> {
        let descriptor = request.claim().descriptor();
        if descriptor.provider_kind() != &ProviderKind::LocalHarness
            || descriptor.provider_profile_id() != self.registration.provider_profile_id()
            || descriptor.provider_model() != self.registration.logical_model()
            || descriptor.registration_fingerprint() != self.registration.identity()
            || descriptor.protocol_version() != self.registration.protocol_version()
            || request.cursor().kind() != "codex.app_server.thread.v2"
        {
            return Err(ProviderError::Rejected);
        }
        self.pool
            .delete_detached_thread(self.registration.clone(), request.cursor())
            .await?;
        Ok(crate::AiProviderSessionAbsenceProof::for_request(
            request,
            self.clock.now(),
        ))
    }
}

#[async_trait]
impl AiProvider for AiCodexAppServerProvider {
    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::LocalHarness
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            custom_tools: self.registration.experimental_dynamic_tools(),
            provider_retained_continuation: true,
            local: true,
            ..ProviderCapabilities::default()
        }
    }

    async fn stream(
        &self,
        request: ModelRequest,
        context: ProviderRequestContext,
    ) -> Result<ProviderEventStream, ProviderError> {
        context.validate_request(&ProviderKind::LocalHarness, &request)?;
        context.validate_provider_profile(
            &ProviderKind::LocalHarness,
            &request,
            self.registration.provider_profile_id(),
        )?;
        let binding = context.run_binding().ok_or(ProviderError::Rejected)?;
        let retained = context.provider_session().cloned();
        let input = if retained.is_some() {
            AiCodexAppServerTurnInput::try_from_retained_model_request(request)?
        } else {
            AiCodexAppServerTurnInput::try_from_model_request(request)?
        };
        if let Some(session) = retained {
            match session.activation() {
                crate::AiProviderSessionActivation::NewlyBoundEmpty => {
                    self.pool
                        .start_bound_turn(binding, self.registration.clone(), session, input)
                        .await
                }
                crate::AiProviderSessionActivation::ExistingRetained => {
                    self.pool
                        .start_retained_turn(binding, self.registration.clone(), session, input)
                        .await
                }
            }
        } else {
            self.pool
                .start_fresh_turn(binding, self.registration.clone(), input)
                .await
        }
    }

    async fn stream_with_dynamic_tools(
        &self,
        request: ModelRequest,
        context: ProviderRequestContext,
        responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        if !self.registration.experimental_dynamic_tools() {
            return Err(ProviderError::Unsupported);
        }
        context.validate_request(&ProviderKind::LocalHarness, &request)?;
        context.validate_provider_profile(
            &ProviderKind::LocalHarness,
            &request,
            self.registration.provider_profile_id(),
        )?;
        let binding = context.run_binding().ok_or(ProviderError::Rejected)?;
        let retained = context.provider_session().cloned();
        let input = AiCodexAppServerTurnInput::try_from_dynamic_request(request)?;
        if let Some(session) = retained {
            match session.activation() {
                crate::AiProviderSessionActivation::NewlyBoundEmpty => {
                    self.pool
                        .start_bound_dynamic_turn(
                            binding,
                            self.registration.clone(),
                            session,
                            input,
                            responder,
                        )
                        .await
                }
                crate::AiProviderSessionActivation::ExistingRetained => {
                    self.pool
                        .start_retained_dynamic_turn(
                            binding,
                            self.registration.clone(),
                            session,
                            input,
                            responder,
                        )
                        .await
                }
            }
        } else {
            self.pool
                .start_dynamic_turn(binding, self.registration.clone(), input, responder)
                .await
        }
    }

    async fn interrupt_run(
        &self,
        binding: &AiProviderRunBinding,
    ) -> Result<AiProviderRunInterruptOutcome, ProviderError> {
        self.pool.interrupt_run(binding).await
    }

    async fn close_run(
        &self,
        binding: &AiProviderRunBinding,
        reason: AiProviderRunCloseReason,
    ) -> Result<AiProviderRunCloseOutcome, ProviderError> {
        self.pool.close_run(binding, reason).await
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    async fn create_empty_session(
        &self,
        binding: &AiProviderRunBinding,
        descriptor: &crate::AiProviderSessionDescriptor,
        request: &ModelRequest,
    ) -> Result<crate::AiProviderSessionCursor, ProviderError> {
        if descriptor.provider_kind() != &ProviderKind::LocalHarness
            || descriptor.provider_profile_id() != self.registration.provider_profile_id()
            || descriptor.provider_model() != self.registration.logical_model()
            || descriptor.registration_fingerprint() != self.registration.identity()
            || descriptor.protocol_version() != self.registration.protocol_version()
        {
            return Err(ProviderError::Rejected);
        }
        let input = if request.tools.is_empty() {
            AiCodexAppServerTurnInput::try_from_retained_model_request(request.clone())?
        } else {
            if !self.registration.experimental_dynamic_tools() {
                return Err(ProviderError::Unsupported);
            }
            AiCodexAppServerTurnInput::try_from_dynamic_request(request.clone())?
        };
        self.pool
            .create_empty_thread(*binding, self.registration.clone(), input.tools().to_vec())
            .await
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    async fn discard_empty_session(
        &self,
        binding: &AiProviderRunBinding,
        descriptor: &crate::AiProviderSessionDescriptor,
        cursor: &crate::AiProviderSessionCursor,
    ) -> Result<(), ProviderError> {
        if descriptor.provider_kind() != &ProviderKind::LocalHarness
            || descriptor.provider_profile_id() != self.registration.provider_profile_id()
            || descriptor.provider_model() != self.registration.logical_model()
            || descriptor.registration_fingerprint() != self.registration.identity()
            || descriptor.protocol_version() != self.registration.protocol_version()
        {
            return Err(ProviderError::Rejected);
        }
        self.pool.discard_empty_thread(*binding, cursor).await
    }
}

/// One launched app-server process owned by an exact run binding.
///
/// Implementations must expose only the reviewed typed operations below. They
/// must not expose a generic JSON-RPC method, must reject forbidden or unknown
/// inbound traffic, and must be wrapped in
/// [`AiCodexAppServerLaunchedProcess`] with an exact process-tree kill action.
#[async_trait]
pub trait AiCodexAppServerRunProcess: Send + Sync {
    /// Creates one durable empty provider thread and returns its opaque cursor.
    ///
    /// No developer instruction, user input, or other business content may
    /// enter the thread before the caller durably binds it. Reviewed dynamic
    /// tool definitions may be installed because app-server cannot add them
    /// during `thread/resume`; implementations must transmit exactly the
    /// supplied definitions or reject them.
    async fn create_empty_thread(
        &self,
        _model: &str,
        _dynamic_tools: Vec<ModelToolDefinition>,
    ) -> Result<crate::AiProviderSessionCursor, ProviderError> {
        Err(ProviderError::Unsupported)
    }

    /// Starts one fresh, text-only thread/turn on the retained process.
    ///
    /// The implementation must enforce the supplied output-token ceiling or
    /// reject the request before starting it. It must normalize only visible
    /// assistant text and authoritative usage/completion events, and abort the
    /// process on every forbidden or unknown protocol event.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive error for protocol, transport, resource, usage,
    /// or normalized-event failure.
    async fn start_fresh_turn(
        &self,
        input: AiCodexAppServerTurnInput,
    ) -> Result<ProviderEventStream, ProviderError>;

    /// Starts one fresh thread/turn with experimental native dynamic tools.
    ///
    /// The process must advertise only `input.tools()`, forward each exact
    /// `item/tool/call` request to `responder`, and write only the returned
    /// response to app-server. It must not execute, route, or discover tools
    /// itself. Implementations that have not reviewed this protocol remain
    /// fail-closed.
    async fn start_dynamic_turn(
        &self,
        _input: AiCodexAppServerTurnInput,
        _responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        Err(ProviderError::Unsupported)
    }

    /// Starts the first text-only turn directly on the exact empty thread
    /// created and durably bound for this run.
    ///
    /// Implementations must not issue `thread/resume`. The supplied opened
    /// session is crate-fenced to the same run and cursor, and the process
    /// pool admits this operation only once on the exact process that created
    /// the empty thread.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive error when the exact loaded thread, cursor,
    /// frozen configuration, or turn cannot be honored.
    async fn start_bound_turn(
        &self,
        _session: crate::AiOpenedProviderSession,
        _input: AiCodexAppServerTurnInput,
    ) -> Result<ProviderEventStream, ProviderError> {
        Err(ProviderError::Unsupported)
    }

    /// Starts the first experimental dynamic-tool turn directly on the exact
    /// empty thread created and durably bound for this run.
    ///
    /// Implementations must not issue `thread/resume` and must preserve the
    /// exact frozen tool definitions installed during empty-thread creation.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive error when the exact loaded thread, cursor,
    /// frozen tool definitions, responder, or turn cannot be honored.
    async fn start_bound_dynamic_turn(
        &self,
        _session: crate::AiOpenedProviderSession,
        _input: AiCodexAppServerTurnInput,
        _responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        Err(ProviderError::Unsupported)
    }

    /// Resumes one exact opened provider-session cursor and starts a text-only
    /// turn on it.
    async fn start_retained_turn(
        &self,
        _session: crate::AiOpenedProviderSession,
        _input: AiCodexAppServerTurnInput,
    ) -> Result<ProviderEventStream, ProviderError> {
        Err(ProviderError::Unsupported)
    }

    /// Resumes one exact opened provider-session cursor and starts an
    /// experimental dynamic-tool turn on it.
    async fn start_retained_dynamic_turn(
        &self,
        _session: crate::AiOpenedProviderSession,
        _input: AiCodexAppServerTurnInput,
        _responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        Err(ProviderError::Unsupported)
    }

    /// Deletes or authoritatively confirms absence of one exact provider
    /// thread cursor.
    async fn delete_thread(
        &self,
        _cursor: &crate::AiProviderSessionCursor,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported)
    }

    /// Interrupts the exact active turn.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive error when interruption cannot be requested or
    /// confirmed within the process boundary.
    async fn interrupt(&self) -> Result<(), ProviderError>;

    /// Attempts bounded graceful shutdown of the complete process tree.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive error when shutdown cannot be confirmed. Drop
    /// of the crate-owned launched-process wrapper still synchronously invokes
    /// forced process-tree termination.
    async fn shutdown(&self, reason: AiProviderRunCloseReason) -> Result<(), ProviderError>;
}

/// Crate-owned launched-process handle with mandatory synchronous drop-kill.
///
/// The callback must own an exact operating-system process-tree handle rather
/// than an unresolved PID and must be idempotent after graceful shutdown. The
/// wrapper invokes it on every final drop, including unwind, task abortion,
/// stream abandonment, startup failure, and shutdown timeout.
pub struct AiCodexAppServerLaunchedProcess {
    process: Arc<dyn AiCodexAppServerRunProcess>,
    kill_on_drop: Arc<dyn Fn() + Send + Sync>,
    kill_requested: AtomicBool,
}

impl std::fmt::Debug for AiCodexAppServerLaunchedProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiCodexAppServerLaunchedProcess")
            .field("process", &"<sandboxed-process-tree>")
            .finish_non_exhaustive()
    }
}

impl AiCodexAppServerLaunchedProcess {
    /// Wraps one process actor and its mandatory synchronous tree-kill action.
    ///
    /// The factory remains responsible for constructing the callback from an
    /// exact race-safe child/process-group/job-object handle and for proving
    /// that invoking it cannot target a replacement process.
    pub fn new(
        process: Arc<dyn AiCodexAppServerRunProcess>,
        kill_on_drop: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            process,
            kill_on_drop,
            kill_requested: AtomicBool::new(false),
        }
    }

    fn force_kill(&self) {
        if self
            .kill_requested
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (self.kill_on_drop)();
            }));
        }
    }

    async fn create_empty_thread(
        &self,
        model: &str,
        dynamic_tools: Vec<ModelToolDefinition>,
    ) -> Result<crate::AiProviderSessionCursor, ProviderError> {
        self.process.create_empty_thread(model, dynamic_tools).await
    }

    async fn start_fresh_turn(
        &self,
        input: AiCodexAppServerTurnInput,
    ) -> Result<ProviderEventStream, ProviderError> {
        self.process.start_fresh_turn(input).await
    }

    async fn start_dynamic_turn(
        &self,
        input: AiCodexAppServerTurnInput,
        responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        self.process.start_dynamic_turn(input, responder).await
    }

    async fn start_bound_turn(
        &self,
        session: crate::AiOpenedProviderSession,
        input: AiCodexAppServerTurnInput,
    ) -> Result<ProviderEventStream, ProviderError> {
        self.process.start_bound_turn(session, input).await
    }

    async fn start_bound_dynamic_turn(
        &self,
        session: crate::AiOpenedProviderSession,
        input: AiCodexAppServerTurnInput,
        responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        self.process
            .start_bound_dynamic_turn(session, input, responder)
            .await
    }

    async fn start_retained_turn(
        &self,
        session: crate::AiOpenedProviderSession,
        input: AiCodexAppServerTurnInput,
    ) -> Result<ProviderEventStream, ProviderError> {
        self.process.start_retained_turn(session, input).await
    }

    async fn start_retained_dynamic_turn(
        &self,
        session: crate::AiOpenedProviderSession,
        input: AiCodexAppServerTurnInput,
        responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        self.process
            .start_retained_dynamic_turn(session, input, responder)
            .await
    }

    async fn delete_thread(
        &self,
        cursor: &crate::AiProviderSessionCursor,
    ) -> Result<(), ProviderError> {
        self.process.delete_thread(cursor).await
    }

    async fn interrupt(&self) -> Result<(), ProviderError> {
        self.process.interrupt().await
    }

    async fn shutdown(&self, reason: AiProviderRunCloseReason) -> Result<(), ProviderError> {
        self.process.shutdown(reason).await
    }
}

impl Drop for AiCodexAppServerLaunchedProcess {
    fn drop(&mut self) {
        self.force_kill();
    }
}

/// Trusted deployment seam that launches one reviewed app-server process.
#[async_trait]
pub trait AiCodexAppServerRunProcessFactory: Send + Sync {
    /// Launches and initializes the exact reviewed registration.
    ///
    /// The factory must directly execute the verified image without a shell,
    /// clear inherited environment and credentials, apply the fixed external
    /// sandbox, deny unreviewed filesystem/network/child-process authority, and
    /// arrange complete process-tree kill on drop.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive error when launch, integrity verification,
    /// sandboxing, or strict protocol initialization fails.
    async fn launch(
        &self,
        registration: Arc<AiCodexAppServerRegistration>,
    ) -> Result<AiCodexAppServerLaunchedProcess, ProviderError>;
}

/// Bounded process pool retaining at most one app-server process per exact
/// claimed-run binding.
///
/// Processes are never shared across bindings. A run freezes its registration
/// identity on first use, rejects concurrent turns, and requires explicit
/// close at every coordinator terminal boundary. This pool contains no
/// durable resume cursor and never resumes a provider thread after restart.
#[derive(Clone)]
pub struct AiCodexAppServerRunPool {
    inner: Arc<RunPoolInner>,
}

impl std::fmt::Debug for AiCodexAppServerRunPool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiCodexAppServerRunPool")
            .field("limits", &self.inner.limits)
            .finish_non_exhaustive()
    }
}

struct RunPoolInner {
    factory: Arc<dyn AiCodexAppServerRunProcessFactory>,
    limits: AiCodexAppServerRunLimits,
    admission: Arc<Semaphore>,
    registration_identities: Mutex<BTreeMap<AiProviderRunBinding, String>>,
    entries: Mutex<BTreeMap<AiProviderRunBinding, Arc<RunEntry>>>,
}

struct RunEntry {
    registration_identity: String,
    process: Arc<AiCodexAppServerLaunchedProcess>,
    _admission: OwnedSemaphorePermit,
    turn_count: AtomicU32,
    turn_active: AtomicBool,
    poisoned: AtomicBool,
    empty_thread: Mutex<EmptyThreadActivation>,
}

enum EmptyThreadActivation {
    Vacant,
    Creating,
    Available {
        cursor_fingerprint: String,
        dynamic_tools: Vec<ModelToolDefinition>,
    },
    Consumed,
}

fn opened_session_matches(
    binding: AiProviderRunBinding,
    registration: &AiCodexAppServerRegistration,
    session: &crate::AiOpenedProviderSession,
) -> bool {
    session.claim().session_id() == binding.session_id()
        && session.claim().run_id() == binding.run_id()
        && session.claim().attempt_id() == binding.attempt_id()
        && session.claim().run_lease_generation() == binding.lease_generation()
        && session.claim().descriptor().provider_profile_id() == registration.provider_profile_id()
        && session.claim().descriptor().provider_model() == registration.logical_model()
        && session.claim().descriptor().registration_fingerprint() == registration.identity()
        && session.claim().descriptor().protocol_version() == registration.protocol_version()
}

struct ActiveTurnGuard {
    entry: Arc<RunEntry>,
    completed: bool,
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.entry.poisoned.store(true, Ordering::Release);
            self.entry.process.force_kill();
        }
        self.entry.turn_active.store(false, Ordering::Release);
    }
}

impl AiCodexAppServerRunPool {
    /// Creates a bounded exact-run process pool.
    pub fn new(
        factory: Arc<dyn AiCodexAppServerRunProcessFactory>,
        limits: AiCodexAppServerRunLimits,
    ) -> Self {
        Self {
            inner: Arc::new(RunPoolInner {
                factory,
                limits,
                admission: Arc::new(Semaphore::new(limits.maximum_processes)),
                registration_identities: Mutex::new(BTreeMap::new()),
                entries: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    async fn create_empty_thread(
        &self,
        binding: AiProviderRunBinding,
        registration: Arc<AiCodexAppServerRegistration>,
        dynamic_tools: Vec<ModelToolDefinition>,
    ) -> Result<crate::AiProviderSessionCursor, ProviderError> {
        if !dynamic_tools.is_empty() && !registration.experimental_dynamic_tools() {
            return Err(ProviderError::Unsupported);
        }
        for tool in &dynamic_tools {
            tool.validate()?;
        }
        let entry = self.entry(binding, registration.clone()).await?;
        if entry.turn_count.load(Ordering::Acquire) != 0 {
            return Err(ProviderError::Rejected);
        }
        if entry
            .turn_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ProviderError::Rejected);
        }
        {
            let mut activation = entry.empty_thread.lock().await;
            if !matches!(*activation, EmptyThreadActivation::Vacant) {
                entry.turn_active.store(false, Ordering::Release);
                return Err(ProviderError::Rejected);
            }
            *activation = EmptyThreadActivation::Creating;
        }
        let outcome = tokio::time::timeout(
            self.inner.limits.startup_timeout,
            entry
                .process
                .create_empty_thread(registration.logical_model(), dynamic_tools.clone()),
        )
        .await;
        entry.turn_active.store(false, Ordering::Release);
        match outcome {
            Ok(Ok(cursor)) if cursor.kind() == "codex.app_server.thread.v2" => {
                *entry.empty_thread.lock().await = EmptyThreadActivation::Available {
                    cursor_fingerprint: cursor.fingerprint(),
                    dynamic_tools,
                };
                Ok(cursor)
            }
            Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                Err(ProviderError::Rejected)
            }
        }
    }

    async fn discard_empty_thread(
        &self,
        binding: AiProviderRunBinding,
        cursor: &crate::AiProviderSessionCursor,
    ) -> Result<(), ProviderError> {
        let entry = self
            .inner
            .entries
            .lock()
            .await
            .get(&binding)
            .cloned()
            .ok_or(ProviderError::Rejected)?;
        if entry
            .turn_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ProviderError::Rejected);
        }
        let cursor_matches = {
            let activation = entry.empty_thread.lock().await;
            matches!(
                &*activation,
                EmptyThreadActivation::Available {
                    cursor_fingerprint,
                    ..
                } if cursor_fingerprint == &cursor.fingerprint()
            )
        };
        if !cursor_matches {
            entry.turn_active.store(false, Ordering::Release);
            self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                .await;
            return Err(ProviderError::Rejected);
        }
        let result = tokio::time::timeout(
            self.inner.limits.shutdown_timeout,
            entry.process.delete_thread(cursor),
        )
        .await;
        entry.turn_active.store(false, Ordering::Release);
        match result {
            Ok(Ok(())) => {
                *entry.empty_thread.lock().await = EmptyThreadActivation::Consumed;
                Ok(())
            }
            Ok(Err(error)) => {
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                Err(error)
            }
            Err(_) => {
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                Err(ProviderError::Cancelled)
            }
        }
    }

    async fn delete_detached_thread(
        &self,
        registration: Arc<AiCodexAppServerRegistration>,
        cursor: &crate::AiProviderSessionCursor,
    ) -> Result<(), ProviderError> {
        let permit = self
            .inner
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| ProviderError::RateLimited)?;
        let process = tokio::time::timeout(
            self.inner.limits.startup_timeout,
            self.inner.factory.launch(registration),
        )
        .await
        .map_err(|_| ProviderError::Cancelled)??;
        let result = tokio::time::timeout(
            self.inner.limits.shutdown_timeout,
            process.delete_thread(cursor),
        )
        .await
        .map_err(|_| ProviderError::Cancelled)?;
        let _ = tokio::time::timeout(
            self.inner.limits.shutdown_timeout,
            process.shutdown(AiProviderRunCloseReason::Completed),
        )
        .await;
        drop(process);
        drop(permit);
        result
    }

    /// Starts one fresh turn, reusing the exact run's existing process.
    ///
    /// This operation does not retain or resume a provider thread. It offers no
    /// application tools, and the returned stream remains subject to the
    /// ordinary provider executor's egress, budget, usage, and fence checks.
    ///
    /// # Errors
    ///
    /// Fails for invalid input, capacity exhaustion, a registration/model swap,
    /// a concurrent turn, the per-run turn ceiling, launch timeout, or process
    /// failure.
    pub(crate) async fn start_fresh_turn(
        &self,
        binding: AiProviderRunBinding,
        registration: Arc<AiCodexAppServerRegistration>,
        input: AiCodexAppServerTurnInput,
    ) -> Result<ProviderEventStream, ProviderError> {
        input.validate()?;
        if input.model() != registration.logical_model() {
            return Err(ProviderError::Rejected);
        }
        let entry = self.entry(binding, registration).await?;
        if entry
            .turn_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ProviderError::Rejected);
        }
        let previous = entry.turn_count.fetch_add(1, Ordering::AcqRel);
        if previous >= self.inner.limits.maximum_turns_per_run {
            entry.turn_count.fetch_sub(1, Ordering::AcqRel);
            entry.turn_active.store(false, Ordering::Release);
            return Err(ProviderError::RateLimited);
        }
        let turn_deadline = tokio::time::Instant::now() + self.inner.limits.turn_timeout;
        let stream =
            match tokio::time::timeout_at(turn_deadline, entry.process.start_fresh_turn(input))
                .await
            {
                Ok(Ok(stream)) => stream,
                Ok(Err(error)) => {
                    entry.turn_active.store(false, Ordering::Release);
                    self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                        .await;
                    return Err(error);
                }
                Err(_) => {
                    entry.turn_active.store(false, Ordering::Release);
                    self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                        .await;
                    return Err(ProviderError::Cancelled);
                }
            };
        let guard = ActiveTurnGuard {
            entry,
            completed: false,
        };
        let pool = self.clone();
        Ok(Box::pin(async_stream::try_stream! {
            let mut guard = guard;
            let mut stream = stream;
            let turn_timeout = tokio::time::sleep_until(turn_deadline);
            tokio::pin!(turn_timeout);
            loop {
                let next = tokio::select! {
                    _ = &mut turn_timeout => {
                        pool.invalidate(
                            binding,
                            &guard.entry,
                            AiProviderRunCloseReason::ProtocolViolation,
                        ).await;
                        guard.completed = true;
                        Err(ProviderError::Cancelled)
                    }
                    event = stream.next() => match event {
                        Some(Ok(event)) => Ok(Some(event)),
                        Some(Err(error)) => {
                            pool.invalidate(
                                binding,
                                &guard.entry,
                                AiProviderRunCloseReason::ProtocolViolation,
                            ).await;
                            guard.completed = true;
                            Err(error)
                        }
                        None => Ok(None),
                    }
                };
                match next? {
                    Some(event) => yield event,
                    None => {
                        guard.completed = true;
                        break;
                    }
                }
            }
        }))
    }

    /// Starts the first tool-free turn directly on the exact newly bound empty
    /// thread without issuing `thread/resume`.
    pub(crate) async fn start_bound_turn(
        &self,
        binding: AiProviderRunBinding,
        registration: Arc<AiCodexAppServerRegistration>,
        session: crate::AiOpenedProviderSession,
        input: AiCodexAppServerTurnInput,
    ) -> Result<ProviderEventStream, ProviderError> {
        input.validate()?;
        if !input.tools().is_empty() {
            return Err(ProviderError::Rejected);
        }
        let entry = self
            .begin_bound_turn(binding, &registration, &session, &input)
            .await?;
        let turn_deadline = tokio::time::Instant::now() + self.inner.limits.turn_timeout;
        let stream = match tokio::time::timeout_at(
            turn_deadline,
            entry.process.start_bound_turn(session, input),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => {
                entry.turn_active.store(false, Ordering::Release);
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                return Err(error);
            }
            Err(_) => {
                entry.turn_active.store(false, Ordering::Release);
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                return Err(ProviderError::Cancelled);
            }
        };
        self.guard_turn_stream(binding, entry, turn_deadline, stream)
    }

    /// Starts the first experimental dynamic-tool turn directly on the exact
    /// newly bound empty thread without issuing `thread/resume`.
    pub(crate) async fn start_bound_dynamic_turn(
        &self,
        binding: AiProviderRunBinding,
        registration: Arc<AiCodexAppServerRegistration>,
        session: crate::AiOpenedProviderSession,
        input: AiCodexAppServerTurnInput,
        responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        input.validate()?;
        if input.tools().is_empty() || !registration.experimental_dynamic_tools() {
            return Err(ProviderError::Unsupported);
        }
        let entry = self
            .begin_bound_turn(binding, &registration, &session, &input)
            .await?;
        let turn_deadline = tokio::time::Instant::now() + self.inner.limits.turn_timeout;
        let stream = match tokio::time::timeout_at(
            turn_deadline,
            entry
                .process
                .start_bound_dynamic_turn(session, input, responder),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => {
                entry.turn_active.store(false, Ordering::Release);
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                return Err(error);
            }
            Err(_) => {
                entry.turn_active.store(false, Ordering::Release);
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                return Err(ProviderError::Cancelled);
            }
        };
        self.guard_turn_stream(binding, entry, turn_deadline, stream)
    }

    async fn begin_bound_turn(
        &self,
        binding: AiProviderRunBinding,
        registration: &AiCodexAppServerRegistration,
        session: &crate::AiOpenedProviderSession,
        input: &AiCodexAppServerTurnInput,
    ) -> Result<Arc<RunEntry>, ProviderError> {
        if session.activation() != crate::AiProviderSessionActivation::NewlyBoundEmpty
            || !opened_session_matches(binding, registration, session)
            || input.model() != registration.logical_model()
        {
            return Err(ProviderError::Rejected);
        }
        let entry = self
            .inner
            .entries
            .lock()
            .await
            .get(&binding)
            .cloned()
            .filter(|entry| {
                entry.registration_identity == registration.identity()
                    && !entry.poisoned.load(Ordering::Acquire)
            })
            .ok_or(ProviderError::Rejected)?;
        if entry
            .turn_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ProviderError::Rejected);
        }
        let activation_matches = {
            let mut activation = entry.empty_thread.lock().await;
            match &*activation {
                EmptyThreadActivation::Available {
                    cursor_fingerprint,
                    dynamic_tools,
                } if cursor_fingerprint == &session.cursor().fingerprint()
                    && dynamic_tools == input.tools() =>
                {
                    *activation = EmptyThreadActivation::Consumed;
                    true
                }
                EmptyThreadActivation::Vacant
                | EmptyThreadActivation::Creating
                | EmptyThreadActivation::Available { .. }
                | EmptyThreadActivation::Consumed => false,
            }
        };
        if !activation_matches {
            entry.turn_active.store(false, Ordering::Release);
            self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                .await;
            return Err(ProviderError::Rejected);
        }
        let previous = entry.turn_count.fetch_add(1, Ordering::AcqRel);
        if previous >= self.inner.limits.maximum_turns_per_run {
            entry.turn_count.fetch_sub(1, Ordering::AcqRel);
            entry.turn_active.store(false, Ordering::Release);
            self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                .await;
            return Err(ProviderError::RateLimited);
        }
        Ok(entry)
    }

    fn guard_turn_stream(
        &self,
        binding: AiProviderRunBinding,
        entry: Arc<RunEntry>,
        turn_deadline: tokio::time::Instant,
        stream: ProviderEventStream,
    ) -> Result<ProviderEventStream, ProviderError> {
        let guard = ActiveTurnGuard {
            entry,
            completed: false,
        };
        let pool = self.clone();
        Ok(Box::pin(async_stream::try_stream! {
            let mut guard = guard;
            let mut stream = stream;
            let turn_timeout = tokio::time::sleep_until(turn_deadline);
            tokio::pin!(turn_timeout);
            loop {
                let next = tokio::select! {
                    _ = &mut turn_timeout => {
                        pool.invalidate(
                            binding,
                            &guard.entry,
                            AiProviderRunCloseReason::ProtocolViolation,
                        ).await;
                        guard.completed = true;
                        Err(ProviderError::Cancelled)
                    }
                    event = stream.next() => match event {
                        Some(Ok(event)) => Ok(Some(event)),
                        Some(Err(error)) => {
                            pool.invalidate(
                                binding,
                                &guard.entry,
                                AiProviderRunCloseReason::ProtocolViolation,
                            ).await;
                            guard.completed = true;
                            Err(error)
                        }
                        None => Ok(None),
                    }
                };
                match next? {
                    Some(event) => yield event,
                    None => {
                        guard.completed = true;
                        break;
                    }
                }
            }
        }))
    }

    pub(crate) async fn start_retained_turn(
        &self,
        binding: AiProviderRunBinding,
        registration: Arc<AiCodexAppServerRegistration>,
        session: crate::AiOpenedProviderSession,
        input: AiCodexAppServerTurnInput,
    ) -> Result<ProviderEventStream, ProviderError> {
        input.validate()?;
        if input.model() != registration.logical_model()
            || session.activation() != crate::AiProviderSessionActivation::ExistingRetained
            || !opened_session_matches(binding, &registration, &session)
        {
            return Err(ProviderError::Rejected);
        }
        let entry = self.entry(binding, registration).await?;
        if entry
            .turn_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ProviderError::Rejected);
        }
        let previous = entry.turn_count.fetch_add(1, Ordering::AcqRel);
        if previous >= self.inner.limits.maximum_turns_per_run {
            entry.turn_count.fetch_sub(1, Ordering::AcqRel);
            entry.turn_active.store(false, Ordering::Release);
            return Err(ProviderError::RateLimited);
        }
        let turn_deadline = tokio::time::Instant::now() + self.inner.limits.turn_timeout;
        let stream = match tokio::time::timeout_at(
            turn_deadline,
            entry.process.start_retained_turn(session, input),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => {
                entry.turn_active.store(false, Ordering::Release);
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                return Err(error);
            }
            Err(_) => {
                entry.turn_active.store(false, Ordering::Release);
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                return Err(ProviderError::Cancelled);
            }
        };
        let guard = ActiveTurnGuard {
            entry,
            completed: false,
        };
        let pool = self.clone();
        Ok(Box::pin(async_stream::try_stream! {
            let mut guard = guard;
            let mut stream = stream;
            let turn_timeout = tokio::time::sleep_until(turn_deadline);
            tokio::pin!(turn_timeout);
            loop {
                let next = tokio::select! {
                    _ = &mut turn_timeout => {
                        pool.invalidate(
                            binding,
                            &guard.entry,
                            AiProviderRunCloseReason::ProtocolViolation,
                        ).await;
                        guard.completed = true;
                        Err(ProviderError::Cancelled)
                    }
                    event = stream.next() => match event {
                        Some(Ok(event)) => Ok(Some(event)),
                        Some(Err(error)) => {
                            pool.invalidate(
                                binding,
                                &guard.entry,
                                AiProviderRunCloseReason::ProtocolViolation,
                            ).await;
                            guard.completed = true;
                            Err(error)
                        }
                        None => Ok(None),
                    }
                };
                match next? {
                    Some(event) => yield event,
                    None => {
                        guard.completed = true;
                        break;
                    }
                }
            }
        }))
    }

    /// Starts one experimental dynamic-tool turn on the exact run process.
    ///
    /// The responder is coordinator-owned and is the only path by which a
    /// provider request can receive application data. Capacity, registration,
    /// concurrency, timeout, poisoning, and kill behavior are identical to a
    /// tool-free turn.
    pub(crate) async fn start_dynamic_turn(
        &self,
        binding: AiProviderRunBinding,
        registration: Arc<AiCodexAppServerRegistration>,
        input: AiCodexAppServerTurnInput,
        responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        input.validate()?;
        if input.model() != registration.logical_model()
            || input.tools().is_empty()
            || !registration.experimental_dynamic_tools()
        {
            return Err(ProviderError::Unsupported);
        }
        let entry = self.entry(binding, registration).await?;
        if entry
            .turn_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ProviderError::Rejected);
        }
        let previous = entry.turn_count.fetch_add(1, Ordering::AcqRel);
        if previous >= self.inner.limits.maximum_turns_per_run {
            entry.turn_count.fetch_sub(1, Ordering::AcqRel);
            entry.turn_active.store(false, Ordering::Release);
            return Err(ProviderError::RateLimited);
        }
        let turn_deadline = tokio::time::Instant::now() + self.inner.limits.turn_timeout;
        let stream = match tokio::time::timeout_at(
            turn_deadline,
            entry.process.start_dynamic_turn(input, responder),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => {
                entry.turn_active.store(false, Ordering::Release);
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                return Err(error);
            }
            Err(_) => {
                entry.turn_active.store(false, Ordering::Release);
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                return Err(ProviderError::Cancelled);
            }
        };
        let guard = ActiveTurnGuard {
            entry,
            completed: false,
        };
        let pool = self.clone();
        Ok(Box::pin(async_stream::try_stream! {
            let mut guard = guard;
            let mut stream = stream;
            let turn_timeout = tokio::time::sleep_until(turn_deadline);
            tokio::pin!(turn_timeout);
            loop {
                let next = tokio::select! {
                    _ = &mut turn_timeout => {
                        pool.invalidate(
                            binding,
                            &guard.entry,
                            AiProviderRunCloseReason::ProtocolViolation,
                        ).await;
                        guard.completed = true;
                        Err(ProviderError::Cancelled)
                    }
                    event = stream.next() => match event {
                        Some(Ok(event)) => Ok(Some(event)),
                        Some(Err(error)) => {
                            pool.invalidate(
                                binding,
                                &guard.entry,
                                AiProviderRunCloseReason::ProtocolViolation,
                            ).await;
                            guard.completed = true;
                            Err(error)
                        }
                        None => Ok(None),
                    }
                };
                match next? {
                    Some(event) => yield event,
                    None => {
                        guard.completed = true;
                        break;
                    }
                }
            }
        }))
    }

    pub(crate) async fn start_retained_dynamic_turn(
        &self,
        binding: AiProviderRunBinding,
        registration: Arc<AiCodexAppServerRegistration>,
        session: crate::AiOpenedProviderSession,
        input: AiCodexAppServerTurnInput,
        responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        input.validate()?;
        if input.model() != registration.logical_model()
            || input.tools().is_empty()
            || !registration.experimental_dynamic_tools()
            || session.activation() != crate::AiProviderSessionActivation::ExistingRetained
            || !opened_session_matches(binding, &registration, &session)
        {
            return Err(ProviderError::Rejected);
        }
        let entry = self.entry(binding, registration).await?;
        if entry
            .turn_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ProviderError::Rejected);
        }
        let previous = entry.turn_count.fetch_add(1, Ordering::AcqRel);
        if previous >= self.inner.limits.maximum_turns_per_run {
            entry.turn_count.fetch_sub(1, Ordering::AcqRel);
            entry.turn_active.store(false, Ordering::Release);
            return Err(ProviderError::RateLimited);
        }
        let turn_deadline = tokio::time::Instant::now() + self.inner.limits.turn_timeout;
        let stream = match tokio::time::timeout_at(
            turn_deadline,
            entry
                .process
                .start_retained_dynamic_turn(session, input, responder),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => {
                entry.turn_active.store(false, Ordering::Release);
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                return Err(error);
            }
            Err(_) => {
                entry.turn_active.store(false, Ordering::Release);
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                return Err(ProviderError::Cancelled);
            }
        };
        let guard = ActiveTurnGuard {
            entry,
            completed: false,
        };
        let pool = self.clone();
        Ok(Box::pin(async_stream::try_stream! {
            let mut guard = guard;
            let mut stream = stream;
            let turn_timeout = tokio::time::sleep_until(turn_deadline);
            tokio::pin!(turn_timeout);
            loop {
                let next = tokio::select! {
                    _ = &mut turn_timeout => {
                        pool.invalidate(
                            binding,
                            &guard.entry,
                            AiProviderRunCloseReason::ProtocolViolation,
                        ).await;
                        guard.completed = true;
                        Err(ProviderError::Cancelled)
                    }
                    event = stream.next() => match event {
                        Some(Ok(event)) => Ok(Some(event)),
                        Some(Err(error)) => {
                            pool.invalidate(
                                binding,
                                &guard.entry,
                                AiProviderRunCloseReason::ProtocolViolation,
                            ).await;
                            guard.completed = true;
                            Err(error)
                        }
                        None => Ok(None),
                    }
                };
                match next? {
                    Some(event) => yield event,
                    None => {
                        guard.completed = true;
                        break;
                    }
                }
            }
        }))
    }

    async fn entry(
        &self,
        binding: AiProviderRunBinding,
        registration: Arc<AiCodexAppServerRegistration>,
    ) -> Result<Arc<RunEntry>, ProviderError> {
        let mut identities = self.inner.registration_identities.lock().await;
        let identity_is_new = if let Some(identity) = identities.get(&binding) {
            if identity != registration.identity() {
                return Err(ProviderError::Rejected);
            }
            false
        } else {
            true
        };
        let mut entries = self.inner.entries.lock().await;
        if entries
            .get(&binding)
            .is_some_and(|entry| entry.poisoned.load(Ordering::Acquire))
        {
            entries.remove(&binding);
        }
        if let Some(entry) = entries.get(&binding) {
            debug_assert_eq!(entry.registration_identity, registration.identity());
            return Ok(entry.clone());
        }
        let owner_fingerprint = binding.owner_fingerprint();
        if entries
            .keys()
            .filter(|candidate| candidate.owner_fingerprint() == owner_fingerprint)
            .count()
            >= self.inner.limits.maximum_processes_per_owner
        {
            return Err(ProviderError::RateLimited);
        }
        let admission = self
            .inner
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| ProviderError::RateLimited)?;
        let process = tokio::time::timeout(
            self.inner.limits.startup_timeout,
            self.inner.factory.launch(registration.clone()),
        )
        .await
        .map_err(|_| ProviderError::Unavailable)??;
        let entry = Arc::new(RunEntry {
            registration_identity: registration.identity().to_owned(),
            process: Arc::new(process),
            _admission: admission,
            turn_count: AtomicU32::new(0),
            turn_active: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            empty_thread: Mutex::new(EmptyThreadActivation::Vacant),
        });
        if identity_is_new {
            identities.insert(binding, registration.identity().to_owned());
        }
        entries.insert(binding, entry.clone());
        Ok(entry)
    }

    async fn remove(&self, binding: &AiProviderRunBinding) -> Option<Arc<RunEntry>> {
        self.inner.entries.lock().await.remove(binding)
    }

    async fn invalidate(
        &self,
        binding: AiProviderRunBinding,
        expected: &Arc<RunEntry>,
        reason: AiProviderRunCloseReason,
    ) {
        let removed = {
            let mut entries = self.inner.entries.lock().await;
            if entries
                .get(&binding)
                .is_some_and(|entry| Arc::ptr_eq(entry, expected))
            {
                entries.remove(&binding)
            } else {
                None
            }
        };
        if let Some(entry) = removed {
            let _ = tokio::time::timeout(
                self.inner.limits.shutdown_timeout,
                entry.process.shutdown(reason),
            )
            .await;
        }
    }

    /// Interrupts the active turn for one exact fenced binding.
    ///
    /// This method contains no cancellation authority. Callers must first
    /// observe authoritative durable cancellation or lease loss.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive provider error when bounded interruption cannot
    /// be requested or confirmed.
    pub async fn interrupt_run(
        &self,
        binding: &AiProviderRunBinding,
    ) -> Result<AiProviderRunInterruptOutcome, ProviderError> {
        let entry = self.inner.entries.lock().await.get(binding).cloned();
        let Some(entry) = entry else {
            return Ok(AiProviderRunInterruptOutcome::NotActive);
        };
        if !entry.turn_active.load(Ordering::Acquire) {
            return Ok(AiProviderRunInterruptOutcome::NotActive);
        }
        tokio::time::timeout(
            self.inner.limits.interrupt_timeout,
            entry.process.interrupt(),
        )
        .await
        .map_err(|_| ProviderError::Cancelled)??;
        Ok(AiProviderRunInterruptOutcome::Requested)
    }

    /// Removes and shuts down one exact run-scoped process.
    ///
    /// The reason is diagnostic lifecycle metadata and cannot alter durable
    /// run state. The process implementation's synchronous drop-kill remains
    /// the final cleanup fallback.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive provider error when bounded shutdown cannot be
    /// confirmed.
    pub async fn close_run(
        &self,
        binding: &AiProviderRunBinding,
        reason: AiProviderRunCloseReason,
    ) -> Result<AiProviderRunCloseOutcome, ProviderError> {
        self.inner
            .registration_identities
            .lock()
            .await
            .remove(binding);
        let Some(entry) = self.remove(binding).await else {
            return Ok(AiProviderRunCloseOutcome::NotActive);
        };
        tokio::time::timeout(
            self.inner.limits.shutdown_timeout,
            entry.process.shutdown(reason),
        )
        .await
        .map_err(|_| ProviderError::Cancelled)??;
        Ok(AiProviderRunCloseOutcome::Closed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClientMethod {
    Initialize,
    ThreadStart,
    ThreadResume,
    ThreadDelete,
    TurnStart,
    TurnInterrupt,
}

/// One strictly admitted inbound Codex app-server envelope.
///
/// The actor admits only responses correlated to one of its closed outbound
/// methods and the exact notification allowlist documented by
/// [`AiCodexAppServerProtocolActor`]. Provider-specific payload normalization
/// remains the process implementation's responsibility.
#[derive(PartialEq)]
#[non_exhaustive]
pub enum AiCodexAppServerInbound {
    /// Successful response to one exact pending client request.
    Response {
        /// Exact JSON-RPC response ID.
        id: u64,
        /// Closed client method name associated with the response ID.
        method: &'static str,
        /// Bounded object result after envelope validation.
        result: Value,
    },
    /// Exact allowlisted server notification.
    Notification {
        /// Allowlisted notification method.
        method: String,
        /// Bounded object parameters after notification validation.
        params: Value,
    },
    /// Content-free notice that app-server reported remote control disabled
    /// during protocol initialization.
    ///
    /// The installation, server, environment, and timestamp fields are
    /// validated but deliberately not exposed through this boundary. This is
    /// protocol compatibility evidence only and grants no remote-control
    /// method or capability.
    RemoteControlDisabled,
    /// Exact experimental dynamic-tool server request matched to one offered
    /// definition. No other server request is admitted.
    DynamicToolCall {
        /// JSON-RPC request identifier to use only for the matching response.
        request_id: u64,
        /// Exact provider thread reference.
        thread_id: String,
        /// Exact active provider turn reference.
        turn_id: String,
        /// Schema-validated application-tool request.
        call: ProviderDynamicToolCall,
    },
    /// Content-free lifecycle for one exact experimental dynamic-tool item.
    DynamicToolLifecycle {
        /// Exact provider turn reference.
        turn_id: String,
        /// Opaque dynamic-tool call/item identifier.
        call_id: String,
        /// Exact offered provider-facing tool name.
        provider_name: String,
        /// Whether this is the pre-execution start or terminal completion.
        completed: bool,
    },
}

impl std::fmt::Debug for AiCodexAppServerInbound {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Response { id, method, .. } => formatter
                .debug_struct("AiCodexAppServerInbound::Response")
                .field("id", id)
                .field("method", method)
                .field("result", &"<provider-content>")
                .finish(),
            Self::Notification { method, .. } => formatter
                .debug_struct("AiCodexAppServerInbound::Notification")
                .field("method", method)
                .field("params", &"<provider-content>")
                .finish(),
            Self::RemoteControlDisabled => {
                formatter.write_str("AiCodexAppServerInbound::RemoteControlDisabled")
            }
            Self::DynamicToolCall {
                request_id,
                thread_id,
                turn_id,
                call,
            } => formatter
                .debug_struct("AiCodexAppServerInbound::DynamicToolCall")
                .field("request_id", request_id)
                .field("thread_id", thread_id)
                .field("turn_id", turn_id)
                .field("call", call)
                .finish(),
            Self::DynamicToolLifecycle {
                turn_id,
                call_id,
                provider_name,
                completed,
            } => formatter
                .debug_struct("AiCodexAppServerInbound::DynamicToolLifecycle")
                .field("turn_id", turn_id)
                .field("call_id", call_id)
                .field("provider_name", provider_name)
                .field("completed", completed)
                .finish(),
        }
    }
}

/// Internal response/notification observation phase for one thread lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThreadLifecyclePhase {
    Ready,
    AwaitingResponseAndStarted,
    AwaitingResponse,
    AwaitingStarted,
    Complete,
    Deleted,
}

/// Closed app-server JSON-RPC encoder/guard.
///
/// There is intentionally no generic request builder. The provider-specific
/// process actor may emit only the explicitly typed initialization, thread,
/// turn, interruption, deletion, and dynamic-tool response methods represented
/// here. Admitted server notifications require the complete positive signed
/// `emittedAtMs` envelope and exact lifecycle correlation. The only admitted
/// thread-status transition is `notLoaded` for the exact thread already being
/// deleted. All other server-initiated requests and non-allowlisted
/// notifications fail closed.
#[derive(Debug)]
pub struct AiCodexAppServerProtocolActor {
    next_id: u64,
    pending: BTreeMap<u64, ClientMethod>,
    active_thread_id: Option<String>,
    pending_turn_thread_id: Option<String>,
    active_turn_id: Option<String>,
    retained_model: Option<String>,
    dynamic_tools: BTreeMap<String, ModelToolDefinition>,
    pending_dynamic_requests: BTreeMap<u64, (String, String)>,
    started_dynamic_calls: BTreeMap<String, String>,
    responded_dynamic_calls: BTreeMap<String, String>,
    started_items: BTreeMap<String, String>,
    completed_items: BTreeSet<String>,
    initialization_complete: bool,
    thread_lifecycle_phase: ThreadLifecyclePhase,
    deleting_thread_id: Option<String>,
    thread_not_loaded_observed: bool,
    turn_response_observed: bool,
    turn_started_observed: bool,
    remote_control_disabled_observed: bool,
    maximum_frame_bytes: usize,
}

impl AiCodexAppServerProtocolActor {
    /// Creates a closed protocol actor with an exact inbound/outbound frame
    /// ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidConfiguration`] unless the ceiling is
    /// in `1..=16 MiB`.
    pub fn new(maximum_frame_bytes: usize) -> Result<Self, ProviderError> {
        if !(1..=MAXIMUM_FRAME_BYTES).contains(&maximum_frame_bytes) {
            return Err(ProviderError::InvalidConfiguration(
                "invalid Codex app-server frame limit".to_owned(),
            ));
        }
        Ok(Self {
            next_id: 1,
            pending: BTreeMap::new(),
            active_thread_id: None,
            pending_turn_thread_id: None,
            active_turn_id: None,
            retained_model: None,
            dynamic_tools: BTreeMap::new(),
            pending_dynamic_requests: BTreeMap::new(),
            started_dynamic_calls: BTreeMap::new(),
            responded_dynamic_calls: BTreeMap::new(),
            started_items: BTreeMap::new(),
            completed_items: BTreeSet::new(),
            initialization_complete: false,
            thread_lifecycle_phase: ThreadLifecyclePhase::Ready,
            deleting_thread_id: None,
            thread_not_loaded_observed: false,
            turn_response_observed: false,
            turn_started_observed: false,
            remote_control_disabled_observed: false,
            maximum_frame_bytes,
        })
    }

    /// Encodes the one allowed protocol-initialization request.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive error for invalid client identity, ID
    /// exhaustion, or frame-size overflow.
    pub fn initialize(
        &mut self,
        client_name: &str,
        client_title: &str,
        client_version: &str,
    ) -> Result<Vec<u8>, ProviderError> {
        if !valid_identifier(client_name)
            || client_title.trim().is_empty()
            || client_title.len() > MAXIMUM_VERSION_BYTES
            || !valid_version(client_version)
        {
            return Err(ProviderError::InvalidConfiguration(
                "invalid Codex app-server client identity".to_owned(),
            ));
        }
        self.request(
            ClientMethod::Initialize,
            "initialize",
            json!({
                "clientInfo": {
                    "name": client_name,
                    "title": client_title,
                    "version": client_version,
                }
            }),
        )
    }

    /// Encodes initialization with only the experimental API capability
    /// required by app-server dynamic tools.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive error for invalid client identity, ID
    /// exhaustion, or frame-size overflow.
    pub fn initialize_with_dynamic_tools(
        &mut self,
        client_name: &str,
        client_title: &str,
        client_version: &str,
    ) -> Result<Vec<u8>, ProviderError> {
        if !valid_identifier(client_name)
            || client_title.trim().is_empty()
            || client_title.len() > MAXIMUM_VERSION_BYTES
            || !valid_version(client_version)
        {
            return Err(ProviderError::InvalidConfiguration(
                "invalid Codex app-server client identity".to_owned(),
            ));
        }
        self.request(
            ClientMethod::Initialize,
            "initialize",
            json!({
                "clientInfo": {
                    "name": client_name,
                    "title": client_title,
                    "version": client_version,
                },
                "capabilities": {"experimentalApi": true},
            }),
        )
    }

    /// Encodes the one allowed post-initialization notification.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidRequest`] when the configured frame
    /// ceiling cannot contain the notification.
    pub fn initialized(&self) -> Result<Vec<u8>, ProviderError> {
        self.encode(json!({"method": "initialized", "params": {}}))
    }

    fn validate_thread_lifecycle_boundary(&self) -> Result<(), ProviderError> {
        if !self.initialization_complete
            || !self.pending.is_empty()
            || !matches!(
                self.thread_lifecycle_phase,
                ThreadLifecyclePhase::Ready | ThreadLifecyclePhase::Complete
            )
            || self.pending_turn_thread_id.is_some()
            || self.active_turn_id.is_some()
            || self.deleting_thread_id.is_some()
            || self.turn_response_observed
            || self.turn_started_observed
            || !self.pending_dynamic_requests.is_empty()
            || !self.started_dynamic_calls.is_empty()
            || !self.responded_dynamic_calls.is_empty()
            || !self.started_items.is_empty()
            || !self.completed_items.is_empty()
        {
            return Err(ProviderError::Rejected);
        }
        Ok(())
    }

    fn begin_new_thread_lifecycle(&mut self) {
        self.active_thread_id = None;
        self.thread_lifecycle_phase = ThreadLifecyclePhase::AwaitingResponseAndStarted;
    }

    fn begin_resume_lifecycle(&mut self, thread_id: &str) {
        self.active_thread_id = Some(thread_id.to_owned());
        self.thread_lifecycle_phase = ThreadLifecyclePhase::AwaitingResponseAndStarted;
    }

    /// Encodes an ephemeral thread start with trusted instructions kept in the
    /// protocol's developer-instruction field.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive error for invalid input, ID exhaustion, or
    /// frame-size overflow.
    pub fn start_fresh_thread(
        &mut self,
        input: &AiCodexAppServerTurnInput,
    ) -> Result<Vec<u8>, ProviderError> {
        input.validate()?;
        self.validate_thread_lifecycle_boundary()?;
        if self.retained_model.is_some()
            || !self.dynamic_tools.is_empty()
            || !self.pending_dynamic_requests.is_empty()
            || !self.started_dynamic_calls.is_empty()
            || !self.responded_dynamic_calls.is_empty()
        {
            return Err(ProviderError::Rejected);
        }
        let developer_instructions = if input.instructions().is_empty() {
            Value::Null
        } else {
            Value::String(input.instructions().join("\n\n"))
        };
        let frame = self.request(
            ClientMethod::ThreadStart,
            "thread/start",
            json!({
                "model": input.model(),
                "developerInstructions": developer_instructions,
                "ephemeral": true,
                "approvalPolicy": "never",
                "sandbox": "read-only",
            }),
        )?;
        self.begin_new_thread_lifecycle();
        Ok(frame)
    }

    /// Creates a durable empty thread before any business content is sent.
    ///
    /// The caller must durably protect and bind the returned thread cursor
    /// before calling [`Self::start_turn`]. Reviewed dynamic-tool definitions
    /// may be installed because app-server cannot add them at resume time, but
    /// no developer or user instructions are included in this request.
    pub fn start_persistent_empty_thread(
        &mut self,
        model: &str,
        dynamic_tools: &[ModelToolDefinition],
    ) -> Result<Vec<u8>, ProviderError> {
        self.validate_thread_lifecycle_boundary()?;
        if !valid_identifier(model)
            || self.thread_lifecycle_phase != ThreadLifecyclePhase::Ready
            || self.active_thread_id.is_some()
            || self.retained_model.is_some()
            || !self.dynamic_tools.is_empty()
            || !self.pending_dynamic_requests.is_empty()
            || !self.started_dynamic_calls.is_empty()
            || !self.responded_dynamic_calls.is_empty()
        {
            return Err(ProviderError::Rejected);
        }
        let mut definitions = BTreeMap::new();
        let dynamic_tools = dynamic_tools
            .iter()
            .map(|tool| {
                tool.validate()?;
                if definitions
                    .insert(tool.provider_name.clone(), tool.clone())
                    .is_some()
                {
                    return Err(ProviderError::Rejected);
                }
                Ok(json!({
                    "type": "function",
                    "name": tool.provider_name,
                    "description": tool.description,
                    "inputSchema": tool.parameters,
                    "deferLoading": false,
                }))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut params = json!({
            "model": model,
            "developerInstructions": null,
            "ephemeral": false,
            "approvalPolicy": "never",
            "sandbox": "read-only",
        });
        if !dynamic_tools.is_empty() {
            params
                .as_object_mut()
                .ok_or(ProviderError::Rejected)?
                .insert("dynamicTools".to_owned(), Value::Array(dynamic_tools));
        }
        let frame = self.request(ClientMethod::ThreadStart, "thread/start", params)?;
        self.begin_new_thread_lifecycle();
        self.retained_model = Some(model.to_owned());
        self.dynamic_tools = definitions;
        Ok(frame)
    }

    /// Resumes one exact protected thread cursor with immutable model and
    /// trusted instruction overrides.
    ///
    /// Dynamic definitions are installed only in the local protocol guard so
    /// later server requests can be matched to the binding's already-frozen
    /// tool/policy fingerprint. The current app-server resume method has no
    /// dynamic-tools field and therefore cannot change the provider-retained
    /// definition set.
    pub fn resume_thread(
        &mut self,
        cursor: &crate::AiProviderSessionCursor,
        input: &AiCodexAppServerTurnInput,
    ) -> Result<Vec<u8>, ProviderError> {
        input.validate()?;
        self.validate_thread_lifecycle_boundary()?;
        if cursor.kind() != "codex.app_server.thread.v2"
            || !valid_reference(cursor.expose_to_provider_adapter())
            || match self.thread_lifecycle_phase {
                ThreadLifecyclePhase::Ready => self.active_thread_id.is_some(),
                ThreadLifecyclePhase::Complete => {
                    self.retained_model.is_none()
                        || self.active_thread_id.as_deref()
                            != Some(cursor.expose_to_provider_adapter())
                }
                _ => true,
            }
            || self
                .retained_model
                .as_deref()
                .is_some_and(|model| model != input.model())
            || !self.pending_dynamic_requests.is_empty()
            || !self.started_dynamic_calls.is_empty()
            || !self.responded_dynamic_calls.is_empty()
        {
            return Err(ProviderError::Rejected);
        }
        let mut definitions = BTreeMap::new();
        for tool in input.tools() {
            if definitions
                .insert(tool.provider_name.clone(), tool.clone())
                .is_some()
            {
                return Err(ProviderError::Rejected);
            }
        }
        let developer_instructions = if input.instructions().is_empty() {
            Value::Null
        } else {
            Value::String(input.instructions().join("\n\n"))
        };
        if self.retained_model.is_some() && self.dynamic_tools != definitions {
            return Err(ProviderError::Rejected);
        }
        let frame = self.request(
            ClientMethod::ThreadResume,
            "thread/resume",
            json!({
                "threadId": cursor.expose_to_provider_adapter(),
                "model": input.model(),
                "developerInstructions": developer_instructions,
                "approvalPolicy": "never",
                "sandbox": "read-only",
            }),
        )?;
        self.begin_resume_lifecycle(cursor.expose_to_provider_adapter());
        self.retained_model = Some(input.model().to_owned());
        self.dynamic_tools = definitions;
        Ok(frame)
    }

    /// Deletes one exact protected thread cursor.
    pub fn delete_thread(
        &mut self,
        cursor: &crate::AiProviderSessionCursor,
    ) -> Result<Vec<u8>, ProviderError> {
        if cursor.kind() != "codex.app_server.thread.v2"
            || !valid_reference(cursor.expose_to_provider_adapter())
            || self.thread_lifecycle_phase != ThreadLifecyclePhase::Complete
            || self.pending_turn_thread_id.is_some()
            || self.active_turn_id.is_some()
            || self.deleting_thread_id.is_some()
            || self
                .active_thread_id
                .as_deref()
                .is_some_and(|thread_id| thread_id != cursor.expose_to_provider_adapter())
        {
            return Err(ProviderError::Rejected);
        }
        let frame = self.request(
            ClientMethod::ThreadDelete,
            "thread/delete",
            json!({"threadId": cursor.expose_to_provider_adapter()}),
        )?;
        self.deleting_thread_id = Some(cursor.expose_to_provider_adapter().to_owned());
        self.thread_not_loaded_observed = false;
        Ok(frame)
    }

    /// Encodes one ephemeral thread start with the exact reviewed dynamic
    /// tools from this turn.
    ///
    /// # Errors
    ///
    /// Returns a safe error for an empty/duplicate tool set, invalid input,
    /// ID exhaustion, or frame-size overflow.
    pub fn start_dynamic_thread(
        &mut self,
        input: &AiCodexAppServerTurnInput,
    ) -> Result<Vec<u8>, ProviderError> {
        input.validate()?;
        self.validate_thread_lifecycle_boundary()?;
        if self.retained_model.is_some()
            || input.tools().is_empty()
            || !self.dynamic_tools.is_empty()
            || !self.pending_dynamic_requests.is_empty()
            || !self.started_dynamic_calls.is_empty()
            || !self.responded_dynamic_calls.is_empty()
        {
            return Err(ProviderError::Rejected);
        }
        let mut definitions = BTreeMap::new();
        let dynamic_tools = input
            .tools()
            .iter()
            .map(|tool| {
                if definitions
                    .insert(tool.provider_name.clone(), tool.clone())
                    .is_some()
                {
                    return Err(ProviderError::Rejected);
                }
                Ok(json!({
                    "type": "function",
                    "name": tool.provider_name,
                    "description": tool.description,
                    "inputSchema": tool.parameters,
                    "deferLoading": false,
                }))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let developer_instructions = if input.instructions().is_empty() {
            Value::Null
        } else {
            Value::String(input.instructions().join("\n\n"))
        };
        let frame = self.request(
            ClientMethod::ThreadStart,
            "thread/start",
            json!({
                "model": input.model(),
                "developerInstructions": developer_instructions,
                "ephemeral": true,
                "dynamicTools": dynamic_tools,
                "approvalPolicy": "never",
                "sandbox": "read-only",
            }),
        )?;
        self.begin_new_thread_lifecycle();
        self.dynamic_tools = definitions;
        Ok(frame)
    }

    /// Encodes the only allowed successful response to an admitted dynamic
    /// tool request.
    ///
    /// The result is serialized as one text content item because arbitrary
    /// image/audio URLs are outside this integration.
    ///
    /// # Errors
    ///
    /// Returns a safe error for an unknown/swapped request, oversized output,
    /// or frame-size overflow.
    pub fn dynamic_tool_response(
        &mut self,
        request_id: u64,
        result: &crate::ProviderDynamicToolResult,
    ) -> Result<Vec<u8>, ProviderError> {
        let Some((call_id, tool_id)) = self.pending_dynamic_requests.remove(&request_id) else {
            return Err(ProviderError::Rejected);
        };
        if call_id != result.call_id() || tool_id != result.tool_id() {
            return Err(ProviderError::Rejected);
        }
        let text = serde_json::to_string(result.output()).map_err(|_| ProviderError::Rejected)?;
        if text.len() > MAXIMUM_TEXT_BYTES {
            return Err(ProviderError::Rejected);
        }
        let frame = self.encode(json!({
            "id": request_id,
            "result": {
                "contentItems": [{"type": "inputText", "text": text}],
                "success": true,
            }
        }))?;
        let definition = self
            .dynamic_tools
            .values()
            .find(|definition| definition.tool_id == tool_id)
            .ok_or(ProviderError::Rejected)?;
        if self
            .responded_dynamic_calls
            .insert(call_id, definition.provider_name.clone())
            .is_some()
        {
            return Err(ProviderError::Rejected);
        }
        Ok(frame)
    }

    /// Encodes text-only user input for one exact lifecycle-complete thread.
    ///
    /// Trusted instructions are deliberately not copied into the user input
    /// list. No tool, path, URL, image, skill, audio, environment, approval,
    /// sandbox, or generic JSON field can be supplied through this method.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive error for invalid references/input, ID
    /// exhaustion, or frame-size overflow.
    pub fn start_turn(
        &mut self,
        thread_id: &str,
        input: &AiCodexAppServerTurnInput,
    ) -> Result<Vec<u8>, ProviderError> {
        input.validate()?;
        let input_dynamic_tools = input
            .tools()
            .iter()
            .map(|tool| (tool.provider_name.clone(), tool.clone()))
            .collect::<BTreeMap<_, _>>();
        if !valid_reference(thread_id)
            || self.active_thread_id.as_deref() != Some(thread_id)
            || self.thread_lifecycle_phase != ThreadLifecyclePhase::Complete
            || self
                .retained_model
                .as_deref()
                .is_some_and(|model| model != input.model())
            || input_dynamic_tools.len() != input.tools().len()
            || self.dynamic_tools != input_dynamic_tools
            || self.pending_turn_thread_id.is_some()
            || self.active_turn_id.is_some()
            || self.turn_response_observed
            || self.turn_started_observed
            || !self.started_items.is_empty()
            || !self.completed_items.is_empty()
        {
            return Err(ProviderError::InvalidRequest);
        }
        let frame = self.request(
            ClientMethod::TurnStart,
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": input.input().iter().map(|text| json!({"type": "text", "text": text})).collect::<Vec<_>>(),
            }),
        )?;
        self.pending_turn_thread_id = Some(thread_id.to_owned());
        Ok(frame)
    }

    /// Encodes interruption for one exact active thread and turn.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive error for invalid references, ID exhaustion,
    /// or frame-size overflow.
    pub fn interrupt_turn(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<Vec<u8>, ProviderError> {
        if !valid_reference(thread_id)
            || !valid_reference(turn_id)
            || self.active_thread_id.as_deref() != Some(thread_id)
            || self.active_turn_id.as_deref() != Some(turn_id)
        {
            return Err(ProviderError::InvalidRequest);
        }
        self.request(
            ClientMethod::TurnInterrupt,
            "turn/interrupt",
            json!({"threadId": thread_id, "turnId": turn_id}),
        )
    }

    fn request(
        &mut self,
        method: ClientMethod,
        name: &'static str,
        params: Value,
    ) -> Result<Vec<u8>, ProviderError> {
        let id = self.next_id;
        let next_id = self.next_id.checked_add(1).ok_or(ProviderError::Rejected)?;
        let encoded = self.encode(json!({"id": id, "method": name, "params": params}))?;
        if self.pending.insert(id, method).is_some() {
            return Err(ProviderError::Rejected);
        }
        self.next_id = next_id;
        Ok(encoded)
    }

    fn encode(&self, value: Value) -> Result<Vec<u8>, ProviderError> {
        let mut encoded = serde_json::to_vec(&value).map_err(|_| ProviderError::InvalidRequest)?;
        encoded.push(b'\n');
        if encoded.len() > self.maximum_frame_bytes {
            return Err(ProviderError::InvalidRequest);
        }
        Ok(encoded)
    }

    /// Admits one exact bounded inbound response or allowlisted notification.
    ///
    /// Every server-initiated request, uncorrelated response, provider error,
    /// forbidden item kind, raw reasoning item, and unknown notification is
    /// rejected.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::Rejected`] for every malformed, uncorrelated,
    /// unsupported, or oversized frame.
    pub fn accept(&mut self, frame: &[u8]) -> Result<AiCodexAppServerInbound, ProviderError> {
        if frame.is_empty() || frame.len() > self.maximum_frame_bytes {
            return Err(ProviderError::Rejected);
        }
        let value: Value = serde_json::from_slice(frame).map_err(|_| ProviderError::Rejected)?;
        let object = value.as_object().ok_or(ProviderError::Rejected)?;
        if object.contains_key("method") && object.contains_key("id") {
            if object
                .keys()
                .any(|key| !matches!(key.as_str(), "id" | "method" | "params"))
                || object.get("method").and_then(Value::as_str) != Some("item/tool/call")
            {
                return Err(ProviderError::Rejected);
            }
            let request_id = object
                .get("id")
                .and_then(Value::as_u64)
                .ok_or(ProviderError::Rejected)?;
            let params = object
                .get("params")
                .and_then(Value::as_object)
                .ok_or(ProviderError::Rejected)?;
            if request_id == 0
                || self.pending_dynamic_requests.contains_key(&request_id)
                || params.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "arguments" | "callId" | "namespace" | "threadId" | "tool" | "turnId"
                    )
                })
                || params
                    .get("namespace")
                    .is_some_and(|value| !value.is_null())
            {
                return Err(ProviderError::Rejected);
            }
            let call_id = params
                .get("callId")
                .and_then(Value::as_str)
                .filter(|value| valid_reference(value))
                .ok_or(ProviderError::Rejected)?;
            let thread_id = params
                .get("threadId")
                .and_then(Value::as_str)
                .filter(|value| valid_reference(value))
                .ok_or(ProviderError::Rejected)?;
            let turn_id = params
                .get("turnId")
                .and_then(Value::as_str)
                .filter(|value| valid_reference(value))
                .ok_or(ProviderError::Rejected)?;
            let tool_name = params
                .get("tool")
                .and_then(Value::as_str)
                .ok_or(ProviderError::Rejected)?;
            let definition = self
                .dynamic_tools
                .get(tool_name)
                .ok_or(ProviderError::Rejected)?;
            if self.started_dynamic_calls.get(call_id).map(String::as_str) != Some(tool_name) {
                return Err(ProviderError::Rejected);
            }
            let call = ProviderDynamicToolCall::from_definition(
                turn_id,
                call_id,
                definition,
                params
                    .get("arguments")
                    .cloned()
                    .ok_or(ProviderError::Rejected)?,
            )?;
            self.validate_active_turn(thread_id, turn_id)?;
            self.pending_dynamic_requests.insert(
                request_id,
                (call.call_id().to_owned(), call.tool_id().to_owned()),
            );
            return Ok(AiCodexAppServerInbound::DynamicToolCall {
                request_id,
                thread_id: thread_id.to_owned(),
                turn_id: turn_id.to_owned(),
                call,
            });
        }
        if let Some(id) = object.get("id").and_then(Value::as_u64) {
            if object
                .keys()
                .any(|key| !matches!(key.as_str(), "id" | "result" | "error"))
                || object.contains_key("result") == object.contains_key("error")
            {
                return Err(ProviderError::Rejected);
            }
            let method = self.pending.remove(&id).ok_or(ProviderError::Rejected)?;
            if object.contains_key("error") {
                return Err(ProviderError::Rejected);
            }
            let result = object
                .get("result")
                .filter(|result| result.is_object())
                .cloned()
                .ok_or(ProviderError::Rejected)?;
            self.accept_correlated_response(method, &result)?;
            return Ok(AiCodexAppServerInbound::Response {
                id,
                method: client_method_name(method),
                result,
            });
        }
        let notification: CodexAppServerNotificationEnvelope =
            serde_json::from_slice(frame).map_err(|_| ProviderError::Rejected)?;
        notification.validate()?;
        if notification.method == REMOTE_CONTROL_STATUS_CHANGED {
            return self.accept_disabled_remote_control_status(notification);
        }
        let method = Some(notification.method.as_str())
            .filter(|method| allowed_notification(method))
            .ok_or(ProviderError::Rejected)?;
        let params = notification.params;
        if matches!(method, "item/started" | "item/completed")
            && params
                .get("item")
                .and_then(Value::as_object)
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
                == Some("dynamicToolCall")
        {
            return self.accept_dynamic_tool_lifecycle(method, &params);
        }
        validate_allowed_notification(method, &params)?;
        self.accept_notification_binding(method, &params)?;
        if method == "turn/completed" {
            if !self.pending_dynamic_requests.is_empty()
                || !self.started_dynamic_calls.is_empty()
                || !self.responded_dynamic_calls.is_empty()
            {
                return Err(ProviderError::Rejected);
            }
            if self.retained_model.is_none() {
                self.dynamic_tools.clear();
            }
            self.pending_turn_thread_id = None;
            self.active_turn_id = None;
            self.turn_response_observed = false;
            self.turn_started_observed = false;
            self.started_items.clear();
            self.completed_items.clear();
        }
        Ok(AiCodexAppServerInbound::Notification {
            method: method.to_owned(),
            params,
        })
    }

    fn accept_dynamic_tool_lifecycle(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<AiCodexAppServerInbound, ProviderError> {
        if self.dynamic_tools.is_empty() {
            return Err(ProviderError::Rejected);
        }
        let params = params.as_object().ok_or(ProviderError::Rejected)?;
        let timestamp_key = if method == "item/started" {
            "startedAtMs"
        } else {
            "completedAtMs"
        };
        if params.keys().any(|key| {
            !matches!(key.as_str(), "item" | "threadId" | "turnId") && key != timestamp_key
        }) || params.get(timestamp_key).and_then(Value::as_i64).is_none()
        {
            return Err(ProviderError::Rejected);
        }
        let thread_id = params
            .get("threadId")
            .and_then(Value::as_str)
            .filter(|value| valid_reference(value))
            .ok_or(ProviderError::Rejected)?;
        let turn_id = params
            .get("turnId")
            .and_then(Value::as_str)
            .filter(|value| valid_reference(value))
            .ok_or(ProviderError::Rejected)?;
        self.validate_active_turn(thread_id, turn_id)?;
        let item = params
            .get("item")
            .and_then(Value::as_object)
            .ok_or(ProviderError::Rejected)?;
        if item.keys().any(|key| {
            !matches!(
                key.as_str(),
                "arguments"
                    | "contentItems"
                    | "durationMs"
                    | "id"
                    | "namespace"
                    | "status"
                    | "success"
                    | "tool"
                    | "type"
            )
        }) || item.get("type").and_then(Value::as_str) != Some("dynamicToolCall")
            || item.get("namespace").is_some_and(|value| !value.is_null())
        {
            return Err(ProviderError::Rejected);
        }
        let call_id = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| valid_reference(value))
            .ok_or(ProviderError::Rejected)?;
        let provider_name = item
            .get("tool")
            .and_then(Value::as_str)
            .ok_or(ProviderError::Rejected)?;
        let definition = self
            .dynamic_tools
            .get(provider_name)
            .ok_or(ProviderError::Rejected)?;
        let arguments = item.get("arguments").ok_or(ProviderError::Rejected)?;
        let validator = jsonschema::validator_for(&definition.parameters)
            .map_err(|_| ProviderError::Rejected)?;
        if !arguments.is_object() || !validator.is_valid(arguments) {
            return Err(ProviderError::Rejected);
        }
        let completed = method == "item/completed";
        if completed {
            if item.get("status").and_then(Value::as_str) != Some("completed")
                || item.get("success").and_then(Value::as_bool) != Some(true)
                || self.started_dynamic_calls.get(call_id).map(String::as_str)
                    != Some(provider_name)
                || self.responded_dynamic_calls.remove(call_id).as_deref() != Some(provider_name)
            {
                return Err(ProviderError::Rejected);
            }
            self.started_dynamic_calls.remove(call_id);
        } else if item.get("status").and_then(Value::as_str) != Some("inProgress")
            || item.get("success").is_some_and(|value| !value.is_null())
            || item
                .get("contentItems")
                .is_some_and(|value| !value.is_null())
            || self
                .started_dynamic_calls
                .insert(call_id.to_owned(), provider_name.to_owned())
                .is_some()
        {
            return Err(ProviderError::Rejected);
        }
        let _ = thread_id;
        Ok(AiCodexAppServerInbound::DynamicToolLifecycle {
            turn_id: turn_id.to_owned(),
            call_id: call_id.to_owned(),
            provider_name: provider_name.to_owned(),
            completed,
        })
    }

    fn accept_correlated_response(
        &mut self,
        method: ClientMethod,
        result: &Value,
    ) -> Result<(), ProviderError> {
        match method {
            ClientMethod::ThreadStart | ClientMethod::ThreadResume => {
                let thread_id = nested_reference(result, "thread", "id")?;
                if !matches!(
                    self.thread_lifecycle_phase,
                    ThreadLifecyclePhase::AwaitingResponseAndStarted
                        | ThreadLifecyclePhase::AwaitingResponse
                ) || self
                    .active_thread_id
                    .as_deref()
                    .is_some_and(|expected| expected != thread_id)
                {
                    return Err(ProviderError::Rejected);
                }
                self.active_thread_id = Some(thread_id.to_owned());
                self.thread_lifecycle_phase = match self.thread_lifecycle_phase {
                    ThreadLifecyclePhase::AwaitingResponseAndStarted => {
                        ThreadLifecyclePhase::AwaitingStarted
                    }
                    ThreadLifecyclePhase::AwaitingResponse => ThreadLifecyclePhase::Complete,
                    _ => return Err(ProviderError::Rejected),
                };
            }
            ClientMethod::TurnStart => {
                let turn_id = nested_reference(result, "turn", "id")?;
                if self.turn_response_observed
                    || self.pending_turn_thread_id.as_deref() != self.active_thread_id.as_deref()
                    || self
                        .active_turn_id
                        .as_deref()
                        .is_some_and(|expected| expected != turn_id)
                {
                    return Err(ProviderError::Rejected);
                }
                self.active_turn_id = Some(turn_id.to_owned());
                self.turn_response_observed = true;
            }
            ClientMethod::Initialize => {
                self.initialization_complete = true;
            }
            ClientMethod::ThreadDelete => {
                self.active_thread_id = None;
                self.thread_lifecycle_phase = ThreadLifecyclePhase::Deleted;
            }
            ClientMethod::TurnInterrupt => {}
        }
        Ok(())
    }

    fn accept_disabled_remote_control_status(
        &mut self,
        notification: CodexAppServerNotificationEnvelope,
    ) -> Result<AiCodexAppServerInbound, ProviderError> {
        let params: DisabledRemoteControlStatusParams =
            serde_json::from_value(notification.params).map_err(|_| ProviderError::Rejected)?;
        if notification.method != REMOTE_CONTROL_STATUS_CHANGED
            || !valid_identifier(&params.server_name)
            || !valid_identifier(&params.installation_id)
            || !self.initialization_complete
            || self.remote_control_disabled_observed
            || matches!(
                self.thread_lifecycle_phase,
                ThreadLifecyclePhase::AwaitingStarted
                    | ThreadLifecyclePhase::Complete
                    | ThreadLifecyclePhase::Deleted
            )
            || self.pending_turn_thread_id.is_some()
            || self.active_turn_id.is_some()
        {
            return Err(ProviderError::Rejected);
        }
        self.remote_control_disabled_observed = true;
        Ok(AiCodexAppServerInbound::RemoteControlDisabled)
    }

    fn accept_notification_binding(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<(), ProviderError> {
        match method {
            "thread/started" => {
                let thread_id = nested_reference(params, "thread", "id")?;
                if !self.initialization_complete
                    || !matches!(
                        self.thread_lifecycle_phase,
                        ThreadLifecyclePhase::AwaitingResponseAndStarted
                            | ThreadLifecyclePhase::AwaitingStarted
                    )
                    || self.pending_turn_thread_id.is_some()
                    || self.active_turn_id.is_some()
                    || self
                        .pending
                        .values()
                        .any(|method| *method == ClientMethod::ThreadDelete)
                    || (self.thread_lifecycle_phase
                        == ThreadLifecyclePhase::AwaitingResponseAndStarted
                        && !self.pending.values().any(|method| {
                            matches!(
                                method,
                                ClientMethod::ThreadStart | ClientMethod::ThreadResume
                            )
                        }))
                    || self
                        .active_thread_id
                        .as_deref()
                        .is_some_and(|expected| expected != thread_id)
                {
                    return Err(ProviderError::Rejected);
                }
                self.active_thread_id = Some(thread_id.to_owned());
                self.thread_lifecycle_phase = match self.thread_lifecycle_phase {
                    ThreadLifecyclePhase::AwaitingResponseAndStarted => {
                        ThreadLifecyclePhase::AwaitingResponse
                    }
                    ThreadLifecyclePhase::AwaitingStarted => ThreadLifecyclePhase::Complete,
                    _ => return Err(ProviderError::Rejected),
                };
            }
            "thread/status/changed" => {
                let status: ThreadNotLoadedStatusChangedParams =
                    serde_json::from_value(params.clone()).map_err(|_| ProviderError::Rejected)?;
                if !self.initialization_complete
                    || !valid_reference(&status.thread_id)
                    || self.deleting_thread_id.as_deref() != Some(status.thread_id.as_str())
                    || self.thread_not_loaded_observed
                    || (self.thread_lifecycle_phase != ThreadLifecyclePhase::Deleted
                        && !self
                            .pending
                            .values()
                            .any(|method| *method == ClientMethod::ThreadDelete))
                    || self.pending_turn_thread_id.is_some()
                    || self.active_turn_id.is_some()
                {
                    return Err(ProviderError::Rejected);
                }
                self.thread_not_loaded_observed = true;
            }
            "turn/started" => {
                let thread_id = direct_reference(params, "threadId")?;
                let turn_id = nested_reference(params, "turn", "id")?;
                if self.turn_started_observed
                    || self.pending_turn_thread_id.as_deref() != Some(thread_id)
                    || self.active_thread_id.as_deref() != Some(thread_id)
                    || (!self.turn_response_observed
                        && !self
                            .pending
                            .values()
                            .any(|method| *method == ClientMethod::TurnStart))
                    || self
                        .active_turn_id
                        .as_deref()
                        .is_some_and(|expected| expected != turn_id)
                {
                    return Err(ProviderError::Rejected);
                }
                self.active_turn_id = Some(turn_id.to_owned());
                self.turn_started_observed = true;
            }
            "turn/completed" => {
                let thread_id = direct_reference(params, "threadId")?;
                let turn_id = nested_reference(params, "turn", "id")?;
                if !self.turn_response_observed || !self.turn_started_observed {
                    return Err(ProviderError::Rejected);
                }
                self.validate_active_turn(thread_id, turn_id)?;
            }
            "item/started" | "item/completed" => {
                self.validate_active_turn(
                    direct_reference(params, "threadId")?,
                    direct_reference(params, "turnId")?,
                )?;
                let item = params
                    .get("item")
                    .and_then(Value::as_object)
                    .ok_or(ProviderError::Rejected)?;
                let item_id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|value| valid_reference(value))
                    .ok_or(ProviderError::Rejected)?;
                let item_type = item
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or(ProviderError::Rejected)?;
                if method == "item/started" {
                    if self.completed_items.contains(item_id)
                        || self.started_items.contains_key(item_id)
                        || self.started_items.len() >= MAXIMUM_TEXT_BLOCKS
                    {
                        return Err(ProviderError::Rejected);
                    }
                    self.started_items
                        .insert(item_id.to_owned(), item_type.to_owned());
                } else {
                    if self.started_items.get(item_id).map(String::as_str) != Some(item_type)
                        || self.completed_items.contains(item_id)
                        || self.completed_items.len() >= MAXIMUM_TEXT_BLOCKS
                    {
                        return Err(ProviderError::Rejected);
                    }
                    self.completed_items.insert(item_id.to_owned());
                    self.started_items.remove(item_id);
                }
            }
            "item/agentMessage/delta" => {
                self.validate_active_turn(
                    direct_reference(params, "threadId")?,
                    direct_reference(params, "turnId")?,
                )?;
                let item_id = direct_reference(params, "itemId")?;
                if self.started_items.get(item_id).map(String::as_str) != Some("agentMessage") {
                    return Err(ProviderError::Rejected);
                }
            }
            "thread/tokenUsage/updated" => {
                self.validate_active_turn(
                    direct_reference(params, "threadId")?,
                    direct_reference(params, "turnId")?,
                )?;
            }
            _ => return Err(ProviderError::Rejected),
        }
        Ok(())
    }

    fn validate_active_turn(&self, thread_id: &str, turn_id: &str) -> Result<(), ProviderError> {
        if self.active_thread_id.as_deref() != Some(thread_id)
            || self.active_turn_id.as_deref() != Some(turn_id)
            || self.thread_lifecycle_phase != ThreadLifecyclePhase::Complete
            || !self.turn_started_observed
        {
            return Err(ProviderError::Rejected);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CodexAppServerNotificationEnvelope {
    method: String,
    params: Value,
    #[serde(rename = "emittedAtMs")]
    emitted_at_ms: i64,
}

impl CodexAppServerNotificationEnvelope {
    fn validate(&self) -> Result<(), ProviderError> {
        if self.emitted_at_ms <= 0 || !self.params.is_object() {
            return Err(ProviderError::Rejected);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DisabledRemoteControlStatusParams {
    #[serde(rename = "status")]
    _status: DisabledRemoteControlStatus,
    server_name: String,
    installation_id: String,
    #[serde(rename = "environmentId")]
    _environment_id: (),
}

#[derive(Deserialize)]
enum DisabledRemoteControlStatus {
    #[serde(rename = "disabled")]
    Disabled,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ThreadNotLoadedStatusChangedParams {
    thread_id: String,
    #[serde(rename = "status")]
    _status: ThreadNotLoadedStatus,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThreadNotLoadedStatus {
    #[serde(rename = "type")]
    _kind: ThreadNotLoadedStatusKind,
}

#[derive(Deserialize)]
enum ThreadNotLoadedStatusKind {
    #[serde(rename = "notLoaded")]
    NotLoaded,
}

fn direct_reference<'a>(value: &'a Value, key: &str) -> Result<&'a str, ProviderError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| valid_reference(value))
        .ok_or(ProviderError::Rejected)
}

fn nested_reference<'a>(
    value: &'a Value,
    object_key: &str,
    value_key: &str,
) -> Result<&'a str, ProviderError> {
    value
        .get(object_key)
        .and_then(Value::as_object)
        .and_then(|object| object.get(value_key))
        .and_then(Value::as_str)
        .filter(|value| valid_reference(value))
        .ok_or(ProviderError::Rejected)
}

fn client_method_name(method: ClientMethod) -> &'static str {
    match method {
        ClientMethod::Initialize => "initialize",
        ClientMethod::ThreadStart => "thread/start",
        ClientMethod::ThreadResume => "thread/resume",
        ClientMethod::ThreadDelete => "thread/delete",
        ClientMethod::TurnStart => "turn/start",
        ClientMethod::TurnInterrupt => "turn/interrupt",
    }
}

fn allowed_notification(method: &str) -> bool {
    matches!(
        method,
        "thread/started"
            | "thread/status/changed"
            | "turn/started"
            | "item/started"
            | "item/completed"
            | "item/agentMessage/delta"
            | "thread/tokenUsage/updated"
            | "turn/completed"
    )
}

fn validate_allowed_notification(method: &str, params: &Value) -> Result<(), ProviderError> {
    let object = params.as_object().ok_or(ProviderError::Rejected)?;
    if matches!(method, "item/started" | "item/completed") {
        let item_type = object
            .get("item")
            .and_then(Value::as_object)
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str)
            .ok_or(ProviderError::Rejected)?;
        if !matches!(item_type, "userMessage" | "agentMessage") {
            return Err(ProviderError::Rejected);
        }
    }
    if method == "item/agentMessage/delta"
        && object
            .get("delta")
            .and_then(Value::as_str)
            .is_none_or(|delta| delta.len() > MAXIMUM_TEXT_BYTES)
    {
        return Err(ProviderError::Rejected);
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAXIMUM_IDENTIFIER_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':' | b'/')
        })
}

fn valid_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAXIMUM_VERSION_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'"' && byte != b'\\')
}

fn valid_reference(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 1_024
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

fn registration_identity(
    provider_profile_id: &str,
    logical_model: &str,
    executable_sha256: &str,
    executable_version: &str,
    sandbox_profile: &str,
    protocol_version: &str,
    experimental_dynamic_tools: bool,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"graphql-orm-ai/codex-app-server-registration/v1\0");
    for value in [
        provider_profile_id,
        logical_model,
        executable_sha256,
        executable_version,
        sandbox_profile,
        protocol_version,
    ] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update([u8::from(experimental_dynamic_tools)]);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::path::PathBuf;
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::thread::{self, JoinHandle};

    use agql_auth::{
        AccessTokenMetadata, AuthPrincipal, AuthUser, PrincipalReference, SessionContext,
    };
    use futures::stream;

    use super::*;
    use crate::{AiRunId, AiSessionId, ProviderDynamicToolResult, ProviderEvent};
    use uuid::Uuid;

    struct LiveCodexProcess {
        child: Child,
        stdin: ChildStdin,
        frames: Receiver<Vec<u8>>,
        reader: Option<JoinHandle<()>>,
    }

    impl LiveCodexProcess {
        fn launch(executable: &str, root: PathBuf) -> Self {
            assert!(root.is_absolute());
            assert!(root.is_dir());
            let mut child = Command::new(executable)
                .args(["app-server", "--stdio"])
                .env_clear()
                .env("CODEX_HOME", &root)
                .env("HOME", &root)
                .env("PATH", "/usr/bin:/bin")
                .current_dir(&root)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("reviewed Codex app-server should launch");
            let stdin = child.stdin.take().expect("stdin should be piped");
            let stdout = child.stdout.take().expect("stdout should be piped");
            let (sender, frames) = mpsc::channel();
            let reader = thread::spawn(move || {
                for line in BufReader::new(stdout).split(b'\n') {
                    let Ok(line) = line else {
                        break;
                    };
                    if sender.send(line).is_err() {
                        break;
                    }
                }
            });
            Self {
                child,
                stdin,
                frames,
                reader: Some(reader),
            }
        }

        fn send(&mut self, frame: &[u8]) {
            self.stdin
                .write_all(frame)
                .expect("protocol frame should write");
            self.stdin.flush().expect("protocol frame should flush");
        }

        fn receive(&self) -> Vec<u8> {
            self.frames
                .recv_timeout(Duration::from_secs(10))
                .expect("app-server should answer within the live-test bound")
        }
    }

    impl Drop for LiveCodexProcess {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }

    struct Counters {
        launches: AtomicUsize,
        turns: AtomicUsize,
        interrupts: AtomicUsize,
        shutdowns: AtomicUsize,
        drops: AtomicUsize,
        kills: AtomicUsize,
        pending: AtomicBool,
        stream_error: AtomicBool,
        created_threads: AtomicUsize,
        created_dynamic_tools: AtomicUsize,
        bound_turns: AtomicUsize,
        retained_turns: AtomicUsize,
        deleted_threads: AtomicUsize,
    }

    impl Counters {
        fn new() -> Self {
            Self {
                launches: AtomicUsize::new(0),
                turns: AtomicUsize::new(0),
                interrupts: AtomicUsize::new(0),
                shutdowns: AtomicUsize::new(0),
                drops: AtomicUsize::new(0),
                kills: AtomicUsize::new(0),
                pending: AtomicBool::new(false),
                stream_error: AtomicBool::new(false),
                created_threads: AtomicUsize::new(0),
                created_dynamic_tools: AtomicUsize::new(0),
                bound_turns: AtomicUsize::new(0),
                retained_turns: AtomicUsize::new(0),
                deleted_threads: AtomicUsize::new(0),
            }
        }
    }

    struct FakeFactory {
        counters: Arc<Counters>,
    }

    #[async_trait]
    impl AiCodexAppServerRunProcessFactory for FakeFactory {
        async fn launch(
            &self,
            _registration: Arc<AiCodexAppServerRegistration>,
        ) -> Result<AiCodexAppServerLaunchedProcess, ProviderError> {
            self.counters.launches.fetch_add(1, Ordering::SeqCst);
            let process = Arc::new(FakeProcess {
                counters: self.counters.clone(),
            });
            let counters = self.counters.clone();
            Ok(AiCodexAppServerLaunchedProcess::new(
                process,
                Arc::new(move || {
                    counters.kills.fetch_add(1, Ordering::SeqCst);
                }),
            ))
        }
    }

    struct FakeProcess {
        counters: Arc<Counters>,
    }

    impl Drop for FakeProcess {
        fn drop(&mut self) {
            self.counters.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl AiCodexAppServerRunProcess for FakeProcess {
        async fn create_empty_thread(
            &self,
            _model: &str,
            dynamic_tools: Vec<ModelToolDefinition>,
        ) -> Result<crate::AiProviderSessionCursor, ProviderError> {
            self.counters.created_threads.fetch_add(1, Ordering::SeqCst);
            self.counters
                .created_dynamic_tools
                .fetch_add(dynamic_tools.len(), Ordering::SeqCst);
            crate::AiProviderSessionCursor::new(
                "codex.app_server.thread.v2",
                "thread-retained-test",
            )
            .map_err(|_| ProviderError::Rejected)
        }

        async fn start_fresh_turn(
            &self,
            _input: AiCodexAppServerTurnInput,
        ) -> Result<ProviderEventStream, ProviderError> {
            self.counters.turns.fetch_add(1, Ordering::SeqCst);
            if self.counters.pending.load(Ordering::SeqCst) {
                return Ok(Box::pin(stream::pending()));
            }
            if self.counters.stream_error.load(Ordering::SeqCst) {
                return Ok(Box::pin(stream::iter([Err(ProviderError::Rejected)])));
            }
            Ok(Box::pin(stream::iter([
                Ok(ProviderEvent::ResponseStarted { response_id: None }),
                Ok(ProviderEvent::TextDelta {
                    text: "ok".to_owned(),
                }),
                Ok(ProviderEvent::Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cached_input_tokens: 0,
                }),
                Ok(ProviderEvent::ResponseCompleted { response_id: None }),
            ])))
        }

        async fn start_dynamic_turn(
            &self,
            input: AiCodexAppServerTurnInput,
            responder: Arc<dyn ProviderDynamicToolResponder>,
        ) -> Result<ProviderEventStream, ProviderError> {
            self.counters.turns.fetch_add(1, Ordering::SeqCst);
            let definition = input.tools().first().ok_or(ProviderError::Rejected)?;
            let call = ProviderDynamicToolCall::from_definition(
                "turn-dynamic-1",
                "call-dynamic-1",
                definition,
                json!({"query": "bounded"}),
            )?;
            let result = responder.respond(call).await?;
            if result.call_id() != "call-dynamic-1"
                || result.tool_id() != definition.tool_id
                || result.output() != &json!({"count": 3})
            {
                return Err(ProviderError::Rejected);
            }
            Ok(Box::pin(stream::iter([
                Ok(ProviderEvent::ResponseStarted {
                    response_id: Some("turn-dynamic-1".to_owned()),
                }),
                Ok(ProviderEvent::ToolCallStarted {
                    call_id: "call-dynamic-1".to_owned(),
                    tool_id: definition.tool_id.clone(),
                }),
                Ok(ProviderEvent::ToolArgumentsDelta {
                    call_id: "call-dynamic-1".to_owned(),
                    delta: "{\"query\":\"bounded\"}".to_owned(),
                }),
                Ok(ProviderEvent::ToolCallCompleted {
                    call_id: "call-dynamic-1".to_owned(),
                    arguments: json!({"query": "bounded"}),
                }),
                Ok(ProviderEvent::TextDelta {
                    text: "There are three.".to_owned(),
                }),
                Ok(ProviderEvent::Usage {
                    input_tokens: 10,
                    output_tokens: 4,
                    cached_input_tokens: 0,
                }),
                Ok(ProviderEvent::ResponseCompleted {
                    response_id: Some("turn-dynamic-1".to_owned()),
                }),
            ])))
        }

        async fn start_retained_turn(
            &self,
            _session: crate::AiOpenedProviderSession,
            input: AiCodexAppServerTurnInput,
        ) -> Result<ProviderEventStream, ProviderError> {
            self.counters.retained_turns.fetch_add(1, Ordering::SeqCst);
            self.start_fresh_turn(input).await
        }

        async fn start_bound_turn(
            &self,
            _session: crate::AiOpenedProviderSession,
            input: AiCodexAppServerTurnInput,
        ) -> Result<ProviderEventStream, ProviderError> {
            self.counters.bound_turns.fetch_add(1, Ordering::SeqCst);
            self.start_fresh_turn(input).await
        }

        async fn start_bound_dynamic_turn(
            &self,
            _session: crate::AiOpenedProviderSession,
            input: AiCodexAppServerTurnInput,
            responder: Arc<dyn ProviderDynamicToolResponder>,
        ) -> Result<ProviderEventStream, ProviderError> {
            self.counters.bound_turns.fetch_add(1, Ordering::SeqCst);
            self.start_dynamic_turn(input, responder).await
        }

        async fn start_retained_dynamic_turn(
            &self,
            _session: crate::AiOpenedProviderSession,
            input: AiCodexAppServerTurnInput,
            responder: Arc<dyn ProviderDynamicToolResponder>,
        ) -> Result<ProviderEventStream, ProviderError> {
            self.counters.retained_turns.fetch_add(1, Ordering::SeqCst);
            self.start_dynamic_turn(input, responder).await
        }

        async fn delete_thread(
            &self,
            cursor: &crate::AiProviderSessionCursor,
        ) -> Result<(), ProviderError> {
            if cursor.kind() != "codex.app_server.thread.v2"
                || cursor.expose_to_provider_adapter() != "thread-retained-test"
            {
                return Err(ProviderError::Rejected);
            }
            self.counters.deleted_threads.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn interrupt(&self) -> Result<(), ProviderError> {
            self.counters.interrupts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn shutdown(&self, _reason: AiProviderRunCloseReason) -> Result<(), ProviderError> {
            self.counters.shutdowns.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn binding_for_owner(owner: u8) -> AiProviderRunBinding {
        AiProviderRunBinding::new_for_principal_reference(
            AiSessionId::new(),
            AiRunId::new(),
            Uuid::new_v4(),
            1,
            &principal_reference_for_owner(owner),
        )
        .expect("test binding should validate")
    }

    fn binding() -> AiProviderRunBinding {
        binding_for_owner(1)
    }

    fn principal_reference() -> PrincipalReference {
        principal_reference_for_owner(1)
    }

    fn principal_reference_for_owner(owner: u8) -> PrincipalReference {
        AuthPrincipal::User(AuthUser {
            user_id: format!("codex-provider-test-{owner}"),
            session_id: Uuid::new_v4(),
            roles: Vec::new(),
            scopes: Vec::new(),
            session: SessionContext::default(),
            token_claims: AccessTokenMetadata::default(),
        })
        .reference()
    }

    fn opened_session(
        binding: AiProviderRunBinding,
        registration: &AiCodexAppServerRegistration,
        cursor: crate::AiProviderSessionCursor,
    ) -> crate::AiOpenedProviderSession {
        let descriptor = crate::AiProviderSessionDescriptor::new(
            ProviderKind::LocalHarness,
            registration.provider_profile_id(),
            registration.logical_model(),
            registration.identity(),
            registration.protocol_version(),
            "d".repeat(64),
        )
        .expect("provider-session descriptor should validate");
        let claim = crate::AiProviderSessionClaim {
            binding_id: Uuid::new_v4(),
            session_id: binding.session_id(),
            run_id: binding.run_id(),
            attempt_id: binding.attempt_id(),
            run_lease_generation: binding.lease_generation(),
            binding_claim_generation: 1,
            binding_row_version: 1,
            claim_expires_at: time::OffsetDateTime::now_utc() + time::Duration::minutes(1),
            through_message_sequence: 0,
            transcript_fingerprint: "c".repeat(64),
            principal_reference: principal_reference(),
            descriptor,
        };
        crate::AiOpenedProviderSession::new(claim, cursor)
    }

    fn registration(version: &str) -> Arc<AiCodexAppServerRegistration> {
        Arc::new(
            AiCodexAppServerRegistration::new(
                "profile-1",
                "model-1",
                "a".repeat(64),
                version,
                "sandbox-empty",
                AI_CODEX_APP_SERVER_PROTOCOL_V2,
            )
            .expect("test registration should validate"),
        )
    }

    fn dynamic_registration(version: &str) -> Arc<AiCodexAppServerRegistration> {
        Arc::new(
            AiCodexAppServerRegistration::new(
                "profile-1",
                "model-1",
                "a".repeat(64),
                version,
                "sandbox-empty",
                AI_CODEX_APP_SERVER_PROTOCOL_V2,
            )
            .expect("test registration should validate")
            .with_experimental_dynamic_tools(),
        )
    }

    fn dynamic_tool() -> ModelToolDefinition {
        ModelToolDefinition {
            tool_id: "inventory.count".to_owned(),
            provider_name: "inventory_count".to_owned(),
            fingerprint: "b".repeat(64),
            description: "Count a bounded reviewed inventory.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {"query": {"type": "string", "maxLength": 100}},
                "required": ["query"],
                "additionalProperties": false
            }),
            strict: true,
        }
    }

    struct FakeDynamicResponder;

    #[async_trait]
    impl ProviderDynamicToolResponder for FakeDynamicResponder {
        async fn respond(
            &self,
            call: ProviderDynamicToolCall,
        ) -> Result<ProviderDynamicToolResult, ProviderError> {
            if call.response_id() != "turn-dynamic-1"
                || call.call_id() != "call-dynamic-1"
                || call.tool_id() != "inventory.count"
                || call.provider_name() != "inventory_count"
                || call.tool_fingerprint() != "b".repeat(64)
                || call.arguments() != &json!({"query": "bounded"})
            {
                return Err(ProviderError::Rejected);
            }
            ProviderDynamicToolResult::new(&call, json!({"count": 3}))
        }
    }

    #[test]
    fn registration_binds_supported_protocol_and_all_immutable_identity_members() {
        let first = registration("1.0.0");
        let same = registration("1.0.0");
        let changed = registration("2.0.0");
        assert_eq!(first.identity(), same.identity());
        assert_ne!(first.identity(), changed.identity());
        assert_ne!(first.identity(), dynamic_registration("1.0.0").identity());
        assert!(matches!(
            AiCodexAppServerRegistration::new(
                "profile-1",
                "model-1",
                "a".repeat(64),
                "1.0.0",
                "sandbox-empty",
                "future-protocol",
            ),
            Err(ProviderError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn provider_advertises_only_implemented_retained_local_capabilities() {
        let counters = Arc::new(Counters::new());
        let provider = AiCodexAppServerProvider::new(registration("1.0.0"), pool(counters, 1, 2));
        let capabilities = provider.capabilities();
        assert!(capabilities.streaming);
        assert!(capabilities.local);
        assert!(capabilities.provider_retained_continuation);
        assert!(!capabilities.custom_tools);
        assert!(!capabilities.stateless_continuation);
        assert!(!capabilities.web_search);
        assert!(!capabilities.code_execution);

        let dynamic_provider = AiCodexAppServerProvider::new(
            dynamic_registration("1.0.0"),
            pool(Arc::new(Counters::new()), 1, 2),
        );
        assert!(dynamic_provider.capabilities().custom_tools);
        assert!(
            dynamic_provider
                .capabilities()
                .provider_retained_continuation
        );
    }

    fn turn() -> AiCodexAppServerTurnInput {
        AiCodexAppServerTurnInput::new(
            "model-1",
            vec!["trusted".to_owned()],
            vec!["hello".to_owned()],
            128,
        )
        .expect("test turn should validate")
    }

    fn model_request() -> ModelRequest {
        ModelRequest {
            model: "model-1".to_owned(),
            instructions: vec!["trusted".to_owned()],
            input: vec![ModelInputBlock::Text {
                text: "hello".to_owned(),
            }],
            continuation: None,
            continuation_mode: ModelContinuationMode::StatelessReplay,
            tools: Vec::new(),
            builtin_tools: Vec::new(),
            maximum_builtin_tool_calls: None,
            reasoning_summary: ModelReasoningSummaryRequest::Disabled,
            output_schema: None,
            maximum_output_tokens: Some(128),
        }
    }

    fn dynamic_model_request() -> ModelRequest {
        ModelRequest {
            continuation_mode: ModelContinuationMode::ProviderRetained,
            tools: vec![dynamic_tool()],
            ..model_request()
        }
    }

    fn initialized_protocol_actor() -> AiCodexAppServerProtocolActor {
        let mut actor =
            AiCodexAppServerProtocolActor::new(64 * 1024).expect("test guard should validate");
        actor
            .initialize("test_client", "Test Client", "0.147.0")
            .expect("initialize should encode");
        assert!(matches!(
            actor.accept(br#"{"id":1,"result":{"userAgent":"test"}}"#),
            Ok(AiCodexAppServerInbound::Response {
                method: "initialize",
                ..
            })
        ));
        actor
            .initialized()
            .expect("initialized notification should encode");
        actor
    }

    fn remote_control_status(status: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "method": REMOTE_CONTROL_STATUS_CHANGED,
            "params": {
                "status": status,
                "serverName": "development",
                "installationId": "84b5c758-086b-41ec-832e-3cfb72779186",
                "environmentId": null,
            },
            "emittedAtMs": 1_786_484_733_704_u64,
        }))
        .expect("status notification should encode")
    }

    fn lifecycle_notification(method: &str, params: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "emittedAtMs": 1_786_484_733_705_i64,
            "method": method,
            "params": params,
        }))
        .expect("lifecycle notification should encode")
    }

    fn thread_started_notification(thread_id: &str) -> Vec<u8> {
        lifecycle_notification("thread/started", json!({"thread": {"id": thread_id}}))
    }

    fn thread_not_loaded_notification(thread_id: &str) -> Vec<u8> {
        lifecycle_notification(
            "thread/status/changed",
            json!({"threadId": thread_id, "status": {"type": "notLoaded"}}),
        )
    }

    fn turn_started_notification(thread_id: &str, turn_id: &str) -> Vec<u8> {
        lifecycle_notification(
            "turn/started",
            json!({
                "threadId": thread_id,
                "turn": {"id": turn_id, "items": [], "status": "inProgress"},
            }),
        )
    }

    fn turn_completed_notification(thread_id: &str, turn_id: &str) -> Vec<u8> {
        lifecycle_notification(
            "turn/completed",
            json!({
                "threadId": thread_id,
                "turn": {"id": turn_id, "items": [], "status": "completed"},
            }),
        )
    }

    fn active_protocol_actor() -> AiCodexAppServerProtocolActor {
        let mut actor = initialized_protocol_actor();
        actor
            .start_fresh_thread(&turn())
            .expect("thread start should encode");
        actor
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
            .expect("thread response should bind");
        actor
            .accept(&thread_started_notification("thread-1"))
            .expect("thread notification should bind");
        actor
            .start_turn("thread-1", &turn())
            .expect("turn start should encode");
        actor
            .accept(br#"{"id":3,"result":{"turn":{"id":"turn-1"}}}"#)
            .expect("turn response should bind");
        actor
            .accept(&turn_started_notification("thread-1", "turn-1"))
            .expect("turn notification should bind");
        actor
    }

    fn deleting_protocol_actor() -> AiCodexAppServerProtocolActor {
        let mut actor = initialized_protocol_actor();
        actor
            .start_fresh_thread(&turn())
            .expect("thread start should encode");
        actor
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
            .expect("thread response should bind");
        actor
            .accept(&thread_started_notification("thread-1"))
            .expect("thread notification should bind");
        let cursor = crate::AiProviderSessionCursor::new("codex.app_server.thread.v2", "thread-1")
            .expect("cursor should validate");
        actor.delete_thread(&cursor).expect("delete should encode");
        actor
    }

    fn provider_context(profile_id: &str, request: &ModelRequest) -> ProviderRequestContext {
        let session_id = AiSessionId::new();
        let run_id = AiRunId::new();
        let attempt_id = Uuid::new_v4();
        let manifest = crate::AiEgressManifest {
            provider_profile_id: profile_id.to_owned(),
            provider_kind: ProviderKind::LocalHarness.as_str().to_owned(),
            model: request.model.clone(),
            destination: "local-codex".to_owned(),
            destination_trust: crate::AiDestinationTrust::Local,
            capability: crate::AiEgressCapability::ModelInference,
            scope: crate::AiScope::new("project", "test"),
            session_id: Some(session_id),
            run_id: Some(run_id),
            sources: vec![crate::AiDataSourceRef {
                kind: "message".to_owned(),
                reference: "synthetic".to_owned(),
                classification: crate::DataClassification::Public,
                trust: crate::AiSourceTrust::UserProvided,
            }],
            estimated_bytes: request.conservative_egress_bytes(),
            estimated_tokens: 100,
            attachment_count: 0,
            purpose: "test".to_owned(),
            retention: "none".to_owned(),
            residency: None,
            policy_version: "test".to_owned(),
            consent_reference: None,
        };
        let proof = crate::AiEgressDecision::allow(&manifest, "test", "test-user")
            .authorize(&manifest)
            .expect("manifest should authorize");
        let budget = crate::AiBudgetReservation::new_reserved(
            crate::AiBudgetReservationId::new(),
            run_id,
            attempt_id,
            1,
            ProviderKind::LocalHarness,
            &request.model,
            "test-pricing-v1",
            crate::AiBudgetAmounts {
                input_tokens: 1_000,
                output_tokens: 1_000,
                runs: 1,
                ..crate::AiBudgetAmounts::default()
            },
            time::OffsetDateTime::now_utc() + time::Duration::hours(1),
        )
        .expect("budget should validate")
        .authorize_provider_call(
            run_id,
            attempt_id,
            1,
            &ProviderKind::LocalHarness,
            &request.model,
            request.maximum_output_tokens.unwrap_or_default(),
            0,
            time::OffsetDateTime::now_utc(),
        )
        .expect("budget should authorize");
        ProviderRequestContext::new(session_id, run_id, "test", budget, manifest, proof)
            .expect("context should validate")
            .with_run_binding(
                AiProviderRunBinding::new_for_principal_reference(
                    session_id,
                    run_id,
                    attempt_id,
                    1,
                    &principal_reference(),
                )
                .expect("binding should validate"),
            )
            .expect("binding should match context")
    }

    fn pool(
        counters: Arc<Counters>,
        maximum_processes: usize,
        maximum_turns: u32,
    ) -> AiCodexAppServerRunPool {
        let limits = AiCodexAppServerRunLimits::new(
            maximum_processes,
            maximum_turns,
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("test limits should validate");
        AiCodexAppServerRunPool::new(Arc::new(FakeFactory { counters }), limits)
    }

    #[tokio::test]
    async fn one_exact_binding_reuses_one_process_across_fresh_turns() {
        let counters = Arc::new(Counters::new());
        let pool = pool(counters.clone(), 2, 4);
        let binding = binding();
        for _ in 0..2 {
            let events = pool
                .start_fresh_turn(binding, registration("1.0.0"), turn())
                .await
                .expect("turn should start")
                .collect::<Vec<_>>()
                .await;
            assert_eq!(events.len(), 4);
        }
        assert_eq!(counters.launches.load(Ordering::SeqCst), 1);
        assert_eq!(counters.turns.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn empty_retained_thread_binds_exact_dynamic_definitions_before_content() {
        let counters = Arc::new(Counters::new());
        let pool = pool(counters.clone(), 1, 2);
        let binding = binding();
        let cursor = pool
            .create_empty_thread(binding, dynamic_registration("1.0.0"), vec![dynamic_tool()])
            .await
            .expect("empty retained thread should create");
        assert_eq!(cursor.kind(), "codex.app_server.thread.v2");
        assert_eq!(counters.created_threads.load(Ordering::SeqCst), 1);
        assert_eq!(counters.created_dynamic_tools.load(Ordering::SeqCst), 1);
        pool.discard_empty_thread(binding, &cursor)
            .await
            .expect("failed durable bind should delete exact empty thread");
        assert_eq!(counters.deleted_threads.load(Ordering::SeqCst), 1);

        assert!(matches!(
            pool.create_empty_thread(binding, registration("1.0.0"), vec![dynamic_tool()])
                .await,
            Err(ProviderError::Unsupported)
        ));
    }

    #[tokio::test]
    async fn newly_bound_dynamic_turn_uses_creating_process_without_resume() {
        let counters = Arc::new(Counters::new());
        let pool = pool(counters.clone(), 1, 2);
        let binding = binding();
        let registration = dynamic_registration("1.0.0");
        let cursor = pool
            .create_empty_thread(binding, registration.clone(), vec![dynamic_tool()])
            .await
            .expect("retained dynamic thread should create");
        let opened = opened_session(binding, &registration, cursor.clone())
            .activate_newly_bound_empty(binding, &cursor)
            .expect("executor activation should match the exact cursor and run");
        let input = AiCodexAppServerTurnInput::try_from_dynamic_request(dynamic_model_request())
            .expect("dynamic request should convert");
        let events = pool
            .start_bound_dynamic_turn(
                binding,
                registration,
                opened,
                input,
                Arc::new(FakeDynamicResponder),
            )
            .await
            .expect("retained dynamic turn should start")
            .collect::<Vec<_>>()
            .await;
        assert_eq!(events.len(), 7);
        assert!(events.iter().all(Result::is_ok));
        assert_eq!(counters.launches.load(Ordering::SeqCst), 1);
        assert_eq!(counters.created_threads.load(Ordering::SeqCst), 1);
        assert_eq!(counters.bound_turns.load(Ordering::SeqCst), 1);
        assert_eq!(counters.retained_turns.load(Ordering::SeqCst), 0);
        assert_eq!(counters.turns.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn provider_dispatches_executor_marked_session_to_initial_direct_turn() {
        let counters = Arc::new(Counters::new());
        let registration = dynamic_registration("1.0.0");
        let provider =
            AiCodexAppServerProvider::new(registration.clone(), pool(counters.clone(), 1, 2));
        let request = dynamic_model_request();
        let context = provider_context(registration.provider_profile_id(), &request);
        let binding = context
            .run_binding()
            .expect("executor context should carry the exact run binding");
        let descriptor = crate::AiProviderSessionDescriptor::new(
            ProviderKind::LocalHarness,
            registration.provider_profile_id(),
            registration.logical_model(),
            registration.identity(),
            registration.protocol_version(),
            "d".repeat(64),
        )
        .expect("descriptor should validate");
        let cursor = provider
            .create_empty_session(&binding, &descriptor, &request)
            .await
            .expect("provider should create an empty thread");
        let opened = opened_session(binding, &registration, cursor.clone())
            .activate_newly_bound_empty(binding, &cursor)
            .expect("executor should mark only the exact new binding");
        let context = context
            .with_provider_session(opened)
            .expect("opened provider session should match the run context");
        let events = provider
            .stream_with_dynamic_tools(request, context, Arc::new(FakeDynamicResponder))
            .await
            .expect("provider should dispatch initial activation directly")
            .collect::<Vec<_>>()
            .await;
        assert_eq!(events.len(), 7);
        assert_eq!(counters.launches.load(Ordering::SeqCst), 1);
        assert_eq!(counters.bound_turns.load(Ordering::SeqCst), 1);
        assert_eq!(counters.retained_turns.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn provider_dispatches_tool_free_newly_bound_session_directly() {
        let counters = Arc::new(Counters::new());
        let registration = registration("1.0.0");
        let provider =
            AiCodexAppServerProvider::new(registration.clone(), pool(counters.clone(), 1, 2));
        let mut request = model_request();
        request.continuation_mode = ModelContinuationMode::ProviderRetained;
        let context = provider_context(registration.provider_profile_id(), &request);
        let binding = context
            .run_binding()
            .expect("executor context should carry the exact run binding");
        let descriptor = crate::AiProviderSessionDescriptor::new(
            ProviderKind::LocalHarness,
            registration.provider_profile_id(),
            registration.logical_model(),
            registration.identity(),
            registration.protocol_version(),
            "d".repeat(64),
        )
        .expect("descriptor should validate");
        let cursor = provider
            .create_empty_session(&binding, &descriptor, &request)
            .await
            .expect("provider should create a tool-free empty thread");
        let opened = opened_session(binding, &registration, cursor.clone())
            .activate_newly_bound_empty(binding, &cursor)
            .expect("executor should mark only the exact new binding");
        let context = context
            .with_provider_session(opened)
            .expect("opened provider session should match the run context");
        let events = provider
            .stream(request, context)
            .await
            .expect("provider should dispatch tool-free activation directly")
            .collect::<Vec<_>>()
            .await;
        assert_eq!(events.len(), 4);
        assert_eq!(counters.launches.load(Ordering::SeqCst), 1);
        assert_eq!(counters.bound_turns.load(Ordering::SeqCst), 1);
        assert_eq!(counters.retained_turns.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn newly_bound_tool_free_activation_is_exact_and_one_shot() {
        let counters = Arc::new(Counters::new());
        let pool = pool(counters.clone(), 2, 3);
        let binding = binding();
        let registration = registration("1.0.0");
        let cursor = pool
            .create_empty_thread(binding, registration.clone(), Vec::new())
            .await
            .expect("empty retained thread should create");
        let opened = opened_session(binding, &registration, cursor.clone())
            .activate_newly_bound_empty(binding, &cursor)
            .expect("executor activation should match");
        let replay = opened.clone();
        let events = pool
            .start_bound_turn(binding, registration.clone(), opened, turn())
            .await
            .expect("initial bound turn should start directly")
            .collect::<Vec<_>>()
            .await;
        assert_eq!(events.len(), 4);
        assert_eq!(counters.launches.load(Ordering::SeqCst), 1);
        assert_eq!(counters.bound_turns.load(Ordering::SeqCst), 1);
        assert_eq!(counters.retained_turns.load(Ordering::SeqCst), 0);
        assert!(matches!(
            pool.start_bound_turn(binding, registration, replay, turn())
                .await,
            Err(ProviderError::Rejected)
        ));
    }

    #[tokio::test]
    async fn newly_bound_activation_rejects_cursor_process_and_tool_swaps() {
        let counters = Arc::new(Counters::new());
        let primary_pool = pool(counters.clone(), 2, 3);
        let binding = binding();
        let registration = dynamic_registration("1.0.0");
        let created = primary_pool
            .create_empty_thread(binding, registration.clone(), vec![dynamic_tool()])
            .await
            .expect("empty retained thread should create");
        let other_binding = AiProviderRunBinding::new_for_principal_reference(
            binding.session_id(),
            binding.run_id(),
            binding.attempt_id(),
            binding.lease_generation(),
            &principal_reference_for_owner(2),
        )
        .expect("same fence with another owner should construct for rejection testing");
        assert!(matches!(
            opened_session(binding, &registration, created.clone())
                .activate_newly_bound_empty(other_binding, &created),
            Err(crate::AiError::Conflict)
        ));
        let swapped = crate::AiProviderSessionCursor::new(
            "codex.app_server.thread.v2",
            "thread-retained-swapped",
        )
        .expect("swapped cursor should be structurally valid");
        let opened = opened_session(binding, &registration, swapped.clone())
            .activate_newly_bound_empty(binding, &swapped)
            .expect("crate marker alone is not the process correlation proof");
        let input = AiCodexAppServerTurnInput::try_from_dynamic_request(dynamic_model_request())
            .expect("dynamic input should validate");
        assert!(matches!(
            primary_pool
                .start_bound_dynamic_turn(
                    binding,
                    registration.clone(),
                    opened,
                    input,
                    Arc::new(FakeDynamicResponder),
                )
                .await,
            Err(ProviderError::Rejected)
        ));
        assert_eq!(counters.bound_turns.load(Ordering::SeqCst), 0);

        let other_counters = Arc::new(Counters::new());
        let other_pool = pool(other_counters.clone(), 1, 2);
        let opened = opened_session(binding, &registration, created.clone())
            .activate_newly_bound_empty(binding, &created)
            .expect("exact activation should validate");
        assert!(matches!(
            other_pool
                .start_bound_dynamic_turn(
                    binding,
                    registration,
                    opened,
                    AiCodexAppServerTurnInput::try_from_dynamic_request(dynamic_model_request())
                        .expect("dynamic input should validate"),
                    Arc::new(FakeDynamicResponder),
                )
                .await,
            Err(ProviderError::Rejected)
        ));
        assert_eq!(other_counters.launches.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn later_run_resumes_committed_cursor_on_a_new_process() {
        let counters = Arc::new(Counters::new());
        let pool = pool(counters.clone(), 1, 2);
        let later_binding = binding_for_owner(1);
        let registration = dynamic_registration("1.0.0");
        let cursor = crate::AiProviderSessionCursor::new(
            "codex.app_server.thread.v2",
            "thread-retained-test",
        )
        .expect("committed cursor should validate");
        let opened = opened_session(later_binding, &registration, cursor);
        let input = AiCodexAppServerTurnInput::try_from_dynamic_request(dynamic_model_request())
            .expect("dynamic input should validate");
        let events = pool
            .start_retained_dynamic_turn(
                later_binding,
                registration,
                opened,
                input,
                Arc::new(FakeDynamicResponder),
            )
            .await
            .expect("later retained claim should use resume path")
            .collect::<Vec<_>>()
            .await;
        assert_eq!(events.len(), 7);
        assert_eq!(counters.launches.load(Ordering::SeqCst), 1);
        assert_eq!(counters.bound_turns.load(Ordering::SeqCst), 0);
        assert_eq!(counters.retained_turns.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn newly_bound_activation_rejects_changed_frozen_tool_definition() {
        let counters = Arc::new(Counters::new());
        let pool = pool(counters.clone(), 1, 2);
        let binding = binding();
        let registration = dynamic_registration("1.0.0");
        let cursor = pool
            .create_empty_thread(binding, registration.clone(), vec![dynamic_tool()])
            .await
            .expect("empty dynamic thread should create");
        let opened = opened_session(binding, &registration, cursor.clone())
            .activate_newly_bound_empty(binding, &cursor)
            .expect("exact activation should validate");
        let mut request = dynamic_model_request();
        request.tools[0].description = "Changed after durable binding.".to_owned();
        let changed = AiCodexAppServerTurnInput::try_from_dynamic_request(request)
            .expect("changed definition remains structurally valid");
        assert!(matches!(
            pool.start_bound_dynamic_turn(
                binding,
                registration,
                opened,
                changed,
                Arc::new(FakeDynamicResponder),
            )
            .await,
            Err(ProviderError::Rejected)
        ));
        assert_eq!(counters.bound_turns.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn close_before_newly_bound_turn_prevents_business_content() {
        let counters = Arc::new(Counters::new());
        let pool = pool(counters.clone(), 1, 2);
        let binding = binding();
        let registration = registration("1.0.0");
        let cursor = pool
            .create_empty_thread(binding, registration.clone(), Vec::new())
            .await
            .expect("empty thread should create");
        let opened = opened_session(binding, &registration, cursor.clone())
            .activate_newly_bound_empty(binding, &cursor)
            .expect("activation should validate");
        assert_eq!(
            pool.close_run(&binding, AiProviderRunCloseReason::Cancelled)
                .await
                .expect("exact cancellation should close the process"),
            AiProviderRunCloseOutcome::Closed
        );
        assert!(matches!(
            pool.start_bound_turn(binding, registration, opened, turn())
                .await,
            Err(ProviderError::Rejected)
        ));
        assert_eq!(counters.bound_turns.load(Ordering::SeqCst), 0);
        assert_eq!(counters.turns.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn model_request_conversion_is_closed_and_preserves_authority_boundaries() {
        let converted = AiCodexAppServerTurnInput::try_from_model_request(model_request())
            .expect("text-only request should convert");
        assert_eq!(converted.instructions(), &["trusted"]);
        assert_eq!(converted.input(), &["hello"]);
        assert!(!format!("{converted:?}").contains("trusted"));
        assert!(!format!("{converted:?}").contains("hello"));

        let mut json = model_request();
        json.input = vec![ModelInputBlock::Json {
            value: json!({"unreviewed": true}),
        }];
        assert!(matches!(
            AiCodexAppServerTurnInput::try_from_model_request(json),
            Err(ProviderError::Unsupported)
        ));

        let mut retained = model_request();
        retained.continuation_mode = ModelContinuationMode::ProviderRetained;
        assert!(matches!(
            AiCodexAppServerTurnInput::try_from_model_request(retained.clone()),
            Err(ProviderError::Unsupported)
        ));
        AiCodexAppServerTurnInput::try_from_retained_model_request(retained)
            .expect("tool-free retained initial input should use its explicit converter");

        let mut reasoning = model_request();
        reasoning.reasoning_summary = ModelReasoningSummaryRequest::auto(1_024)
            .expect("test summary request should validate");
        assert!(matches!(
            AiCodexAppServerTurnInput::try_from_model_request(reasoning),
            Err(ProviderError::Unsupported)
        ));
    }

    #[tokio::test]
    async fn provider_rejects_profile_swap_before_process_launch() {
        let counters = Arc::new(Counters::new());
        let provider =
            AiCodexAppServerProvider::new(registration("1.0.0"), pool(counters.clone(), 1, 2));
        let request = model_request();
        let context = provider_context("another-profile", &request);
        assert!(matches!(
            provider.stream(request, context).await,
            Err(ProviderError::EgressDenied)
        ));
        assert_eq!(counters.launches.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn experimental_dynamic_tools_are_explicit_and_process_bounded() {
        let counters = Arc::new(Counters::new());
        let request = dynamic_model_request();
        let context = provider_context("profile-1", &request);
        let disabled =
            AiCodexAppServerProvider::new(registration("1.0.0"), pool(counters.clone(), 1, 2));
        assert!(matches!(
            disabled
                .stream_with_dynamic_tools(
                    request.clone(),
                    context.clone(),
                    Arc::new(FakeDynamicResponder),
                )
                .await,
            Err(ProviderError::Unsupported)
        ));
        assert_eq!(counters.launches.load(Ordering::SeqCst), 0);

        let enabled = AiCodexAppServerProvider::new(
            dynamic_registration("1.0.0"),
            pool(counters.clone(), 1, 2),
        );
        let events = enabled
            .stream_with_dynamic_tools(request, context, Arc::new(FakeDynamicResponder))
            .await
            .expect("explicit dynamic turn should start")
            .collect::<Vec<_>>()
            .await;
        assert_eq!(events.len(), 7);
        assert!(events.iter().all(Result::is_ok));
        assert_eq!(counters.launches.load(Ordering::SeqCst), 1);
        assert_eq!(counters.turns.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn provider_trait_dispatches_exact_interrupt_and_terminal_close() {
        let counters = Arc::new(Counters::new());
        counters.pending.store(true, Ordering::SeqCst);
        let provider: Arc<dyn AiProvider> = Arc::new(AiCodexAppServerProvider::new(
            registration("1.0.0"),
            pool(counters.clone(), 1, 2),
        ));
        let request = model_request();
        let context = provider_context("profile-1", &request);
        let binding = context
            .run_binding()
            .expect("test context should carry the exact run binding");
        let active = provider
            .stream(request, context)
            .await
            .expect("provider turn should start");
        assert_eq!(
            provider
                .interrupt_run(&binding)
                .await
                .expect("interrupt should dispatch"),
            AiProviderRunInterruptOutcome::Requested
        );
        assert_eq!(
            provider
                .close_run(&binding, AiProviderRunCloseReason::Cancelled)
                .await
                .expect("close should dispatch"),
            AiProviderRunCloseOutcome::Closed
        );
        drop(active);
        assert_eq!(counters.interrupts.load(Ordering::SeqCst), 1);
        assert_eq!(counters.shutdowns.load(Ordering::SeqCst), 1);
        assert_eq!(counters.kills.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn registration_swap_and_concurrent_turn_fail_closed() {
        let counters = Arc::new(Counters::new());
        counters.pending.store(true, Ordering::SeqCst);
        let pool = pool(counters, 2, 4);
        let binding = binding();
        let active = pool
            .start_fresh_turn(binding, registration("1.0.0"), turn())
            .await
            .expect("first turn should start");
        assert!(matches!(
            pool.start_fresh_turn(binding, registration("1.0.0"), turn())
                .await,
            Err(ProviderError::Rejected)
        ));
        drop(active);
        assert!(matches!(
            pool.start_fresh_turn(binding, registration("2.0.0"), turn())
                .await,
            Err(ProviderError::Rejected)
        ));
    }

    #[tokio::test]
    async fn stream_failure_invalidates_process_before_retry() {
        let counters = Arc::new(Counters::new());
        counters.stream_error.store(true, Ordering::SeqCst);
        let pool = pool(counters.clone(), 2, 4);
        let binding = binding();
        let events = pool
            .start_fresh_turn(binding, registration("1.0.0"), turn())
            .await
            .expect("turn should start")
            .collect::<Vec<_>>()
            .await;
        assert!(matches!(events.as_slice(), [Err(ProviderError::Rejected)]));
        assert_eq!(counters.shutdowns.load(Ordering::SeqCst), 1);

        counters.stream_error.store(false, Ordering::SeqCst);
        pool.start_fresh_turn(binding, registration("1.0.0"), turn())
            .await
            .expect("retry should use a replacement process")
            .collect::<Vec<_>>()
            .await;
        assert_eq!(counters.launches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn capacity_and_turn_limits_apply_without_extra_launches() {
        let counters = Arc::new(Counters::new());
        let pool = pool(counters.clone(), 1, 1);
        let first = binding();
        pool.start_fresh_turn(first, registration("1.0.0"), turn())
            .await
            .expect("first turn should start")
            .collect::<Vec<_>>()
            .await;
        assert!(matches!(
            pool.start_fresh_turn(first, registration("1.0.0"), turn())
                .await,
            Err(ProviderError::RateLimited)
        ));
        let second = binding();
        assert!(matches!(
            pool.start_fresh_turn(second, registration("1.0.0"), turn())
                .await,
            Err(ProviderError::RateLimited)
        ));
        assert_eq!(counters.launches.load(Ordering::SeqCst), 1);

        pool.close_run(&first, AiProviderRunCloseReason::Completed)
            .await
            .expect("closing the admitted run should succeed");
        pool.start_fresh_turn(second, registration("2.0.0"), turn())
            .await
            .expect("rejected admission must not freeze a registration identity")
            .collect::<Vec<_>>()
            .await;
        assert_eq!(counters.launches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn per_owner_admission_prevents_one_subject_from_exhausting_the_pool() {
        let counters = Arc::new(Counters::new());
        let limits = AiCodexAppServerRunLimits::new(
            3,
            2,
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("test limits should validate")
        .with_maximum_processes_per_owner(1)
        .expect("per-owner limit should validate");
        let pool = AiCodexAppServerRunPool::new(
            Arc::new(FakeFactory {
                counters: counters.clone(),
            }),
            limits,
        );
        pool.start_fresh_turn(binding_for_owner(1), registration("1.0.0"), turn())
            .await
            .expect("first owner turn should start")
            .collect::<Vec<_>>()
            .await;
        assert!(matches!(
            pool.start_fresh_turn(binding_for_owner(1), registration("1.0.0"), turn())
                .await,
            Err(ProviderError::RateLimited)
        ));
        pool.start_fresh_turn(binding_for_owner(2), registration("1.0.0"), turn())
            .await
            .expect("another owner should retain independent capacity")
            .collect::<Vec<_>>()
            .await;
        assert_eq!(counters.launches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn interrupt_and_close_are_exact_and_idempotent() {
        let counters = Arc::new(Counters::new());
        counters.pending.store(true, Ordering::SeqCst);
        let pool = pool(counters.clone(), 1, 2);
        let binding = binding();
        let active = pool
            .start_fresh_turn(binding, registration("1.0.0"), turn())
            .await
            .expect("turn should start");
        assert_eq!(
            pool.interrupt_run(&binding)
                .await
                .expect("interrupt should succeed"),
            AiProviderRunInterruptOutcome::Requested
        );
        assert_eq!(counters.interrupts.load(Ordering::SeqCst), 1);
        assert_eq!(
            pool.close_run(&binding, AiProviderRunCloseReason::Cancelled)
                .await
                .expect("close should succeed"),
            AiProviderRunCloseOutcome::Closed
        );
        assert_eq!(
            pool.close_run(&binding, AiProviderRunCloseReason::Cancelled)
                .await
                .expect("duplicate close should be inert"),
            AiProviderRunCloseOutcome::NotActive
        );
        assert_eq!(counters.shutdowns.load(Ordering::SeqCst), 1);
        drop(active);
        assert_eq!(counters.kills.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn final_pool_drop_synchronously_invokes_process_tree_kill() {
        let counters = Arc::new(Counters::new());
        let pool = pool(counters.clone(), 1, 2);
        let binding = binding();
        pool.start_fresh_turn(binding, registration("1.0.0"), turn())
            .await
            .expect("turn should start")
            .collect::<Vec<_>>()
            .await;
        assert_eq!(counters.kills.load(Ordering::SeqCst), 0);
        drop(pool);
        assert_eq!(counters.kills.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn protocol_encoder_has_no_generic_or_experimental_method() {
        let mut guard =
            AiCodexAppServerProtocolActor::new(16 * 1024).expect("test guard should validate");
        let initialize = String::from_utf8(
            guard
                .initialize("test_client", "Test Client", "1.0.0")
                .expect("initialize should encode"),
        )
        .expect("frame should be UTF-8");
        guard
            .accept(br#"{"id":1,"result":{"userAgent":"test"}}"#)
            .expect("initialize response should bind");
        let thread = String::from_utf8(
            guard
                .start_fresh_thread(&turn())
                .expect("thread should encode"),
        )
        .expect("frame should be UTF-8");
        guard
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
            .expect("thread response should bind the exact thread");
        guard
            .accept(&thread_started_notification("thread-1"))
            .expect("thread notification should bind the exact thread");
        let turn = String::from_utf8(
            guard
                .start_turn("thread-1", &turn())
                .expect("turn should encode"),
        )
        .expect("frame should be UTF-8");
        assert!(initialize.contains("\"method\":\"initialize\""));
        assert!(!initialize.contains("experimentalApi"));
        assert!(thread.contains("\"ephemeral\":true"));
        assert!(thread.contains("\"developerInstructions\":\"trusted\""));
        assert!(thread.contains("\"approvalPolicy\":\"never\""));
        assert!(thread.contains("\"sandbox\":\"read-only\""));
        assert!(!thread.contains("dynamicTools"));
        assert!(!turn.contains("trusted"));
        assert!(!turn.contains("\"model\""));
    }

    #[test]
    fn protocol_admits_only_disabled_remote_control_status_during_initialization() {
        let mut before_initialize =
            AiCodexAppServerProtocolActor::new(64 * 1024).expect("test guard should validate");
        assert!(matches!(
            before_initialize.accept(&remote_control_status("disabled")),
            Err(ProviderError::Rejected)
        ));

        let mut actor = initialized_protocol_actor();
        assert!(matches!(
            actor.accept(&remote_control_status("disabled")),
            Ok(AiCodexAppServerInbound::RemoteControlDisabled)
        ));
        let thread = String::from_utf8(
            actor
                .start_fresh_thread(&turn())
                .expect("locked-down thread start should encode"),
        )
        .expect("thread request should be UTF-8");
        assert!(thread.contains("\"approvalPolicy\":\"never\""));
        assert!(thread.contains("\"sandbox\":\"read-only\""));
        assert!(matches!(
            actor.accept(&remote_control_status("disabled")),
            Err(ProviderError::Rejected)
        ));

        let mut read_after_start_request = initialized_protocol_actor();
        read_after_start_request
            .start_fresh_thread(&turn())
            .expect("thread start should encode");
        assert!(matches!(
            read_after_start_request.accept(&remote_control_status("disabled")),
            Ok(AiCodexAppServerInbound::RemoteControlDisabled)
        ));
        read_after_start_request
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
            .expect("thread response should bind");
        assert!(matches!(
            read_after_start_request.accept(&remote_control_status("disabled")),
            Err(ProviderError::Rejected)
        ));

        let cursor =
            crate::AiProviderSessionCursor::new("codex.app_server.thread.v2", "thread-retained-1")
                .expect("retained cursor should validate");
        let mut read_after_resume_request = initialized_protocol_actor();
        read_after_resume_request
            .resume_thread(&cursor, &turn())
            .expect("thread resume should encode");
        assert!(matches!(
            read_after_resume_request.accept(&remote_control_status("disabled")),
            Ok(AiCodexAppServerInbound::RemoteControlDisabled)
        ));
        read_after_resume_request
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("resume response should bind");
    }

    #[test]
    fn protocol_rejects_non_disabled_or_malformed_remote_control_status() {
        for status in [
            "connecting",
            "connected",
            "errored",
            "enabled",
            "error",
            "unknown",
        ] {
            let mut actor = initialized_protocol_actor();
            assert!(matches!(
                actor.accept(&remote_control_status(status)),
                Err(ProviderError::Rejected)
            ));
        }

        let invalid_values = [
            json!({
                "method": REMOTE_CONTROL_STATUS_CHANGED,
                "params": {
                    "status": "disabled",
                    "serverName": "development",
                    "installationId": "installation-1",
                    "environmentId": "remote-environment",
                },
                "emittedAtMs": 1,
            }),
            json!({
                "method": REMOTE_CONTROL_STATUS_CHANGED,
                "params": {
                    "status": "disabled",
                    "serverName": "development",
                    "installationId": "installation-1",
                    "environmentId": null,
                    "enabled": false,
                },
                "emittedAtMs": 1,
            }),
            json!({
                "method": REMOTE_CONTROL_STATUS_CHANGED,
                "params": {
                    "status": "disabled",
                    "serverName": "development",
                    "installationId": "installation-1",
                    "environmentId": null,
                },
                "emittedAtMs": 1,
                "remoteControl": false,
            }),
            json!({
                "method": REMOTE_CONTROL_STATUS_CHANGED,
                "params": {
                    "status": "disabled",
                    "serverName": "not a bounded identifier",
                    "installationId": "installation-1",
                    "environmentId": null,
                },
                "emittedAtMs": 1,
            }),
            json!({
                "method": REMOTE_CONTROL_STATUS_CHANGED,
                "params": {
                    "status": "disabled",
                    "serverName": "development",
                    "installationId": "x".repeat(MAXIMUM_IDENTIFIER_BYTES + 1),
                    "environmentId": null,
                },
                "emittedAtMs": 1,
            }),
            json!({
                "method": REMOTE_CONTROL_STATUS_CHANGED,
                "params": {
                    "status": "disabled",
                    "serverName": "development",
                    "installationId": "installation-1",
                    "environmentId": null,
                },
                "emittedAtMs": 0,
            }),
            json!({
                "method": REMOTE_CONTROL_STATUS_CHANGED,
                "params": {
                    "status": "disabled",
                    "serverName": "development",
                    "installationId": "installation-1",
                },
                "emittedAtMs": 1,
            }),
        ];
        for invalid in invalid_values {
            let mut actor = initialized_protocol_actor();
            let frame = serde_json::to_vec(&invalid).expect("invalid fixture should encode");
            assert!(matches!(actor.accept(&frame), Err(ProviderError::Rejected)));
        }
    }

    #[test]
    fn protocol_accepts_timestamped_thread_started_in_either_correlated_order() {
        let mut response_first = initialized_protocol_actor();
        response_first
            .start_fresh_thread(&turn())
            .expect("thread start should encode");
        response_first
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
            .expect("thread response should bind");
        let started = thread_started_notification("thread-1");
        assert!(matches!(
            response_first.accept(&started),
            Ok(AiCodexAppServerInbound::Notification { method, params })
                if method == "thread/started"
                    && params.pointer("/thread/id").and_then(Value::as_str) == Some("thread-1")
                    && params.get("emittedAtMs").is_none()
        ));
        assert!(matches!(
            response_first.accept(&started),
            Err(ProviderError::Rejected)
        ));

        let mut notification_first = initialized_protocol_actor();
        notification_first
            .start_fresh_thread(&turn())
            .expect("thread start should encode");
        notification_first
            .accept(&thread_started_notification("thread-2"))
            .expect("notification may precede its correlated response");
        notification_first
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-2"}}}"#)
            .expect("matching response should bind after the notification");

        let cursor =
            crate::AiProviderSessionCursor::new("codex.app_server.thread.v2", "thread-retained-1")
                .expect("retained cursor should validate");
        let mut resumed = initialized_protocol_actor();
        resumed
            .resume_thread(&cursor, &turn())
            .expect("thread resume should encode");
        resumed
            .accept(&thread_started_notification("thread-retained-1"))
            .expect("resume notification may precede its response");
        resumed
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("resume response should match the protected cursor");
    }

    #[test]
    fn retained_actor_repeats_response_first_create_resume_and_turn_lifecycle() {
        let mut actor = initialized_protocol_actor();
        actor
            .start_persistent_empty_thread("model-1", &[])
            .expect("persistent create should encode");
        actor
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("create response should bind");
        actor
            .accept(&thread_started_notification("thread-retained-1"))
            .expect("create notification should complete the lifecycle");

        let cursor =
            crate::AiProviderSessionCursor::new("codex.app_server.thread.v2", "thread-retained-1")
                .expect("retained cursor should validate");
        actor
            .resume_thread(&cursor, &turn())
            .expect("the same actor should begin a new resume lifecycle");
        actor
            .accept(br#"{"id":3,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("resume response should belong to the new lifecycle");
        actor
            .accept(&thread_started_notification("thread-retained-1"))
            .expect("resume notification should complete the new lifecycle");
        actor
            .start_turn("thread-retained-1", &turn())
            .expect("turn should start only after both resume observations");
    }

    #[test]
    fn retained_actor_repeats_notification_first_create_and_resume_lifecycle() {
        let mut actor = initialized_protocol_actor();
        actor
            .start_persistent_empty_thread("model-1", &[])
            .expect("persistent create should encode");
        actor
            .accept(&thread_started_notification("thread-retained-1"))
            .expect("create notification may arrive first");
        let cursor =
            crate::AiProviderSessionCursor::new("codex.app_server.thread.v2", "thread-retained-1")
                .expect("retained cursor should validate");
        assert!(matches!(
            actor.resume_thread(&cursor, &turn()),
            Err(ProviderError::Rejected)
        ));
        actor
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("create response should complete the lifecycle");

        actor
            .resume_thread(&cursor, &turn())
            .expect("resume should begin a fresh observation phase");
        actor
            .accept(&thread_started_notification("thread-retained-1"))
            .expect("resume notification may arrive first");
        actor
            .accept(br#"{"id":3,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("resume response should complete the lifecycle");
        actor
            .start_turn("thread-retained-1", &turn())
            .expect("turn should start after notification-first correlation");
    }

    #[test]
    fn retained_actor_requires_each_lifecycle_pair_and_preserves_dynamic_definitions() {
        let input = AiCodexAppServerTurnInput::try_from_dynamic_request(dynamic_model_request())
            .expect("dynamic input should validate");
        let mut actor = initialized_protocol_actor();
        actor
            .start_persistent_empty_thread("model-1", input.tools())
            .expect("persistent dynamic create should encode");
        actor
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("create response should bind");

        let cursor =
            crate::AiProviderSessionCursor::new("codex.app_server.thread.v2", "thread-retained-1")
                .expect("retained cursor should validate");
        assert!(matches!(
            actor.resume_thread(&cursor, &input),
            Err(ProviderError::Rejected)
        ));
        actor
            .accept(&thread_started_notification("thread-retained-1"))
            .expect("missing create notification should complete the lifecycle");
        actor
            .resume_thread(&cursor, &input)
            .expect("complete create lifecycle should permit exact resume");
        actor
            .accept(br#"{"id":3,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("resume response should bind");
        assert!(matches!(
            actor.resume_thread(&cursor, &input),
            Err(ProviderError::Rejected)
        ));
        actor
            .accept(&thread_started_notification("thread-retained-1"))
            .expect("resume notification should complete the lifecycle");

        actor
            .start_turn("thread-retained-1", &input)
            .expect("first retained turn should encode");
        actor
            .accept(br#"{"id":4,"result":{"turn":{"id":"turn-retained-1"}}}"#)
            .expect("turn response should bind");
        actor
            .accept(&turn_started_notification(
                "thread-retained-1",
                "turn-retained-1",
            ))
            .expect("turn notification should bind");
        actor
            .accept(&turn_completed_notification(
                "thread-retained-1",
                "turn-retained-1",
            ))
            .expect("tool-free terminal turn should complete");

        let mut changed_request = dynamic_model_request();
        changed_request.tools[0].description = "Changed after binding.".to_owned();
        let changed_input = AiCodexAppServerTurnInput::try_from_dynamic_request(changed_request)
            .expect("changed definition remains structurally valid");
        assert!(matches!(
            actor.resume_thread(&cursor, &changed_input),
            Err(ProviderError::Rejected)
        ));
        actor
            .resume_thread(&cursor, &input)
            .expect("second exact resume should begin after terminal turn");
        actor
            .accept(br#"{"id":5,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("second resume response should bind");
        actor
            .accept(&thread_started_notification("thread-retained-1"))
            .expect("second resume notification should bind");
        assert!(matches!(
            actor.start_turn("thread-retained-1", &changed_input),
            Err(ProviderError::InvalidRequest)
        ));
        actor
            .start_turn("thread-retained-1", &input)
            .expect("frozen definitions should remain usable");
    }

    #[test]
    fn protocol_rejects_invalid_lifecycle_envelopes_and_thread_correlation() {
        let invalid_frames: [&[u8]; 7] = [
            br#"{"method":"thread/started","params":{"thread":{"id":"thread-1"}}}"#,
            br#"{"emittedAtMs":0,"method":"thread/started","params":{"thread":{"id":"thread-1"}}}"#,
            br#"{"emittedAtMs":-1,"method":"thread/started","params":{"thread":{"id":"thread-1"}}}"#,
            br#"{"emittedAtMs":9223372036854775808,"method":"thread/started","params":{"thread":{"id":"thread-1"}}}"#,
            br#"{"emittedAtMs":"1","method":"thread/started","params":{"thread":{"id":"thread-1"}}}"#,
            br#"{"emittedAtMs":1,"emittedAtMs":2,"method":"thread/started","params":{"thread":{"id":"thread-1"}}}"#,
            br#"{"emittedAtMs":1,"method":"thread/started","params":{"thread":{"id":"thread-1"}},"unexpected":true}"#,
        ];
        for frame in invalid_frames {
            let mut actor = initialized_protocol_actor();
            actor
                .start_fresh_thread(&turn())
                .expect("thread start should encode");
            assert!(matches!(actor.accept(frame), Err(ProviderError::Rejected)));
        }

        let mut unknown = initialized_protocol_actor();
        assert!(matches!(
            unknown.accept(&lifecycle_notification("thread/unknown", json!({}))),
            Err(ProviderError::Rejected)
        ));

        let mut response_first_mismatch = initialized_protocol_actor();
        response_first_mismatch
            .start_fresh_thread(&turn())
            .expect("thread start should encode");
        response_first_mismatch
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
            .expect("thread response should bind");
        assert!(matches!(
            response_first_mismatch.accept(&thread_started_notification("thread-other")),
            Err(ProviderError::Rejected)
        ));

        let mut notification_first_mismatch = initialized_protocol_actor();
        notification_first_mismatch
            .start_fresh_thread(&turn())
            .expect("thread start should encode");
        notification_first_mismatch
            .accept(&thread_started_notification("thread-other"))
            .expect("first notification binds its claimed thread");
        assert!(matches!(
            notification_first_mismatch
                .accept(br#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#),
            Err(ProviderError::Rejected)
        ));

        let mut after_delete = initialized_protocol_actor();
        after_delete
            .start_fresh_thread(&turn())
            .expect("thread start should encode");
        after_delete
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
            .expect("thread response should bind");
        after_delete
            .accept(&thread_started_notification("thread-1"))
            .expect("thread notification should bind");
        let cursor = crate::AiProviderSessionCursor::new("codex.app_server.thread.v2", "thread-1")
            .expect("cursor should validate");
        after_delete
            .delete_thread(&cursor)
            .expect("thread delete should encode");
        after_delete
            .accept(br#"{"id":3,"result":{}}"#)
            .expect("thread delete response should bind");
        assert!(matches!(
            after_delete.accept(&thread_started_notification("thread-1")),
            Err(ProviderError::Rejected)
        ));

        for status in ["idle", "systemError", "active", "unknown"] {
            let mut deleting = deleting_protocol_actor();
            let status = if status == "active" {
                json!({"type": status, "activeFlags": []})
            } else {
                json!({"type": status})
            };
            assert!(matches!(
                deleting.accept(&lifecycle_notification(
                    "thread/status/changed",
                    json!({"threadId": "thread-1", "status": status}),
                )),
                Err(ProviderError::Rejected)
            ));
        }
        let mut wrong_status_thread = deleting_protocol_actor();
        assert!(matches!(
            wrong_status_thread.accept(&thread_not_loaded_notification("thread-other")),
            Err(ProviderError::Rejected)
        ));
        let mut extra_status_field = deleting_protocol_actor();
        assert!(matches!(
            extra_status_field.accept(&lifecycle_notification(
                "thread/status/changed",
                json!({
                    "threadId": "thread-1",
                    "status": {"type": "notLoaded", "activeFlags": []},
                }),
            )),
            Err(ProviderError::Rejected)
        ));
    }

    #[test]
    fn protocol_rejects_every_forbidden_item_and_server_request() {
        let forbidden_items = [
            "commandExecution",
            "fileChange",
            "mcpToolCall",
            "dynamicToolCall",
            "collabToolCall",
            "webSearch",
            "imageView",
            "reasoning",
        ];
        for item_type in forbidden_items {
            let mut guard = active_protocol_actor();
            let frame = lifecycle_notification(
                "item/started",
                json!({
                    "item": {"type": item_type, "id": "item-1"},
                    "startedAtMs": 1,
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                }),
            );
            assert!(matches!(guard.accept(&frame), Err(ProviderError::Rejected)));
        }

        let mut guard =
            AiCodexAppServerProtocolActor::new(16 * 1024).expect("test guard should validate");
        let server_request = br#"{"id":7,"method":"item/tool/call","params":{}}"#;
        assert!(matches!(
            guard.accept(server_request),
            Err(ProviderError::Rejected)
        ));
    }

    #[test]
    fn protocol_accepts_only_correlated_responses_and_allowlisted_notifications() {
        let mut unbound =
            AiCodexAppServerProtocolActor::new(16 * 1024).expect("test guard should validate");
        let delta = lifecycle_notification(
            "item/agentMessage/delta",
            json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-1",
                "delta": "hello",
            }),
        );
        assert!(matches!(
            unbound.accept(&delta),
            Err(ProviderError::Rejected)
        ));

        let mut guard =
            AiCodexAppServerProtocolActor::new(16 * 1024).expect("test guard should validate");
        guard
            .initialize("test_client", "Test Client", "1.0.0")
            .expect("initialize should encode");
        let response = br#"{"id":1,"result":{"userAgent":"test"}}"#;
        assert!(matches!(
            guard.accept(response),
            Ok(AiCodexAppServerInbound::Response {
                method: "initialize",
                ..
            })
        ));
        assert!(matches!(
            guard.accept(response),
            Err(ProviderError::Rejected)
        ));

        guard
            .start_fresh_thread(&turn())
            .expect("thread request should encode");
        guard
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
            .expect("thread response should bind");
        guard
            .accept(&thread_started_notification("thread-1"))
            .expect("thread notification should bind");
        guard
            .start_turn("thread-1", &turn())
            .expect("turn request should encode");
        guard
            .accept(br#"{"id":3,"result":{"turn":{"id":"turn-1"}}}"#)
            .expect("turn response should bind");
        guard
            .accept(&turn_started_notification("thread-1", "turn-1"))
            .expect("turn notification should bind");
        guard
            .accept(&lifecycle_notification(
                "item/started",
                json!({
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "startedAtMs": 1,
                    "item": {"id": "item-1", "type": "agentMessage", "text": ""},
                }),
            ))
            .expect("agent message lifecycle should bind");
        assert!(matches!(
            guard.accept(&delta),
            Ok(AiCodexAppServerInbound::Notification { .. })
        ));
        let unknown = lifecycle_notification("turn/steer", json!({}));
        assert!(matches!(
            guard.accept(&unknown),
            Err(ProviderError::Rejected)
        ));
    }

    #[test]
    fn protocol_fences_timestamped_turn_item_usage_completion_and_interruption() {
        let mut actor = active_protocol_actor();
        assert!(matches!(
            actor.accept(&turn_started_notification("thread-1", "turn-1")),
            Err(ProviderError::Rejected)
        ));

        let item_started = lifecycle_notification(
            "item/started",
            json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "startedAtMs": 1,
                "item": {"id": "item-1", "type": "agentMessage", "text": ""},
            }),
        );
        actor
            .accept(&item_started)
            .expect("first exact item lifecycle should be admitted");
        assert!(matches!(
            actor.accept(&item_started),
            Err(ProviderError::Rejected)
        ));

        actor
            .accept(&lifecycle_notification(
                "item/agentMessage/delta",
                json!({
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "itemId": "item-1",
                    "delta": "hello",
                }),
            ))
            .expect("delta should bind to the active agent-message item");
        actor
            .accept(&lifecycle_notification(
                "thread/tokenUsage/updated",
                json!({
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "tokenUsage": {
                        "last": {
                            "cachedInputTokens": 0,
                            "inputTokens": 1,
                            "outputTokens": 1,
                            "reasoningOutputTokens": 0,
                            "totalTokens": 2,
                        },
                        "total": {
                            "cachedInputTokens": 0,
                            "inputTokens": 1,
                            "outputTokens": 1,
                            "reasoningOutputTokens": 0,
                            "totalTokens": 2,
                        },
                    },
                }),
            ))
            .expect("usage should remain bound to the active turn");

        let item_completed = lifecycle_notification(
            "item/completed",
            json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "completedAtMs": 2,
                "item": {"id": "item-1", "type": "agentMessage", "text": "hello"},
            }),
        );
        actor
            .accept(&item_completed)
            .expect("matching item completion should be admitted");
        assert!(matches!(
            actor.accept(&item_completed),
            Err(ProviderError::Rejected)
        ));

        actor
            .interrupt_turn("thread-1", "turn-1")
            .expect("exact turn interruption should encode");
        actor
            .accept(br#"{"id":4,"result":{}}"#)
            .expect("interruption response should correlate");
        let completed = lifecycle_notification(
            "turn/completed",
            json!({
                "threadId": "thread-1",
                "turn": {
                    "id": "turn-1",
                    "items": [],
                    "status": "interrupted",
                },
            }),
        );
        actor
            .accept(&completed)
            .expect("interrupted terminal turn should be admitted once");
        assert!(matches!(
            actor.accept(&completed),
            Err(ProviderError::Rejected)
        ));
        assert!(matches!(
            actor.accept(&lifecycle_notification(
                "item/agentMessage/delta",
                json!({
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "itemId": "item-1",
                    "delta": "late",
                }),
            )),
            Err(ProviderError::Rejected)
        ));

        let mut mismatched = active_protocol_actor();
        assert!(matches!(
            mismatched.accept(&lifecycle_notification(
                "thread/tokenUsage/updated",
                json!({
                    "threadId": "thread-other",
                    "turnId": "turn-1",
                    "tokenUsage": {},
                }),
            )),
            Err(ProviderError::Rejected)
        ));
        assert!(matches!(
            mismatched.accept(&lifecycle_notification(
                "thread/tokenUsage/updated",
                json!({
                    "threadId": "thread-1",
                    "turnId": "turn-other",
                    "tokenUsage": {},
                }),
            )),
            Err(ProviderError::Rejected)
        ));
    }

    #[test]
    fn experimental_protocol_admits_only_an_exact_offered_dynamic_call_and_response() {
        let mut guard =
            AiCodexAppServerProtocolActor::new(64 * 1024).expect("test guard should validate");
        let initialize = String::from_utf8(
            guard
                .initialize_with_dynamic_tools("test_client", "Test Client", "1.0.0")
                .expect("experimental initialize should encode"),
        )
        .expect("frame should be UTF-8");
        assert!(initialize.contains("\"experimentalApi\":true"));
        guard
            .accept(br#"{"id":1,"result":{"userAgent":"test"}}"#)
            .expect("initialize response should bind");
        guard
            .initialized()
            .expect("initialized notification should encode");
        let input = AiCodexAppServerTurnInput::try_from_dynamic_request(dynamic_model_request())
            .expect("dynamic request should convert");
        let start = String::from_utf8(
            guard
                .start_dynamic_thread(&input)
                .expect("dynamic thread should encode"),
        )
        .expect("frame should be UTF-8");
        assert!(start.contains("\"dynamicTools\""));
        assert!(start.contains("\"inventory_count\""));
        assert!(start.contains("\"ephemeral\":true"));
        assert!(start.contains("\"approvalPolicy\":\"never\""));
        assert!(start.contains("\"sandbox\":\"read-only\""));
        guard
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
            .expect("thread response should bind");
        guard
            .accept(&thread_started_notification("thread-1"))
            .expect("thread notification should bind");
        guard
            .start_turn("thread-1", &input)
            .expect("turn request should encode");
        guard
            .accept(br#"{"id":3,"result":{"turn":{"id":"turn-dynamic-1"}}}"#)
            .expect("turn response should bind");
        guard
            .accept(&turn_started_notification("thread-1", "turn-dynamic-1"))
            .expect("turn notification should bind");

        let started = lifecycle_notification(
            "item/started",
            json!({
                "item": {
                    "arguments": {"query": "bounded"},
                    "id": "call-dynamic-1",
                    "namespace": null,
                    "status": "inProgress",
                    "tool": "inventory_count",
                    "type": "dynamicToolCall"
                },
                "startedAtMs": 1,
                "threadId": "thread-1",
                "turnId": "turn-dynamic-1"
            }),
        );
        assert!(matches!(
            guard.accept(&started),
            Ok(AiCodexAppServerInbound::DynamicToolLifecycle {
                completed: false,
                ..
            })
        ));

        let request = serde_json::to_vec(&json!({
            "id": 41,
            "method": "item/tool/call",
            "params": {
                "arguments": {"query": "bounded"},
                "callId": "call-dynamic-1",
                "namespace": null,
                "threadId": "thread-1",
                "tool": "inventory_count",
                "turnId": "turn-dynamic-1"
            }
        }))
        .expect("request should encode");
        let call = match guard
            .accept(&request)
            .expect("exact call should be admitted")
        {
            AiCodexAppServerInbound::DynamicToolCall {
                request_id,
                thread_id,
                turn_id,
                call,
            } => {
                assert_eq!(request_id, 41);
                assert_eq!(thread_id, "thread-1");
                assert_eq!(turn_id, "turn-dynamic-1");
                call
            }
            _ => panic!("unexpected protocol value"),
        };
        let result = ProviderDynamicToolResult::new(&call, json!({"count": 3}))
            .expect("result should validate");
        let response = String::from_utf8(
            guard
                .dynamic_tool_response(41, &result)
                .expect("exact response should encode"),
        )
        .expect("response should be UTF-8");
        assert!(response.contains("\"success\":true"));
        assert!(response.contains("\\\"count\\\":3"));
        let completed = lifecycle_notification(
            "item/completed",
            json!({
                "item": {
                    "arguments": {"query": "bounded"},
                    "contentItems": [{"type": "inputText", "text": "{\"count\":3}"}],
                    "durationMs": 2,
                    "id": "call-dynamic-1",
                    "namespace": null,
                    "status": "completed",
                    "success": true,
                    "tool": "inventory_count",
                    "type": "dynamicToolCall"
                },
                "completedAtMs": 3,
                "threadId": "thread-1",
                "turnId": "turn-dynamic-1"
            }),
        );
        assert!(matches!(
            guard.accept(&completed),
            Ok(AiCodexAppServerInbound::DynamicToolLifecycle {
                completed: true,
                ..
            })
        ));
        assert!(matches!(
            guard.dynamic_tool_response(41, &result),
            Err(ProviderError::Rejected)
        ));

        let unknown = serde_json::to_vec(&json!({
            "id": 42,
            "method": "item/tool/call",
            "params": {
                "arguments": {},
                "callId": "call-2",
                "namespace": null,
                "threadId": "thread-1",
                "tool": "unregistered",
                "turnId": "turn-dynamic-1"
            }
        }))
        .expect("request should encode");
        assert!(matches!(
            guard.accept(&unknown),
            Err(ProviderError::Rejected)
        ));

        let swapped_thread = serde_json::to_vec(&json!({
            "id": 43,
            "method": "item/tool/call",
            "params": {
                "arguments": {"query": "bounded"},
                "callId": "call-dynamic-2",
                "namespace": null,
                "threadId": "thread-other",
                "tool": "inventory_count",
                "turnId": "turn-dynamic-1"
            }
        }))
        .expect("request should encode");
        assert!(matches!(
            guard.accept(&swapped_thread),
            Err(ProviderError::Rejected)
        ));
    }

    #[test]
    fn retained_thread_protocol_creates_empty_resumes_exact_and_deletes_exact_cursor() {
        let mut empty_guard = initialized_protocol_actor();
        let create = String::from_utf8(
            empty_guard
                .start_persistent_empty_thread("codex-test-model", &[])
                .expect("empty retained thread should encode"),
        )
        .expect("frame should be UTF-8");
        assert!(create.contains("\"method\":\"thread/start\""));
        assert!(create.contains("\"ephemeral\":false"));
        assert!(create.contains("\"developerInstructions\":null"));
        assert!(create.contains("\"approvalPolicy\":\"never\""));
        assert!(create.contains("\"sandbox\":\"read-only\""));
        assert!(!create.contains("dynamicTools"));
        assert!(!create.contains("turn/start"));
        empty_guard
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-retained-empty"}}}"#)
            .expect("persistent create response should bind");
        empty_guard
            .accept(&thread_started_notification("thread-retained-empty"))
            .expect("persistent create notification should bind");
        let empty_cursor = crate::AiProviderSessionCursor::new(
            "codex.app_server.thread.v2",
            "thread-retained-empty",
        )
        .expect("empty cursor should validate");
        empty_guard
            .delete_thread(&empty_cursor)
            .expect("persistent readiness thread should delete");
        empty_guard
            .accept(&thread_not_loaded_notification("thread-retained-empty"))
            .expect("not-loaded status may precede its delete response");
        empty_guard
            .accept(br#"{"id":3,"result":{}}"#)
            .expect("persistent delete response should bind");
        assert!(matches!(
            empty_guard.accept(&thread_not_loaded_notification("thread-retained-empty")),
            Err(ProviderError::Rejected)
        ));

        let input = AiCodexAppServerTurnInput::try_from_dynamic_request(dynamic_model_request())
            .expect("dynamic request should convert");
        let mut dynamic_create = initialized_protocol_actor();
        let create = String::from_utf8(
            dynamic_create
                .start_persistent_empty_thread("model-1", input.tools())
                .expect("empty dynamic retained thread should encode"),
        )
        .expect("frame should be UTF-8");
        assert!(create.contains("\"dynamicTools\""));
        assert!(create.contains("\"inventory_count\""));
        assert!(!create.contains("turn/start"));
        dynamic_create
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("persistent dynamic create response should bind");
        dynamic_create
            .accept(&thread_started_notification("thread-retained-1"))
            .expect("persistent dynamic create notification should bind");
        let direct = String::from_utf8(
            dynamic_create
                .start_turn("thread-retained-1", &input)
                .expect("newly bound persistent thread should start directly"),
        )
        .expect("direct turn frame should be UTF-8");
        assert!(direct.contains("\"method\":\"turn/start\""));
        assert!(!direct.contains("thread/resume"));

        let cursor =
            crate::AiProviderSessionCursor::new("codex.app_server.thread.v2", "thread-retained-1")
                .expect("retained cursor should validate");
        let mut swapped_request = dynamic_model_request();
        swapped_request.model = "other-model".to_owned();
        let swapped_input = AiCodexAppServerTurnInput::try_from_dynamic_request(swapped_request)
            .expect("bounded swapped request should convert");
        assert!(matches!(
            dynamic_create.resume_thread(&cursor, &swapped_input),
            Err(ProviderError::Rejected)
        ));

        let mut guard = initialized_protocol_actor();
        let resume = String::from_utf8(
            guard
                .resume_thread(&cursor, &input)
                .expect("exact retained thread should resume"),
        )
        .expect("frame should be UTF-8");
        assert!(resume.contains("\"method\":\"thread/resume\""));
        assert!(resume.contains("\"threadId\":\"thread-retained-1\""));
        assert!(resume.contains("\"model\":\"model-1\""));
        assert!(resume.contains("\"approvalPolicy\":\"never\""));
        assert!(resume.contains("\"sandbox\":\"read-only\""));
        assert!(!resume.contains("dynamicTools"));
        assert!(!resume.contains("\"input\""));
        guard
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("resume response should bind");
        guard
            .accept(&thread_started_notification("thread-retained-1"))
            .expect("resume notification should bind");

        let delete = String::from_utf8(
            guard
                .delete_thread(&cursor)
                .expect("exact retained thread should delete"),
        )
        .expect("frame should be UTF-8");
        assert!(delete.contains("\"method\":\"thread/delete\""));
        assert!(delete.contains("\"threadId\":\"thread-retained-1\""));
        guard
            .accept(br#"{"id":3,"result":{}}"#)
            .expect("delete response should bind");
        guard
            .accept(&thread_not_loaded_notification("thread-retained-1"))
            .expect("not-loaded status may follow its delete response");

        let swapped = crate::AiProviderSessionCursor::new("other.thread", "thread-retained-1")
            .expect("bounded swapped cursor should construct");
        assert!(matches!(
            guard.resume_thread(&swapped, &input),
            Err(ProviderError::Rejected)
        ));
        assert!(matches!(
            guard.delete_thread(&swapped),
            Err(ProviderError::Rejected)
        ));
    }

    #[test]
    #[ignore = "requires a reviewed Codex CLI 0.147.0 binary and disposable configured home"]
    fn live_codex_0147_bound_first_turn_then_later_resume_uses_strict_actor() {
        let executable = std::env::var("GRAPHQL_ORM_AI_CODEX_0147_BIN")
            .expect("set GRAPHQL_ORM_AI_CODEX_0147_BIN to the reviewed absolute binary path");
        assert!(PathBuf::from(&executable).is_absolute());
        let version = Command::new(&executable)
            .arg("--version")
            .env_clear()
            .output()
            .expect("reviewed Codex version should execute");
        assert!(version.status.success());
        assert_eq!(
            String::from_utf8(version.stdout)
                .expect("version should be UTF-8")
                .trim(),
            "codex-cli 0.147.0"
        );

        let configured_home = PathBuf::from(
            std::env::var_os("GRAPHQL_ORM_AI_CODEX_0147_HOME")
                .expect("set GRAPHQL_ORM_AI_CODEX_0147_HOME to a disposable configured home"),
        );
        let mut process = LiveCodexProcess::launch(&executable, configured_home.clone());
        let mut actor =
            AiCodexAppServerProtocolActor::new(MAXIMUM_FRAME_BYTES).expect("actor should validate");
        process.send(
            &actor
                .initialize_with_dynamic_tools(
                    "graphql_orm_ai_live_test",
                    "GraphQL ORM AI live test",
                    "0.147.0",
                )
                .expect("initialize should encode"),
        );
        loop {
            match actor
                .accept(&process.receive())
                .expect("initialization frame should be strictly admitted")
            {
                AiCodexAppServerInbound::Response {
                    method: "initialize",
                    ..
                } => break,
                AiCodexAppServerInbound::RemoteControlDisabled => {}
                other => panic!("unexpected initialization frame: {other:?}"),
            }
        }
        process.send(
            &actor
                .initialized()
                .expect("initialized notification should encode"),
        );
        process.send(
            &actor
                .start_persistent_empty_thread("gpt-5.6-sol", &[dynamic_tool()])
                .expect("persistent thread start should encode"),
        );

        let mut response_thread_id = None;
        let mut notification_thread_id = None;
        for _ in 0..8 {
            let frame = process.receive();
            let inbound = actor.accept(&frame).unwrap_or_else(|error| {
                let envelope: Value = serde_json::from_slice(&frame)
                    .expect("rejected live frame should remain valid JSON");
                let keys = envelope
                    .as_object()
                    .map(|object| object.keys().cloned().collect::<Vec<_>>());
                panic!(
                    "thread lifecycle frame was rejected: {error:?}; method={:?}; keys={keys:?}",
                    envelope.get("method").and_then(Value::as_str),
                );
            });
            match inbound {
                AiCodexAppServerInbound::RemoteControlDisabled => {}
                AiCodexAppServerInbound::Response {
                    method: "thread/start",
                    result,
                    ..
                } => {
                    response_thread_id = Some(
                        nested_reference(&result, "thread", "id")
                            .expect("response thread should be valid")
                            .to_owned(),
                    );
                }
                AiCodexAppServerInbound::Notification { method, params }
                    if method == "thread/started" =>
                {
                    notification_thread_id = Some(
                        nested_reference(&params, "thread", "id")
                            .expect("notification thread should be valid")
                            .to_owned(),
                    );
                }
                other => panic!("unexpected thread lifecycle frame: {other:?}"),
            }
            if response_thread_id.is_some() && notification_thread_id.is_some() {
                break;
            }
        }
        assert_eq!(response_thread_id, notification_thread_id);
        let thread_id = response_thread_id.expect("both thread lifecycle frames should arrive");
        let cursor = crate::AiProviderSessionCursor::new("codex.app_server.thread.v2", thread_id)
            .expect("live thread cursor should validate");
        let mut live_request = dynamic_model_request();
        live_request.model = "gpt-5.6-sol".to_owned();
        live_request.instructions.clear();
        live_request.input = vec![ModelInputBlock::Text {
            text: "Call inventory_count once with query bounded, then report the count.".to_owned(),
        }];
        live_request.maximum_output_tokens = Some(128);
        let input = AiCodexAppServerTurnInput::try_from_dynamic_request(live_request)
            .expect("live retained dynamic input should validate");

        process.send(
            &actor
                .start_turn(cursor.expose_to_provider_adapter(), &input)
                .expect("newly bound thread should start its first turn without resume"),
        );
        let mut turn_response_observed = false;
        let mut turn_started_observed = false;
        let mut turn_completed_observed = false;
        for _ in 0..64 {
            let frame = process.receive();
            let inbound = actor.accept(&frame).unwrap_or_else(|error| {
                let envelope: Value = serde_json::from_slice(&frame)
                    .expect("rejected turn frame should remain valid JSON");
                panic!(
                    "retained turn frame was rejected: {error:?}; method={:?}",
                    envelope.get("method").and_then(Value::as_str),
                );
            });
            match inbound {
                AiCodexAppServerInbound::Response {
                    method: "turn/start",
                    ..
                } => turn_response_observed = true,
                AiCodexAppServerInbound::Notification { method, .. }
                    if method == "turn/started" =>
                {
                    turn_started_observed = true;
                }
                AiCodexAppServerInbound::Notification { method, .. }
                    if method == "turn/completed" =>
                {
                    turn_completed_observed = true;
                }
                AiCodexAppServerInbound::DynamicToolCall {
                    request_id, call, ..
                } => {
                    let result = ProviderDynamicToolResult::new(&call, json!({"count": 3}))
                        .expect("live dynamic result should bind to the exact call");
                    process.send(
                        &actor
                            .dynamic_tool_response(request_id, &result)
                            .expect("live dynamic response should encode"),
                    );
                }
                AiCodexAppServerInbound::DynamicToolLifecycle { .. } => {}
                AiCodexAppServerInbound::Notification { .. } => {}
                other => panic!("unexpected retained turn frame: {other:?}"),
            }
            if turn_completed_observed {
                break;
            }
        }
        assert!(turn_response_observed);
        assert!(turn_started_observed);
        assert!(turn_completed_observed);

        drop(process);
        let mut process = LiveCodexProcess::launch(&executable, configured_home);
        let mut actor = AiCodexAppServerProtocolActor::new(MAXIMUM_FRAME_BYTES)
            .expect("resume actor should validate");
        process.send(
            &actor
                .initialize_with_dynamic_tools(
                    "graphql_orm_ai_live_test",
                    "GraphQL ORM AI live test",
                    "0.147.0",
                )
                .expect("resume initialize should encode"),
        );
        loop {
            match actor
                .accept(&process.receive())
                .expect("resume initialization frame should be admitted")
            {
                AiCodexAppServerInbound::Response {
                    method: "initialize",
                    ..
                } => break,
                AiCodexAppServerInbound::RemoteControlDisabled => {}
                other => panic!("unexpected resume initialization frame: {other:?}"),
            }
        }
        process.send(
            &actor
                .initialized()
                .expect("resume initialized notification should encode"),
        );
        process.send(
            &actor
                .resume_thread(&cursor, &input)
                .expect("later process should resume the committed thread"),
        );
        let mut resume_response_observed = false;
        let mut resume_notification_observed = false;
        for _ in 0..8 {
            match actor
                .accept(&process.receive())
                .expect("later resume frame should be admitted")
            {
                AiCodexAppServerInbound::Response {
                    method: "thread/resume",
                    result,
                    ..
                } => {
                    assert_eq!(
                        nested_reference(&result, "thread", "id")
                            .expect("resume response thread should validate"),
                        cursor.expose_to_provider_adapter()
                    );
                    resume_response_observed = true;
                }
                AiCodexAppServerInbound::Notification { method, params }
                    if method == "thread/started" =>
                {
                    assert_eq!(
                        nested_reference(&params, "thread", "id")
                            .expect("resume notification thread should validate"),
                        cursor.expose_to_provider_adapter()
                    );
                    resume_notification_observed = true;
                }
                AiCodexAppServerInbound::RemoteControlDisabled => {}
                other => panic!("unexpected later resume frame: {other:?}"),
            }
            if resume_response_observed && resume_notification_observed {
                break;
            }
        }
        assert!(resume_response_observed && resume_notification_observed);

        process.send(
            &actor
                .start_turn(cursor.expose_to_provider_adapter(), &input)
                .expect("resumed thread should start a second turn"),
        );
        let mut second_completed = false;
        for _ in 0..64 {
            match actor
                .accept(&process.receive())
                .expect("second turn frame should be admitted")
            {
                AiCodexAppServerInbound::DynamicToolCall {
                    request_id, call, ..
                } => {
                    let result = ProviderDynamicToolResult::new(&call, json!({"count": 3}))
                        .expect("second dynamic result should bind");
                    process.send(
                        &actor
                            .dynamic_tool_response(request_id, &result)
                            .expect("second dynamic response should encode"),
                    );
                }
                AiCodexAppServerInbound::Notification { method, .. }
                    if method == "turn/completed" =>
                {
                    second_completed = true
                }
                AiCodexAppServerInbound::Response { .. }
                | AiCodexAppServerInbound::Notification { .. }
                | AiCodexAppServerInbound::DynamicToolLifecycle { .. } => {}
                other => panic!("unexpected second turn frame: {other:?}"),
            }
            if second_completed {
                break;
            }
        }
        assert!(second_completed);

        process.send(
            &actor
                .delete_thread(&cursor)
                .expect("live readiness thread delete should encode"),
        );
        let mut delete_response_observed = false;
        let mut not_loaded_observed = false;
        for _ in 0..4 {
            let frame = process.receive();
            let inbound = actor.accept(&frame).unwrap_or_else(|error| {
                let envelope: Value = serde_json::from_slice(&frame)
                    .expect("rejected delete frame should remain valid JSON");
                let keys = envelope
                    .as_object()
                    .map(|object| object.keys().cloned().collect::<Vec<_>>());
                panic!(
                    "delete frame was rejected: {error:?}; method={:?}; keys={keys:?}",
                    envelope.get("method").and_then(Value::as_str),
                );
            });
            match inbound {
                AiCodexAppServerInbound::Response {
                    method: "thread/delete",
                    ..
                } => delete_response_observed = true,
                AiCodexAppServerInbound::Notification { method, params }
                    if method == "thread/status/changed"
                        && params.pointer("/status/type").and_then(Value::as_str)
                            == Some("notLoaded") =>
                {
                    not_loaded_observed = true;
                }
                other => panic!("unexpected delete frame: {other:?}"),
            }
            if delete_response_observed && not_loaded_observed {
                break;
            }
        }
        assert!(delete_response_observed);
        assert!(not_loaded_observed);
    }
}
