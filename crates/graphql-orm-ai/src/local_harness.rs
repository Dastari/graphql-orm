//! Deployment-registered installed local-harness boundary.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use thiserror::Error;

use crate::{
    AiProvider, ModelContinuationMode, ModelInputBlock, ModelRequest, ProviderCapabilities,
    ProviderError, ProviderEvent, ProviderEventStream, ProviderKind, ProviderRequestContext,
};

const MAXIMUM_REGISTRATIONS: usize = 128;
const MAXIMUM_ARGUMENTS: usize = 64;
const MAXIMUM_ARGUMENT_BYTES: usize = 8 * 1024;
const MAXIMUM_ID_BYTES: usize = 200;
const MAXIMUM_VERSION_BYTES: usize = 200;
const MAXIMUM_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAXIMUM_TOTAL_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const MAXIMUM_STDERR_BYTES: usize = 64 * 1024;
const MAXIMUM_FRAMES: usize = 65_536;
const MAXIMUM_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const MAXIMUM_MEMORY_BYTES: u64 = 1024 * 1024 * 1024 * 1024;

/// Resource ceilings that a trusted local-harness launcher must enforce.
///
/// These values are deployment limits, not evidence that an operating-system
/// sandbox actually applied them. Implementations of
/// [`AiLocalHarnessProcessLauncher`] remain responsible for fail-closed
/// enforcement before an executable receives request bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiLocalHarnessLimits {
    maximum_request_bytes: usize,
    maximum_frame_bytes: usize,
    maximum_total_output_bytes: usize,
    maximum_stderr_bytes: usize,
    maximum_frames: usize,
    startup_timeout: Duration,
    turn_timeout: Duration,
    shutdown_timeout: Duration,
    maximum_memory_bytes: u64,
    maximum_cpu_time: Duration,
}

impl AiLocalHarnessLimits {
    /// Builds an exact set of process and protocol ceilings.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidConfiguration`] when a value is zero,
    /// internally inconsistent, or exceeds the crate's hard safety ceiling.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        maximum_request_bytes: usize,
        maximum_frame_bytes: usize,
        maximum_total_output_bytes: usize,
        maximum_stderr_bytes: usize,
        maximum_frames: usize,
        startup_timeout: Duration,
        turn_timeout: Duration,
        shutdown_timeout: Duration,
        maximum_memory_bytes: u64,
        maximum_cpu_time: Duration,
    ) -> Result<Self, ProviderError> {
        let limits = Self {
            maximum_request_bytes,
            maximum_frame_bytes,
            maximum_total_output_bytes,
            maximum_stderr_bytes,
            maximum_frames,
            startup_timeout,
            turn_timeout,
            shutdown_timeout,
            maximum_memory_bytes,
            maximum_cpu_time,
        };
        limits.validate()?;
        Ok(limits)
    }

    fn validate(self) -> Result<(), ProviderError> {
        if self.maximum_request_bytes == 0
            || self.maximum_request_bytes > self.maximum_frame_bytes
            || self.maximum_frame_bytes == 0
            || self.maximum_frame_bytes > MAXIMUM_FRAME_BYTES
            || self.maximum_total_output_bytes < self.maximum_frame_bytes
            || self.maximum_total_output_bytes > MAXIMUM_TOTAL_OUTPUT_BYTES
            || self.maximum_stderr_bytes == 0
            || self.maximum_stderr_bytes > MAXIMUM_STDERR_BYTES
            || self.maximum_frames == 0
            || self.maximum_frames > MAXIMUM_FRAMES
            || invalid_timeout(self.startup_timeout)
            || invalid_timeout(self.turn_timeout)
            || invalid_timeout(self.shutdown_timeout)
            || self.maximum_memory_bytes == 0
            || self.maximum_memory_bytes > MAXIMUM_MEMORY_BYTES
            || invalid_timeout(self.maximum_cpu_time)
        {
            return Err(ProviderError::InvalidConfiguration(
                "invalid local-harness resource limits".to_owned(),
            ));
        }
        Ok(())
    }

    /// Maximum serialized request-frame bytes.
    pub const fn maximum_request_bytes(self) -> usize {
        self.maximum_request_bytes
    }

    /// Maximum bytes in one stdout JSON line.
    pub const fn maximum_frame_bytes(self) -> usize {
        self.maximum_frame_bytes
    }

    /// Maximum total stdout bytes for one turn.
    pub const fn maximum_total_output_bytes(self) -> usize {
        self.maximum_total_output_bytes
    }

    /// Maximum discarded stderr bytes for one turn.
    pub const fn maximum_stderr_bytes(self) -> usize {
        self.maximum_stderr_bytes
    }

    /// Maximum stdout event frames for one turn.
    pub const fn maximum_frames(self) -> usize {
        self.maximum_frames
    }

    /// Maximum launch plus initial-input handoff time.
    pub const fn startup_timeout(self) -> Duration {
        self.startup_timeout
    }

    /// Maximum wall time after the request handoff.
    pub const fn turn_timeout(self) -> Duration {
        self.turn_timeout
    }

    /// Maximum graceful/forced termination wait.
    pub const fn shutdown_timeout(self) -> Duration {
        self.shutdown_timeout
    }

    /// Maximum process-tree memory requested from the sandbox.
    pub const fn maximum_memory_bytes(self) -> u64 {
        self.maximum_memory_bytes
    }

    /// Maximum process-tree CPU time requested from the sandbox.
    pub const fn maximum_cpu_time(self) -> Duration {
        self.maximum_cpu_time
    }
}

