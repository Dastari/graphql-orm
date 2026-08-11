//! Strict run-scoped Codex app-server process boundary.
//!
//! This module deliberately models process reuse without claiming durable
//! provider-thread resumption or application-tool execution. Each turn starts
//! a fresh bounded thread on the already-admitted process. Dynamic tools and
//! every other server-initiated request remain forbidden because the ordinary
//! coordinator executes application tools only after a provider turn ends.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::{
    AiProvider, AiProviderRunBinding, AiProviderRunCloseOutcome, AiProviderRunCloseReason,
    AiProviderRunInterruptOutcome, ModelContinuationMode, ModelInputBlock,
    ModelReasoningSummaryRequest, ModelRequest, ProviderCapabilities, ProviderError,
    ProviderEventStream, ProviderKind, ProviderRequestContext,
};

const MAXIMUM_PROCESSES: usize = 4_096;
const MAXIMUM_TURNS_PER_RUN: u32 = 1_024;
const MAXIMUM_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAXIMUM_TEXT_BYTES: usize = 16 * 1024 * 1024;
const MAXIMUM_TEXT_BLOCKS: usize = 256;
const MAXIMUM_IDENTIFIER_BYTES: usize = 200;
const MAXIMUM_VERSION_BYTES: usize = 200;
const MAXIMUM_TIMEOUT: Duration = Duration::from_secs(60 * 60);

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
        );
        Ok(Self {
            provider_profile_id,
            logical_model,
            executable_sha256,
            executable_version,
            sandbox_profile,
            protocol_version,
            identity,
        })
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
#[derive(Clone, PartialEq, Eq)]
pub struct AiCodexAppServerTurnInput {
    model: String,
    instructions: Vec<String>,
    input: Vec<String>,
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

    /// Requested output-token ceiling.
    pub const fn maximum_output_tokens(&self) -> u64 {
        self.maximum_output_tokens
    }

    fn try_from_model_request(request: ModelRequest) -> Result<Self, ProviderError> {
        request.validate()?;
        if request.continuation.is_some()
            || request.continuation_mode != ModelContinuationMode::StatelessReplay
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

/// Strict tool-free Codex app-server provider adapter.
///
/// This first-phase adapter reuses one admitted process for the exact claimed
/// run while starting a fresh ephemeral provider thread for each call. It does
/// not advertise or accept application tools, provider built-ins, retained or
/// stateless conversation continuations, attachments, structured output, or
/// reasoning. Later phases must add those capabilities through separate
/// reviewed contracts rather than widening this adapter implicitly.
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
}

#[async_trait]
impl AiProvider for AiCodexAppServerProvider {
    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::LocalHarness
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
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
        let input = AiCodexAppServerTurnInput::try_from_model_request(request)?;
        self.pool
            .start_fresh_turn(binding, self.registration.clone(), input)
            .await
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
}

/// One launched app-server process owned by an exact run binding.
///
/// Implementations must expose only the reviewed typed operations below. They
/// must not expose a generic JSON-RPC method, must reject forbidden or unknown
/// inbound traffic, and must be wrapped in
/// [`AiCodexAppServerLaunchedProcess`] with an exact process-tree kill action.
#[async_trait]
pub trait AiCodexAppServerRunProcess: Send + Sync {
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

    async fn start_fresh_turn(
        &self,
        input: AiCodexAppServerTurnInput,
    ) -> Result<ProviderEventStream, ProviderError> {
        self.process.start_fresh_turn(input).await
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
        }
    }
}

/// Closed app-server JSON-RPC encoder/guard.
///
/// There is intentionally no generic request builder. The provider-specific
/// process actor may emit only the four methods represented here. All
/// server-initiated requests and non-allowlisted notifications fail closed.
#[derive(Debug)]
pub struct AiCodexAppServerProtocolActor {
    next_id: u64,
    pending: BTreeMap<u64, ClientMethod>,
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

