//! Strict run-scoped Codex app-server process and retained-thread boundary.
//!
//! The default path reuses one bounded process while starting a fresh ephemeral
//! thread per call. A separately planned provider-session path may resume one
//! exact protected thread cursor. Experimental dynamic tools are disabled by
//! default; when a registration explicitly enables them, the adapter admits
//! only exact reviewed definitions and delegates execution back to the
//! ordinary coordinator. Every other server-initiated request remains
//! forbidden.

#![cfg_attr(feature = "mssql", allow(dead_code))]

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
    AiProvider, AiProviderFailureCategory, AiProviderRunBinding, AiProviderRunCloseOutcome,
    AiProviderRunCloseReason, AiProviderRunInterruptOutcome, ModelContinuationMode,
    ModelInputBlock, ModelReasoningSummaryRequest, ModelRequest, ModelToolDefinition,
    ProviderCapabilities, ProviderDynamicToolCall, ProviderDynamicToolResponder, ProviderError,
    ProviderEventStream, ProviderKind, ProviderRequestContext,
};

const MAXIMUM_PROCESSES: usize = 4_096;
const MAXIMUM_TURNS_PER_RUN: u32 = 1_024;
const MAXIMUM_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAXIMUM_TEXT_BYTES: usize = 16 * 1024 * 1024;
const MAXIMUM_TEXT_BLOCKS: usize = 256;
const MAXIMUM_BOOTSTRAP_INSTRUCTION_BLOCKS: usize = 16;
const MAXIMUM_BOOTSTRAP_INSTRUCTION_BYTES: usize = 64 * 1024;
const MAXIMUM_IDENTIFIER_BYTES: usize = 200;
const MAXIMUM_VERSION_BYTES: usize = 200;
const MAXIMUM_RUNTIME_WARNING_MESSAGE_BYTES: usize = 4 * 1024;
const MAXIMUM_RUNTIME_WARNING_BYTES_PER_TURN: usize = 16 * 1024;
const MAXIMUM_RUNTIME_WARNINGS_PER_TURN: usize = 8;
const MAXIMUM_TIMEOUT: Duration = Duration::from_secs(60 * 60);

fn provider_timeout_error() -> ProviderError {
    ProviderError::Classified(AiProviderFailureCategory::Timeout)
}

const OPTED_OUT_NOTIFICATION_METHODS: [&str; 5] = [
    "thread/status/changed",
    "thread/settings/updated",
    "thread/goal/cleared",
    "mcpServer/startupStatus/updated",
    "account/rateLimits/updated",
];
const REMOTE_CONTROL_STATUS_CHANGED: &str = "remoteControl/status/changed";
const RUNTIME_WARNING: &str = "warning";
const THREAD_TOKEN_USAGE_UPDATED: &str = "thread/tokenUsage/updated";

const DYNAMIC_TOOLS_ONLY_DISABLED_FEATURES: &[&str] = &[
    "apps",
    "auth_elicitation",
    "browser_use",
    "browser_use_external",
    "browser_use_full_cdp_access",
    "code_mode",
    "code_mode_host",
    "code_mode_only",
    "computer_use",
    "current_time_reminder",
    "default_mode_request_user_input",
    "deferred_executor",
    "enable_mcp_apps",
    "goals",
    "hooks",
    "image_generation",
    "in_app_browser",
    "multi_agent",
    "plugins",
    "recommended_plugins",
    "remote_plugin",
    "request_permissions_tool",
    "shell_snapshot",
    "shell_tool",
    "skill_mcp_dependency_install",
    "skill_search",
    "standalone_web_search",
    "token_budget",
    "tool_call_mcp_elicitation",
    "tool_suggest",
    "unified_exec",
    "view_image",
    "workspace_dependencies",
];

/// Exact reviewed Codex app-server protocol contract supported by this
/// adapter.
pub const AI_CODEX_APP_SERVER_PROTOCOL_V2: &str = "app-server-v2";

/// Static deployment-owned instructions installed when a retained Codex
/// thread is created.
///
/// This proof is deliberately separate from [`ModelRequest::instructions`].
/// A retained thread accepts no request-local instructions: browser input,
/// route context, secrets, resolver output, and model-authored text therefore
/// cannot be smuggled into the privileged developer-instruction channel. The
/// exact bounded text is fingerprinted into the immutable provider
/// registration and is rechecked at empty-thread creation, first activation,
/// and every resume.
#[derive(Clone, PartialEq, Eq)]
pub struct AiCodexAppServerBootstrapInstructions {
    blocks: Vec<String>,
    fingerprint: String,
}

impl std::fmt::Debug for AiCodexAppServerBootstrapInstructions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiCodexAppServerBootstrapInstructions")
            .field("blocks", &"<protected-static-configuration>")
            .field("block_count", &self.blocks.len())
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

impl AiCodexAppServerBootstrapInstructions {
    /// Creates a disabled bootstrap with no developer instructions.
    pub fn disabled() -> Self {
        Self::from_blocks(Vec::new()).expect("an empty static bootstrap is valid")
    }

    /// Creates one bounded bootstrap from compile-time static host text.
    ///
    /// The static lifetime makes the intended trust boundary explicit. Hosts
    /// must keep only reusable deployment policy here; per-user or per-request
    /// data belongs in ordinary model input and application-tool results.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidConfiguration`] for an empty block,
    /// control characters other than tab/newline, excessive block count, or
    /// an aggregate size above 64 KiB.
    pub fn from_static(blocks: &'static [&'static str]) -> Result<Self, ProviderError> {
        Self::from_blocks(blocks.iter().map(|value| (*value).to_owned()).collect())
    }

    fn from_blocks(blocks: Vec<String>) -> Result<Self, ProviderError> {
        let total_bytes = blocks.iter().try_fold(0_usize, |total, block| {
            total
                .checked_add(block.len())
                .ok_or(ProviderError::InvalidConfiguration(
                    "Codex bootstrap instructions are too large".to_owned(),
                ))
        })?;
        if blocks.len() > MAXIMUM_BOOTSTRAP_INSTRUCTION_BLOCKS
            || total_bytes > MAXIMUM_BOOTSTRAP_INSTRUCTION_BYTES
            || blocks.iter().any(|block| {
                block.trim().is_empty()
                    || block
                        .chars()
                        .any(|value| value.is_control() && !matches!(value, '\n' | '\t'))
            })
        {
            return Err(ProviderError::InvalidConfiguration(
                "invalid Codex bootstrap instructions".to_owned(),
            ));
        }
        let mut hasher = Sha256::new();
        hasher.update(b"graphql-orm-ai/codex-app-server-bootstrap/v1\0");
        for block in &blocks {
            hasher.update((block.len() as u64).to_be_bytes());
            hasher.update(block.as_bytes());
        }
        Ok(Self {
            blocks,
            fingerprint: hex::encode(hasher.finalize()),
        })
    }

    /// Stable content fingerprint included in the registration identity.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Whether the retained thread has no static developer instructions.
    pub fn is_disabled(&self) -> bool {
        self.blocks.is_empty()
    }

    fn joined(&self) -> Option<String> {
        (!self.blocks.is_empty()).then(|| self.blocks.join("\n\n"))
    }
}

impl Default for AiCodexAppServerBootstrapInstructions {
    fn default() -> Self {
        Self::disabled()
    }
}

/// Tool-delivery mode declared by the reviewed Codex model catalogue.
///
/// The declaration is deployment evidence bound to the exact executable
/// digest and model registration. It is not selected by the model or a
/// request. Codex models declared as Code Mode-only cannot safely advertise
/// direct dynamic tools when the Code Mode host is unavailable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiCodexAppServerModelToolMode {
    /// Ordinary Responses function tools are model-visible directly.
    Direct,
    /// The model prefers Code Mode but may fall back to direct tools.
    CodeMode,
    /// The model exposes tools only through Code Mode.
    CodeModeOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AiCodexAppServerLaunchProfileKind {
    StrictTextOnlyV1,
    ExperimentalDynamicToolsOnlyV1,
}

/// Closed Codex app-server launch and thread-isolation contract.
///
/// The dynamic-tools-only profile fixes the reviewed CLI feature disables,
/// supplies an empty environment list for every dynamic thread/turn, disables
/// ordinary utility and hosted-search tools in thread configuration, and
/// requires an isolated configuration home. The process factory remains
/// responsible for applying the returned argument vector and operating-system
/// sandbox exactly; it cannot substitute a broader profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiCodexAppServerLaunchProfile {
    kind: AiCodexAppServerLaunchProfileKind,
}

impl AiCodexAppServerLaunchProfile {
    /// Strict protocol profile for tool-free text turns.
    pub const fn strict_text_only_v1() -> Self {
        Self {
            kind: AiCodexAppServerLaunchProfileKind::StrictTextOnlyV1,
        }
    }

    /// Creates the experimental dynamic-tools-only profile for a direct-tool
    /// model registration.
    ///
    /// Code Mode and Code Mode-only catalogue declarations are rejected
    /// because disabling the native Code Mode surface makes their direct
    /// dynamic-tool availability unreliable or impossible.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidConfiguration`] unless the reviewed
    /// model catalogue declares [`AiCodexAppServerModelToolMode::Direct`].
    pub fn experimental_dynamic_tools_only_v1(
        model_tool_mode: AiCodexAppServerModelToolMode,
    ) -> Result<Self, ProviderError> {
        if model_tool_mode != AiCodexAppServerModelToolMode::Direct {
            return Err(ProviderError::InvalidConfiguration(
                "Codex dynamic-tools-only profile requires a direct-tool model".to_owned(),
            ));
        }
        Ok(Self {
            kind: AiCodexAppServerLaunchProfileKind::ExperimentalDynamicToolsOnlyV1,
        })
    }