impl Default for AiLocalHarnessLimits {
    fn default() -> Self {
        Self {
            maximum_request_bytes: 2 * 1024 * 1024,
            maximum_frame_bytes: 2 * 1024 * 1024,
            maximum_total_output_bytes: 16 * 1024 * 1024,
            maximum_stderr_bytes: 16 * 1024,
            maximum_frames: 16_384,
            startup_timeout: Duration::from_secs(10),
            turn_timeout: Duration::from_secs(120),
            shutdown_timeout: Duration::from_secs(5),
            maximum_memory_bytes: 4 * 1024 * 1024 * 1024,
            maximum_cpu_time: Duration::from_secs(120),
        }
    }
}

/// Immutable deployment registration for one installed harness model.
///
/// Registrations are ordinary host configuration, never GraphQL input or model
/// output. This type deliberately has no environment, shell, mount, credential,
/// or network field. A launcher must execute the absolute path directly with
/// the fixed arguments, clear the inherited environment, deny network, verify
/// the executable digest without a time-of-check/time-of-use gap, apply the
/// named sandbox profile and resource limits, and contain the complete process
/// tree. Constructing this value proves only syntactic registration validity.
#[derive(Clone)]
pub struct AiLocalHarnessRegistration {
    logical_model: String,
    executable: PathBuf,
    arguments: Arc<[String]>,
    working_directory: PathBuf,
    executable_sha256: String,
    executable_version: String,
    sandbox_profile: String,
    limits: AiLocalHarnessLimits,
    capabilities: ProviderCapabilities,
    resolved_entity_reference_delivery: Option<Duration>,
}

impl std::fmt::Debug for AiLocalHarnessRegistration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiLocalHarnessRegistration")
            .field("logical_model", &self.logical_model)
            .field("executable", &"<deployment-registered>")
            .field("arguments", &"<deployment-registered>")
            .field("working_directory", &"<sandboxed>")
            .field("executable_sha256", &"<verified-digest>")
            .field("executable_version", &"<deployment-reviewed>")
            .field("sandbox_profile", &"<deployment-reviewed>")
            .field("limits", &self.limits)
            .field("capabilities", &self.capabilities)
            .field(
                "resolved_entity_reference_delivery",
                &self.resolved_entity_reference_delivery,
            )
            .finish()
    }
}

impl AiLocalHarnessRegistration {
    /// Creates one immutable deployment-owned registration.
    ///
    /// The safe protocol supports streaming text, optional structured output,
    /// and an optional exact stateless application-tool contract. File/image
    /// input, built-in tools, provider-retained continuation, background work,
    /// embeddings, and coding authority must be false in `capabilities`.
    /// `custom_tools` and `stateless_continuation` must be enabled together;
    /// `parallel_tool_calls` may be enabled only with that pair.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidConfiguration`] for malformed IDs,
    /// relative/non-normal paths, missing digest/sandbox/version, excessive
    /// arguments, invalid limits, or a capability that this safe boundary does
    /// not implement.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        logical_model: impl Into<String>,
        executable: impl Into<PathBuf>,
        arguments: Vec<String>,
        working_directory: impl Into<PathBuf>,
        executable_sha256: impl Into<String>,
        executable_version: impl Into<String>,
        sandbox_profile: impl Into<String>,
        limits: AiLocalHarnessLimits,
        capabilities: ProviderCapabilities,
    ) -> Result<Self, ProviderError> {
        let registration = Self {
            logical_model: logical_model.into(),
            executable: executable.into(),
            arguments: arguments.into(),
            working_directory: working_directory.into(),
            executable_sha256: executable_sha256.into(),
            executable_version: executable_version.into(),
            sandbox_profile: sandbox_profile.into(),
            limits,
            capabilities,
            resolved_entity_reference_delivery: None,
        };
        registration.validate()?;
        Ok(registration)
    }

    /// Opts this registration in to receiving what entity references
    /// resolved to. Off by default: without this call the harness process is
    /// sent the unavailable marker for every reference, whatever the
    /// deployment's [`crate::AiEntityReferenceRegistry`] resolves.
    ///
    /// `maximum_age` bounds the time from the start of resolution to the
    /// moment the one stdin write that carries the data may begin. It covers
    /// the host hook, process launch and frame encoding. A request whose
    /// resolution is older than that when its write would start is written
    /// with markers instead. A write that has started may complete; one
    /// still in progress when the bound passes is stopped, the turn fails
    /// and the process is killed without a graceful close, so no remainder
    /// of the frame is delivered.
    ///
    /// Opting in is a disclosure decision the crate cannot check. Reference
    /// data written to the process is outside the crate from then on: the
    /// host must make sure the harness, its sandbox and any log or transcript
    /// it keeps may hold that data and are cleaned up under the host's
    /// retention rules.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidConfiguration`] when `maximum_age` is
    /// zero or exceeds
    /// [`crate::AI_MAXIMUM_ENTITY_REFERENCE_DELIVERY_AGE`].
    pub fn with_resolved_entity_reference_delivery(
        mut self,
        maximum_age: Duration,
    ) -> Result<Self, ProviderError> {
        if maximum_age.is_zero() || maximum_age > crate::AI_MAXIMUM_ENTITY_REFERENCE_DELIVERY_AGE {
            return Err(ProviderError::InvalidConfiguration(
                "invalid entity-reference delivery age".to_owned(),
            ));
        }
        self.resolved_entity_reference_delivery = Some(maximum_age);
        Ok(self)
    }

    /// The freshness bound of resolved entity-reference delivery, or `None`
    /// when the registration has not opted in and receives markers only.
    pub const fn resolved_entity_reference_delivery(&self) -> Option<Duration> {
        self.resolved_entity_reference_delivery
    }

    fn validate(&self) -> Result<(), ProviderError> {
        self.limits.validate()?;
        if !valid_identifier(&self.logical_model)
            || !normal_absolute_path(&self.executable)
            || !normal_absolute_path(&self.working_directory)
            || self.arguments.len() > MAXIMUM_ARGUMENTS
            || self.arguments.iter().any(|argument| {
                argument.is_empty()
                    || argument.len() > MAXIMUM_ARGUMENT_BYTES
                    || argument.contains('\0')
            })
            || !crate::valid_sha256(&self.executable_sha256)
            || self.executable_version.is_empty()
            || self.executable_version.len() > MAXIMUM_VERSION_BYTES
            || !valid_identifier(&self.sandbox_profile)
            || !safe_capabilities(&self.capabilities)
        {
            return Err(ProviderError::InvalidConfiguration(
                "invalid local-harness registration".to_owned(),
            ));
        }
        Ok(())
    }

    /// Logical model selected by a server-authored route.
    pub fn logical_model(&self) -> &str {
        &self.logical_model
    }

    /// Absolute executable path supplied only to the trusted launcher.
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Fixed argument vector supplied only to the trusted launcher.
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    /// Absolute sandbox working directory supplied only to the trusted
    /// launcher. This is not an application workspace grant.
    pub fn working_directory(&self) -> &Path {
        &self.working_directory
    }

    /// Required lowercase SHA-256 of the executable image.
    pub fn executable_sha256(&self) -> &str {
        &self.executable_sha256
    }

    /// Deployment-reviewed executable/protocol version.
    pub fn executable_version(&self) -> &str {
        &self.executable_version
    }

    /// Deployment-owned operating-system/container sandbox profile.
    pub fn sandbox_profile(&self) -> &str {
        &self.sandbox_profile
    }

    /// Required process and protocol ceilings.
    pub const fn limits(&self) -> AiLocalHarnessLimits {
        self.limits
    }

    /// Capabilities reviewed for this exact registration.
    pub fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }
}