    /// Encodes the one allowed post-initialization notification.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidRequest`] when the configured frame
    /// ceiling cannot contain the notification.
    pub fn initialized(&self) -> Result<Vec<u8>, ProviderError> {
        self.encode(json!({"method": "initialized", "params": {}}))
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
        let developer_instructions = if input.instructions().is_empty() {
            Value::Null
        } else {
            Value::String(input.instructions().join("\n\n"))
        };
        self.request(
            ClientMethod::ThreadStart,
            "thread/start",
            json!({
                "model": input.model(),
                "developerInstructions": developer_instructions,
                "ephemeral": true,
            }),
        )
    }

    /// Encodes text-only user input for one exact fresh thread.
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
        if !valid_reference(thread_id) {
            return Err(ProviderError::InvalidRequest);
        }
        self.request(
            ClientMethod::TurnStart,
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": input.input().iter().map(|text| json!({"type": "text", "text": text})).collect::<Vec<_>>(),
            }),
        )
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
        if !valid_reference(thread_id) || !valid_reference(turn_id) {
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
            // Every server-initiated request is forbidden in this phase.
            return Err(ProviderError::Rejected);
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
            return Ok(AiCodexAppServerInbound::Response {
                id,
                method: client_method_name(method),
                result,
            });
        }
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "method" | "params"))
        {
            return Err(ProviderError::Rejected);
        }
        let method = object
            .get("method")
            .and_then(Value::as_str)
            .filter(|method| allowed_notification(method))
            .ok_or(ProviderError::Rejected)?;
        let params = object
            .get("params")
            .filter(|params| params.is_object())
            .cloned()
            .ok_or(ProviderError::Rejected)?;
        validate_allowed_notification(method, &params)?;
        Ok(AiCodexAppServerInbound::Notification {
            method: method.to_owned(),
            params,
        })
    }
}

fn client_method_name(method: ClientMethod) -> &'static str {
    match method {
        ClientMethod::Initialize => "initialize",
        ClientMethod::ThreadStart => "thread/start",
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
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::stream;

    use super::*;
    use crate::{AiRunId, AiSessionId, ProviderEvent};
    use uuid::Uuid;

    struct Counters {
        launches: AtomicUsize,
        turns: AtomicUsize,
        interrupts: AtomicUsize,
        shutdowns: AtomicUsize,
        drops: AtomicUsize,
        kills: AtomicUsize,
        pending: AtomicBool,
        stream_error: AtomicBool,
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
        AiProviderRunBinding::new(
            AiSessionId::new(),
            AiRunId::new(),
            Uuid::new_v4(),
            1,
            [owner; 32],
        )
        .expect("test binding should validate")
    }

    fn binding() -> AiProviderRunBinding {
        binding_for_owner(1)
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

    #[test]
    fn registration_binds_supported_protocol_and_all_immutable_identity_members() {
        let first = registration("1.0.0");
        let same = registration("1.0.0");
        let changed = registration("2.0.0");
        assert_eq!(first.identity(), same.identity());
        assert_ne!(first.identity(), changed.identity());
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
                AiProviderRunBinding::new(session_id, run_id, attempt_id, 1, [1; 32])
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
            AiCodexAppServerTurnInput::try_from_model_request(retained),
            Err(ProviderError::Unsupported)
        ));

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
        let thread = String::from_utf8(
            guard
                .start_fresh_thread(&turn())
                .expect("thread should encode"),
        )
        .expect("frame should be UTF-8");
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
        assert!(!thread.contains("dynamicTools"));
        assert!(!turn.contains("trusted"));
        assert!(!turn.contains("\"model\""));
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
            let mut guard =
                AiCodexAppServerProtocolActor::new(16 * 1024).expect("test guard should validate");
            let frame = serde_json::to_vec(&json!({
                "method": "item/started",
                "params": {"item": {"type": item_type, "id": "item-1"}}
            }))
            .expect("test frame should encode");
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

        let delta = br#"{"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","delta":"hello"}}"#;
        assert!(matches!(
            guard.accept(delta),
            Ok(AiCodexAppServerInbound::Notification { .. })
        ));
        let unknown = br#"{"method":"turn/steer","params":{}}"#;
        assert!(matches!(
            guard.accept(unknown),
            Err(ProviderError::Rejected)
        ));
    }
}