    /// Exact CLI arguments following the reviewed Codex executable path.
    ///
    /// The dynamic profile deliberately disables every native execution,
    /// browser, hosted-search, connector, collaboration, image, plugin, and
    /// interactive tool feature it relies on being absent. The factory must
    /// also clear the environment, use a private configuration home containing
    /// no project configuration or MCP servers, use an empty working
    /// directory, and apply its reviewed external sandbox.
    #[must_use]
    pub fn codex_arguments(self) -> Vec<&'static str> {
        let mut arguments = vec!["app-server", "--stdio", "--strict-config"];
        if self.supports_experimental_dynamic_tools() {
            for feature in DYNAMIC_TOOLS_ONLY_DISABLED_FEATURES {
                arguments.extend(["--disable", *feature]);
            }
        }
        arguments
    }

    /// Whether this closed profile supports the experimental dynamic-tool
    /// protocol without native execution tools.
    pub const fn supports_experimental_dynamic_tools(self) -> bool {
        matches!(
            self.kind,
            AiCodexAppServerLaunchProfileKind::ExperimentalDynamicToolsOnlyV1
        )
    }

    /// Whether the process must use a private configuration home containing
    /// only the minimum provider authentication material.
    pub const fn requires_isolated_configuration_home(self) -> bool {
        true
    }

    fn identity_label(self) -> &'static str {
        match self.kind {
            AiCodexAppServerLaunchProfileKind::StrictTextOnlyV1 => "strict-text-only-v1",
            AiCodexAppServerLaunchProfileKind::ExperimentalDynamicToolsOnlyV1 => {
                "experimental-dynamic-tools-only-v1"
            }
        }
    }
}

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
    launch_profile: AiCodexAppServerLaunchProfile,
    bootstrap_instructions: AiCodexAppServerBootstrapInstructions,
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
        let launch_profile = AiCodexAppServerLaunchProfile::strict_text_only_v1();
        let bootstrap_instructions = AiCodexAppServerBootstrapInstructions::disabled();
        let mut registration = Self {
            provider_profile_id,
            logical_model,
            executable_sha256,
            executable_version,
            sandbox_profile,
            protocol_version,
            launch_profile,
            bootstrap_instructions,
            identity: String::new(),
        };
        registration.refresh_identity();
        Ok(registration)
    }

    /// Applies one closed reviewed app-server launch profile.
    ///
    /// This changes the immutable registration identity. It only permits the
    /// adapter to forward an exact provider request to a coordinator-owned
    /// responder; it grants no application-tool or resolver authority. A
    /// process factory must separately attest that it implements this exact
    /// profile before the provider advertises custom tools.
    #[must_use]
    pub fn with_launch_profile(mut self, launch_profile: AiCodexAppServerLaunchProfile) -> Self {
        self.launch_profile = launch_profile;
        self.refresh_identity();
        self
    }

    /// Installs immutable static developer instructions for retained threads.
    ///
    /// The instructions become part of the registration identity and cannot
    /// vary by request. Changing them invalidates existing provider-session
    /// bindings so a stale thread cannot inherit a new trust policy.
    #[must_use]
    pub fn with_bootstrap_instructions(
        mut self,
        bootstrap_instructions: AiCodexAppServerBootstrapInstructions,
    ) -> Self {
        self.bootstrap_instructions = bootstrap_instructions;
        self.refresh_identity();
        self
    }

    fn refresh_identity(&mut self) {
        self.identity = registration_identity(self);
    }

    /// Exact immutable launch profile included in the registration identity.
    pub const fn launch_profile(&self) -> AiCodexAppServerLaunchProfile {
        self.launch_profile
    }

    /// Exact static bootstrap proof bound to this registration.
    pub fn bootstrap_instructions(&self) -> &AiCodexAppServerBootstrapInstructions {
        &self.bootstrap_instructions
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
        self.launch_profile.supports_experimental_dynamic_tools()
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
    retained_bootstrap_fingerprint: Option<String>,
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
            .field(
                "retained_bootstrap_fingerprint",
                &self.retained_bootstrap_fingerprint,
            )
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
            retained_bootstrap_fingerprint: None,
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
        if let Some(fingerprint) = &self.retained_bootstrap_fingerprint {
            let bootstrap =
                AiCodexAppServerBootstrapInstructions::from_blocks(self.instructions.clone())
                    .map_err(|_| ProviderError::InvalidRequest)?;
            if !crate::valid_sha256(fingerprint) || bootstrap.fingerprint() != fingerprint {
                return Err(ProviderError::InvalidRequest);
            }
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

    fn try_from_retained_model_request(
        request: ModelRequest,
        bootstrap: &AiCodexAppServerBootstrapInstructions,
    ) -> Result<Self, ProviderError> {
        let request = retained_request_with_bootstrap(request, bootstrap)?;
        let mut input =
            Self::try_from_tool_free_request(request, ModelContinuationMode::ProviderRetained)?;
        input.retained_bootstrap_fingerprint = Some(bootstrap.fingerprint().to_owned());
        input.validate()?;
        Ok(input)
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

    fn try_from_retained_dynamic_request(
        request: ModelRequest,
        bootstrap: &AiCodexAppServerBootstrapInstructions,
    ) -> Result<Self, ProviderError> {
        let request = retained_request_with_bootstrap(request, bootstrap)?;
        let mut input = Self::try_from_dynamic_request(request)?;
        input.retained_bootstrap_fingerprint = Some(bootstrap.fingerprint().to_owned());
        input.validate()?;
        Ok(input)
    }

    fn instruction_fingerprint(&self) -> Result<String, ProviderError> {
        Ok(
            AiCodexAppServerBootstrapInstructions::from_blocks(self.instructions.clone())?
                .fingerprint()
                .to_owned(),
        )
    }
}

fn retained_request_with_bootstrap(
    mut request: ModelRequest,
    bootstrap: &AiCodexAppServerBootstrapInstructions,
) -> Result<ModelRequest, ProviderError> {
    if request.instructions.is_empty() {
        request.instructions = bootstrap.blocks.clone();
        return Ok(request);
    }
    if request.instructions == bootstrap.blocks {
        return Ok(request);
    }
    Err(ProviderError::Rejected)
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
        let dynamic_tools_available = self.registration.experimental_dynamic_tools()
            && self
                .pool
                .supports_launch_profile(self.registration.launch_profile());
        ProviderCapabilities {
            streaming: true,
            custom_tools: dynamic_tools_available,
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
            AiCodexAppServerTurnInput::try_from_retained_model_request(
                request,
                self.registration.bootstrap_instructions(),
            )?
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
        if !self.registration.experimental_dynamic_tools()
            || !self
                .pool
                .supports_launch_profile(self.registration.launch_profile())
        {
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
        let input = if retained.is_some() {
            AiCodexAppServerTurnInput::try_from_retained_dynamic_request(
                request,
                self.registration.bootstrap_instructions(),
            )?
        } else {
            AiCodexAppServerTurnInput::try_from_dynamic_request(request)?
        };
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
            AiCodexAppServerTurnInput::try_from_retained_model_request(
                request.clone(),
                self.registration.bootstrap_instructions(),
            )?
        } else {
            if !self.registration.experimental_dynamic_tools()
                || !self
                    .pool
                    .supports_launch_profile(self.registration.launch_profile())
            {
                return Err(ProviderError::Unsupported);
            }
            AiCodexAppServerTurnInput::try_from_retained_dynamic_request(
                request.clone(),
                self.registration.bootstrap_instructions(),
            )?
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
    /// Only the immutable registration-bound bootstrap and reviewed dynamic
    /// tool definitions may enter the thread before the caller durably binds
    /// it. No user input, request-local instruction, route context, secret, or
    /// resolver output is permitted. App-server cannot add dynamic tools
    /// during `thread/resume`; implementations must transmit exactly the
    /// supplied bootstrap and definitions or reject them.
    async fn create_empty_thread(
        &self,
        _model: &str,
        _bootstrap: &AiCodexAppServerBootstrapInstructions,
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
        bootstrap: &AiCodexAppServerBootstrapInstructions,
        dynamic_tools: Vec<ModelToolDefinition>,
    ) -> Result<crate::AiProviderSessionCursor, ProviderError> {
        self.process
            .create_empty_thread(model, bootstrap, dynamic_tools)
            .await
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
    /// Whether this factory implements one exact closed launch profile.
    ///
    /// The default keeps existing text-only factories compatible and refuses
    /// the experimental dynamic-tools-only profile. A factory may return true
    /// for that profile only when it uses [`AiCodexAppServerLaunchProfile::codex_arguments`]
    /// unchanged and enforces the documented isolated-home, environment,
    /// working-directory, integrity, and process-sandbox requirements.
    fn supports_launch_profile(&self, profile: AiCodexAppServerLaunchProfile) -> bool {
        profile == AiCodexAppServerLaunchProfile::strict_text_only_v1()
    }

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
        bootstrap_fingerprint: String,
        dynamic_tools: Vec<ModelToolDefinition>,
    },
    Consumed,
}

fn bound_turn_rejected(reason: crate::AiCodexBoundTurnRejection) -> ProviderError {
    ProviderError::NewlyBoundTurnRejected(reason)
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
        && binding.matches_principal_reference(&session.claim().principal_reference)
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

    /// Whether the trusted process factory implements one exact closed launch
    /// profile.
    pub fn supports_launch_profile(&self, profile: AiCodexAppServerLaunchProfile) -> bool {
        self.inner.factory.supports_launch_profile(profile)
    }

    async fn create_empty_thread(
        &self,
        binding: AiProviderRunBinding,
        registration: Arc<AiCodexAppServerRegistration>,
        dynamic_tools: Vec<ModelToolDefinition>,
    ) -> Result<crate::AiProviderSessionCursor, ProviderError> {
        if !self.supports_launch_profile(registration.launch_profile())
            || (!dynamic_tools.is_empty() && !registration.experimental_dynamic_tools())
        {
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
            entry.process.create_empty_thread(
                registration.logical_model(),
                registration.bootstrap_instructions(),
                dynamic_tools.clone(),
            ),
        )
        .await;
        entry.turn_active.store(false, Ordering::Release);
        match outcome {
            Ok(Ok(cursor)) if cursor.kind() == "codex.app_server.thread.v2" => {
                *entry.empty_thread.lock().await = EmptyThreadActivation::Available {
                    cursor_fingerprint: cursor.fingerprint(),
                    bootstrap_fingerprint: registration
                        .bootstrap_instructions()
                        .fingerprint()
                        .to_owned(),
                    dynamic_tools,
                };
                Ok(cursor)
            }
            Ok(Ok(_)) => {
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                Err(ProviderError::Classified(
                    AiProviderFailureCategory::ProtocolViolation,
                ))
            }
            Ok(Err(error)) => {
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                Err(error)
            }
            Err(_) => {
                self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                    .await;
                Err(provider_timeout_error())
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
                Err(provider_timeout_error())
            }
        }
    }

    async fn delete_detached_thread(
        &self,
        registration: Arc<AiCodexAppServerRegistration>,
        cursor: &crate::AiProviderSessionCursor,
    ) -> Result<(), ProviderError> {
        if !self.supports_launch_profile(registration.launch_profile()) {
            return Err(ProviderError::Unsupported);
        }
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
        .map_err(|_| provider_timeout_error())??;
        let result = tokio::time::timeout(
            self.inner.limits.shutdown_timeout,
            process.delete_thread(cursor),
        )
        .await
        .map_err(|_| provider_timeout_error())?;
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
                    return Err(provider_timeout_error());
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
                        Err(provider_timeout_error())
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
                return Err(provider_timeout_error());
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
                return Err(provider_timeout_error());
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
        let input_instruction_fingerprint = input.instruction_fingerprint()?;
        if session.activation() != crate::AiProviderSessionActivation::NewlyBoundEmpty {
            return Err(bound_turn_rejected(
                crate::AiCodexBoundTurnRejection::ActivationNotNewlyBound,
            ));
        }
        if !opened_session_matches(binding, registration, session) {
            return Err(bound_turn_rejected(
                crate::AiCodexBoundTurnRejection::OpenedSessionMismatch,
            ));
        }
        if input.model() != registration.logical_model() {
            return Err(bound_turn_rejected(
                crate::AiCodexBoundTurnRejection::ModelMismatch,
            ));
        }
        let Some(entry) = self.inner.entries.lock().await.get(&binding).cloned() else {
            return Err(bound_turn_rejected(
                crate::AiCodexBoundTurnRejection::ProcessBindingMissing,
            ));
        };
        if entry.poisoned.load(Ordering::Acquire) {
            return Err(bound_turn_rejected(
                crate::AiCodexBoundTurnRejection::ProcessPoisoned,
            ));
        }
        if entry.registration_identity != registration.identity() {
            return Err(bound_turn_rejected(
                crate::AiCodexBoundTurnRejection::RegistrationIdentityMismatch,
            ));
        }
        if entry
            .turn_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(bound_turn_rejected(
                crate::AiCodexBoundTurnRejection::TurnAlreadyActive,
            ));
        }
        let activation_rejection = {
            let mut activation = entry.empty_thread.lock().await;
            match &*activation {
                EmptyThreadActivation::Available {
                    cursor_fingerprint,
                    bootstrap_fingerprint,
                    dynamic_tools,
                } => {
                    if cursor_fingerprint != &session.cursor().fingerprint() {
                        Some(crate::AiCodexBoundTurnRejection::CursorFingerprintMismatch)
                    } else if bootstrap_fingerprint != &input_instruction_fingerprint {
                        Some(crate::AiCodexBoundTurnRejection::BootstrapFingerprintMismatch)
                    } else if dynamic_tools != input.tools() {
                        Some(crate::AiCodexBoundTurnRejection::FrozenDefinitionMismatch)
                    } else {
                        *activation = EmptyThreadActivation::Consumed;
                        None
                    }
                }
                EmptyThreadActivation::Vacant
                | EmptyThreadActivation::Creating
                | EmptyThreadActivation::Consumed => {
                    Some(crate::AiCodexBoundTurnRejection::ActivationUnavailable)
                }
            }
        };
        if let Some(reason) = activation_rejection {
            entry.turn_active.store(false, Ordering::Release);
            self.invalidate(binding, &entry, AiProviderRunCloseReason::ProtocolViolation)
                .await;
            return Err(bound_turn_rejected(reason));
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
                        Err(provider_timeout_error())
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
                return Err(provider_timeout_error());
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
                        Err(provider_timeout_error())
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
                return Err(provider_timeout_error());
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
                        Err(provider_timeout_error())
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
                return Err(provider_timeout_error());
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
                        Err(provider_timeout_error())
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
        if !self.supports_launch_profile(registration.launch_profile()) {
            return Err(ProviderError::Unsupported);
        }
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
        .map_err(|_| provider_timeout_error())??;
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
        .map_err(|_| provider_timeout_error())??;
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
    /// Content-free notice that app-server emitted one bounded non-fatal
    /// warning during the current correlated turn.
    ///
    /// The timestamp, optional thread reference, and warning text are
    /// validated and discarded inside the actor. No warning content or
    /// identifier crosses this boundary into events, logs, or model context.
    RuntimeWarning,
    /// Content-free lifecycle for one exact provider reasoning item.
    ///
    /// The actor accepts only an empty `content` and `summary` shape while
    /// reasoning summaries are disabled on the turn. The item identifier,
    /// payload, and timestamp are correlated and discarded inside the actor;
    /// hidden reasoning never crosses this boundary.
    ReasoningLifecycle {
        /// Whether this is the item start or terminal completion.
        completed: bool,
    },
    /// Content-free retained-thread usage snapshot observed during an exact
    /// resume lifecycle.
    ///
    /// App-server may replay one cumulative snapshot while loading a thread.
    /// Its complete nonnegative generated shape and exact thread binding are
    /// validated, then all turn identifiers and token values are discarded so
    /// they cannot be charged again to the next run. On Codex versions that do
    /// not emit `thread/started` after `thread/resume`, this signal may complete
    /// only a typed resume after its exact correlated response is observed; it
    /// can never complete a new thread-start lifecycle.
    RetainedResumeUsageSnapshot,
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
            Self::RuntimeWarning => formatter.write_str("AiCodexAppServerInbound::RuntimeWarning"),
            Self::ReasoningLifecycle { completed } => formatter
                .debug_struct("AiCodexAppServerInbound::ReasoningLifecycle")
                .field("completed", completed)
                .finish(),
            Self::RetainedResumeUsageSnapshot => {
                formatter.write_str("AiCodexAppServerInbound::RetainedResumeUsageSnapshot")
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThreadLifecycleOperation {
    Start,
    Resume,
}

/// Closed app-server JSON-RPC encoder/guard.
///
/// There is intentionally no generic request builder. The provider-specific
/// process actor may emit only the explicitly typed initialization, thread,
/// turn, interruption, deletion, and dynamic-tool response methods represented
/// here. Admitted server notifications require the complete positive signed
/// `emittedAtMs` envelope and exact lifecycle correlation. Initialization
/// negotiates one fixed opt-out profile for unused thread, MCP, and account
/// notifications; those methods remain rejected if the server emits them.
/// Deletion uses only its exact correlated response. A documented generic
/// `warning` is admitted only as a content-free, turn-correlated,
/// flood-bounded control event. Empty reasoning-item lifecycles are admitted
/// only as content-free signals while turn-level reasoning summaries remain
/// explicitly disabled. All other server-initiated requests and
/// non-allowlisted notifications fail closed.
#[derive(Debug)]
pub struct AiCodexAppServerProtocolActor {
    next_id: u64,
    pending: BTreeMap<u64, ClientMethod>,
    active_thread_id: Option<String>,
    pending_turn_thread_id: Option<String>,
    active_turn_id: Option<String>,
    retained_model: Option<String>,
    retained_bootstrap_fingerprint: Option<String>,
    dynamic_tools: BTreeMap<String, ModelToolDefinition>,
    dynamic_tool_projection_fingerprints: BTreeMap<String, String>,
    pending_dynamic_requests: BTreeMap<u64, (String, String)>,
    started_dynamic_calls: BTreeMap<String, String>,
    responded_dynamic_calls: BTreeMap<String, String>,
    started_items: BTreeMap<String, String>,
    completed_items: BTreeSet<String>,
    initialization_complete: bool,
    thread_lifecycle_phase: ThreadLifecyclePhase,
    thread_lifecycle_operation: Option<ThreadLifecycleOperation>,
    deleting_thread_id: Option<String>,
    retained_usage_snapshot_observed: bool,
    turn_response_observed: bool,
    turn_started_observed: bool,
    remote_control_disabled_observed: bool,
    runtime_warning_count: usize,
    runtime_warning_bytes: usize,
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
            retained_bootstrap_fingerprint: None,
            dynamic_tools: BTreeMap::new(),
            dynamic_tool_projection_fingerprints: BTreeMap::new(),
            pending_dynamic_requests: BTreeMap::new(),
            started_dynamic_calls: BTreeMap::new(),
            responded_dynamic_calls: BTreeMap::new(),
            started_items: BTreeMap::new(),
            completed_items: BTreeSet::new(),
            initialization_complete: false,
            thread_lifecycle_phase: ThreadLifecyclePhase::Ready,
            thread_lifecycle_operation: None,
            deleting_thread_id: None,
            retained_usage_snapshot_observed: false,
            turn_response_observed: false,
            turn_started_observed: false,
            remote_control_disabled_observed: false,
            runtime_warning_count: 0,
            runtime_warning_bytes: 0,
            maximum_frame_bytes,
        })
    }

    /// Encodes the one allowed stable protocol-initialization request.
    ///
    /// The actor always suppresses the exact thread-status, thread-settings,
    /// MCP-startup, and account-rate-limit notifications that this closed
    /// adapter neither consumes nor admits. The host cannot alter that profile
    /// or suppress authoritative thread, turn, item, usage, dynamic-tool, or
    /// completion traffic.
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
                },
                "capabilities": initialization_capabilities(false),
            }),
        )
    }

    /// Encodes initialization with the experimental API capability required
    /// by app-server dynamic tools and the same closed notification profile.
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
                "capabilities": initialization_capabilities(true),
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
            || self.thread_lifecycle_operation.is_some()
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
        self.thread_lifecycle_operation = Some(ThreadLifecycleOperation::Start);
        self.retained_usage_snapshot_observed = false;
    }

    fn begin_resume_lifecycle(&mut self, thread_id: &str) {
        self.active_thread_id = Some(thread_id.to_owned());
        self.thread_lifecycle_phase = ThreadLifecyclePhase::AwaitingResponseAndStarted;
        self.thread_lifecycle_operation = Some(ThreadLifecycleOperation::Resume);
        self.retained_usage_snapshot_observed = false;
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
    /// before calling [`Self::start_turn`]. Only the supplied immutable static
    /// bootstrap may enter `developerInstructions`; no request-local or user
    /// content is included. Reviewed dynamic-tool definitions may be installed
    /// because app-server cannot add them at resume time.
    pub fn start_persistent_empty_thread(
        &mut self,
        model: &str,
        bootstrap: &AiCodexAppServerBootstrapInstructions,
        dynamic_tools: &[ModelToolDefinition],
    ) -> Result<Vec<u8>, ProviderError> {
        self.validate_thread_lifecycle_boundary()?;
        if !valid_identifier(model)
            || self.thread_lifecycle_phase != ThreadLifecyclePhase::Ready
            || self.active_thread_id.is_some()
            || self.retained_model.is_some()
            || self.retained_bootstrap_fingerprint.is_some()
            || !self.dynamic_tools.is_empty()
            || !self.pending_dynamic_requests.is_empty()
            || !self.started_dynamic_calls.is_empty()
            || !self.responded_dynamic_calls.is_empty()
        {
            return Err(ProviderError::Rejected);
        }
        let projected_tools = project_codex_dynamic_tools(dynamic_tools)?;
        let mut params = json!({
            "model": model,
            "developerInstructions": bootstrap.joined().map_or(Value::Null, Value::String),
            "ephemeral": false,
            "approvalPolicy": "never",
            "sandbox": "read-only",
        });
        if !projected_tools.protocol_values.is_empty() {
            let params = params.as_object_mut().ok_or(ProviderError::Rejected)?;
            params.insert(
                "dynamicTools".to_owned(),
                Value::Array(projected_tools.protocol_values),
            );
            params.insert("config".to_owned(), dynamic_tools_only_thread_config());
            params.insert("environments".to_owned(), Value::Array(Vec::new()));
        }
        let frame = self.request(ClientMethod::ThreadStart, "thread/start", params)?;
        self.begin_new_thread_lifecycle();
        self.retained_model = Some(model.to_owned());
        self.retained_bootstrap_fingerprint = Some(bootstrap.fingerprint().to_owned());
        self.dynamic_tools = projected_tools.definitions;
        self.dynamic_tool_projection_fingerprints = projected_tools.fingerprints;
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
        let input_instruction_fingerprint = input.instruction_fingerprint()?;
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
            || self
                .retained_bootstrap_fingerprint
                .as_deref()
                .is_some_and(|fingerprint| fingerprint != input_instruction_fingerprint.as_str())
            || !self.pending_dynamic_requests.is_empty()
            || !self.started_dynamic_calls.is_empty()
            || !self.responded_dynamic_calls.is_empty()
        {
            return Err(ProviderError::Rejected);
        }
        let projected_tools = project_codex_dynamic_tools(input.tools())?;
        let developer_instructions = if input.instructions().is_empty() {
            Value::Null
        } else {
            Value::String(input.instructions().join("\n\n"))
        };
        if self.retained_model.is_some()
            && (self.dynamic_tools != projected_tools.definitions
                || self.dynamic_tool_projection_fingerprints != projected_tools.fingerprints)
        {
            return Err(ProviderError::Rejected);
        }
        let mut params = json!({
            "threadId": cursor.expose_to_provider_adapter(),
            "model": input.model(),
            "developerInstructions": developer_instructions,
            "approvalPolicy": "never",
            "sandbox": "read-only",
        });
        if !input.tools().is_empty() {
            params
                .as_object_mut()
                .ok_or(ProviderError::Rejected)?
                .insert("config".to_owned(), dynamic_tools_only_thread_config());
        }
        let frame = self.request(ClientMethod::ThreadResume, "thread/resume", params)?;
        self.begin_resume_lifecycle(cursor.expose_to_provider_adapter());
        self.retained_model = Some(input.model().to_owned());
        self.retained_bootstrap_fingerprint = Some(input_instruction_fingerprint);
        self.dynamic_tools = projected_tools.definitions;
        self.dynamic_tool_projection_fingerprints = projected_tools.fingerprints;
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
        self.thread_lifecycle_operation = None;
        self.deleting_thread_id = Some(cursor.expose_to_provider_adapter().to_owned());
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
        let projected_tools = project_codex_dynamic_tools(input.tools())?;
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
                "dynamicTools": projected_tools.protocol_values,
                "approvalPolicy": "never",
                "sandbox": "read-only",
                "config": dynamic_tools_only_thread_config(),
                "environments": [],
            }),
        )?;
        self.begin_new_thread_lifecycle();
        self.dynamic_tools = projected_tools.definitions;
        self.dynamic_tool_projection_fingerprints = projected_tools.fingerprints;
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
        let input_instruction_fingerprint = input.instruction_fingerprint()?;
        let consumes_retained_resume_fallback = self.thread_lifecycle_phase
            == ThreadLifecyclePhase::Complete
            && self.thread_lifecycle_operation == Some(ThreadLifecycleOperation::Resume)
            && self.retained_usage_snapshot_observed;
        let projected_tools = project_codex_dynamic_tools(input.tools())?;
        if !valid_reference(thread_id)
            || self.active_thread_id.as_deref() != Some(thread_id)
            || self.thread_lifecycle_phase != ThreadLifecyclePhase::Complete
            || (self.thread_lifecycle_operation.is_some() && !consumes_retained_resume_fallback)
            || self
                .retained_model
                .as_deref()
                .is_some_and(|model| model != input.model())
            || self
                .retained_bootstrap_fingerprint
                .as_deref()
                .is_some_and(|fingerprint| fingerprint != input_instruction_fingerprint)
            || self.dynamic_tools != projected_tools.definitions
            || self.dynamic_tool_projection_fingerprints != projected_tools.fingerprints
            || self.pending_turn_thread_id.is_some()
            || self.active_turn_id.is_some()
            || self.turn_response_observed
            || self.turn_started_observed
            || !self.started_items.is_empty()
            || !self.completed_items.is_empty()
        {
            return Err(ProviderError::InvalidRequest);
        }
        let mut params = json!({
            "threadId": thread_id,
            "input": input.input().iter().map(|text| json!({"type": "text", "text": text})).collect::<Vec<_>>(),
            "summary": "none",
        });
        if !input.tools().is_empty() {
            params
                .as_object_mut()
                .ok_or(ProviderError::Rejected)?
                .insert("environments".to_owned(), Value::Array(Vec::new()));
        }
        let frame = self.request(ClientMethod::TurnStart, "turn/start", params)?;
        self.thread_lifecycle_operation = None;
        self.pending_turn_thread_id = Some(thread_id.to_owned());
        self.runtime_warning_count = 0;
        self.runtime_warning_bytes = 0;
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
    /// forbidden item kind, non-empty reasoning content, and unknown
    /// notification is rejected.
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
            if self.pending_dynamic_requests.contains_key(&request_id)
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
        if notification.method == RUNTIME_WARNING {
            return self.accept_runtime_warning(notification);
        }
        let method = Some(notification.method.as_str())
            .filter(|method| allowed_notification(method))
            .ok_or(ProviderError::Rejected)?;
        let params = notification.params;
        if method == THREAD_TOKEN_USAGE_UPDATED && self.active_turn_id.is_none() {
            return self.accept_retained_usage_snapshot(&params);
        }
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
        if matches!(method, "item/started" | "item/completed")
            && params
                .get("item")
                .and_then(Value::as_object)
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
                == Some("reasoning")
        {
            return self.accept_empty_reasoning_lifecycle(method, &params);
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
                self.dynamic_tool_projection_fingerprints.clear();
            }
            self.pending_turn_thread_id = None;
            self.active_turn_id = None;
            self.turn_response_observed = false;
            self.turn_started_observed = false;
            self.runtime_warning_count = 0;
            self.runtime_warning_bytes = 0;
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

    fn accept_empty_reasoning_lifecycle(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<AiCodexAppServerInbound, ProviderError> {
        self.validate_active_turn(
            direct_reference(params, "threadId")?,
            direct_reference(params, "turnId")?,
        )?;
        let params = params.as_object().ok_or(ProviderError::Rejected)?;
        let timestamp_key = if method == "item/started" {
            "startedAtMs"
        } else {
            "completedAtMs"
        };
        if params.keys().any(|key| {
            !matches!(key.as_str(), "item" | "threadId" | "turnId") && key != timestamp_key
        }) || params
            .get(timestamp_key)
            .and_then(Value::as_i64)
            .is_none_or(|timestamp| timestamp <= 0)
        {
            return Err(ProviderError::Rejected);
        }
        let item = params
            .get("item")
            .and_then(Value::as_object)
            .ok_or(ProviderError::Rejected)?;
        if item
            .keys()
            .any(|key| !matches!(key.as_str(), "content" | "id" | "summary" | "type"))
            || item.get("type").and_then(Value::as_str) != Some("reasoning")
            || item
                .get("content")
                .is_some_and(|content| content.as_array().is_none_or(|content| !content.is_empty()))
            || item
                .get("summary")
                .is_some_and(|summary| summary.as_array().is_none_or(|summary| !summary.is_empty()))
        {
            return Err(ProviderError::Rejected);
        }
        let item_id = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|item_id| valid_reference(item_id))
            .ok_or(ProviderError::Rejected)?;
        let completed = method == "item/completed";
        if completed {
            if self.started_items.remove(item_id).as_deref() != Some("reasoning")
                || !self.completed_items.insert(item_id.to_owned())
                || self.completed_items.len() > MAXIMUM_TEXT_BLOCKS
            {
                return Err(ProviderError::Rejected);
            }
        } else if self.completed_items.contains(item_id)
            || self
                .started_items
                .insert(item_id.to_owned(), "reasoning".to_owned())
                .is_some()
            || self.started_items.len() > MAXIMUM_TEXT_BLOCKS
        {
            return Err(ProviderError::Rejected);
        }
        Ok(AiCodexAppServerInbound::ReasoningLifecycle { completed })
    }

    fn accept_retained_usage_snapshot(
        &mut self,
        params: &Value,
    ) -> Result<AiCodexAppServerInbound, ProviderError> {
        let usage = validate_thread_token_usage(params)?;
        if !self.initialization_complete
            || self.retained_usage_snapshot_observed
            || self.thread_lifecycle_operation != Some(ThreadLifecycleOperation::Resume)
            || self.active_thread_id.as_deref() != Some(usage.thread_id.as_str())
            || self.pending_turn_thread_id.is_some()
            || self.active_turn_id.is_some()
            || !matches!(
                self.thread_lifecycle_phase,
                ThreadLifecyclePhase::AwaitingResponseAndStarted
                    | ThreadLifecyclePhase::AwaitingResponse
                    | ThreadLifecyclePhase::AwaitingStarted
                    | ThreadLifecyclePhase::Complete
            )
        {
            return Err(ProviderError::Rejected);
        }
        self.retained_usage_snapshot_observed = true;
        if self.thread_lifecycle_phase == ThreadLifecyclePhase::AwaitingStarted
            && self.thread_lifecycle_operation == Some(ThreadLifecycleOperation::Resume)
        {
            self.thread_lifecycle_phase = ThreadLifecyclePhase::Complete;
        }
        Ok(AiCodexAppServerInbound::RetainedResumeUsageSnapshot)
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
                        if self.thread_lifecycle_operation == Some(ThreadLifecycleOperation::Resume)
                            && self.retained_usage_snapshot_observed
                        {
                            ThreadLifecyclePhase::Complete
                        } else {
                            ThreadLifecyclePhase::AwaitingStarted
                        }
                    }
                    ThreadLifecyclePhase::AwaitingResponse => {
                        self.thread_lifecycle_operation = None;
                        ThreadLifecyclePhase::Complete
                    }
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
                let deleting_thread_id = self
                    .deleting_thread_id
                    .as_deref()
                    .ok_or(ProviderError::Rejected)?;
                if !result.as_object().is_some_and(serde_json::Map::is_empty)
                    || self.active_thread_id.as_deref() != Some(deleting_thread_id)
                    || self.pending_turn_thread_id.is_some()
                    || self.active_turn_id.is_some()
                {
                    return Err(ProviderError::Rejected);
                }
                self.active_thread_id = None;
                self.retained_model = None;
                self.retained_bootstrap_fingerprint = None;
                self.dynamic_tools.clear();
                self.dynamic_tool_projection_fingerprints.clear();
                self.deleting_thread_id = None;
                self.thread_lifecycle_operation = None;
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

    fn accept_runtime_warning(
        &mut self,
        notification: CodexAppServerNotificationEnvelope,
    ) -> Result<AiCodexAppServerInbound, ProviderError> {
        let params: RuntimeWarningParams =
            serde_json::from_value(notification.params).map_err(|_| ProviderError::Rejected)?;
        let active_thread_id = self
            .active_thread_id
            .as_deref()
            .ok_or(ProviderError::Rejected)?;
        let message_bytes = params.message.len();
        let next_bytes = self
            .runtime_warning_bytes
            .checked_add(message_bytes)
            .ok_or(ProviderError::Rejected)?;
        if notification.method != RUNTIME_WARNING
            || !self.initialization_complete
            || self.thread_lifecycle_phase != ThreadLifecyclePhase::Complete
            || self.pending_turn_thread_id.as_deref() != Some(active_thread_id)
            || self.deleting_thread_id.is_some()
            || (!self
                .pending
                .values()
                .any(|method| *method == ClientMethod::TurnStart)
                && !self.turn_response_observed
                && !self.turn_started_observed)
            || params
                .thread_id
                .as_deref()
                .is_some_and(|thread_id| thread_id != active_thread_id)
            || params.message.trim().is_empty()
            || message_bytes > MAXIMUM_RUNTIME_WARNING_MESSAGE_BYTES
            || params.message.chars().any(char::is_control)
            || self.runtime_warning_count >= MAXIMUM_RUNTIME_WARNINGS_PER_TURN
            || next_bytes > MAXIMUM_RUNTIME_WARNING_BYTES_PER_TURN
        {
            return Err(ProviderError::Rejected);
        }
        self.runtime_warning_count += 1;
        self.runtime_warning_bytes = next_bytes;
        Ok(AiCodexAppServerInbound::RuntimeWarning)
    }

    fn accept_notification_binding(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<(), ProviderError> {
        match method {
            "thread/started" => {
                let thread_id = nested_reference(params, "thread", "id")?;
                let optional_late_resume_started = self.thread_lifecycle_phase
                    == ThreadLifecyclePhase::Complete
                    && self.thread_lifecycle_operation == Some(ThreadLifecycleOperation::Resume)
                    && self.retained_usage_snapshot_observed;
                if !self.initialization_complete
                    || (!optional_late_resume_started
                        && !matches!(
                            self.thread_lifecycle_phase,
                            ThreadLifecyclePhase::AwaitingResponseAndStarted
                                | ThreadLifecyclePhase::AwaitingStarted
                        ))
                    || self.pending_turn_thread_id.is_some()
                    || self.active_turn_id.is_some()
                    || self
                        .pending
                        .values()
                        .any(|method| *method == ClientMethod::ThreadDelete)
                    || (!optional_late_resume_started
                        && self.thread_lifecycle_phase
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
                if optional_late_resume_started {
                    self.thread_lifecycle_operation = None;
                    return Ok(());
                }
                self.thread_lifecycle_phase = match self.thread_lifecycle_phase {
                    ThreadLifecyclePhase::AwaitingResponseAndStarted => {
                        ThreadLifecyclePhase::AwaitingResponse
                    }
                    ThreadLifecyclePhase::AwaitingStarted => {
                        self.thread_lifecycle_operation = None;
                        ThreadLifecyclePhase::Complete
                    }
                    _ => return Err(ProviderError::Rejected),
                };
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
            THREAD_TOKEN_USAGE_UPDATED => {
                let usage = validate_thread_token_usage(params)?;
                self.validate_active_turn(&usage.thread_id, &usage.turn_id)?;
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
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RuntimeWarningParams {
    thread_id: Option<String>,
    message: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ThreadTokenUsageUpdatedParams {
    thread_id: String,
    turn_id: String,
    token_usage: ThreadTokenUsage,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ThreadTokenUsage {
    last: TokenUsageBreakdown,
    total: TokenUsageBreakdown,
    model_context_window: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct TokenUsageBreakdown {
    input_tokens: i64,
    cached_input_tokens: i64,
    #[serde(default)]
    cache_write_input_tokens: i64,
    output_tokens: i64,
    reasoning_output_tokens: i64,
    total_tokens: i64,
}

impl TokenUsageBreakdown {
    fn validate(&self) -> Result<(), ProviderError> {
        if [
            self.input_tokens,
            self.cached_input_tokens,
            self.cache_write_input_tokens,
            self.output_tokens,
            self.reasoning_output_tokens,
            self.total_tokens,
        ]
        .into_iter()
        .any(|value| value < 0)
        {
            return Err(ProviderError::Rejected);
        }
        Ok(())
    }
}

fn validate_thread_token_usage(
    params: &Value,
) -> Result<ThreadTokenUsageUpdatedParams, ProviderError> {
    let usage: ThreadTokenUsageUpdatedParams =
        serde_json::from_value(params.clone()).map_err(|_| ProviderError::Rejected)?;
    usage.token_usage.last.validate()?;
    usage.token_usage.total.validate()?;
    if !valid_reference(&usage.thread_id)
        || !valid_reference(&usage.turn_id)
        || usage
            .token_usage
            .model_context_window
            .is_some_and(|value| value < 0)
        || usage.token_usage.total.total_tokens < usage.token_usage.last.total_tokens
    {
        return Err(ProviderError::Rejected);
    }
    Ok(usage)
}

#[derive(Deserialize)]
enum DisabledRemoteControlStatus {
    #[serde(rename = "disabled")]
    Disabled,
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
            | "turn/started"
            | "item/started"
            | "item/completed"
            | "item/agentMessage/delta"
            | "thread/tokenUsage/updated"
            | "turn/completed"
    )
}

fn initialization_capabilities(experimental_api: bool) -> Value {
    let mut capabilities = serde_json::Map::from_iter([(
        "optOutNotificationMethods".to_owned(),
        json!(OPTED_OUT_NOTIFICATION_METHODS),
    )]);
    if experimental_api {
        capabilities.insert("experimentalApi".to_owned(), Value::Bool(true));
    }
    Value::Object(capabilities)
}

fn dynamic_tools_only_thread_config() -> Value {
    let mut config = serde_json::Map::new();
    for feature in DYNAMIC_TOOLS_ONLY_DISABLED_FEATURES {
        config.insert(format!("features.{feature}"), Value::Bool(false));
    }
    config.insert("tools.update_plan.enabled".to_owned(), Value::Bool(false));
    config.insert(
        "web_search".to_owned(),
        Value::String("disabled".to_owned()),
    );
    Value::Object(config)
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

struct ProjectedCodexDynamicTools {
    protocol_values: Vec<Value>,
    definitions: BTreeMap<String, ModelToolDefinition>,
    fingerprints: BTreeMap<String, String>,
}

fn project_codex_dynamic_tools(
    tools: &[ModelToolDefinition],
) -> Result<ProjectedCodexDynamicTools, ProviderError> {
    let mut protocol_values = Vec::with_capacity(tools.len());
    let mut definitions = BTreeMap::new();
    let mut fingerprints = BTreeMap::new();
    for tool in tools {
        tool.validate()?;
        if !tool.strict
            || definitions
                .insert(tool.provider_name.clone(), tool.clone())
                .is_some()
        {
            return Err(ProviderError::Rejected);
        }
        let projected_schema = project_codex_argument_schema(&tool.parameters)?;
        let fingerprint = codex_schema_projection_fingerprint(tool, &projected_schema)?;
        fingerprints.insert(tool.provider_name.clone(), fingerprint);
        protocol_values.push(json!({
            "type": "function",
            "name": tool.provider_name,
            "description": tool.description,
            "inputSchema": projected_schema,
            "deferLoading": false,
        }));
    }
    Ok(ProjectedCodexDynamicTools {
        protocol_values,
        definitions,
        fingerprints,
    })
}

fn projected_closed_object_required(
    object: &serde_json::Map<String, Value>,
    properties: &serde_json::Map<String, Value>,
) -> Result<Vec<Value>, ProviderError> {
    let required = match object.get("required") {
        None => Vec::new(),
        Some(Value::Array(values)) => values.clone(),
        Some(_) => return Err(ProviderError::Rejected),
    };
    let mut required_names = BTreeSet::new();
    for value in &required {
        let name = value.as_str().ok_or(ProviderError::Rejected)?;
        if !properties.contains_key(name) || !required_names.insert(name.to_owned()) {
            return Err(ProviderError::Rejected);
        }
    }
    Ok(required)
}

fn project_codex_argument_schema(schema: &Value) -> Result<Value, ProviderError> {
    let object = schema.as_object().ok_or(ProviderError::Rejected)?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "$schema" | "type" | "properties" | "required" | "additionalProperties"
        )
    }) || object
        .get("$schema")
        .is_some_and(|value| value.as_str() != Some("https://json-schema.org/draft/2020-12/schema"))
        || object.get("type").and_then(Value::as_str) != Some("object")
        || object.get("additionalProperties").and_then(Value::as_bool) != Some(false)
    {
        return Err(ProviderError::Rejected);
    }
    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .ok_or(ProviderError::Rejected)?;
    if properties.len() > 128 {
        return Err(ProviderError::Rejected);
    }
    let mut projected_properties = serde_json::Map::new();
    for (name, property) in properties {
        if !valid_identifier(name) {
            return Err(ProviderError::Rejected);
        }
        projected_properties.insert(name.clone(), project_codex_schema_node(property, 0)?);
    }
    let required = projected_closed_object_required(object, properties)?;
    Ok(json!({
        "type": "object",
        "properties": projected_properties,
        "required": required,
        "additionalProperties": false,
    }))
}

fn project_codex_schema_node(schema: &Value, depth: usize) -> Result<Value, ProviderError> {
    if depth > 16 {
        return Err(ProviderError::Rejected);
    }
    let object = schema.as_object().ok_or(ProviderError::Rejected)?;
    if let Some(any_of) = object.get("anyOf") {
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "anyOf" | "description"))
        {
            return Err(ProviderError::Rejected);
        }
        let alternatives = any_of.as_array().ok_or(ProviderError::Rejected)?;
        let [value, null] = alternatives.as_slice() else {
            return Err(ProviderError::Rejected);
        };
        let (value, null) = if null.get("type").and_then(Value::as_str) == Some("null") {
            (value, null)
        } else if value.get("type").and_then(Value::as_str) == Some("null") {
            (null, value)
        } else {
            return Err(ProviderError::Rejected);
        };
        if null.as_object().is_none_or(|object| object.len() != 1) {
            return Err(ProviderError::Rejected);
        }
        let mut projected = project_codex_schema_node(value, depth + 1)?;
        if let Some(description) = object.get("description") {
            let description = description.as_str().ok_or(ProviderError::Rejected)?;
            validate_codex_schema_description(description)?;
            projected
                .as_object_mut()
                .ok_or(ProviderError::Rejected)?
                .insert(
                    "description".to_owned(),
                    Value::String(description.to_owned()),
                );
        }
        return Ok(projected);
    }
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "type"
                | "description"
                | "enum"
                | "minLength"
                | "maxLength"
                | "minimum"
                | "maximum"
                | "properties"
                | "required"
                | "additionalProperties"
                | "items"
                | "minItems"
                | "maxItems"
                | "uniqueItems"
        )
    }) {
        return Err(ProviderError::Rejected);
    }
    let schema_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or(ProviderError::Rejected)?;
    if !matches!(
        schema_type,
        "string" | "integer" | "number" | "boolean" | "object" | "array"
    ) {
        return Err(ProviderError::Rejected);
    }
    let description = object
        .get("description")
        .map(|value| value.as_str().ok_or(ProviderError::Rejected))
        .transpose()?
        .unwrap_or_default();
    validate_codex_schema_description(description)?;
    let mut constraint_notes = Vec::new();
    match schema_type {
        "string" => {
            let minimum = optional_u64(object, "minLength")?;
            let maximum = optional_u64(object, "maxLength")?;
            if minimum
                .zip(maximum)
                .is_some_and(|(minimum, maximum)| minimum > maximum)
            {
                return Err(ProviderError::Rejected);
            }
            if let Some(minimum) = minimum {
                constraint_notes.push(format!("minimum length {minimum}"));
            }
            if let Some(maximum) = maximum {
                constraint_notes.push(format!("maximum length {maximum}"));
            }
        }
        "integer" | "number" => {
            let minimum = optional_number(object, "minimum")?;
            let maximum = optional_number(object, "maximum")?;
            if minimum
                .zip(maximum)
                .is_some_and(|(minimum, maximum)| minimum > maximum)
            {
                return Err(ProviderError::Rejected);
            }
            if let Some(minimum) = minimum {
                constraint_notes.push(format!("minimum {minimum}"));
            }
            if let Some(maximum) = maximum {
                constraint_notes.push(format!("maximum {maximum}"));
            }
        }
        "boolean" => {
            if object.contains_key("minLength")
                || object.contains_key("maxLength")
                || object.contains_key("minimum")
                || object.contains_key("maximum")
            {
                return Err(ProviderError::Rejected);
            }
        }
        "object" => {
            if object.contains_key("enum")
                || object.contains_key("minLength")
                || object.contains_key("maxLength")
                || object.contains_key("minimum")
                || object.contains_key("maximum")
                || object.contains_key("items")
                || object.contains_key("minItems")
                || object.contains_key("maxItems")
                || object.contains_key("uniqueItems")
                || object.get("additionalProperties").and_then(Value::as_bool) != Some(false)
            {
                return Err(ProviderError::Rejected);
            }
            let properties = object
                .get("properties")
                .and_then(Value::as_object)
                .ok_or(ProviderError::Rejected)?;
            if properties.len() > 128 {
                return Err(ProviderError::Rejected);
            }
            let mut projected_properties = serde_json::Map::new();
            for (name, property) in properties {
                if !valid_identifier(name) {
                    return Err(ProviderError::Rejected);
                }
                projected_properties.insert(
                    name.clone(),
                    project_codex_schema_node(property, depth + 1)?,
                );
            }
            let required = projected_closed_object_required(object, properties)?;
            let mut projected = serde_json::Map::from_iter([
                ("type".to_owned(), Value::String("object".to_owned())),
                ("properties".to_owned(), Value::Object(projected_properties)),
                ("required".to_owned(), Value::Array(required)),
                ("additionalProperties".to_owned(), Value::Bool(false)),
            ]);
            if !description.is_empty() {
                projected.insert(
                    "description".to_owned(),
                    Value::String(description.to_owned()),
                );
            }
            return Ok(Value::Object(projected));
        }
        "array" => {
            if object.contains_key("enum")
                || object.contains_key("minLength")
                || object.contains_key("maxLength")
                || object.contains_key("minimum")
                || object.contains_key("maximum")
                || object.contains_key("properties")
                || object.contains_key("required")
                || object.contains_key("additionalProperties")
            {
                return Err(ProviderError::Rejected);
            }
            let items = object.get("items").ok_or(ProviderError::Rejected)?;
            let minimum = optional_u64(object, "minItems")?;
            let maximum = optional_u64(object, "maxItems")?;
            if minimum
                .zip(maximum)
                .is_some_and(|(minimum, maximum)| minimum > maximum)
            {
                return Err(ProviderError::Rejected);
            }
            if object
                .get("uniqueItems")
                .is_some_and(|value| value.as_bool().is_none())
            {
                return Err(ProviderError::Rejected);
            }
            if let Some(minimum) = minimum {
                constraint_notes.push(format!("minimum items {minimum}"));
            }
            if let Some(maximum) = maximum {
                constraint_notes.push(format!("maximum items {maximum}"));
            }
            let mut projected = serde_json::Map::from_iter([
                ("type".to_owned(), Value::String("array".to_owned())),
                (
                    "items".to_owned(),
                    project_codex_schema_node(items, depth + 1)?,
                ),
            ]);
            if object.get("uniqueItems").and_then(Value::as_bool) == Some(true) {
                projected.insert("uniqueItems".to_owned(), Value::Bool(true));
            }
            let projected_description =
                projected_codex_description(description, &constraint_notes)?;
            if !projected_description.is_empty() {
                projected.insert(
                    "description".to_owned(),
                    Value::String(projected_description),
                );
            }
            return Ok(Value::Object(projected));
        }
        _ => unreachable!("schema type was closed above"),
    }
    let mut projected = serde_json::Map::new();
    projected.insert("type".to_owned(), Value::String(schema_type.to_owned()));
    if let Some(values) = object.get("enum") {
        let values = values.as_array().ok_or(ProviderError::Rejected)?;
        if schema_type != "string"
            || values.is_empty()
            || values.len() > 100
            || values.iter().any(|value| {
                value
                    .as_str()
                    .is_none_or(|value| value.is_empty() || value.len() > 200)
            })
        {
            return Err(ProviderError::Rejected);
        }
        projected.insert("enum".to_owned(), Value::Array(values.clone()));
    }
    let projected_description = projected_codex_description(description, &constraint_notes)?;
    if !projected_description.is_empty() {
        projected.insert(
            "description".to_owned(),
            Value::String(projected_description),
        );
    }
    Ok(Value::Object(projected))
}

fn validate_codex_schema_description(description: &str) -> Result<(), ProviderError> {
    if description.len() > 2_000
        || description
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(ProviderError::Rejected);
    }
    Ok(())
}