/// Immutable logical-model registry for installed harnesses.
///
/// All registrations must expose the same safe provider capability contract so
/// the provider-neutral router cannot mistake a union for model-specific
/// authority. Individual executable details remain inaccessible to model
/// input and GraphQL configuration.
#[derive(Clone)]
pub struct AiLocalHarnessRegistry {
    registrations: Arc<BTreeMap<String, Arc<AiLocalHarnessRegistration>>>,
    capabilities: ProviderCapabilities,
}

impl std::fmt::Debug for AiLocalHarnessRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiLocalHarnessRegistry")
            .field("registration_count", &self.registrations.len())
            .field("capabilities", &self.capabilities)
            .finish()
    }
}

impl AiLocalHarnessRegistry {
    /// Validates and freezes deployment registrations.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidConfiguration`] for an empty/oversized
    /// set, duplicate logical model, invalid registration, or capability
    /// mismatch.
    pub fn new(
        registrations: impl IntoIterator<Item = AiLocalHarnessRegistration>,
    ) -> Result<Self, ProviderError> {
        let mut by_model = BTreeMap::new();
        let mut capabilities = None;
        for registration in registrations {
            registration.validate()?;
            if capabilities
                .as_ref()
                .is_some_and(|expected| expected != registration.capabilities())
            {
                return Err(ProviderError::InvalidConfiguration(
                    "local-harness capabilities differ by registration".to_owned(),
                ));
            }
            capabilities.get_or_insert_with(|| registration.capabilities().clone());
            let model = registration.logical_model().to_owned();
            if by_model.insert(model, Arc::new(registration)).is_some()
                || by_model.len() > MAXIMUM_REGISTRATIONS
            {
                return Err(ProviderError::InvalidConfiguration(
                    "invalid local-harness registry".to_owned(),
                ));
            }
        }
        let capabilities = capabilities.ok_or_else(|| {
            ProviderError::InvalidConfiguration("empty local-harness registry".to_owned())
        })?;
        Ok(Self {
            registrations: Arc::new(by_model),
            capabilities,
        })
    }

    /// Returns one exact deployment registration by server-authored logical
    /// model name.
    pub fn registration(&self, logical_model: &str) -> Option<Arc<AiLocalHarnessRegistration>> {
        self.registrations.get(logical_model).cloned()
    }

    /// Common safe capability contract for every registered model.
    pub fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }
}

/// Non-sensitive local process transport failure.
///
/// Variants intentionally contain no stderr, command, path, request, or
/// environment text. Launchers must place any redacted diagnostics in a
/// deployment-owned sink rather than return raw process data.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiLocalHarnessProcessError {
    /// Process could not be safely launched or became unavailable.
    #[error("local harness process unavailable")]
    Unavailable,
    /// Bounded input could not be delivered completely.
    #[error("local harness input failed")]
    InputFailed,
    /// Output transport failed.
    #[error("local harness output failed")]
    OutputFailed,
    /// Process cancellation/termination was requested.
    #[error("local harness process cancelled")]
    Cancelled,
}

/// One raw process observation delivered to the bounded protocol driver.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiLocalHarnessProcessOutput {
    /// Arbitrarily chunked stdout bytes.
    Stdout(Vec<u8>),
    /// Arbitrarily chunked stderr bytes. Content is counted and discarded.
    Stderr(Vec<u8>),
    /// Complete process-tree exit observation.
    Exited {
        /// Whether the reviewed harness exited successfully.
        success: bool,
    },
}

/// One safely launched local process session.
///
/// Implementations must own the complete process tree and synchronously
/// initiate forced tree termination when dropped before `Exited`. This drop
/// requirement is what protects stream cancellation, because Rust `Drop`
/// cannot await an asynchronous cleanup. `terminate` remains the bounded
/// explicit cleanup path for protocol errors and timeouts.
#[async_trait]
pub trait AiLocalHarnessProcess: Send {
    /// Writes the one bounded JSON-lines request frame exactly.
    ///
    /// The returned future may be dropped before it completes. That happens
    /// when the frame carries resolved entity-reference data and the
    /// registration's freshness bound passes during the write. The driver
    /// then drops the process without calling [`Self::terminate`] or
    /// [`Self::close_stdin`]. From the moment the future is dropped, no byte
    /// of the frame that the harness has not yet received may reach it: the
    /// implementation must discard anything it buffered, must not flush it
    /// from `Drop` or from any later call, and its `Drop` must kill the
    /// process tree without a graceful close.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive transport error when the complete frame was not
    /// accepted. Partial delivery must be treated as failure.
    async fn write_stdin(&mut self, bytes: &[u8]) -> Result<(), AiLocalHarnessProcessError>;

    /// Closes stdin after the single request frame.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive transport error when EOF could not be delivered.
    async fn close_stdin(&mut self) -> Result<(), AiLocalHarnessProcessError>;

    /// Returns the next stdout/stderr/exit observation.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive transport error when bounded output or a proven
    /// exit observation cannot be obtained.
    async fn next_output(
        &mut self,
    ) -> Result<AiLocalHarnessProcessOutput, AiLocalHarnessProcessError>;

    /// Terminates the complete process tree. Implementations should first use
    /// a supported protocol close only when it stays within the supplied host
    /// sandbox policy, then force termination. Termination must never deliver
    /// input the harness has not yet received: bytes buffered from an
    /// incomplete [`Self::write_stdin`] are discarded, not flushed.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive cancellation error when termination could not
    /// be confirmed within the launcher's own hard boundary. Drop must still
    /// synchronously initiate forced process-tree termination.
    async fn terminate(&mut self) -> Result<(), AiLocalHarnessProcessError>;
}

/// Trusted deployment seam that applies an immutable process registration.
///
/// The launcher must verify the executable digest and open/execute the same
/// image atomically, use direct exec without a shell, pass only the fixed
/// arguments, clear the complete inherited environment, supply no stdin/TTY
/// except the returned mediated pipe, deny network and filesystem authority
/// beyond the named sandbox, enforce memory/CPU/wall/output/process-tree
/// limits, and arrange kill-on-drop descendant cleanup. It must never infer
/// extra authority from the initiating user or request payload.
#[async_trait]
pub trait AiLocalHarnessProcessLauncher: Send + Sync {
    /// Launches the exact reviewed registration.
    ///
    /// # Errors
    ///
    /// Returns a non-sensitive error when digest verification, sandbox setup,
    /// direct execution, pipe setup, or process-tree ownership cannot be
    /// established. Cancellation of this future must not leak a partially
    /// created process.
    async fn launch(
        &self,
        registration: Arc<AiLocalHarnessRegistration>,
    ) -> Result<Box<dyn AiLocalHarnessProcess>, AiLocalHarnessProcessError>;
}

/// Provider-facing installed harness driver.
#[async_trait]
pub trait AiLocalHarnessDriver: Send + Sync {
    /// Starts one already-authorized request against the exact registration.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError`] for registration/request mismatch, unsupported
    /// authority, launch/input failure, protocol/limit rejection, timeout, or
    /// cancellation.
    async fn stream(
        &self,
        registration: Arc<AiLocalHarnessRegistration>,
        request: ModelRequest,
    ) -> Result<ProviderEventStream, ProviderError>;