fn projected_codex_description(
    description: &str,
    constraint_notes: &[String],
) -> Result<String, ProviderError> {
    let mut projected = description.to_owned();
    if !constraint_notes.is_empty() {
        if !projected.is_empty() {
            projected.push(' ');
        }
        projected.push_str("Accepted value constraints: ");
        projected.push_str(&constraint_notes.join(", "));
        projected.push('.');
    }
    if projected.len() > 4_096 {
        return Err(ProviderError::Rejected);
    }
    Ok(projected)
}

fn optional_u64(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, ProviderError> {
    object
        .get(key)
        .map(|value| value.as_u64().ok_or(ProviderError::Rejected))
        .transpose()
}

fn optional_number(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<f64>, ProviderError> {
    object
        .get(key)
        .map(|value| {
            value
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or(ProviderError::Rejected)
        })
        .transpose()
}

fn codex_schema_projection_fingerprint(
    tool: &ModelToolDefinition,
    projected_schema: &Value,
) -> Result<String, ProviderError> {
    let canonical = canonical_json_value(json!({
        "format": "graphql-orm-ai/codex-dynamic-tool-schema-projection/v1",
        "descriptorFingerprint": tool.fingerprint,
        "canonicalSchema": tool.parameters,
        "projectedSchema": projected_schema,
    }));
    let encoded = serde_json::to_vec(&canonical).map_err(|_| ProviderError::Rejected)?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

fn canonical_json_value(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, canonical_json_value(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(values) => {
            Value::Array(values.into_iter().map(canonical_json_value).collect())
        }
        value => value,
    }
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

fn registration_identity(registration: &AiCodexAppServerRegistration) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"graphql-orm-ai/codex-app-server-registration/v3\0");
    for value in [
        registration.provider_profile_id.as_str(),
        registration.logical_model.as_str(),
        registration.executable_sha256.as_str(),
        registration.executable_version.as_str(),
        registration.sandbox_profile.as_str(),
        registration.protocol_version.as_str(),
    ] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    let profile = registration.launch_profile.identity_label();
    hasher.update((profile.len() as u64).to_be_bytes());
    hasher.update(profile.as_bytes());
    hasher.update((registration.bootstrap_instructions.fingerprint().len() as u64).to_be_bytes());
    hasher.update(registration.bootstrap_instructions.fingerprint().as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::path::PathBuf;
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::thread::{self, JoinHandle};

    use agql_auth::{
        AccessTokenMetadata, AuthPrincipal, AuthUser, PrincipalReference, SessionContext,
    };
    use futures::stream;
    use graphql_orm::graphql::orm::{
        AiMutationExecutionPolicy, GraphqlEntitySemanticMetadata, GraphqlOperationCatalog,
        GraphqlOperationKind, GraphqlSemanticArgumentDescriptor, GraphqlSemanticCatalog,
        GraphqlSemanticClassification, GraphqlSemanticExport, GraphqlSemanticFieldMetadata,
        GraphqlSemanticOperationDescriptor, GraphqlSemanticRelationshipCardinality,
        GraphqlSemanticRelationshipDescriptor, GraphqlSemanticResultDisclosure,
        GraphqlSemanticTypeKind, GraphqlSemanticTypeRef, GraphqlSubscriptionConditionField,
        GraphqlSubscriptionConditionOperator, GraphqlSubscriptionObservationDescriptor,
        GraphqlSubscriptionReplayMode,
    };
    use graphql_orm::prelude::*;

    use super::*;
    use crate::{
        AiGraphqlMutationCapabilityCatalog, AiGraphqlQueryCapabilityCatalog,
        AiGraphqlQueryCapabilityLimits, AiGraphqlSubscriptionCapabilityCatalog,
        AiGraphqlSubscriptionCapabilityLimits, AiRunId, AiSessionId, AiToolCatalog, AiToolId,
        GraphqlExecutionTargetId, ProviderDynamicToolResult, ProviderEvent,
    };
    use uuid::Uuid;

    mod canonical_tool_surface {
        use graphql_orm::prelude::*;

        #[derive(
            GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
        )]
        #[graphql_entity(
            table = "codex_profile_inventory",
            plural = "CodexProfileInventory",
            description = "Reviewed inventory records available to application workflows"
        )]
        pub struct CodexProfileInventoryRecord {
            #[primary_key]
            #[filterable(type = "string")]
            #[sortable]
            #[graphql_orm(description = "Stable public inventory identity")]
            pub id: String,
            #[graphql_orm(description = "Human-facing inventory label")]
            pub label: String,
            #[graphql_orm(description = "Internal field excluded from the AI projection")]
            pub internal_marker: String,
        }

        schema_roots! {
            entities: [CodexProfileInventoryRecord],
        }
    }

    struct AdmitCanonicalGeneratedTool;

    impl crate::AiGeneratedGraphqlOperationPolicy for AdmitCanonicalGeneratedTool {
        fn is_application_operation(&self, operation: &GraphqlResolverOperationDescriptor) -> bool {
            operation.entity_name() == "CodexProfileInventoryRecord"
        }
    }

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
            let profile = AiCodexAppServerLaunchProfile::experimental_dynamic_tools_only_v1(
                AiCodexAppServerModelToolMode::Direct,
            )
            .expect("direct-tool live profile should validate");
            let mut command = Command::new(executable);
            command.args(profile.codex_arguments());
            let mut child = command
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
        tool_free: AtomicBool,
        dynamic_arguments: StdMutex<Option<Value>>,
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
                tool_free: AtomicBool::new(false),
                dynamic_arguments: StdMutex::new(None),
            }
        }
    }

    struct FakeFactory {
        counters: Arc<Counters>,
    }

    #[async_trait]
    impl AiCodexAppServerRunProcessFactory for FakeFactory {
        fn supports_launch_profile(&self, _profile: AiCodexAppServerLaunchProfile) -> bool {
            true
        }

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

    struct TextOnlyFactory;

    #[async_trait]
    impl AiCodexAppServerRunProcessFactory for TextOnlyFactory {
        async fn launch(
            &self,
            _registration: Arc<AiCodexAppServerRegistration>,
        ) -> Result<AiCodexAppServerLaunchedProcess, ProviderError> {
            Err(ProviderError::Unavailable)
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
            _bootstrap: &AiCodexAppServerBootstrapInstructions,
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
            if self.counters.pending.load(Ordering::SeqCst) {
                return Ok(Box::pin(stream::pending()));
            }
            if self.counters.tool_free.load(Ordering::SeqCst) {
                return Ok(Box::pin(stream::iter([
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
                ])));
            }
            let definition = input
                .tools()
                .iter()
                .find(|tool| tool.provider_name == "inventory_count")
                .or_else(|| input.tools().first())
                .ok_or(ProviderError::Rejected)?;
            let arguments = self
                .counters
                .dynamic_arguments
                .lock()
                .expect("dynamic argument fixture should not be poisoned")
                .clone()
                .unwrap_or_else(|| json!({"Limit": 3}));
            let Ok(call) = ProviderDynamicToolCall::from_definition(
                "turn-dynamic-1",
                "call-dynamic-1",
                definition,
                arguments.clone(),
            ) else {
                return Ok(Box::pin(stream::iter([
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
                ])));
            };
            let result = responder.respond(call).await?;
            if result.call_id() != "call-dynamic-1" || result.tool_id() != definition.tool_id {
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
                    delta: arguments.to_string(),
                }),
                Ok(ProviderEvent::ToolCallCompleted {
                    call_id: "call-dynamic-1".to_owned(),
                    arguments,
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
            .expect("test registration should validate")
            .with_bootstrap_instructions(trusted_bootstrap()),
        )
    }

    fn dynamic_registration(version: &str) -> Arc<AiCodexAppServerRegistration> {
        let profile = AiCodexAppServerLaunchProfile::experimental_dynamic_tools_only_v1(
            AiCodexAppServerModelToolMode::Direct,
        )
        .expect("direct-tool launch profile should validate");
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
            .with_launch_profile(profile),
        )
    }

    fn bootstrap_instructions() -> AiCodexAppServerBootstrapInstructions {
        AiCodexAppServerBootstrapInstructions::from_static(&[
            "Use the exact registered application tool when it is required to answer the request.",
        ])
        .expect("test bootstrap should validate")
    }

    fn trusted_bootstrap() -> AiCodexAppServerBootstrapInstructions {
        AiCodexAppServerBootstrapInstructions::from_static(&["trusted"])
            .expect("trusted test bootstrap should validate")
    }

    pub(crate) fn canonical_dynamic_tool_catalog() -> (crate::AiToolCatalog, ModelToolDefinition) {
        let operation_catalog = canonical_tool_surface::graphql_orm_operation_catalog();
        let operation = operation_catalog
            .exposed_operations()
            .find(|operation| {
                operation.category() == GeneratedGraphqlOperationCategory::List
                    && operation.entity_name() == "CodexProfileInventoryRecord"
            })
            .expect("generated inventory list operation should exist");
        let arguments = operation
            .arguments()
            .iter()
            .map(|argument| format!("{}: {}", argument.graphql_name(), argument.graphql_type()))
            .collect::<Vec<_>>()
            .join(", ");
        let (page_argument, page_first, page_info, total_count) =
            if cfg!(feature = "graphql-case-pascal") {
                ("Page", "First", "PageInfo", "TotalCount")
            } else {
                ("page", "first", "pageInfo", "totalCount")
            };
        let sdl = format!(
            r#"
            schema {{ query: Query }}
            type Query {{ {}({arguments}): CodexProfileInventoryRecordConnection! }}
            input CodexProfileInventoryRecordWhereInput {{ id: StringFilter }}
            input CodexProfileInventoryRecordOrderByInput {{ id: SortDirection }}
            input PageInput {{ {page_first}: Int }}
            input StringFilter {{ eq: String }}
            enum SortDirection {{ ASC DESC }}
            type CodexProfileInventoryRecordConnection {{
                nodes: [CodexProfileInventoryRecord!]!
                {page_info}: PageInfo!
            }}
            type PageInfo {{ {total_count}: Int! }}
            type CodexProfileInventoryRecord {{ id: String!, label: String!, internalMarker: String! }}
            "#,
            operation.field_name(),
        );
        let disclosure_rule =
            crate::AiDisclosureRule::exportable(crate::DataClassification::Internal);
        let disclosure = crate::AiDisclosureSchema::new(
            "codex-generated-count-v1",
            crate::AiDisclosureShape::object(
                disclosure_rule,
                [(
                    operation.field_name().to_owned(),
                    crate::AiDisclosureShape::object(
                        disclosure_rule,
                        [(
                            page_info.to_owned(),
                            crate::AiDisclosureShape::object(
                                disclosure_rule,
                                [(
                                    total_count.to_owned(),
                                    crate::AiDisclosureShape::scalar(disclosure_rule),
                                )],
                            ),
                        )],
                    ),
                )],
            ),
        )
        .expect("generated inventory disclosure should validate");
        let profile = crate::AiGraphqlToolProfile::read_only(
            "bounded-count",
            operation.field_name(),
            "Count a bounded set of visible inventory records",
            vec![crate::AiGraphqlSelection::object(
                page_info,
                [crate::AiGraphqlSelection::scalar(total_count)],
            )],
            disclosure,
            4_096,
            2,
        )
        .with_inputs([crate::AiGraphqlProfileInput::integer(
            "Limit",
            "Maximum records to consider",
            true,
            1,
            25,
        )])
        .with_arguments([crate::AiGraphqlArgumentPlan::new(
            page_argument,
            crate::AiGraphqlArgumentValue::object([(
                page_first,
                crate::AiGraphqlArgumentValue::input("Limit"),
            )]),
        )]);
        let mut builder = crate::AiGraphqlToolManifestBuilder::new(
            "canonical-inventory-service",
            crate::GraphqlExecutionTargetId::parse("canonical-inventory-graph")
                .expect("generated inventory target should validate"),
            &sdl,
        )
        .expect("generated inventory manifest builder should validate");
        builder
            .add_generated_profile(profile, operation_catalog, &AdmitCanonicalGeneratedTool)
            .expect("generated inventory profile should compile");
        let manifest = builder
            .build()
            .expect("generated inventory manifest should build");
        let tool_id = manifest.entries[0].descriptor.id.clone();
        let mut catalog = crate::AiToolCatalog::new();
        manifest
            .register_into(
                &mut catalog,
                operation_catalog,
                &AdmitCanonicalGeneratedTool,
            )
            .expect("generated inventory manifest should register");
        let definition = catalog
            .read_only_model_definition(&tool_id, "inventory_count")
            .expect("catalog should project the generated definition");
        (catalog, definition)
    }

    fn dynamic_tool() -> ModelToolDefinition {
        canonical_dynamic_tool_catalog().1
    }

    fn mixed_read_surface() -> (
        crate::AiToolCatalog,
        ModelToolDefinition,
        ModelToolDefinition,
        Value,
        ModelRequest,
    ) {
        let (mut catalog, static_definition) = canonical_dynamic_tool_catalog();
        let (compiled, _, generated_definition, generated_plan) =
            generated_relational_query_surface();
        catalog
            .register_query_capability_catalog(&compiled)
            .expect("generated query catalogue should join the static catalogue");
        let request = ModelRequest {
            instructions: Vec::new(),
            continuation_mode: ModelContinuationMode::ProviderRetained,
            tools: vec![static_definition.clone(), generated_definition.clone()],
            ..model_request()
        };
        (
            catalog,
            static_definition,
            generated_definition,
            generated_plan,
            request,
        )
    }

    fn mixed_registration() -> Arc<AiCodexAppServerRegistration> {
        Arc::new(
            (*dynamic_registration("1.0.0"))
                .clone()
                .with_bootstrap_instructions(bootstrap_instructions()),
        )
    }

    struct MixedDynamicResponder {
        generated: ModelToolDefinition,
        plan: Value,
    }

    #[async_trait]
    impl ProviderDynamicToolResponder for MixedDynamicResponder {
        async fn respond(
            &self,
            call: ProviderDynamicToolCall,
        ) -> Result<ProviderDynamicToolResult, ProviderError> {
            if call.tool_id() == self.generated.tool_id
                && call.tool_fingerprint() == self.generated.fingerprint
                && call.arguments() == &self.plan
            {
                return ProviderDynamicToolResult::new(&call, json!({"ok": true}));
            }
            let expected = dynamic_tool();
            if call.tool_id() == expected.tool_id
                && call.tool_fingerprint() == expected.fingerprint
                && call.arguments() == &json!({"Limit": 3})
            {
                return ProviderDynamicToolResult::new(&call, json!({"count": 3}));
            }
            Err(ProviderError::Rejected)
        }
    }

    struct FakeDynamicResponder;

    #[async_trait]
    impl ProviderDynamicToolResponder for FakeDynamicResponder {
        async fn respond(
            &self,
            call: ProviderDynamicToolCall,
        ) -> Result<ProviderDynamicToolResult, ProviderError> {
            let expected = dynamic_tool();
            if call.response_id() != "turn-dynamic-1"
                || call.call_id() != "call-dynamic-1"
                || call.tool_id() != expected.tool_id
                || call.provider_name() != expected.provider_name
                || call.tool_fingerprint() != expected.fingerprint
                || call.arguments() != &json!({"Limit": 3})
            {
                return Err(ProviderError::Rejected);
            }
            ProviderDynamicToolResult::new(&call, json!({"count": 3}))
        }
    }

    struct LiveDynamicResponder;

    #[async_trait]
    impl ProviderDynamicToolResponder for LiveDynamicResponder {
        async fn respond(
            &self,
            call: ProviderDynamicToolCall,
        ) -> Result<ProviderDynamicToolResult, ProviderError> {
            let expected = dynamic_tool();
            if call.tool_id() != expected.tool_id
                || call.provider_name() != expected.provider_name
                || call.tool_fingerprint() != expected.fingerprint
                || call.arguments() != &json!({"Limit": 3})
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
    fn retained_bootstrap_is_static_registration_bound_and_active_on_first_turn() {
        let bootstrap = bootstrap_instructions();
        let without_bootstrap = dynamic_registration("1.0.0");
        let with_bootstrap = AiCodexAppServerRegistration::new(
            "profile-1",
            "model-1",
            "a".repeat(64),
            "1.0.0",
            "sandbox-empty",
            AI_CODEX_APP_SERVER_PROTOCOL_V2,
        )
        .expect("registration should validate")
        .with_launch_profile(
            AiCodexAppServerLaunchProfile::experimental_dynamic_tools_only_v1(
                AiCodexAppServerModelToolMode::Direct,
            )
            .expect("direct profile should validate"),
        )
        .with_bootstrap_instructions(bootstrap.clone());
        assert_ne!(without_bootstrap.identity(), with_bootstrap.identity());

        let mut actor = initialized_protocol_actor();
        let frame = actor
            .start_persistent_empty_thread("model-1", &bootstrap, &[dynamic_tool()])
            .expect("static bootstrap thread should encode");
        let value: Value = serde_json::from_slice(&frame).expect("frame should be JSON");
        assert_eq!(
            value.pointer("/params/developerInstructions"),
            Some(&Value::String(
                "Use the exact registered application tool when it is required to answer the request."
                    .to_owned(),
            ))
        );
        assert!(value.pointer("/params/input").is_none());
        actor
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-bootstrap"}}}"#)
            .expect("thread response should bind");
        actor
            .accept(&thread_started_notification("thread-bootstrap"))
            .expect("thread notification should bind");

        let mut request = dynamic_model_request();
        request.instructions.clear();
        let input = AiCodexAppServerTurnInput::try_from_retained_dynamic_request(
            request.clone(),
            &bootstrap,
        )
        .expect("registration bootstrap should project into retained input");
        actor
            .start_turn("thread-bootstrap", &input)
            .expect("first bound turn should retain the static bootstrap");

        request.instructions = vec!["request-local replacement".to_owned()];
        assert!(matches!(
            AiCodexAppServerTurnInput::try_from_retained_dynamic_request(request, &bootstrap),
            Err(ProviderError::Rejected)
        ));
        let mut exact_bootstrap = dynamic_model_request();
        exact_bootstrap.instructions = vec![
            "Use the exact registered application tool when it is required to answer the request."
                .to_owned(),
        ];
        AiCodexAppServerTurnInput::try_from_retained_dynamic_request(exact_bootstrap, &bootstrap)
            .expect("an exact registration bootstrap copy is not request-local");
        assert!(AiCodexAppServerBootstrapInstructions::from_static(&["bad\0text"]).is_err());
    }

    #[test]
    fn canonical_generated_schema_projects_closed_bounds_for_codex() {
        let tool = dynamic_tool();
        assert_eq!(
            tool.parameters.pointer("/$schema").and_then(Value::as_str),
            Some("https://json-schema.org/draft/2020-12/schema")
        );
        assert_eq!(
            tool.parameters
                .pointer("/properties/Limit/minimum")
                .and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            tool.parameters
                .pointer("/properties/Limit/maximum")
                .and_then(Value::as_i64),
            Some(25)
        );
        let projected = project_codex_dynamic_tools(std::slice::from_ref(&tool))
            .expect("canonical generated schema should project");
        let schema = &projected.protocol_values[0]["inputSchema"];
        assert!(schema.get("$schema").is_none());
        assert!(schema.pointer("/properties/Limit/minimum").is_none());
        assert!(schema.pointer("/properties/Limit/maximum").is_none());
        let description = schema
            .pointer("/properties/Limit/description")
            .and_then(Value::as_str)
            .expect("Codex projection should preserve constraints semantically");
        assert!(description.contains("minimum 1"));
        assert!(description.contains("maximum 25"));

        let mut reordered = tool.clone();
        reordered.parameters = json!({
            "additionalProperties": false,
            "required": ["Limit"],
            "properties": {
                "Limit": {
                    "maximum": 25,
                    "minimum": 1,
                    "description": "Maximum records to consider",
                    "type": "integer"
                }
            },
            "type": "object",
            "$schema": "https://json-schema.org/draft/2020-12/schema"
        });
        let reordered_projection = project_codex_dynamic_tools(&[reordered])
            .expect("object-key ordering must not alter projection");
        assert_eq!(projected.fingerprints, reordered_projection.fingerprints);

        let mut substituted = tool;
        substituted.fingerprint = "f".repeat(64);
        let substituted_projection = project_codex_dynamic_tools(&[substituted])
            .expect("syntactically valid substituted descriptor should project distinctly");
        assert_ne!(projected.fingerprints, substituted_projection.fingerprints);
    }

    #[test]
    fn finite_relational_query_plan_projects_without_generic_schema_passthrough() {
        let tool = ModelToolDefinition {
            tool_id: "inventory.query.records.auto".to_owned(),
            provider_name: "inventory_records".to_owned(),
            fingerprint: "a".repeat(64),
            description: "Read reviewed inventory records.".to_owned(),
            parameters: json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "arguments": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "anyOf": [
                                    { "type": "string", "maxLength": 80 },
                                    { "type": "null" }
                                ]
                            }
                        },
                        "required": [],
                        "additionalProperties": false
                    },
                    "fields": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["id", "name"] },
                        "uniqueItems": true,
                        "maxItems": 2
                    },
                    "relationships": {
                        "type": "object",
                        "properties": {
                            "children": {
                                "type": "object",
                                "properties": {
                                    "fields": {
                                        "type": "array",
                                        "items": { "type": "string", "enum": ["id"] },
                                        "maxItems": 1,
                                        "uniqueItems": true
                                    },
                                    "maximumItems": {
                                        "type": "integer",
                                        "minimum": 1,
                                        "maximum": 25
                                    }
                                },
                                "required": ["fields", "maximumItems"],
                                "additionalProperties": false
                            }
                        },
                        "required": [],
                        "additionalProperties": false
                    }
                },
                "required": ["arguments", "fields", "relationships"],
                "additionalProperties": false
            }),
            strict: true,
        };
        let projected = project_codex_dynamic_tools(&[tool])
            .expect("finite nested capability schema should project");
        let schema = &projected.protocol_values[0]["inputSchema"];
        assert_eq!(
            schema
                .pointer("/properties/relationships/properties/children/type")
                .and_then(Value::as_str),
            Some("object")
        );
        assert_eq!(
            schema
                .pointer("/properties/fields/items/enum/1")
                .and_then(Value::as_str),
            Some("name")
        );
        assert!(
            schema
                .pointer("/properties/relationships/properties/children/properties/maximumItems/description")
                .and_then(Value::as_str)
                .is_some_and(|description| description.contains("maximum 25"))
        );
        assert!(schema.to_string().find("anyOf").is_none());
    }

    fn named_semantic_type(name: &str, nullable: bool) -> GraphqlSemanticTypeRef {
        GraphqlSemanticTypeRef::named(name, GraphqlSemanticTypeKind::Scalar, nullable)
    }

    fn exportable_scalar_field(
        name: &str,
        classification: GraphqlSemanticClassification,
    ) -> GraphqlSemanticFieldMetadata {
        GraphqlSemanticFieldMetadata {
            field_name: name.to_owned(),
            description: format!("Reviewed public {name}."),
            type_ref: named_semantic_type("String", false),
            selectable: true,
            filter_operators: Vec::new(),
            sortable: false,
            groupable: false,
            aggregate_operators: Vec::new(),
            aggregate_value_kind: None,
            relationship: None,
            classification,
            export: GraphqlSemanticExport::Exportable,
            has_field_policy: false,
        }
    }

    fn generated_relational_query_surface() -> (
        crate::AiGraphqlQueryCapabilityCatalog,
        AiToolCatalog,
        ModelToolDefinition,
        Value,
    ) {
        const SDL: &str = r#"
            schema { query: Query }
            type Query { ReadParent(id: ID!): Parent! }
            type Parent {
              id: String!
              name: String!
              children(page: PageInput): ChildConnection!
            }
            type Child { id: String!, label: String! }
            type ChildConnection { edges: [ChildEdge!]!, pageInfo: PageInfo! }
            type ChildEdge { node: Child!, cursor: String! }
            type PageInfo { totalCount: Int!, hasNextPage: Boolean! }
            input PageInput { limit: Int, offset: Int }
        "#;
        let child = GraphqlEntitySemanticMetadata {
            entity_name: "Child".to_owned(),
            description: "A bounded child record.".to_owned(),
            default_classification: GraphqlSemanticClassification::Internal,
            fields: vec![
                exportable_scalar_field("id", GraphqlSemanticClassification::Internal),
                exportable_scalar_field("label", GraphqlSemanticClassification::Confidential),
            ]
            .into_boxed_slice(),
        };
        let parent = GraphqlEntitySemanticMetadata {
            entity_name: "Parent".to_owned(),
            description: "A reviewed parent record.".to_owned(),
            default_classification: GraphqlSemanticClassification::Internal,
            fields: vec![
                exportable_scalar_field("id", GraphqlSemanticClassification::Internal),
                exportable_scalar_field("name", GraphqlSemanticClassification::Confidential),
                GraphqlSemanticFieldMetadata {
                    field_name: "children".to_owned(),
                    description: "Bounded related child records.".to_owned(),
                    type_ref: GraphqlSemanticTypeRef::list(
                        false,
                        Some(10),
                        GraphqlSemanticTypeRef::named(
                            "Child",
                            GraphqlSemanticTypeKind::Object,
                            false,
                        ),
                    ),
                    selectable: true,
                    filter_operators: Vec::new(),
                    sortable: false,
                    groupable: false,
                    aggregate_operators: Vec::new(),
                    aggregate_value_kind: None,
                    relationship: Some(GraphqlSemanticRelationshipDescriptor {
                        target: "Child".to_owned(),
                        cardinality: GraphqlSemanticRelationshipCardinality::Many,
                        arguments: vec![GraphqlSemanticArgumentDescriptor {
                            graphql_name: "page".to_owned(),
                            description: "Bounded relationship page.".to_owned(),
                            type_ref: GraphqlSemanticTypeRef::named(
                                "PageInput",
                                GraphqlSemanticTypeKind::Object,
                                true,
                            ),
                        }],
                    }),
                    classification: GraphqlSemanticClassification::Confidential,
                    export: GraphqlSemanticExport::Exportable,
                    has_field_policy: true,
                },
            ]
            .into_boxed_slice(),
        };
        let operation = GraphqlSemanticOperationDescriptor::custom(
            GraphqlOperationKind::Query,
            "ReadParent",
            "Read one reviewed parent record.",
            vec![GraphqlSemanticArgumentDescriptor {
                graphql_name: "id".to_owned(),
                description: "Exact public parent identity.".to_owned(),
                type_ref: named_semantic_type("ID", false),
            }],
            GraphqlSemanticTypeRef::named("Parent", GraphqlSemanticTypeKind::Object, false),
            true,
        )
        .expect("relational query semantics should validate");
        let semantics = GraphqlSemanticCatalog::compose_with_custom(
            [parent, child],
            &GraphqlOperationCatalog::compose(std::iter::empty()),
            [operation],
        )
        .expect("relational semantic catalogue should validate");
        let compiled = AiGraphqlQueryCapabilityCatalog::compile(
            "inventory",
            GraphqlExecutionTargetId::parse("inventory.graphql")
                .expect("relational target should validate"),
            SDL,
            &semantics,
            AiGraphqlQueryCapabilityLimits::default(),
        )
        .expect("relational query capabilities should compile");
        let mut catalog = AiToolCatalog::new();
        catalog
            .register_query_capability_catalog(&compiled)
            .expect("relational query catalogue should register");
        let capability = compiled
            .capabilities()
            .next()
            .expect("relational query capability should exist");
        let definition = catalog
            .query_capability_model_definition(capability.id(), "read_parent")
            .expect("relational query definition should project");
        let plan = json!({
            "arguments": { "id": "parent-1" },
            "fields": { "id": true, "name": true },
            "relationships": {
                "children": {
                    "arguments": {},
                    "fields": { "id": true, "label": true },
                    "relationships": {},
                    "maximumItems": 2
                }
            }
        });
        (compiled, catalog, definition, plan)
    }

    fn generated_scalar_query_definition() -> ModelToolDefinition {
        const SDL: &str = r#"
            schema { query: Query }
            type Query { Health: String! }
        "#;
        let operation = GraphqlSemanticOperationDescriptor::custom(
            GraphqlOperationKind::Query,
            "Health",
            "Read bounded public health status.",
            Vec::new(),
            named_semantic_type("String", false),
            true,
        )
        .expect("scalar query semantics should validate")
        .with_result_disclosure(GraphqlSemanticResultDisclosure::new(
            GraphqlSemanticClassification::Public,
            GraphqlSemanticExport::Exportable,
        ))
        .expect("scalar result disclosure should validate");
        let semantics = GraphqlSemanticCatalog::compose_with_custom(
            [],
            &GraphqlOperationCatalog::compose(std::iter::empty()),
            [operation],
        )
        .expect("scalar semantic catalogue should validate");
        let compiled = AiGraphqlQueryCapabilityCatalog::compile(
            "inventory",
            GraphqlExecutionTargetId::parse("inventory.graphql")
                .expect("scalar target should validate"),
            SDL,
            &semantics,
            AiGraphqlQueryCapabilityLimits::default(),
        )
        .expect("scalar query capabilities should compile");
        let mut catalog = AiToolCatalog::new();
        catalog
            .register_query_capability_catalog(&compiled)
            .expect("scalar query catalogue should register");
        catalog
            .query_capability_model_definition(
                compiled
                    .capabilities()
                    .next()
                    .expect("scalar query capability should exist")
                    .id(),
                "health_status",
            )
            .expect("scalar query definition should project")
    }

    fn generated_automatic_mutation_definition() -> ModelToolDefinition {
        const SDL: &str = r#"
            schema { query: Query, mutation: Mutation }
            type Query { Health: String! }
            type Mutation { CreateParent(input: ParentMutationInput!): Parent! }
            type Parent { id: String!, name: String! }
            input ParentMutationInput { name: String! }
        "#;
        let parent = GraphqlEntitySemanticMetadata {
            entity_name: "Parent".to_owned(),
            description: "A reviewed parent record.".to_owned(),
            default_classification: GraphqlSemanticClassification::Internal,
            fields: vec![
                exportable_scalar_field("id", GraphqlSemanticClassification::Internal),
                exportable_scalar_field("name", GraphqlSemanticClassification::Confidential),
            ]
            .into_boxed_slice(),
        };
        let mutation = GraphqlSemanticOperationDescriptor::custom(
            GraphqlOperationKind::Mutation,
            "CreateParent",
            "Create one reviewed parent record.",
            vec![GraphqlSemanticArgumentDescriptor {
                graphql_name: "input".to_owned(),
                description: "Reviewed parent mutation input.".to_owned(),
                type_ref: GraphqlSemanticTypeRef::named(
                    "ParentMutationInput",
                    GraphqlSemanticTypeKind::Object,
                    false,
                ),
            }],
            GraphqlSemanticTypeRef::named("Parent", GraphqlSemanticTypeKind::Object, false),
            true,
        )
        .expect("mutation semantics should validate")
        .with_ai_mutation_execution(AiMutationExecutionPolicy::Automatic)
        .expect("automatic mutation classification should validate");
        let semantics = GraphqlSemanticCatalog::compose_with_custom(
            [parent],
            &GraphqlOperationCatalog::compose(std::iter::empty()),
            [mutation],
        )
        .expect("mutation semantic catalogue should validate");
        let compiled = AiGraphqlMutationCapabilityCatalog::compile(
            "inventory",
            GraphqlExecutionTargetId::parse("inventory.graphql")
                .expect("mutation target should validate"),
            SDL,
            &semantics,
            AiGraphqlQueryCapabilityLimits::default(),
        )
        .expect("automatic mutation capabilities should compile");
        let mut catalog = AiToolCatalog::new();
        catalog
            .register_mutation_capability_catalog(&compiled)
            .expect("mutation catalogue should register");
        catalog
            .mutation_capability_model_definition(
                compiled
                    .capabilities()
                    .next()
                    .expect("automatic mutation should exist")
                    .id(),
                "create_parent",
            )
            .expect("mutation definition should project")
    }

    fn generated_subscription_definition() -> ModelToolDefinition {
        const SDL: &str = r#"
            schema { query: Query, subscription: Subscription }
            type Query { Health: Boolean! }
            type Subscription { ParentChanged: Parent! }
            type Parent { id: String!, name: String! }
        "#;
        let parent = GraphqlEntitySemanticMetadata {
            entity_name: "Parent".to_owned(),
            description: "A reviewed parent event.".to_owned(),
            default_classification: GraphqlSemanticClassification::Internal,
            fields: vec![
                exportable_scalar_field("id", GraphqlSemanticClassification::Internal),
                exportable_scalar_field("name", GraphqlSemanticClassification::Confidential),
            ]
            .into_boxed_slice(),
        };
        let subscription = GraphqlSemanticOperationDescriptor::custom(
            GraphqlOperationKind::Subscription,
            "ParentChanged",
            "Observe reviewed parent changes.",
            Vec::new(),
            GraphqlSemanticTypeRef::named("Parent", GraphqlSemanticTypeKind::Object, false),
            true,
        )
        .expect("subscription semantics should validate")
        .with_subscription_observation(GraphqlSubscriptionObservationDescriptor {
            replay_mode: GraphqlSubscriptionReplayMode::ReplayThenLive,
            maximum_duration_seconds: Some(120),
            maximum_events: Some(20),
            condition_fields: vec![GraphqlSubscriptionConditionField {
                field_name: "id".to_owned(),
                operators: vec![GraphqlSubscriptionConditionOperator::Equal],
            }],
        })
        .expect("replayable subscription should validate");
        let semantics = GraphqlSemanticCatalog::compose_with_custom(
            [parent],
            &GraphqlOperationCatalog::compose(std::iter::empty()),
            [subscription],
        )
        .expect("subscription semantic catalogue should validate");
        let compiled = AiGraphqlSubscriptionCapabilityCatalog::compile(
            "inventory",
            GraphqlExecutionTargetId::parse("inventory.graphql")
                .expect("subscription target should validate"),
            SDL,
            &semantics,
            AiGraphqlSubscriptionCapabilityLimits::default(),
        )
        .expect("subscription capabilities should compile");
        let mut catalog = AiToolCatalog::new();
        catalog
            .register_subscription_capability_catalog(&compiled)
            .expect("subscription catalogue should register");
        catalog
            .subscription_capability_model_definition(
                compiled
                    .capabilities()
                    .next()
                    .expect("subscription capability should exist")
                    .id(),
                "parent_changed",
            )
            .expect("subscription definition should project")
    }

    fn closed_object_schema(required: Option<Value>) -> Value {
        let mut schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "optional": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        });
        if let Some(required) = required {
            schema
                .as_object_mut()
                .expect("fixture schema should be an object")
                .insert("required".to_owned(), required);
        }
        schema
    }

    fn assert_projected_objects_are_closed(schema: &Value) {
        match schema {
            Value::Object(object) => {
                if object.get("type").and_then(Value::as_str) == Some("object") {
                    assert_eq!(
                        object.get("additionalProperties"),
                        Some(&Value::Bool(false))
                    );
                    let required = object
                        .get("required")
                        .and_then(Value::as_array)
                        .expect("projected objects must emit a required array");
                    let properties = object
                        .get("properties")
                        .and_then(Value::as_object)
                        .expect("projected objects must emit properties");
                    let mut names = BTreeSet::new();
                    for value in required {
                        let name = value.as_str().expect("required names must be strings");
                        assert!(properties.contains_key(name));
                        assert!(names.insert(name));
                    }
                }
                for value in object.values() {
                    assert_projected_objects_are_closed(value);
                }
            }
            Value::Array(values) => {
                for value in values {
                    assert_projected_objects_are_closed(value);
                }
            }
            _ => {}
        }
    }

    fn start_bound_dynamic_thread(
        actor: &mut AiCodexAppServerProtocolActor,
        tools: &[ModelToolDefinition],
    ) -> String {
        actor
            .start_persistent_empty_thread(
                "model-1",
                &AiCodexAppServerBootstrapInstructions::disabled(),
                tools,
            )
            .expect("generated query definition set should start a persistent empty thread");
        actor
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-generated-1"}}}"#)
            .expect("thread response should bind");
        actor
            .accept(&thread_started_notification("thread-generated-1"))
            .expect("thread notification should bind");
        "thread-generated-1".to_owned()
    }

    fn start_bound_dynamic_turn(
        actor: &mut AiCodexAppServerProtocolActor,
        thread_id: &str,
        input: &AiCodexAppServerTurnInput,
        turn_id: &str,
        request_id: u64,
    ) {
        actor
            .start_turn(thread_id, input)
            .expect("generated query turn should start");
        let response = serde_json::to_vec(&json!({
            "id": request_id,
            "result": { "turn": { "id": turn_id } }
        }))
        .expect("turn response should encode");
        actor.accept(&response).expect("turn response should bind");
        actor
            .accept(&turn_started_notification(thread_id, turn_id))
            .expect("turn notification should bind");
    }

    #[test]
    fn generated_query_catalog_omits_empty_required_and_codex_emits_the_empty_set() {
        let (_compiled, catalog, definition, plan) = generated_relational_query_surface();
        assert!(
            definition
                .parameters
                .pointer("/properties/relationships/required")
                .is_none()
        );
        assert_eq!(
            definition
                .parameters
                .pointer("/properties/relationships/additionalProperties"),
            Some(&Value::Bool(false))
        );
        assert!(
            definition
                .parameters
                .pointer("/properties/relationships/properties/children/properties/relationships/required")
                .is_none()
        );
        assert_eq!(
            definition.parameters.pointer(
                "/properties/relationships/properties/children/properties/relationships/additionalProperties"
            ),
            Some(&Value::Bool(false))
        );

        let projected = project_codex_dynamic_tools(std::slice::from_ref(&definition))
            .expect("canonical generated query schema should project");
        let schema = &projected.protocol_values[0]["inputSchema"];
        assert_eq!(
            schema.pointer("/properties/relationships/required"),
            Some(&json!([]))
        );
        assert_eq!(
            schema.pointer("/properties/relationships/additionalProperties"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            schema.pointer(
                "/properties/relationships/properties/children/properties/relationships/required"
            ),
            Some(&json!([]))
        );
        assert_eq!(
            schema.pointer(
                "/properties/relationships/properties/children/properties/relationships/additionalProperties"
            ),
            Some(&Value::Bool(false))
        );
        assert_projected_objects_are_closed(schema);

        let scalar = generated_scalar_query_definition();
        assert!(
            scalar
                .parameters
                .pointer("/properties/fields/required")
                .is_none()
        );
        assert!(
            scalar
                .parameters
                .pointer("/properties/relationships/required")
                .is_none()
        );
        let scalar_projected = project_codex_dynamic_tools(std::slice::from_ref(&scalar))
            .expect("scalar generated query empty objects should project");
        assert_eq!(
            scalar_projected.protocol_values[0].pointer("/inputSchema/properties/fields/required"),
            Some(&json!([]))
        );
        assert_eq!(
            scalar_projected.protocol_values[0]
                .pointer("/inputSchema/properties/relationships/required"),
            Some(&json!([]))
        );
        assert_projected_objects_are_closed(&scalar_projected.protocol_values[0]["inputSchema"]);

        let compiled = catalog
            .compile_query_capability(
                &AiToolId::parse(&definition.tool_id).expect("capability ID should parse"),
                &definition.fingerprint,
                plan,
            )
            .expect("nested generated relationship plan should compile");
        assert!(compiled.descriptor().document.contains("children"));
    }

    #[test]
    fn generated_and_mixed_definition_sets_start_persistent_empty_threads() {
        let (_, _, generated, _) = generated_relational_query_surface();
        let static_tool = dynamic_tool();
        let mut generated_only = initialized_protocol_actor();
        let _ = start_bound_dynamic_thread(&mut generated_only, std::slice::from_ref(&generated));

        let mut mixed = initialized_protocol_actor();
        let mixed_tools = [static_tool, generated];
        let create = String::from_utf8(
            mixed
                .start_persistent_empty_thread(
                    "model-1",
                    &AiCodexAppServerBootstrapInstructions::disabled(),
                    &mixed_tools,
                )
                .expect("mixed static and generated definitions should start"),
        )
        .expect("create frame should be UTF-8");
        assert!(create.contains("\"dynamicTools\""));
        assert!(create.contains("inventory_count"));
        assert!(create.contains("read_parent"));
    }

    #[test]
    fn generated_nested_plan_follows_catalog_definition_dynamic_call_and_compile_path() {
        let (_compiled, catalog, definition, plan) = generated_relational_query_surface();
        let mut actor = initialized_protocol_actor();
        let thread_id = start_bound_dynamic_thread(&mut actor, std::slice::from_ref(&definition));
        let input = AiCodexAppServerTurnInput::try_from_retained_dynamic_request(
            ModelRequest {
                instructions: Vec::new(),
                continuation_mode: ModelContinuationMode::ProviderRetained,
                tools: vec![definition.clone()],
                ..model_request()
            },
            &AiCodexAppServerBootstrapInstructions::disabled(),
        )
        .expect("generated query retained input should validate");
        start_bound_dynamic_turn(&mut actor, &thread_id, &input, "turn-generated-1", 3);

        let started = lifecycle_notification(
            "item/started",
            json!({
                "item": {
                    "arguments": plan,
                    "id": "call-generated-1",
                    "namespace": null,
                    "status": "inProgress",
                    "tool": "read_parent",
                    "type": "dynamicToolCall"
                },
                "startedAtMs": 1,
                "threadId": thread_id,
                "turnId": "turn-generated-1"
            }),
        );
        assert!(matches!(
            actor.accept(&started),
            Ok(AiCodexAppServerInbound::DynamicToolLifecycle {
                completed: false,
                ..
            })
        ));

        let request = serde_json::to_vec(&json!({
            "id": 0,
            "method": "item/tool/call",
            "params": {
                "arguments": plan,
                "callId": "call-generated-1",
                "namespace": null,
                "threadId": thread_id,
                "tool": "read_parent",
                "turnId": "turn-generated-1"
            }
        }))
        .expect("generated query call should encode");
        let (request_id, call) = match actor
            .accept(&request)
            .expect("canonical generated nested plan should be admitted")
        {
            AiCodexAppServerInbound::DynamicToolCall {
                request_id, call, ..
            } => (request_id, call),
            other => panic!("unexpected inbound: {other:?}"),
        };
        assert_eq!(call.tool_id(), definition.tool_id);
        assert_eq!(call.tool_fingerprint(), definition.fingerprint);
        let compiled = catalog
            .compile_query_capability(
                &AiToolId::parse(call.tool_id()).expect("admitted capability ID should parse"),
                call.tool_fingerprint(),
                call.arguments().clone(),
            )
            .expect("admitted generated plan should compile");
        assert!(
            compiled
                .descriptor()
                .document
                .contains("children(page: $v1) { edges { node { id label } } }")
        );
        let result = ProviderDynamicToolResult::new(&call, json!({"ok": true}))
            .expect("compiled generated result should validate");
        actor
            .dynamic_tool_response(request_id, &result)
            .expect("exact generated responder path should encode");
    }

    #[test]
    fn generated_query_retained_create_and_resume_keep_the_frozen_definition_set() {
        let (_, _, definition, _) = generated_relational_query_surface();
        let input = AiCodexAppServerTurnInput::try_from_retained_dynamic_request(
            ModelRequest {
                instructions: Vec::new(),
                continuation_mode: ModelContinuationMode::ProviderRetained,
                tools: vec![definition.clone()],
                ..model_request()
            },
            &AiCodexAppServerBootstrapInstructions::disabled(),
        )
        .expect("frozen generated input should validate");
        let mut actor = initialized_protocol_actor();
        let thread_id = start_bound_dynamic_thread(&mut actor, input.tools());
        start_bound_dynamic_turn(&mut actor, &thread_id, &input, "turn-generated-1", 3);
        actor
            .accept(&turn_completed_notification(&thread_id, "turn-generated-1"))
            .expect("first generated turn should complete");

        let cursor = crate::AiProviderSessionCursor::new("codex.app_server.thread.v2", &thread_id)
            .expect("generated cursor should validate");
        actor
            .resume_thread(&cursor, &input)
            .expect("unchanged frozen generated definitions should resume");
        actor
            .accept(br#"{"id":4,"result":{"thread":{"id":"thread-generated-1"}}}"#)
            .expect("resume response should bind");
        actor
            .accept(&thread_started_notification(&thread_id))
            .expect("resume notification should bind");
        start_bound_dynamic_turn(&mut actor, &thread_id, &input, "turn-generated-2", 5);

        let mut changed = definition;
        changed.description = "Changed after binding.".to_owned();
        let changed_input = AiCodexAppServerTurnInput::try_from_retained_dynamic_request(
            ModelRequest {
                instructions: Vec::new(),
                continuation_mode: ModelContinuationMode::ProviderRetained,
                tools: vec![changed],
                ..model_request()
            },
            &AiCodexAppServerBootstrapInstructions::disabled(),
        )
        .expect("changed generated definition remains structurally valid");
        actor
            .accept(&turn_completed_notification(&thread_id, "turn-generated-2"))
            .expect("second generated turn should complete");
        assert!(matches!(
            actor.resume_thread(&cursor, &changed_input),
            Err(ProviderError::Rejected)
        ));
        actor
            .resume_thread(&cursor, &input)
            .expect("exact frozen generated definitions should remain resumable");
    }

    #[test]
    fn codex_projection_rejects_malformed_required_and_unrepresentable_generated_shapes() {
        let (_compiled, catalog, definition, plan) = generated_relational_query_surface();
        let catalog_id = AiToolId::parse(&definition.tool_id).expect("capability ID should parse");
        assert!(matches!(
            catalog.compile_query_capability(&catalog_id, "stale-fingerprint", plan.clone()),
            Err(crate::AiError::Forbidden)
        ));

        let mutation = generated_automatic_mutation_definition();
        let mutation_id = AiToolId::parse(&mutation.tool_id).expect("mutation ID should parse");
        assert!(matches!(
            catalog.query_capability_model_definition(&mutation_id, "create_parent"),
            Err(crate::AiError::Forbidden)
        ));
        assert!(matches!(
            catalog.compile_query_capability(&mutation_id, &mutation.fingerprint, plan),
            Err(crate::AiError::Forbidden)
        ));

        let subscription = generated_subscription_definition();
        assert!(
            serde_json::to_string(&subscription.parameters)
                .expect("subscription schema should encode")
                .contains("oneOf")
        );
        assert!(matches!(
            project_codex_dynamic_tools(std::slice::from_ref(&subscription)),
            Err(ProviderError::Rejected)
        ));

        for required in [
            json!("fields"),
            json!({"fields": true}),
            json!([1]),
            json!(["fields", "fields"]),
            json!(["unknown"]),
        ] {
            let mut malformed = definition.clone();
            malformed.parameters = closed_object_schema(Some(required));
            malformed
                .parameters
                .as_object_mut()
                .expect("schema")
                .insert(
                    "properties".to_owned(),
                    json!({
                        "fields": {
                            "type": "object",
                            "properties": {},
                            "additionalProperties": false
                        }
                    }),
                );
            assert!(
                matches!(
                    project_codex_dynamic_tools(std::slice::from_ref(&malformed)),
                    Err(ProviderError::Rejected)
                ),
                "malformed required must remain rejected"
            );
        }

        let mut unknown_keyword = definition.clone();
        unknown_keyword.parameters = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "fields": { "$ref": "#/definitions/fields" }
            },
            "required": ["fields"],
            "additionalProperties": false
        });
        assert!(matches!(
            project_codex_dynamic_tools(std::slice::from_ref(&unknown_keyword)),
            Err(ProviderError::Rejected)
        ));

        let mut unbounded = definition.clone();
        unbounded.parameters = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "fields": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": true
                }
            },
            "required": [],
            "additionalProperties": false
        });
        assert!(matches!(
            project_codex_dynamic_tools(std::slice::from_ref(&unbounded)),
            Err(ProviderError::Rejected)
        ));

        let omitted = ModelToolDefinition {
            tool_id: "closed.optional".to_owned(),
            provider_name: "closed_optional".to_owned(),
            fingerprint: "b".repeat(64),
            description: "Closed optional object.".to_owned(),
            parameters: closed_object_schema(None),
            strict: true,
        };
        let explicit_empty = ModelToolDefinition {
            parameters: closed_object_schema(Some(json!([]))),
            ..omitted.clone()
        };
        let omitted_projection = project_codex_dynamic_tools(std::slice::from_ref(&omitted))
            .expect("omitted required is the empty set");
        let explicit_projection =
            project_codex_dynamic_tools(std::slice::from_ref(&explicit_empty))
                .expect("explicit empty required remains admitted");
        assert_eq!(
            omitted_projection.protocol_values[0]["inputSchema"],
            explicit_projection.protocol_values[0]["inputSchema"]
        );
        assert_eq!(
            omitted_projection.protocol_values[0].pointer("/inputSchema/required"),
            Some(&json!([]))
        );
        assert_eq!(
            omitted_projection.protocol_values[0]
                .pointer("/inputSchema/properties/optional/required"),
            Some(&json!([]))
        );
        assert_ne!(
            omitted_projection.fingerprints,
            explicit_projection.fingerprints
        );
    }

    #[test]
    fn dynamic_tools_only_profile_requires_direct_model_and_closes_native_surfaces() {
        for mode in [
            AiCodexAppServerModelToolMode::CodeMode,
            AiCodexAppServerModelToolMode::CodeModeOnly,
        ] {
            assert!(matches!(
                AiCodexAppServerLaunchProfile::experimental_dynamic_tools_only_v1(mode),
                Err(ProviderError::InvalidConfiguration(_))
            ));
        }
        let profile = AiCodexAppServerLaunchProfile::experimental_dynamic_tools_only_v1(
            AiCodexAppServerModelToolMode::Direct,
        )
        .expect("direct-tool profile should validate");
        assert!(profile.supports_experimental_dynamic_tools());
        assert!(profile.requires_isolated_configuration_home());
        let arguments = profile.codex_arguments();
        assert_eq!(
            &arguments[..3],
            ["app-server", "--stdio", "--strict-config"]
        );
        assert!(!arguments.contains(&"--enable"));
        for feature in DYNAMIC_TOOLS_ONLY_DISABLED_FEATURES {
            assert!(
                arguments
                    .windows(2)
                    .any(|pair| pair == ["--disable", *feature]),
                "missing disabled feature {feature}"
            );
        }
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

        let unverified_dynamic_provider = AiCodexAppServerProvider::new(
            dynamic_registration("1.0.0"),
            AiCodexAppServerRunPool::new(
                Arc::new(TextOnlyFactory),
                AiCodexAppServerRunLimits::default(),
            ),
        );
        assert!(!unverified_dynamic_provider.capabilities().custom_tools);
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
            instructions: Vec::new(),
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

    fn runtime_warning_notification(thread_id: Option<&str>, message: &str) -> Vec<u8> {
        let params = thread_id.map_or_else(
            || json!({"message": message}),
            |thread_id| json!({"threadId": thread_id, "message": message}),
        );
        lifecycle_notification(RUNTIME_WARNING, params)
    }

    fn reasoning_lifecycle_notification(
        method: &str,
        item_id: &str,
        content: Value,
        summary: Value,
    ) -> Vec<u8> {
        let timestamp_key = if method == "item/started" {
            "startedAtMs"
        } else {
            "completedAtMs"
        };
        let mut params = json!({
            "threadId": "thread-1",
            "turnId": "turn-1",
            "item": {
                "id": item_id,
                "type": "reasoning",
                "content": content,
                "summary": summary,
            },
        });
        params
            .as_object_mut()
            .expect("reasoning params should be an object")
            .insert(timestamp_key.to_owned(), json!(1));
        lifecycle_notification(method, params)
    }

    fn thread_started_notification(thread_id: &str) -> Vec<u8> {
        lifecycle_notification("thread/started", json!({"thread": {"id": thread_id}}))
    }

    fn thread_status_notification(thread_id: &str, status: Value) -> Vec<u8> {
        lifecycle_notification(
            "thread/status/changed",
            json!({"threadId": thread_id, "status": status}),
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

    fn token_usage_notification(thread_id: &str, turn_id: &str) -> Vec<u8> {
        lifecycle_notification(
            THREAD_TOKEN_USAGE_UPDATED,
            json!({
                "threadId": thread_id,
                "turnId": turn_id,
                "tokenUsage": {
                    "last": {
                        "cacheWriteInputTokens": 0,
                        "cachedInputTokens": 0,
                        "inputTokens": 1,
                        "outputTokens": 1,
                        "reasoningOutputTokens": 0,
                        "totalTokens": 2,
                    },
                    "total": {
                        "cacheWriteInputTokens": 0,
                        "cachedInputTokens": 0,
                        "inputTokens": 1,
                        "outputTokens": 1,
                        "reasoningOutputTokens": 0,
                        "totalTokens": 2,
                    },
                    "modelContextWindow": 128000,
                },
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
    async fn mixed_read_capabilities_start_newly_bound_dynamic_turn_without_resume() {
        let counters = Arc::new(Counters::new());
        let registration = mixed_registration();
        let provider =
            AiCodexAppServerProvider::new(registration.clone(), pool(counters.clone(), 1, 4));
        let (_catalog, _static_definition, _generated_definition, _generated_plan, request) =
            mixed_read_surface();
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
            .expect("mixed definition set should create a persistent empty thread");
        let opened = opened_session(binding, &registration, cursor.clone())
            .activate_newly_bound_empty(binding, &cursor)
            .expect("newly bound empty activation should match the created cursor");
        let context = context
            .with_provider_session(opened)
            .expect("opened provider session should match the run context");
        counters.tool_free.store(true, Ordering::SeqCst);
        let events = provider
            .stream_with_dynamic_tools(request, context, Arc::new(FakeDynamicResponder))
            .await
            .expect("newly bound mixed turn should start without resume");
        let events = events.collect::<Vec<_>>().await;
        assert!(events.iter().all(Result::is_ok));
        assert_eq!(counters.created_threads.load(Ordering::SeqCst), 1);
        assert_eq!(counters.created_dynamic_tools.load(Ordering::SeqCst), 2);
        assert_eq!(counters.bound_turns.load(Ordering::SeqCst), 1);
        assert_eq!(counters.retained_turns.load(Ordering::SeqCst), 0);
        assert_eq!(counters.turns.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn mixed_read_plan_first_turn_accepts_exact_bootstrap_and_rebuilt_definitions() {
        let counters = Arc::new(Counters::new());
        let registration = mixed_registration();
        let provider =
            AiCodexAppServerProvider::new(registration.clone(), pool(counters.clone(), 1, 4));
        let (catalog, static_definition, generated_definition, _, create_request) =
            mixed_read_surface();
        let rebuilt_static = catalog
            .read_only_model_definition(
                &crate::AiToolId::parse(&static_definition.tool_id)
                    .expect("static tool ID should parse"),
                static_definition.provider_name.clone(),
            )
            .expect("static catalog definition should rebuild identically");
        let rebuilt_generated = catalog
            .query_capability_model_definition(
                &crate::AiToolId::parse(&generated_definition.tool_id)
                    .expect("generated tool ID should parse"),
                generated_definition.provider_name.clone(),
            )
            .expect("generated catalog definition should rebuild identically");
        let mut first_turn = create_request.clone();
        first_turn.instructions = vec![
            "Use the exact registered application tool when it is required to answer the request."
                .to_owned(),
        ];
        first_turn.tools = vec![rebuilt_static, rebuilt_generated];
        let context = provider_context(registration.provider_profile_id(), &first_turn);
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
        crate::AiReadOnlyAgentTurnPlan::new_experimental_dynamic_tools(
            crate::AiProviderCallPlan::new_with_read_capabilities(
                ProviderKind::LocalHarness,
                first_turn.clone(),
                crate::AiBudgetReservationRequest {
                    scope: crate::AiScope::new("project", "test"),
                    session_id: binding.session_id(),
                    run_id: binding.run_id(),
                    attempt_id: binding.attempt_id(),
                    lease_generation: binding.lease_generation(),
                    provider_kind: ProviderKind::LocalHarness,
                    model: first_turn.model.clone(),
                    pricing_policy_version: "test-pricing-v1".to_owned(),
                    estimate: crate::AiBudgetAmounts {
                        input_tokens: 1_000,
                        output_tokens: 1_000,
                        tool_units: 0,
                        image_units: 0,
                        cost_microunits: 0,
                        runs: 1,
                    },
                    idempotency_key: "mixed-bootstrap-first-turn".to_owned(),
                    expires_at: time::OffsetDateTime::now_utc() + time::Duration::hours(1),
                },
                vec![crate::AiEgressManifest {
                    provider_profile_id: registration.provider_profile_id().to_owned(),
                    provider_kind: ProviderKind::LocalHarness.as_str().to_owned(),
                    model: first_turn.model.clone(),
                    destination: "local-codex".to_owned(),
                    destination_trust: crate::AiDestinationTrust::Local,
                    capability: crate::AiEgressCapability::ModelInference,
                    scope: crate::AiScope::new("project", "test"),
                    session_id: Some(binding.session_id()),
                    run_id: Some(binding.run_id()),
                    sources: vec![crate::AiDataSourceRef {
                        kind: "message".to_owned(),
                        reference: "synthetic".to_owned(),
                        classification: crate::DataClassification::Public,
                        trust: crate::AiSourceTrust::UserProvided,
                    }],
                    estimated_bytes: first_turn.conservative_egress_bytes(),
                    estimated_tokens: 100,
                    attachment_count: 0,
                    purpose: "test".to_owned(),
                    retention: "none".to_owned(),
                    residency: None,
                    policy_version: "test".to_owned(),
                    consent_reference: None,
                }],
                "mixed-bootstrap-first-turn",
                &catalog,
                &{
                    let mut static_policy =
                        crate::AiToolPolicySet::new(crate::ToolMaturity::ReadOnly);
                    static_policy.bind(crate::AiToolPolicyBinding {
                        tool_id: crate::AiToolId::parse(&static_definition.tool_id)
                            .expect("static tool ID should parse"),
                        fingerprint: static_definition.fingerprint.clone(),
                        enabled: true,
                    });
                    static_policy
                },
                &{
                    let generated_capability = catalog
                        .query_capabilities()
                        .find(|capability| capability.id().as_str() == generated_definition.tool_id)
                        .expect("generated capability should be registered");
                    let mut generated_targets = crate::AiGeneratedGraphqlTargetPolicySet::new();
                    generated_targets
                        .bind(
                            crate::AiGeneratedGraphqlTargetPolicyBinding::new(
                                generated_capability.target_id().clone(),
                                generated_capability.finished_schema_fingerprint(),
                                generated_capability.semantic_catalog_fingerprint(),
                            )
                            .expect("generated target binding should validate")
                            .allow_queries(),
                        )
                        .expect("generated target should bind");
                    generated_targets
                },
            )
            .expect("mixed static and generated read plan should validate"),
            crate::AiToolResultEgressRoute::new(
                "canonical-codex-profile",
                "sandboxed-local-harness",
                crate::AiDestinationTrust::Local,
                "answer-with-registered-tool",
                "provider-session",
                "canonical-egress-v1",
            )
            .expect("mixed result route should validate"),
            crate::AiResolvedRuleSet::new(
                crate::AiScope::new("project", "test"),
                crate::AiRuleConstraints {
                    enabled: true,
                    maximum_classification: crate::DataClassification::Restricted,
                    maximum_tool_maturity: crate::ToolMaturity::ReadOnly,
                    approval_requirement: crate::AiRuleApprovalRequirement::DescriptorPolicy,
                    allowed_tool_fingerprints: None,
                    allowed_provider_kinds: None,
                    allowed_provider_capabilities: None,
                    allow_provider_retention: true,
                    allow_byok: false,
                    budget: crate::AiRuleBudgetCeilings {
                        maximum_steps: Some(32),
                        maximum_duration_seconds: Some(3_600),
                        maximum_output_tokens: Some(100_000),
                        maximum_cost_microunits: Some(100_000_000),
                        maximum_provider_calls: Some(16),
                        maximum_tool_units: Some(1_000),
                        maximum_web_search_calls: Some(4),
                        maximum_image_units: Some(1_000),
                    },
                },
                Vec::new(),
            )
            .expect("mixed rule set should validate"),
            false,
        )
        .expect("mixed experimental dynamic-tool plan should validate")
        .with_provider_session(
            crate::AiProviderSessionTurnPlan::new(descriptor.clone(), "d".repeat(64))
                .expect("provider-session plan should validate"),
        )
        .expect("provider-session plan should match the mixed retained call");

        let cursor = provider
            .create_empty_session(&binding, &descriptor, &create_request)
            .await
            .expect("mixed definition set should create a persistent empty thread");
        let opened = opened_session(binding, &registration, cursor.clone())
            .activate_newly_bound_empty(binding, &cursor)
            .expect("newly bound empty activation should match the created cursor");
        counters.tool_free.store(true, Ordering::SeqCst);
        let events = provider
            .stream_with_dynamic_tools(
                first_turn,
                context
                    .with_provider_session(opened)
                    .expect("opened provider session should match the run context"),
                Arc::new(FakeDynamicResponder),
            )
            .await
            .expect("exact bootstrap copy and rebuilt canonical definitions should start once");
        let events = events.collect::<Vec<_>>().await;
        assert!(events.iter().all(Result::is_ok));
        assert_eq!(counters.created_threads.load(Ordering::SeqCst), 1);
        assert_eq!(counters.bound_turns.load(Ordering::SeqCst), 1);
        assert_eq!(counters.retained_turns.load(Ordering::SeqCst), 0);
        assert_eq!(counters.turns.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn mixed_read_plan_creates_newly_bound_generated_call_and_resumes() {
        let counters = Arc::new(Counters::new());
        let registration = mixed_registration();
        let provider =
            AiCodexAppServerProvider::new(registration.clone(), pool(counters.clone(), 1, 4));
        let (catalog, static_definition, generated_definition, generated_plan, request) =
            mixed_read_surface();
        let context = provider_context(registration.provider_profile_id(), &request);
        let binding = context
            .run_binding()
            .expect("executor context should carry the exact run binding");
        let mut static_policy = crate::AiToolPolicySet::new(crate::ToolMaturity::ReadOnly);
        static_policy.bind(crate::AiToolPolicyBinding {
            tool_id: crate::AiToolId::parse(&static_definition.tool_id)
                .expect("static tool ID should parse"),
            fingerprint: static_definition.fingerprint.clone(),
            enabled: true,
        });
        let generated_capability = catalog
            .query_capabilities()
            .find(|capability| capability.id().as_str() == generated_definition.tool_id)
            .expect("generated capability should be registered");
        let mut generated_targets = crate::AiGeneratedGraphqlTargetPolicySet::new();
        generated_targets
            .bind(
                crate::AiGeneratedGraphqlTargetPolicyBinding::new(
                    generated_capability.target_id().clone(),
                    generated_capability.finished_schema_fingerprint(),
                    generated_capability.semantic_catalog_fingerprint(),
                )
                .expect("generated target binding should validate")
                .allow_queries(),
            )
            .expect("generated target should bind");
        let scope = crate::AiScope::new("project", "test");
        let budget = crate::AiBudgetReservationRequest {
            scope: scope.clone(),
            session_id: binding.session_id(),
            run_id: binding.run_id(),
            attempt_id: binding.attempt_id(),
            lease_generation: binding.lease_generation(),
            provider_kind: ProviderKind::LocalHarness,
            model: request.model.clone(),
            pricing_policy_version: "test-pricing-v1".to_owned(),
            estimate: crate::AiBudgetAmounts {
                input_tokens: 1_000,
                output_tokens: 1_000,
                tool_units: 0,
                image_units: 0,
                cost_microunits: 0,
                runs: 1,
            },
            idempotency_key: "mixed-read-plan".to_owned(),
            expires_at: time::OffsetDateTime::now_utc() + time::Duration::hours(1),
        };
        let manifest = crate::AiEgressManifest {
            provider_profile_id: registration.provider_profile_id().to_owned(),
            provider_kind: ProviderKind::LocalHarness.as_str().to_owned(),
            model: request.model.clone(),
            destination: "local-codex".to_owned(),
            destination_trust: crate::AiDestinationTrust::Local,
            capability: crate::AiEgressCapability::ModelInference,
            scope,
            session_id: Some(binding.session_id()),
            run_id: Some(binding.run_id()),
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
        crate::AiProviderCallPlan::new_with_read_capabilities(
            ProviderKind::LocalHarness,
            request.clone(),
            budget,
            vec![manifest],
            "mixed-read-plan",
            &catalog,
            &static_policy,
            &generated_targets,
        )
        .expect("mixed static and generated read plan should validate");

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
            .expect("mixed definition set should create a persistent empty thread");
        let opened = opened_session(binding, &registration, cursor.clone())
            .activate_newly_bound_empty(binding, &cursor)
            .expect("newly bound empty activation should match the created cursor");
        *counters
            .dynamic_arguments
            .lock()
            .expect("dynamic argument fixture should not be poisoned") =
            Some(generated_plan.clone());
        let context = context
            .with_provider_session(opened)
            .expect("opened provider session should match the run context");
        let responder = MixedDynamicResponder {
            generated: generated_definition.clone(),
            plan: generated_plan,
        };
        let events = provider
            .stream_with_dynamic_tools(request.clone(), context, Arc::new(responder))
            .await
            .expect("newly bound generated-query turn should start once");
        let events = events.collect::<Vec<_>>().await;
        assert!(events.iter().all(Result::is_ok));
        assert_eq!(counters.bound_turns.load(Ordering::SeqCst), 1);
        assert_eq!(counters.retained_turns.load(Ordering::SeqCst), 0);

        let resumed = opened_session(binding, &registration, cursor);
        let resume_input = AiCodexAppServerTurnInput::try_from_retained_dynamic_request(
            request,
            registration.bootstrap_instructions(),
        )
        .expect("later turn should reuse the frozen mixed definitions");
        let events = provider
            .pool
            .start_retained_dynamic_turn(
                binding,
                registration,
                resumed,
                resume_input,
                Arc::new(FakeDynamicResponder),
            )
            .await
            .expect("later turn should resume the same frozen thread");
        let events = events.collect::<Vec<_>>().await;
        assert!(events.iter().all(Result::is_ok));
        assert_eq!(counters.bound_turns.load(Ordering::SeqCst), 1);
        assert_eq!(counters.retained_turns.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn newly_bound_turn_reports_content_free_activation_phase() {
        let counters = Arc::new(Counters::new());
        let registration = mixed_registration();
        let pool = pool(counters.clone(), 1, 2);
        let binding = binding();
        let (_, _, _, _, request) = mixed_read_surface();
        let cursor = pool
            .create_empty_thread(binding, registration.clone(), request.tools.clone())
            .await
            .expect("mixed empty thread should create");
        let opened = opened_session(binding, &registration, cursor.clone())
            .activate_newly_bound_empty(binding, &cursor)
            .expect("activation should validate");
        let mut mismatched = request.clone();
        mismatched.model = "other-model".to_owned();
        let mismatched_input = AiCodexAppServerTurnInput::try_from_retained_dynamic_request(
            mismatched,
            registration.bootstrap_instructions(),
        )
        .expect("structurally valid model swap should convert");
        assert!(matches!(
            pool.start_bound_dynamic_turn(
                binding,
                registration.clone(),
                opened.clone(),
                mismatched_input,
                Arc::new(FakeDynamicResponder),
            )
            .await,
            Err(ProviderError::NewlyBoundTurnRejected(
                crate::AiCodexBoundTurnRejection::ModelMismatch
            ))
        ));

        let other_registration = dynamic_registration("2.0.0");
        let swapped_session = opened_session(binding, &other_registration, cursor.clone())
            .activate_newly_bound_empty(binding, &cursor)
            .expect("crate marker alone is not registration proof");
        let input = AiCodexAppServerTurnInput::try_from_retained_dynamic_request(
            request.clone(),
            registration.bootstrap_instructions(),
        )
        .expect("exact mixed input should convert");
        assert!(matches!(
            pool.start_bound_dynamic_turn(
                binding,
                registration.clone(),
                swapped_session,
                input,
                Arc::new(FakeDynamicResponder),
            )
            .await,
            Err(ProviderError::NewlyBoundTurnRejected(
                crate::AiCodexBoundTurnRejection::OpenedSessionMismatch
            ))
        ));

        let empty_bootstrap_input = AiCodexAppServerTurnInput::try_from_dynamic_request(request)
            .expect("empty-instruction dynamic input should convert");
        assert!(matches!(
            pool.start_bound_dynamic_turn(
                binding,
                registration,
                opened,
                empty_bootstrap_input,
                Arc::new(FakeDynamicResponder),
            )
            .await,
            Err(ProviderError::NewlyBoundTurnRejected(
                crate::AiCodexBoundTurnRejection::BootstrapFingerprintMismatch
            ))
        ));
        assert_eq!(counters.bound_turns.load(Ordering::SeqCst), 0);
        let encoded = format!(
            "{}",
            crate::AiCodexBoundTurnRejection::FrozenDefinitionMismatch
        );
        assert_eq!(encoded, "codex_bound_turn_frozen_definition_mismatch");
        assert!(!encoded.contains("cursor"));
        assert!(!encoded.contains("prompt"));
    }

    #[tokio::test]
    async fn provider_dispatches_tool_free_newly_bound_session_directly() {
        let counters = Arc::new(Counters::new());
        let registration = registration("1.0.0");
        let provider =
            AiCodexAppServerProvider::new(registration.clone(), pool(counters.clone(), 1, 2));
        let mut request = model_request();
        request.continuation_mode = ModelContinuationMode::ProviderRetained;
        request.instructions.clear();
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
            Err(ProviderError::NewlyBoundTurnRejected(
                crate::AiCodexBoundTurnRejection::ActivationUnavailable
            ))
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
            Err(ProviderError::NewlyBoundTurnRejected(
                crate::AiCodexBoundTurnRejection::CursorFingerprintMismatch
            ))
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
            Err(ProviderError::NewlyBoundTurnRejected(
                crate::AiCodexBoundTurnRejection::ProcessBindingMissing
            ))
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
            Err(ProviderError::NewlyBoundTurnRejected(
                crate::AiCodexBoundTurnRejection::FrozenDefinitionMismatch
            ))
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
            Err(ProviderError::NewlyBoundTurnRejected(
                crate::AiCodexBoundTurnRejection::ProcessBindingMissing
            ))
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
        retained.instructions.clear();
        AiCodexAppServerTurnInput::try_from_retained_model_request(
            retained,
            &bootstrap_instructions(),
        )
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

        let unverified = AiCodexAppServerProvider::new(
            dynamic_registration("1.0.0"),
            AiCodexAppServerRunPool::new(
                Arc::new(TextOnlyFactory),
                AiCodexAppServerRunLimits::default(),
            ),
        );
        assert!(matches!(
            unverified
                .stream_with_dynamic_tools(
                    request.clone(),
                    context.clone(),
                    Arc::new(FakeDynamicResponder),
                )
                .await,
            Err(ProviderError::Unsupported)
        ));

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
    async fn active_dynamic_turn_uses_exact_interrupt_and_close_lifecycle() {
        let counters = Arc::new(Counters::new());
        counters.pending.store(true, Ordering::SeqCst);
        let provider: Arc<dyn AiProvider> = Arc::new(AiCodexAppServerProvider::new(
            dynamic_registration("1.0.0"),
            pool(counters.clone(), 1, 2),
        ));
        let request = dynamic_model_request();
        let context = provider_context("profile-1", &request);
        let binding = context
            .run_binding()
            .expect("test context should carry the exact run binding");
        let active = provider
            .stream_with_dynamic_tools(request, context, Arc::new(FakeDynamicResponder))
            .await
            .expect("dynamic provider turn should start");
        assert_eq!(
            provider
                .interrupt_run(&binding)
                .await
                .expect("dynamic interrupt should dispatch"),
            AiProviderRunInterruptOutcome::Requested
        );
        assert_eq!(
            provider
                .close_run(&binding, AiProviderRunCloseReason::Cancelled)
                .await
                .expect("dynamic close should dispatch"),
            AiProviderRunCloseOutcome::Closed
        );
        drop(active);
        assert_eq!(counters.interrupts.load(Ordering::SeqCst), 1);
        assert_eq!(counters.shutdowns.load(Ordering::SeqCst), 1);
        assert_eq!(counters.kills.load(Ordering::SeqCst), 1);
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
        assert!(initialize.contains("\"optOutNotificationMethods\""));
        assert!(thread.contains("\"ephemeral\":true"));
        assert!(thread.contains("\"developerInstructions\":\"trusted\""));
        assert!(thread.contains("\"approvalPolicy\":\"never\""));
        assert!(thread.contains("\"sandbox\":\"read-only\""));
        assert!(!thread.contains("dynamicTools"));
        assert!(!turn.contains("trusted"));
        assert!(!turn.contains("\"model\""));
        assert!(turn.contains("\"summary\":\"none\""));
    }

    #[test]
    fn protocol_initialization_uses_only_the_closed_notification_opt_out_profile() {
        let expected_opt_outs = json!([
            "thread/status/changed",
            "thread/settings/updated",
            "thread/goal/cleared",
            "mcpServer/startupStatus/updated",
            "account/rateLimits/updated",
        ]);

        let mut text_actor =
            AiCodexAppServerProtocolActor::new(64 * 1024).expect("actor should validate");
        let text: Value = serde_json::from_slice(
            &text_actor
                .initialize("test_client", "Test Client", "1.0.0")
                .expect("stable initialization should encode"),
        )
        .expect("stable initialization should be JSON");
        assert_eq!(
            text,
            json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "clientInfo": {
                        "name": "test_client",
                        "title": "Test Client",
                        "version": "1.0.0",
                    },
                    "capabilities": {
                        "optOutNotificationMethods": expected_opt_outs,
                    },
                },
            })
        );

        let mut dynamic_actor =
            AiCodexAppServerProtocolActor::new(64 * 1024).expect("actor should validate");
        let dynamic: Value = serde_json::from_slice(
            &dynamic_actor
                .initialize_with_dynamic_tools("test_client", "Test Client", "1.0.0")
                .expect("dynamic initialization should encode"),
        )
        .expect("dynamic initialization should be JSON");
        assert_eq!(
            dynamic,
            json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "clientInfo": {
                        "name": "test_client",
                        "title": "Test Client",
                        "version": "1.0.0",
                    },
                    "capabilities": {
                        "experimentalApi": true,
                        "optOutNotificationMethods": expected_opt_outs,
                    },
                },
            })
        );
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
    fn protocol_admits_only_content_free_runtime_warnings_during_a_correlated_turn() {
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

        let warning = runtime_warning_notification(
            Some("thread-1"),
            "Code Mode is unavailable and remains disabled.",
        );
        assert!(matches!(
            actor.accept(&warning),
            Err(ProviderError::Rejected)
        ));

        actor
            .start_turn("thread-1", &turn())
            .expect("turn should begin the warning admission window");
        let inbound = actor
            .accept(&warning)
            .expect("correlated pending-turn warning should be admitted");
        assert!(matches!(inbound, AiCodexAppServerInbound::RuntimeWarning));
        assert_eq!(
            format!("{inbound:?}"),
            "AiCodexAppServerInbound::RuntimeWarning"
        );
        assert!(!format!("{inbound:?}").contains("Code Mode"));
        assert!(matches!(
            actor.accept(&runtime_warning_notification(None, "Still unavailable.")),
            Ok(AiCodexAppServerInbound::RuntimeWarning)
        ));
        assert!(matches!(
            actor.accept(&lifecycle_notification(
                RUNTIME_WARNING,
                json!({"threadId": null, "message": "Still safely unavailable."}),
            )),
            Ok(AiCodexAppServerInbound::RuntimeWarning)
        ));

        actor
            .accept(br#"{"id":3,"result":{"turn":{"id":"turn-1"}}}"#)
            .expect("turn response should bind");
        actor
            .accept(&turn_started_notification("thread-1", "turn-1"))
            .expect("turn notification should bind");
        assert!(matches!(
            actor.accept(&runtime_warning_notification(
                Some("thread-1"),
                "Warning while the correlated turn is active.",
            )),
            Ok(AiCodexAppServerInbound::RuntimeWarning)
        ));
        actor
            .accept(&turn_completed_notification("thread-1", "turn-1"))
            .expect("turn should complete");
        assert!(matches!(
            actor.accept(&warning),
            Err(ProviderError::Rejected)
        ));

        actor
            .start_turn("thread-1", &turn())
            .expect("a later turn should have an independent warning budget");
        assert!(matches!(
            actor.accept(&warning),
            Ok(AiCodexAppServerInbound::RuntimeWarning)
        ));
    }

    #[test]
    fn protocol_rejects_malformed_mismatched_late_or_flooding_runtime_warnings() {
        let malformed = [
            br#"{"method":"warning","params":{"message":"bounded"}}"#.as_slice(),
            br#"{"emittedAtMs":0,"method":"warning","params":{"message":"bounded"}}"#,
            br#"{"emittedAtMs":-1,"method":"warning","params":{"message":"bounded"}}"#,
            br#"{"emittedAtMs":9223372036854775808,"method":"warning","params":{"message":"bounded"}}"#,
            br#"{"emittedAtMs":"1","method":"warning","params":{"message":"bounded"}}"#,
            br#"{"emittedAtMs":1,"emittedAtMs":2,"method":"warning","params":{"message":"bounded"}}"#,
            br#"{"emittedAtMs":1,"method":"warning","params":{"message":"bounded"},"extra":true}"#,
            br#"{"emittedAtMs":1,"method":"warning","params":{}}"#,
            br#"{"emittedAtMs":1,"method":"warning","params":{"message":"bounded","extra":true}}"#,
            br#"{"emittedAtMs":1,"method":"warning","params":{"message":"bounded","threadId":7}}"#,
        ];
        for frame in malformed {
            let mut actor = active_protocol_actor();
            assert!(matches!(actor.accept(frame), Err(ProviderError::Rejected)));
        }

        for message in ["", "   ", "contains\ncontrol", "contains\u{7f}control"] {
            let mut actor = active_protocol_actor();
            assert!(matches!(
                actor.accept(&runtime_warning_notification(Some("thread-1"), message)),
                Err(ProviderError::Rejected)
            ));
        }
        let mut oversized = active_protocol_actor();
        assert!(matches!(
            oversized.accept(&runtime_warning_notification(
                Some("thread-1"),
                &"x".repeat(MAXIMUM_RUNTIME_WARNING_MESSAGE_BYTES + 1),
            )),
            Err(ProviderError::Rejected)
        ));
        let mut mismatched = active_protocol_actor();
        assert!(matches!(
            mismatched.accept(&runtime_warning_notification(
                Some("thread-other"),
                "bounded",
            )),
            Err(ProviderError::Rejected)
        ));

        let mut count_limited = active_protocol_actor();
        for _ in 0..MAXIMUM_RUNTIME_WARNINGS_PER_TURN {
            assert!(matches!(
                count_limited.accept(&runtime_warning_notification(None, "bounded")),
                Ok(AiCodexAppServerInbound::RuntimeWarning)
            ));
        }
        assert!(matches!(
            count_limited.accept(&runtime_warning_notification(None, "one too many")),
            Err(ProviderError::Rejected)
        ));

        let mut byte_limited = active_protocol_actor();
        let maximum_message = "x".repeat(MAXIMUM_RUNTIME_WARNING_MESSAGE_BYTES);
        for _ in 0..(MAXIMUM_RUNTIME_WARNING_BYTES_PER_TURN / MAXIMUM_RUNTIME_WARNING_MESSAGE_BYTES)
        {
            assert!(matches!(
                byte_limited.accept(&runtime_warning_notification(None, &maximum_message)),
                Ok(AiCodexAppServerInbound::RuntimeWarning)
            ));
        }
        assert!(matches!(
            byte_limited.accept(&runtime_warning_notification(None, "overflow")),
            Err(ProviderError::Rejected)
        ));
    }

    #[test]
    fn protocol_admits_only_content_free_reasoning_item_lifecycle() {
        let mut actor = active_protocol_actor();
        let started = actor
            .accept(&reasoning_lifecycle_notification(
                "item/started",
                "reasoning-1",
                json!([]),
                json!([]),
            ))
            .expect("empty reasoning start should be admitted");
        assert_eq!(
            started,
            AiCodexAppServerInbound::ReasoningLifecycle { completed: false }
        );
        assert_eq!(
            format!("{started:?}"),
            "AiCodexAppServerInbound::ReasoningLifecycle { completed: false }"
        );
        let completed = actor
            .accept(&reasoning_lifecycle_notification(
                "item/completed",
                "reasoning-1",
                json!([]),
                json!([]),
            ))
            .expect("empty reasoning completion should be admitted");
        assert_eq!(
            completed,
            AiCodexAppServerInbound::ReasoningLifecycle { completed: true }
        );

        for (content, summary) in [
            (json!(["hidden reasoning"]), json!([])),
            (json!([]), json!(["unrequested summary"])),
            (json!({}), json!([])),
            (json!([]), json!({})),
        ] {
            let mut actor = active_protocol_actor();
            assert!(matches!(
                actor.accept(&reasoning_lifecycle_notification(
                    "item/started",
                    "reasoning-1",
                    content,
                    summary,
                )),
                Err(ProviderError::Rejected)
            ));
        }

        let mut actor = active_protocol_actor();
        assert!(matches!(
            actor.accept(&lifecycle_notification(
                "item/started",
                json!({
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "startedAtMs": 1,
                    "item": {
                        "id": "reasoning-1",
                        "type": "reasoning",
                        "content": [],
                        "summary": [],
                        "extra": true,
                    },
                }),
            )),
            Err(ProviderError::Rejected)
        ));
    }

    #[test]
    fn protocol_admits_one_content_free_retained_usage_snapshot_without_recharging_it() {
        let cursor =
            crate::AiProviderSessionCursor::new("codex.app_server.thread.v2", "thread-retained-1")
                .expect("cursor should validate");
        let mut actor = initialized_protocol_actor();
        actor
            .resume_thread(&cursor, &turn())
            .expect("resume should begin a retained lifecycle");
        actor
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("resume response should bind");
        let snapshot = token_usage_notification("thread-retained-1", "turn-previous");
        let inbound = actor
            .accept(&snapshot)
            .expect("one retained cumulative snapshot should be admitted");
        assert_eq!(
            inbound,
            AiCodexAppServerInbound::RetainedResumeUsageSnapshot
        );
        assert_eq!(
            format!("{inbound:?}"),
            "AiCodexAppServerInbound::RetainedResumeUsageSnapshot"
        );
        assert!(matches!(
            actor.accept(&snapshot),
            Err(ProviderError::Rejected)
        ));
        assert_eq!(actor.thread_lifecycle_phase, ThreadLifecyclePhase::Complete);
        actor
            .accept(&thread_started_notification("thread-retained-1"))
            .expect("a provider that also emits thread started remains supported");
        assert!(matches!(
            actor.accept(&thread_started_notification("thread-retained-1")),
            Err(ProviderError::Rejected)
        ));
        actor
            .start_turn("thread-retained-1", &turn())
            .expect("the new turn should start");
        actor
            .accept(br#"{"id":3,"result":{"turn":{"id":"turn-current"}}}"#)
            .expect("turn response should bind");
        actor
            .accept(&turn_started_notification(
                "thread-retained-1",
                "turn-current",
            ))
            .expect("turn notification should bind");
        assert!(matches!(
            actor.accept(&token_usage_notification(
                "thread-retained-1",
                "turn-current",
            )),
            Ok(AiCodexAppServerInbound::Notification { ref method, .. })
                if method == THREAD_TOKEN_USAGE_UPDATED
        ));

        let mut notification_first = initialized_protocol_actor();
        notification_first
            .resume_thread(&cursor, &turn())
            .expect("resume should begin");
        assert!(matches!(
            notification_first.accept(&snapshot),
            Ok(AiCodexAppServerInbound::RetainedResumeUsageSnapshot)
        ));
        notification_first
            .accept(&thread_started_notification("thread-retained-1"))
            .expect("thread notification should bind first");
        notification_first
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("thread response should complete the lifecycle");

        let mut snapshot_first = initialized_protocol_actor();
        snapshot_first
            .resume_thread(&cursor, &turn())
            .expect("snapshot-first resume should begin");
        snapshot_first
            .accept(&snapshot)
            .expect("snapshot may precede the response");
        snapshot_first
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("response should complete snapshot-first resume");
        snapshot_first
            .start_turn("thread-retained-1", &turn())
            .expect("snapshot-first resume should permit a bounded turn");

        let mut fallback_consumed = initialized_protocol_actor();
        fallback_consumed
            .resume_thread(&cursor, &turn())
            .expect("fallback fixture resume should begin");
        fallback_consumed
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("resume response should bind");
        fallback_consumed
            .accept(&snapshot)
            .expect("usage snapshot should make a direct turn available");
        fallback_consumed
            .start_turn("thread-retained-1", &turn())
            .expect("turn start should consume the resume fallback");
        assert!(matches!(
            fallback_consumed.accept(&thread_started_notification("thread-retained-1")),
            Err(ProviderError::Rejected)
        ));

        let mut new_thread = initialized_protocol_actor();
        new_thread
            .start_persistent_empty_thread(
                "model-1",
                &AiCodexAppServerBootstrapInstructions::disabled(),
                &[],
            )
            .expect("new persistent thread should start");
        new_thread
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-new"}}}"#)
            .expect("new thread response should bind");
        assert!(matches!(
            new_thread.accept(&token_usage_notification("thread-new", "turn-empty")),
            Err(ProviderError::Rejected)
        ));
        assert_eq!(
            new_thread.thread_lifecycle_phase,
            ThreadLifecyclePhase::AwaitingStarted
        );
        new_thread
            .accept(&thread_started_notification("thread-new"))
            .expect("usage cannot replace new-thread lifecycle evidence");

        let mut negative = initialized_protocol_actor();
        negative
            .resume_thread(&cursor, &turn())
            .expect("negative fixture resume should begin");
        let mut invalid_usage: Value = serde_json::from_slice(&token_usage_notification(
            "thread-retained-1",
            "turn-previous",
        ))
        .expect("usage fixture should decode");
        *invalid_usage
            .pointer_mut("/params/tokenUsage/last/inputTokens")
            .expect("input token field should exist") = json!(-1);
        assert!(matches!(
            negative
                .accept(&serde_json::to_vec(&invalid_usage).expect("invalid usage should encode")),
            Err(ProviderError::Rejected)
        ));
    }

    #[test]
    fn protocol_admits_runtime_warning_after_strict_retained_resume() {
        let mut actor = initialized_protocol_actor();
        actor
            .start_persistent_empty_thread("model-1", &trusted_bootstrap(), &[])
            .expect("persistent empty thread should encode");
        actor
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("thread response should bind");
        actor
            .accept(&thread_started_notification("thread-retained-1"))
            .expect("thread notification should bind");
        actor
            .start_turn("thread-retained-1", &turn())
            .expect("newly bound first turn should start directly");
        actor
            .accept(br#"{"id":3,"result":{"turn":{"id":"turn-1"}}}"#)
            .expect("first turn response should bind");
        actor
            .accept(&turn_started_notification("thread-retained-1", "turn-1"))
            .expect("first turn notification should bind");
        actor
            .accept(&turn_completed_notification("thread-retained-1", "turn-1"))
            .expect("first turn should complete");

        let cursor =
            crate::AiProviderSessionCursor::new("codex.app_server.thread.v2", "thread-retained-1")
                .expect("cursor should validate");
        actor
            .resume_thread(&cursor, &turn())
            .expect("later retained lifecycle should resume");
        actor
            .accept(br#"{"id":4,"result":{"thread":{"id":"thread-retained-1"}}}"#)
            .expect("resume response should bind");
        actor
            .accept(&thread_started_notification("thread-retained-1"))
            .expect("resume notification should bind");
        actor
            .start_turn("thread-retained-1", &turn())
            .expect("resumed turn should start");
        assert!(matches!(
            actor.accept(&runtime_warning_notification(
                Some("thread-retained-1"),
                "Code Mode remains unavailable after resume.",
            )),
            Ok(AiCodexAppServerInbound::RuntimeWarning)
        ));
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
            .start_persistent_empty_thread("model-1", &trusted_bootstrap(), &[])
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
            .start_persistent_empty_thread("model-1", &trusted_bootstrap(), &[])
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
            .start_persistent_empty_thread(
                "model-1",
                &AiCodexAppServerBootstrapInstructions::disabled(),
                input.tools(),
            )
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

        let mut malformed_delete = deleting_protocol_actor();
        assert!(matches!(
            malformed_delete.accept(br#"{"id":3,"result":{"deleted":true}}"#),
            Err(ProviderError::Rejected)
        ));

        for status in ["idle", "systemError", "active", "notLoaded", "unknown"] {
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
            wrong_status_thread.accept(&thread_status_notification(
                "thread-other",
                json!({"type": "notLoaded"}),
            )),
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

        let mut mcp = active_protocol_actor();
        assert!(matches!(
            mcp.accept(&lifecycle_notification(
                "mcpServer/startupStatus/updated",
                json!({
                    "threadId": "thread-1",
                    "name": "untrusted",
                    "status": "ready",
                    "error": null,
                    "failureReason": null,
                }),
            )),
            Err(ProviderError::Rejected)
        ));

        let mut account_limits = active_protocol_actor();
        assert!(matches!(
            account_limits.accept(&lifecycle_notification(
                "account/rateLimits/updated",
                json!({"rateLimits": {"primary": {"usedPercent": 1.0}}}),
            )),
            Err(ProviderError::Rejected)
        ));

        let mut thread_settings = active_protocol_actor();
        assert!(matches!(
            thread_settings.accept(&lifecycle_notification(
                "thread/settings/updated",
                json!({"threadId": "thread-1", "threadSettings": {"summary": "none"}}),
            )),
            Err(ProviderError::Rejected)
        ));

        let mut thread_goal = active_protocol_actor();
        assert!(matches!(
            thread_goal.accept(&lifecycle_notification(
                "thread/goal/cleared",
                json!({"threadId": "thread-1"}),
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
        let start_value: Value =
            serde_json::from_str(start.trim()).expect("dynamic start should remain valid JSON");
        assert_eq!(
            start_value.pointer("/params/environments"),
            Some(&json!([]))
        );
        assert_eq!(
            start_value.pointer("/params/config/features.shell_tool"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            start_value.pointer("/params/config/features.unified_exec"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            start_value.pointer("/params/config/features.code_mode"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            start_value.pointer("/params/config/features.apps"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            start_value.pointer("/params/config/features.browser_use"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            start_value.pointer("/params/config/features.computer_use"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            start_value.pointer("/params/config/tools.update_plan.enabled"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            start_value.pointer("/params/config/web_search"),
            Some(&Value::String("disabled".to_owned()))
        );
        guard
            .accept(br#"{"id":2,"result":{"thread":{"id":"thread-1"}}}"#)
            .expect("thread response should bind");
        guard
            .accept(&thread_started_notification("thread-1"))
            .expect("thread notification should bind");
        let turn = String::from_utf8(
            guard
                .start_turn("thread-1", &input)
                .expect("turn request should encode"),
        )
        .expect("turn frame should be UTF-8");
        let turn_value: Value =
            serde_json::from_str(turn.trim()).expect("turn should remain valid JSON");
        assert_eq!(turn_value.pointer("/params/environments"), Some(&json!([])));
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
                    "arguments": {"Limit": 3},
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
            "id": 0,
            "method": "item/tool/call",
            "params": {
                "arguments": {"Limit": 3},
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
                assert_eq!(request_id, 0);
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
                .dynamic_tool_response(0, &result)
                .expect("exact response should encode"),
        )
        .expect("response should be UTF-8");
        assert!(response.contains("\"success\":true"));
        assert!(response.contains("\\\"count\\\":3"));
        let completed = lifecycle_notification(
            "item/completed",
            json!({
                "item": {
                    "arguments": {"Limit": 3},
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
            guard.dynamic_tool_response(0, &result),
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
                "arguments": {"Limit": 3},
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
                .start_persistent_empty_thread(
                    "codex-test-model",
                    &AiCodexAppServerBootstrapInstructions::disabled(),
                    &[],
                )
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
        assert!(matches!(
            empty_guard.accept(&thread_status_notification(
                "thread-retained-empty",
                json!({"type": "notLoaded"}),
            )),
            Err(ProviderError::Rejected)
        ));
        empty_guard
            .accept(br#"{"id":3,"result":{}}"#)
            .expect("persistent delete response should bind");
        assert!(matches!(
            empty_guard.accept(&thread_status_notification(
                "thread-retained-empty",
                json!({"type": "notLoaded"}),
            )),
            Err(ProviderError::Rejected)
        ));

        let input = AiCodexAppServerTurnInput::try_from_dynamic_request(dynamic_model_request())
            .expect("dynamic request should convert");
        let mut dynamic_create = initialized_protocol_actor();
        let create = String::from_utf8(
            dynamic_create
                .start_persistent_empty_thread(
                    "model-1",
                    &AiCodexAppServerBootstrapInstructions::disabled(),
                    input.tools(),
                )
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
        assert!(matches!(
            guard.accept(&thread_status_notification(
                "thread-retained-1",
                json!({"type": "notLoaded"}),
            )),
            Err(ProviderError::Rejected)
        ));

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

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires a reviewed Codex CLI 0.147.0 binary and disposable configured home"]
    async fn live_codex_0147_bound_first_turn_then_later_resume_uses_strict_actor() {
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
                .start_persistent_empty_thread(
                    "gpt-5.4",
                    &bootstrap_instructions(),
                    &[dynamic_tool()],
                )
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
        live_request.model = "gpt-5.4".to_owned();
        live_request.instructions.clear();
        live_request.input = vec![ModelInputBlock::Text {
            text: "Call inventory_count exactly once with Limit set to 3, then report the count."
                .to_owned(),
        }];
        live_request.maximum_output_tokens = Some(128);
        let input = AiCodexAppServerTurnInput::try_from_retained_dynamic_request(
            live_request,
            &bootstrap_instructions(),
        )
        .expect("live retained dynamic input should validate");

        process.send(
            &actor
                .start_turn(cursor.expose_to_provider_adapter(), &input)
                .expect("newly bound thread should start its first turn without resume"),
        );
        let mut turn_response_observed = false;
        let mut turn_started_observed = false;
        let mut turn_completed_observed = false;
        let mut dynamic_tool_calls = 0;
        for _ in 0..64 {
            let frame = process.receive();
            let inbound = actor.accept(&frame).unwrap_or_else(|error| {
                let envelope: Value = serde_json::from_slice(&frame)
                    .expect("rejected turn frame should remain valid JSON");
                let params_keys = envelope
                    .get("params")
                    .and_then(Value::as_object)
                    .map(|object| object.keys().cloned().collect::<Vec<_>>());
                let item = envelope.pointer("/params/item");
                let item_keys = item
                    .and_then(Value::as_object)
                    .map(|object| object.keys().cloned().collect::<Vec<_>>());
                panic!(
                    "retained turn frame was rejected: {error:?}; id_is_unsigned={}; method={:?}; params_keys={params_keys:?}; active_thread_matches={}; active_turn_matches={}; started_dynamic_call_count={}; item_type={:?}; item_keys={item_keys:?}",
                    envelope.get("id").and_then(Value::as_u64).is_some(),
                    envelope.get("method").and_then(Value::as_str),
                    envelope.pointer("/params/threadId").and_then(Value::as_str)
                        == actor.active_thread_id.as_deref(),
                    envelope.pointer("/params/turnId").and_then(Value::as_str)
                        == actor.active_turn_id.as_deref(),
                    actor.started_dynamic_calls.len(),
                    item.and_then(|item| item.get("type")).and_then(Value::as_str),
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
                    dynamic_tool_calls += 1;
                    let result = LiveDynamicResponder
                        .respond(call)
                        .await
                        .expect("live responder should bind to the exact canonical call");
                    process.send(
                        &actor
                            .dynamic_tool_response(request_id, &result)
                            .expect("live dynamic response should encode"),
                    );
                }
                AiCodexAppServerInbound::DynamicToolLifecycle { .. } => {}
                AiCodexAppServerInbound::RuntimeWarning => {}
                AiCodexAppServerInbound::ReasoningLifecycle { .. } => {}
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
        assert_eq!(dynamic_tool_calls, 1);

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
            let frame = process
                .frames
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|error| {
                    panic!(
                        "resume lifecycle timed out: {error:?}; response_observed={resume_response_observed}; notification_observed={resume_notification_observed}; phase={:?}",
                        actor.thread_lifecycle_phase,
                    )
                });
            let inbound = actor.accept(&frame).unwrap_or_else(|error| {
                let envelope: Value = serde_json::from_slice(&frame)
                    .expect("rejected resume frame should remain valid JSON");
                let keys = envelope
                    .as_object()
                    .map(|object| object.keys().cloned().collect::<Vec<_>>());
                let params_keys = envelope
                    .get("params")
                    .and_then(Value::as_object)
                    .map(|object| object.keys().cloned().collect::<Vec<_>>());
                let thread_matches = envelope
                    .pointer("/params/threadId")
                    .and_then(Value::as_str)
                    == Some(cursor.expose_to_provider_adapter());
                let usage_keys = envelope
                    .pointer("/params/tokenUsage")
                    .and_then(Value::as_object)
                    .map(|object| object.keys().cloned().collect::<Vec<_>>());
                let last_keys = envelope
                    .pointer("/params/tokenUsage/last")
                    .and_then(Value::as_object)
                    .map(|object| object.keys().cloned().collect::<Vec<_>>());
                let total_keys = envelope
                    .pointer("/params/tokenUsage/total")
                    .and_then(Value::as_object)
                    .map(|object| object.keys().cloned().collect::<Vec<_>>());
                let turn_valid = envelope
                    .pointer("/params/turnId")
                    .and_then(Value::as_str)
                    .is_some_and(valid_reference);
                panic!(
                    "later resume frame was rejected: {error:?}; method={:?}; keys={keys:?}; params_keys={params_keys:?}; thread_matches={thread_matches}; turn_valid={turn_valid}; usage_keys={usage_keys:?}; last_keys={last_keys:?}; total_keys={total_keys:?}; phase={:?}; snapshot_observed={}",
                    envelope.get("method").and_then(Value::as_str),
                    actor.thread_lifecycle_phase,
                    actor.retained_usage_snapshot_observed,
                );
            });
            match inbound {
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
                AiCodexAppServerInbound::RetainedResumeUsageSnapshot => {
                    resume_notification_observed = true;
                }
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
        let mut second_dynamic_tool_calls = 0;
        for _ in 0..64 {
            match actor
                .accept(&process.receive())
                .expect("second turn frame should be admitted")
            {
                AiCodexAppServerInbound::DynamicToolCall {
                    request_id, call, ..
                } => {
                    second_dynamic_tool_calls += 1;
                    let result = LiveDynamicResponder
                        .respond(call)
                        .await
                        .expect("resumed responder should bind to the exact canonical call");
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
                | AiCodexAppServerInbound::DynamicToolLifecycle { .. }
                | AiCodexAppServerInbound::RuntimeWarning
                | AiCodexAppServerInbound::ReasoningLifecycle { .. } => {}
                other => panic!("unexpected second turn frame: {other:?}"),
            }
            if second_completed {
                break;
            }
        }
        assert!(second_completed);
        assert_eq!(second_dynamic_tool_calls, 1);

        process.send(
            &actor
                .delete_thread(&cursor)
                .expect("live readiness thread delete should encode"),
        );
        let mut delete_response_observed = false;
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
                other => panic!("unexpected delete frame: {other:?}"),
            }
            if delete_response_observed {
                break;
            }
        }
        assert!(delete_response_observed);
    }
}