    /// Starts one already-authorized request and consumes what its entity
    /// references resolved to for this one dispatch.
    ///
    /// [`AiLocalHarnessProvider`] calls this method only for a registration
    /// that opted in with
    /// [`AiLocalHarnessRegistration::with_resolved_entity_reference_delivery`];
    /// otherwise it calls [`Self::stream`] with markers. The default discards
    /// everything resolved and calls [`Self::stream`] with a plain request in
    /// which every entity reference is the unavailable marker, so a driver
    /// written before this method existed keeps sending markers.
    ///
    /// A driver that overrides it must move `dispatch` into the one write
    /// that delivers the request to the harness process: serialize each JSON
    /// input block through [`crate::ModelDispatchReferences::wire_json`],
    /// write those bytes once, and drop them. It must not copy what
    /// `wire_json` returns into anything that is cloned, stored, logged,
    /// queued or retried. [`crate::ModelDispatchRequest`] is not `Clone`, so
    /// a second write needs a second dispatch, which resolves again.
    ///
    /// It must also honour the registration's freshness bound: take
    /// [`crate::ModelDispatchReferences::write_deadline`] for
    /// [`AiLocalHarnessRegistration::resolved_entity_reference_delivery`],
    /// compare it with the current time after encoding and immediately
    /// before the write starts, and send markers if it has passed. A write
    /// that has started may complete; if it is still in progress when the
    /// bound passes, stop it, fail the request and kill the process without
    /// a graceful close, so that no remainder of the frame is delivered.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError`] for the same failures as [`Self::stream`].
    async fn stream_dispatch(
        &self,
        registration: Arc<AiLocalHarnessRegistration>,
        dispatch: crate::ModelDispatchRequest,
    ) -> Result<ProviderEventStream, ProviderError> {
        self.stream(registration, dispatch.into_unavailable_request())
            .await
    }
}

/// Bounded JSON-lines driver over a trusted process launcher.
///
/// The request is one versioned JSON line. Output is a sequence of normalized
/// [`ProviderEvent`] JSON lines followed by a successful process exit. Only
/// started/text/tool/usage/completed events are accepted by protocol v2.
/// Tool events must name one exact offered tool and form a bounded,
/// start-before-delta-before-complete sequence. Response IDs, reasoning,
/// citations, built-ins, and unknown events fail closed.
pub struct AiJsonLinesLocalHarnessDriver {
    launcher: Arc<dyn AiLocalHarnessProcessLauncher>,
}

impl std::fmt::Debug for AiJsonLinesLocalHarnessDriver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiJsonLinesLocalHarnessDriver")
            .finish_non_exhaustive()
    }
}

impl AiJsonLinesLocalHarnessDriver {
    /// Creates a bounded driver over a deployment-owned safe launcher.
    pub fn new(launcher: Arc<dyn AiLocalHarnessProcessLauncher>) -> Self {
        Self { launcher }
    }
}

#[async_trait]
impl AiLocalHarnessDriver for AiJsonLinesLocalHarnessDriver {
    async fn stream(
        &self,
        registration: Arc<AiLocalHarnessRegistration>,
        request: ModelRequest,
    ) -> Result<ProviderEventStream, ProviderError> {
        // A plain request carries no resolutions: every entity reference in
        // it is written as the unavailable marker.
        self.stream_dispatch(
            registration,
            crate::ModelDispatchRequest::withholding(request),
        )
        .await
    }

    async fn stream_dispatch(
        &self,
        registration: Arc<AiLocalHarnessRegistration>,
        dispatch: crate::ModelDispatchRequest,
    ) -> Result<ProviderEventStream, ProviderError> {
        // `request` holds handles only. `references` is what they resolved to
        // for this dispatch. It is read at most once, after the process is
        // ready and immediately before the one stdin write.
        let (request, references) = dispatch.into_parts();
        validate_registered_request(&registration, &request)?;
        let limits = registration.limits();
        let offered_tool_ids = request
            .tools
            .iter()
            .map(|tool| tool.tool_id.clone())
            .collect::<BTreeSet<_>>();
        let maximum_output_tokens = request
            .maximum_output_tokens
            .expect("validated local-harness output ceiling");
        let fits = |frame: &[u8]| {
            frame.len() <= limits.maximum_request_bytes()
                && frame.len() <= limits.maximum_frame_bytes()
        };
        // Resolved data may be written only if the registration opted in,
        // and only until this instant, which is counted from the start of
        // resolution in the executor.
        let resolved_deadline = registration
            .resolved_entity_reference_delivery()
            .and_then(|maximum_age| references.write_deadline(maximum_age));
        // The frame that is sent whenever resolved data is not: every
        // reference is the marker. It must fit, and it is what the request is
        // refused or admitted on before any process is launched.
        let marker_frame = {
            let (request, markers) =
                crate::ModelDispatchRequest::withholding(request.clone()).into_parts();
            encode_request_frame(&request, &markers)?
        };
        if !fits(&marker_frame) {
            return Err(ProviderError::InvalidRequest);
        }

        let startup_deadline = tokio::time::Instant::now() + limits.startup_timeout();
        let mut process =
            timeout_until(startup_deadline, self.launcher.launch(registration.clone()))
                .await
                .map_err(map_process_error)?;
        // The process is ready. Encode the resolved frame; a frame over the
        // request ceiling is not sent, the marker frame is.
        let mut resolved_frame = None;
        if let Some(deadline) = resolved_deadline
            && let Ok(frame) = encode_request_frame(&request, &references)
            && fits(&frame)
        {
            resolved_frame = Some((frame, deadline));
        }
        drop(references);
        let mut written = false;
        if let Some((frame, deadline)) = resolved_frame {
            // The freshness deadline bounds when this write may begin. It is
            // checked after encoding, on the first poll and before the
            // process is given a byte, and it is polled before every later
            // poll of the write.
            let outcome = crate::entity_references::write_resolved_before(
                deadline,
                timeout_until(startup_deadline, process.write_stdin(&frame)),
            )
            .await;
            drop(frame);
            match outcome {
                crate::entity_references::ResolvedWrite::Completed(Ok(())) => written = true,
                // Stale before the write began: nothing was written, and the
                // turn continues with the marker frame.
                crate::entity_references::ResolvedWrite::NotStarted => {}
                // The resolved write began and did not complete: the deadline
                // or the startup timeout passed, or the pipe failed. The
                // process may hold part of the frame. It is dropped, which
                // the process contract defines as a forced tree kill that
                // discards unwritten input. `terminate` is deliberately not
                // called: its graceful protocol close could flush.
                crate::entity_references::ResolvedWrite::Abandoned => {
                    drop(process);
                    return Err(map_process_error(AiLocalHarnessProcessError::Cancelled));
                }
                crate::entity_references::ResolvedWrite::Completed(Err(error)) => {
                    drop(process);
                    return Err(map_process_error(error));
                }
            }
        }
        if !written
            && let Err(error) =
                timeout_until(startup_deadline, process.write_stdin(&marker_frame)).await
        {
            stop_process(process.as_mut(), limits.shutdown_timeout()).await;
            return Err(map_process_error(error));
        }
        drop(marker_frame);
        if let Err(error) = timeout_until(startup_deadline, process.close_stdin()).await {
            stop_process(process.as_mut(), limits.shutdown_timeout()).await;
            return Err(map_process_error(error));
        }

        let maximum_input_tokens = registration
            .capabilities()
            .maximum_context_tokens
            .expect("validated local-harness context ceiling");
        Ok(normalized_process_stream(
            process,
            limits,
            maximum_input_tokens,
            maximum_output_tokens,
            offered_tool_ids,
        ))
    }
}

/// `AiProvider` adapter for immutable installed-harness registrations.
///
/// The adapter selects only a deployment-registered logical model and validates
/// the ordinary exact egress and atomic budget context as
/// [`ProviderKind::LocalHarness`] before any process launch. It passes no
/// principal, bearer credential, provider key, endpoint, executable, argument,
/// or environment choice from model input to the driver.
pub struct AiLocalHarnessProvider {
    registry: AiLocalHarnessRegistry,
    driver: Arc<dyn AiLocalHarnessDriver>,
}

impl std::fmt::Debug for AiLocalHarnessProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiLocalHarnessProvider")
            .field("registry", &self.registry)
            .finish_non_exhaustive()
    }
}

impl AiLocalHarnessProvider {
    /// Creates a provider over a frozen deployment registry and safe driver.
    pub fn new(registry: AiLocalHarnessRegistry, driver: Arc<dyn AiLocalHarnessDriver>) -> Self {
        Self { registry, driver }
    }
}

#[async_trait]
impl AiProvider for AiLocalHarnessProvider {
    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::LocalHarness
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.registry.capabilities().clone()
    }

    async fn stream(
        &self,
        dispatch: crate::ModelDispatchRequest,
        context: ProviderRequestContext,
    ) -> Result<ProviderEventStream, ProviderError> {
        // Everything is validated against the handle-only request. For a
        // registration that opted in to resolved delivery, the dispatch
        // value, which alone holds what the request's entity references
        // resolved to, is moved to the driver unsplit: the driver either
        // consumes it in its one write to the harness process or, by default,
        // discards it and sends the unavailable marker.
        context.validate_request(&ProviderKind::LocalHarness, dispatch.request())?;
        let registration = self
            .registry
            .registration(&dispatch.request().model)
            .ok_or(ProviderError::Unsupported)?;
        validate_registered_request(&registration, dispatch.request())?;
        if registration.resolved_entity_reference_delivery().is_none() {
            // Not opted in: the driver is given a plain request with markers
            // and never sees the dispatch value.
            return self
                .driver
                .stream(registration, dispatch.into_unavailable_request())
                .await;
        }
        self.driver.stream_dispatch(registration, dispatch).await
    }
}

fn normalized_process_stream(
    mut process: Box<dyn AiLocalHarnessProcess>,
    limits: AiLocalHarnessLimits,
    maximum_input_tokens: u64,
    maximum_output_tokens: u64,
    offered_tool_ids: BTreeSet<String>,
) -> ProviderEventStream {
    Box::pin(async_stream::try_stream! {
        let deadline = tokio::time::Instant::now() + limits.turn_timeout();
        let mut buffer = Vec::new();
        let mut total_output_bytes = 0usize;
        let mut stderr_bytes = 0usize;
        let mut frame_count = 0usize;
        let mut state = HarnessEventState::new(
            maximum_input_tokens,
            maximum_output_tokens,
            offered_tool_ids,
        );
        loop {
            let observation = match timeout_until(deadline, process.next_output()).await {
                Ok(observation) => observation,
                Err(error) => {
                    stop_process(process.as_mut(), limits.shutdown_timeout()).await;
                    Err(map_process_error(error))?
                }
            };
            match observation {
                AiLocalHarnessProcessOutput::Stdout(chunk) => {
                    total_output_bytes = total_output_bytes.saturating_add(chunk.len());
                    if total_output_bytes > limits.maximum_total_output_bytes() {
                        stop_process(process.as_mut(), limits.shutdown_timeout()).await;
                        Err(ProviderError::Rejected)?;
                    }
                    buffer.extend_from_slice(&chunk);
                    loop {
                        let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') else {
                            if buffer.len() > limits.maximum_frame_bytes() {
                                stop_process(process.as_mut(), limits.shutdown_timeout()).await;
                                Err(ProviderError::Rejected)?;
                            }
                            break;
                        };
                        let mut frame = buffer.drain(..=newline).collect::<Vec<_>>();
                        frame.pop();
                        if frame.last() == Some(&b'\r') {
                            frame.pop();
                        }
                        frame_count = frame_count.saturating_add(1);
                        if frame.is_empty()
                            || frame.len() > limits.maximum_frame_bytes()
                            || frame_count > limits.maximum_frames()
                        {
                            stop_process(process.as_mut(), limits.shutdown_timeout()).await;
                            Err(ProviderError::Rejected)?;
                        }
                        let event: ProviderEvent = match serde_json::from_slice(&frame) {
                            Ok(event) => event,
                            Err(_) => {
                                stop_process(process.as_mut(), limits.shutdown_timeout()).await;
                                Err(ProviderError::Rejected)?
                            }
                        };
                        if state.accept(&event).is_err() {
                            stop_process(process.as_mut(), limits.shutdown_timeout()).await;
                            Err(ProviderError::Rejected)?;
                        }
                        yield event;
                    }
                }
                AiLocalHarnessProcessOutput::Stderr(chunk) => {
                    stderr_bytes = stderr_bytes.saturating_add(chunk.len());
                    if stderr_bytes > limits.maximum_stderr_bytes() {
                        stop_process(process.as_mut(), limits.shutdown_timeout()).await;
                        Err(ProviderError::Rejected)?;
                    }
                }
                AiLocalHarnessProcessOutput::Exited { success } => {
                    if !success || !buffer.is_empty() || !state.complete() {
                        stop_process(process.as_mut(), limits.shutdown_timeout()).await;
                        Err(ProviderError::Rejected)?;
                    }
                    break;
                }
            }
        }
    })
}

struct HarnessEventState {
    started: bool,
    usage: bool,
    completed: bool,
    maximum_input_tokens: u64,
    maximum_output_tokens: u64,
    offered_tool_ids: BTreeSet<String>,
    tool_calls: BTreeMap<String, bool>,
}

impl HarnessEventState {
    fn new(
        maximum_input_tokens: u64,
        maximum_output_tokens: u64,
        offered_tool_ids: BTreeSet<String>,
    ) -> Self {
        Self {
            started: false,
            usage: false,
            completed: false,
            maximum_input_tokens,
            maximum_output_tokens,
            offered_tool_ids,
            tool_calls: BTreeMap::new(),
        }
    }

    fn accept(&mut self, event: &ProviderEvent) -> Result<(), ()> {
        match event {
            ProviderEvent::ResponseStarted { response_id: None }
                if !self.started && !self.usage && !self.completed =>
            {
                self.started = true;
            }
            ProviderEvent::TextDelta { .. } if self.started && !self.usage && !self.completed => {}
            ProviderEvent::ToolCallStarted { call_id, tool_id }
                if self.started
                    && !self.usage
                    && !self.completed
                    && valid_event_reference(call_id)
                    && self.offered_tool_ids.contains(tool_id)
                    && self.tool_calls.len() < 64
                    && !self.tool_calls.contains_key(call_id) =>
            {
                self.tool_calls.insert(call_id.clone(), false);
            }
            ProviderEvent::ToolArgumentsDelta { call_id, .. }
                if self.started
                    && !self.usage
                    && !self.completed
                    && self.tool_calls.get(call_id) == Some(&false) => {}
            ProviderEvent::ToolCallCompleted { call_id, arguments }
                if self.started
                    && !self.usage
                    && !self.completed
                    && arguments.is_object()
                    && self.tool_calls.get(call_id) == Some(&false) =>
            {
                self.tool_calls.insert(call_id.clone(), true);
            }
            ProviderEvent::Usage {
                input_tokens,
                output_tokens,
                cached_input_tokens,
            } if self.started
                && !self.usage
                && !self.completed
                && *input_tokens <= self.maximum_input_tokens
                && *output_tokens <= self.maximum_output_tokens
                && *cached_input_tokens <= *input_tokens
                && self.tool_calls.values().all(|complete| *complete) =>
            {
                self.usage = true;
            }
            ProviderEvent::ResponseCompleted { response_id: None }
                if self.started && self.usage && !self.completed =>
            {
                self.completed = true;
            }
            ProviderEvent::ResponseStarted { .. }
            | ProviderEvent::TextDelta { .. }
            | ProviderEvent::ReasoningSummaryDelta { .. }
            | ProviderEvent::ToolCallStarted { .. }
            | ProviderEvent::ToolArgumentsDelta { .. }
            | ProviderEvent::ToolCallCompleted { .. }
            | ProviderEvent::BuiltinToolStarted { .. }
            | ProviderEvent::BuiltinToolCompleted { .. }
            | ProviderEvent::Citation { .. }
            | ProviderEvent::Usage { .. }
            | ProviderEvent::ResponseCompleted { .. }
            | ProviderEvent::Unknown { .. } => return Err(()),
        }
        Ok(())
    }

    const fn complete(&self) -> bool {
        self.started && self.usage && self.completed
    }
}

/// Encodes the one JSON-lines request frame written to a harness process.
///
/// `request` is the handle-only request. Each JSON input block, in the
/// current input and in the replayed user messages of a stateless
/// continuation, is written as [`crate::ModelDispatchReferences::wire_json`]
/// returns it, so an entity-reference handle becomes what it resolved to for
/// this dispatch or the unavailable marker. The returned bytes are the only
/// copy of resolved data outside `references`.
fn encode_request_frame(
    request: &ModelRequest,
    references: &crate::ModelDispatchReferences,
) -> Result<Vec<u8>, ProviderError> {
    fn rewrite(
        blocks: Option<&mut serde_json::Value>,
        references: &crate::ModelDispatchReferences,
    ) {
        let Some(blocks) = blocks.and_then(serde_json::Value::as_array_mut) else {
            return;
        };
        for block in blocks {
            if block.get("type").and_then(serde_json::Value::as_str) != Some("json") {
                continue;
            }
            let Some(value) = block.get_mut("value") else {
                continue;
            };
            let wire = references.wire_json(value);
            if !std::ptr::eq(wire, &*value) {
                *value = wire.clone();
            }
        }
    }
    let invalid = |_| ProviderError::InvalidRequest;
    let mut input = serde_json::to_value(&request.input).map_err(invalid)?;
    rewrite(Some(&mut input), references);
    let mut continuation = serde_json::to_value(&request.continuation).map_err(invalid)?;
    if let Some(messages) = continuation
        .get_mut("messages")
        .and_then(serde_json::Value::as_array_mut)
    {
        for message in messages {
            if message.get("role").and_then(serde_json::Value::as_str) == Some("user") {
                rewrite(message.get_mut("content"), references);
            }
        }
    }
    let mut encoded = serde_json::to_vec(&json!({
        "protocol": "graphql-orm-ai/local-harness-jsonl/v2",
        "type": "request",
        "model": request.model,
        "instructions": request.instructions,
        "input": input,
        "continuation_mode": request.continuation_mode,
        "continuation": continuation,
        "tools": request.tools,
        "output_schema": request.output_schema,
        "maximum_output_tokens": request.maximum_output_tokens,
    }))
    .map_err(invalid)?;
    encoded.push(b'\n');
    Ok(encoded)
}

fn validate_registered_request(
    registration: &AiLocalHarnessRegistration,
    request: &ModelRequest,
) -> Result<(), ProviderError> {
    request.validate()?;
    let capabilities = registration.capabilities();
    let invalid_continuation = match request.continuation_mode {
        ModelContinuationMode::ProviderRetained => {
            request.continuation.is_some()
                || !request.tools.is_empty()
                || request
                    .input
                    .iter()
                    .any(|block| matches!(block, ModelInputBlock::ToolResult { .. }))
        }
        ModelContinuationMode::StatelessReplay => {
            !capabilities.stateless_continuation
                || (!request.tools.is_empty() && !capabilities.custom_tools)
        }
    };
    if request.model != registration.logical_model()
        || invalid_continuation
        || !request.builtin_tools.is_empty()
        || request
            .input
            .iter()
            .any(|block| matches!(block, ModelInputBlock::Attachment { .. }))
        || (request.output_schema.is_some() && !capabilities.structured_output)
        || request.maximum_output_tokens.is_none()
        || request.maximum_output_tokens.is_some_and(|requested| {
            capabilities
                .maximum_output_tokens
                .is_some_and(|maximum| requested > maximum)
        })
    {
        return Err(ProviderError::Unsupported);
    }
    Ok(())
}

fn safe_capabilities(capabilities: &ProviderCapabilities) -> bool {
    capabilities.streaming
        && capabilities.local
        && !capabilities.image_input
        && !capabilities.file_input
        && capabilities.custom_tools == capabilities.stateless_continuation
        && (!capabilities.parallel_tool_calls || capabilities.custom_tools)
        && !capabilities.web_search
        && !capabilities.file_search
        && !capabilities.code_execution
        && !capabilities.image_generation
        && !capabilities.embeddings
        && !capabilities.background
        && !capabilities.provider_retained_continuation
        && capabilities
            .maximum_context_tokens
            .is_some_and(|value| value > 0)
        && capabilities
            .maximum_output_tokens
            .is_some_and(|value| value > 0)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAXIMUM_ID_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':' | b'/')
        })
}

fn valid_event_reference(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 1_024
        && value.bytes().all(|byte| !byte.is_ascii_control())
}

fn normal_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
}

fn invalid_timeout(duration: Duration) -> bool {
    duration.is_zero() || duration > MAXIMUM_TIMEOUT
}

async fn timeout_until<T>(
    deadline: tokio::time::Instant,
    future: impl std::future::Future<Output = Result<T, AiLocalHarnessProcessError>>,
) -> Result<T, AiLocalHarnessProcessError> {
    let remaining = deadline
        .checked_duration_since(tokio::time::Instant::now())
        .ok_or(AiLocalHarnessProcessError::Cancelled)?;
    tokio::time::timeout(remaining, future)
        .await
        .map_err(|_| AiLocalHarnessProcessError::Cancelled)?
}

async fn stop_process(process: &mut dyn AiLocalHarnessProcess, timeout: Duration) {
    let _ = tokio::time::timeout(timeout, process.terminate()).await;
}

fn map_process_error(error: AiLocalHarnessProcessError) -> ProviderError {
    match error {
        AiLocalHarnessProcessError::Unavailable | AiLocalHarnessProcessError::OutputFailed => {
            ProviderError::Unavailable
        }
        AiLocalHarnessProcessError::InputFailed => ProviderError::Rejected,
        AiLocalHarnessProcessError::Cancelled => ProviderError::Cancelled,
    }
}
