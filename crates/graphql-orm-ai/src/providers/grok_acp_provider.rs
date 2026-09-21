//! Exact Grok ACP launch, provider-run and retained-session boundary.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use agql_auth::Clock;
use async_trait::async_trait;
use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::{
    AiCapabilityDeliveryMode, AiOpenedProviderSession, AiProvider,
    AiProviderCapabilitySessionBinding, AiProviderRunBinding, AiProviderRunCloseOutcome,
    AiProviderRunCloseReason, AiProviderRunInterruptOutcome, AiProviderSessionCursor,
    AiProviderSessionDescriptor, ModelContinuationMode, ModelInputBlock, ModelReasoningEffort,
    ModelReasoningEffortProfile, ModelRequest, ModelToolDefinition, ProviderCapabilities,
    ProviderDynamicToolResponder, ProviderError, ProviderEvent, ProviderEventStream, ProviderKind,
    ProviderRequestContext,
};

const CURSOR_KIND: &str = "grok.acp.session.v1";
const PROTOCOL: &str = "grok-acp-sdk-v1";

fn rejected() -> ProviderError {
    ProviderError::Rejected
}
fn timeout() -> ProviderError {
    ProviderError::Classified(crate::AiProviderFailureCategory::Timeout)
}

/// Immutable deployment registration for the curated SDK-broker Grok profile.
/// This configuration is not sandbox, login, budget or provider admission proof.
/// The trusted factory must attest all those launch properties before use.
#[derive(Clone)]
pub struct AiGrokAcpRegistration {
    profile_id: String,
    model: String,
    usage_model: String,
    executable_sha256: String,
    executable_version: String,
    sandbox_profile: String,
    effort_profile: ModelReasoningEffortProfile,
    effort: ModelReasoningEffort,
    bootstrap: &'static str,
    tools: Vec<ModelToolDefinition>,
    maximum_input_tokens: u64,
    maximum_output_tokens: u64,
    maximum_model_calls: u32,
    identity: String,
    capability_binding: Option<AiProviderCapabilitySessionBinding>,
}

impl std::fmt::Debug for AiGrokAcpRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiGrokAcpRegistration")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl AiGrokAcpRegistration {
    /// Freezes the executable, sandbox, model, effort, static bootstrap, broker
    /// definitions and aggregate prompt ceilings into one registration identity.
    ///
    /// `maximum_output_tokens` is an aggregate provider-enforced ceiling across
    /// every sampler round, not a post-hoc measurement. The factory must refuse
    /// admission if it cannot enforce this or exclude unmetered side calls.
    ///
    /// # Errors
    /// Rejects malformed identities, unsupported effort, unbounded token/round
    /// ceilings, non-static bootstrap and invalid/duplicate tool definitions.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        profile_id: String,
        model: String,
        executable_sha256: String,
        executable_version: String,
        sandbox_profile: String,
        effort_profile: ModelReasoningEffortProfile,
        effort: ModelReasoningEffort,
        bootstrap: &'static str,
        tools: Vec<ModelToolDefinition>,
        maximum_input_tokens: u64,
        maximum_output_tokens: u64,
        maximum_model_calls: u32,
    ) -> Result<Self, ProviderError> {
        if [&profile_id, &model, &executable_version, &sandbox_profile]
            .iter()
            .any(|v| v.is_empty() || v.len() > 200 || v.chars().any(char::is_control))
            || !crate::valid_sha256(&executable_sha256)
            || effort_profile.model() != model
            || effort == ModelReasoningEffort::Unspecified
            || !effort_profile.supports(effort)
            || bootstrap.is_empty()
            || bootstrap.len() > 64 * 1024
            || !(1..=16_000_000).contains(&maximum_input_tokens)
            || !(1..=1_000_000).contains(&maximum_output_tokens)
            || !(1..=64).contains(&maximum_model_calls)
        {
            return Err(ProviderError::InvalidRequest);
        }
        super::grok_acp::AiGrokAcpSdkBroker::new(
            "validation".into(),
            "validation".into(),
            tools.clone(),
            64,
            1024 * 1024,
            64 * 1024 * 1024,
        )?;
        let canonical = serde_json::to_vec(&serde_json::json!({
            "protocol":PROTOCOL,"profile":profile_id,"model":model,"executable":executable_sha256,
            "version":executable_version,"sandbox":sandbox_profile,"effort":effort,"effort_profile":effort_profile,
            "bootstrap":bootstrap,"tools":tools,"input":maximum_input_tokens,
            "output":maximum_output_tokens,"rounds":maximum_model_calls,
        }))
        .map_err(|_| ProviderError::InvalidRequest)?;
        let identity = hex::encode(Sha256::digest(canonical));
        Ok(Self {
            profile_id,
            usage_model: model.clone(),
            model,
            executable_sha256,
            executable_version,
            sandbox_profile,
            effort_profile,
            effort,
            bootstrap,
            tools,
            maximum_input_tokens,
            maximum_output_tokens,
            maximum_model_calls,
            identity,
            capability_binding: None,
        })
    }

    /// Admits one exact FixedBroker capability-session descriptor.
    ///
    /// # Errors
    /// Rejects delivery/model/effort/registration drift or any definition set
    /// different from the static broker surface frozen into this registration.
    pub fn with_capability_binding(
        mut self,
        binding: AiProviderCapabilitySessionBinding,
    ) -> Result<Self, ProviderError> {
        if binding.delivery_mode() != AiCapabilityDeliveryMode::FixedBroker
            || binding.model() != self.model
            || binding.reasoning_effort() != self.effort
            || binding.registration_identity() != self.identity
            || binding.static_bootstrap_tool_fingerprints()
                != &self.tools.iter().map(|t| t.fingerprint.clone()).collect()
        {
            return Err(rejected());
        }
        self.capability_binding = Some(binding);
        Ok(self)
    }
    /// Freezes the provider's exact billing/model-usage alias. Changing it
    /// invalidates previously admitted capability-session bindings.
    ///
    /// # Errors
    /// Rejects empty, oversized or control-bearing aliases.
    pub fn with_usage_model(mut self, model: String) -> Result<Self, ProviderError> {
        if model.is_empty() || model.len() > 200 || model.chars().any(char::is_control) {
            return Err(ProviderError::InvalidRequest);
        }
        self.identity = hex::encode(Sha256::digest(format!("{}\0{}", self.identity, model)));
        self.usage_model = model;
        self.capability_binding = None;
        Ok(self)
    }
    /// Exact provider-reported model usage alias frozen by registration.
    pub fn usage_model(&self) -> &str {
        &self.usage_model
    }
    /// Raw registration fingerprint for the selected effort, before binding.
    ///
    /// # Errors
    /// Rejects an effort different from this immutable registration.
    pub fn provider_session_fingerprint(
        &self,
        effort: ModelReasoningEffort,
    ) -> Result<String, ProviderError> {
        if effort != self.effort {
            return Err(rejected());
        }
        Ok(self.identity.clone())
    }
    /// Stable launch identity before the capability-session overlay.
    pub fn identity(&self) -> &str {
        &self.identity
    }
    /// Final descriptor fingerprint; binding a capability overlay changes it.
    pub fn session_fingerprint(&self) -> &str {
        self.capability_binding
            .as_ref()
            .map_or(&self.identity, |b| b.fingerprint())
    }
    /// Stable provider profile; retained cleanup uses this namespace.
    pub fn provider_profile_id(&self) -> &str {
        &self.profile_id
    }
    /// Registered exact model.
    pub fn model(&self) -> &str {
        &self.model
    }
    /// Registered exact executable SHA-256.
    pub fn executable_sha256(&self) -> &str {
        &self.executable_sha256
    }
    /// Reviewed executable version.
    pub fn executable_version(&self) -> &str {
        &self.executable_version
    }
    /// Deployment's reviewed OS sandbox profile.
    pub fn sandbox_profile(&self) -> &str {
        &self.sandbox_profile
    }
    /// Frozen effort; no turn may mutate it.
    pub fn reasoning_effort(&self) -> ModelReasoningEffort {
        self.effort
    }
    /// Static host policy; never populated from model or user content.
    pub fn bootstrap(&self) -> &'static str {
        self.bootstrap
    }
    /// Exact frozen broker definitions.
    pub fn tools(&self) -> &[ModelToolDefinition] {
        &self.tools
    }
    /// Aggregate input ceiling including retained provider context.
    pub fn maximum_input_tokens(&self) -> u64 {
        self.maximum_input_tokens
    }
    /// Aggregate output ceiling across every model round.
    pub fn maximum_output_tokens(&self) -> u64 {
        self.maximum_output_tokens
    }
    /// Hard sampler-round ceiling, including rounds hidden inside one prompt.
    pub fn maximum_model_calls(&self) -> u32 {
        self.maximum_model_calls
    }
    /// Versioned protocol identifier for the ordinary durable descriptor.
    pub fn protocol_version(&self) -> &'static str {
        PROTOCOL
    }

    fn matches(&self, descriptor: &AiProviderSessionDescriptor) -> bool {
        descriptor.provider_kind() == &ProviderKind::LocalHarness
            && descriptor.provider_profile_id() == self.profile_id
            && descriptor.provider_model() == self.model
            && descriptor.protocol_version() == PROTOCOL
            && descriptor.registration_fingerprint() == self.session_fingerprint()
    }
    fn validate_request(&self, request: &ModelRequest) -> Result<(), ProviderError> {
        request.validate()?;
        if request.model != self.model
            || request.reasoning_effort != self.effort
            || request.tools != self.tools
            || !request.instructions.is_empty()
            || request.continuation_mode != ModelContinuationMode::ProviderRetained
            || request.continuation.is_some()
            || !request.builtin_tools.is_empty()
            || request.output_schema.is_some()
            || request.maximum_builtin_tool_calls.is_some()
            || request.reasoning_summary != crate::ModelReasoningSummaryRequest::Disabled
            || request
                .maximum_output_tokens
                .is_none_or(|n| n < self.maximum_output_tokens)
            || request.input.is_empty()
            || request.input.iter().any(|b| {
                !matches!(
                    b,
                    ModelInputBlock::Text { .. } | ModelInputBlock::Json { .. }
                )
            })
            || request.conservative_egress_bytes() > self.maximum_input_tokens
        {
            return Err(ProviderError::Unsupported);
        }
        Ok(())
    }
}

/// Trusted deployment-owned, strictly typed Grok process operations.
/// Implementations must use the upstream ACP codec/actor, never execute tools,
/// disclose credentials, replay completed callbacks, or infer session absence
/// from process death. Input is admitted only after durable cursor binding.
#[async_trait]
pub trait AiGrokAcpRunProcess: Send + Sync {
    /// Creates an empty session with only registration bootstrap and tools.
    ///
    /// # Errors
    /// Rejects failed authentication, configuration or protocol validation.
    async fn create_empty_session(&self) -> Result<AiProviderSessionCursor, ProviderError>;
    /// Loads the exact retained cursor, checks frozen profile/model/effort and
    /// discards history notifications without routing callbacks or user input.
    ///
    /// # Errors
    /// Rejects missing sessions, replay callbacks or frozen-profile drift.
    async fn resume_session(&self, cursor: &AiProviderSessionCursor) -> Result<(), ProviderError>;
    /// Starts one exact admitted prompt; custom calls go only to `responder`.
    /// Must enforce the immutable aggregate sampler limits before execution.
    ///
    /// # Errors
    /// Rejects unsupported state, transport failures and uncertain terminals.
    async fn prompt(
        &self,
        input: Vec<String>,
        responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError>;
    /// Sends `session/cancel` for this exact live session. This is not settled
    /// cancellation proof and must not report partial context as reusable.
    ///
    /// # Errors
    /// Reports transport failure or a missing active session.
    async fn cancel(&self) -> Result<(), ProviderError>;
    /// Deletes this exact session and authoritatively confirms durable absence.
    /// Unsupported or ambiguous deletion must return an error.
    ///
    /// # Errors
    /// Rejects unsupported, failed or non-authoritative absence evidence.
    async fn delete_session(&self, cursor: &AiProviderSessionCursor) -> Result<(), ProviderError>;
}

/// A process and idempotent synchronous whole-tree termination callback.
/// Final drop always kills; graceful transport is never the cleanup backstop.
pub struct AiGrokAcpLaunchedProcess {
    process: Arc<dyn AiGrokAcpRunProcess>,
    kill: Box<dyn Fn() + Send + Sync>,
    killed: AtomicBool,
}
impl AiGrokAcpLaunchedProcess {
    /// Wraps a trusted process with its actual OS process-tree kill operation.
    pub fn new(
        process: Arc<dyn AiGrokAcpRunProcess>,
        kill: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            process,
            kill: Box::new(kill),
            killed: AtomicBool::new(false),
        }
    }
    fn terminate(&self) {
        if !self.killed.swap(true, Ordering::AcqRel) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.kill)()));
        }
    }
}
impl Drop for AiGrokAcpLaunchedProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// Trusted OS launcher, separate from provider and application authority.
/// Verify and execute the same digest; clear ambient environment; isolate
/// home/cwd; expose only the explicitly authorized existing managed login;
/// disable API-key fallback, hooks/plugins/instructions/other MCP/native tools,
/// web search, subagents, telemetry payloads and unmetered side-model work.
/// Enforce the registration's aggregate sampler/token ceilings plus process,
/// memory, CPU, output and wall time. Never mutate authentication or billing.
#[async_trait]
pub trait AiGrokAcpProcessFactory: Send + Sync {
    /// True only with reviewed exact-version live evidence for every launch,
    /// budget, tool, resume, cancellation and deletion property above.
    /// Default false keeps incomplete implementations unavailable.
    fn admits(&self, _registration: &AiGrokAcpRegistration) -> bool {
        false
    }
    /// Launches one isolated process without business input. The factory must
    /// kill its complete descendant tree if this future is cancelled.
    ///
    /// # Errors
    /// Rejects unavailable isolation, identity drift or process startup failure.
    async fn launch(
        &self,
        registration: Arc<AiGrokAcpRegistration>,
    ) -> Result<AiGrokAcpLaunchedProcess, ProviderError>;
}

struct Entry {
    _slot: tokio::sync::OwnedSemaphorePermit,
    process: Arc<AiGrokAcpLaunchedProcess>,
    cursor: Mutex<Option<AiProviderSessionCursor>>,
    newly_created: AtomicBool,
    active: AtomicBool,
}
struct TurnGuard {
    entry: Arc<Entry>,
    complete: bool,
}
impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.entry.active.store(false, Ordering::Release);
        if !self.complete {
            self.entry.process.terminate();
        }
    }
}

/// Grok ACP provider using the existing fenced coordinator and session service.
/// No process is admitted until its factory proves the exact closed registration.
pub struct AiGrokAcpProvider {
    registration: Arc<AiGrokAcpRegistration>,
    factory: Arc<dyn AiGrokAcpProcessFactory>,
    entries: Mutex<BTreeMap<AiProviderRunBinding, Arc<Entry>>>,
    launches: Mutex<()>,
    maximum_processes: usize,
    slots: Arc<tokio::sync::Semaphore>,
    turn_timeout: Duration,
    startup_timeout: Duration,
}
impl AiGrokAcpProvider {
    /// Constructs a bounded provider; a non-admitting factory remains disabled.
    ///
    /// # Errors
    /// Rejects zero/over-64 process capacity or turn timeouts over one hour.
    pub fn new(
        registration: Arc<AiGrokAcpRegistration>,
        factory: Arc<dyn AiGrokAcpProcessFactory>,
        maximum_processes: usize,
        turn_timeout: Duration,
    ) -> Result<Self, ProviderError> {
        if !(1..=64).contains(&maximum_processes)
            || turn_timeout.is_zero()
            || turn_timeout > Duration::from_secs(3600)
        {
            return Err(ProviderError::InvalidRequest);
        }
        Ok(Self {
            registration,
            factory,
            entries: Mutex::new(BTreeMap::new()),
            launches: Mutex::new(()),
            maximum_processes,
            slots: Arc::new(tokio::sync::Semaphore::new(maximum_processes)),
            turn_timeout,
            startup_timeout: Duration::from_secs(30),
        })
    }
    /// Whether exact-version launcher and protocol acceptance is proven.
    pub fn is_admitted(&self) -> bool {
        self.factory.admits(&self.registration)
    }
    async fn entry(&self, binding: AiProviderRunBinding) -> Result<Arc<Entry>, ProviderError> {
        if !self.is_admitted() {
            return Err(ProviderError::Unsupported);
        }
        {
            let entries = self.entries.lock().await;
            if let Some(entry) = entries.get(&binding) {
                if entry.process.killed.load(Ordering::Acquire) {
                    return Err(rejected());
                }
                return Ok(entry.clone());
            }
        }
        // Serialize admission without blocking cancellation/close of existing runs.
        let _launch = tokio::time::timeout(self.startup_timeout, self.launches.lock())
            .await
            .map_err(|_| timeout())?;
        let entries = self.entries.lock().await;
        if let Some(entry) = entries.get(&binding) {
            if entry.process.killed.load(Ordering::Acquire) {
                return Err(rejected());
            }
            return Ok(entry.clone());
        }
        if entries.len() >= self.maximum_processes
            || entries
                .keys()
                .filter(|b| b.owner_fingerprint() == binding.owner_fingerprint())
                .count()
                >= 1
        {
            return Err(ProviderError::RateLimited);
        }
        drop(entries);
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| ProviderError::RateLimited)?;
        let process = tokio::time::timeout(
            self.startup_timeout,
            self.factory.launch(self.registration.clone()),
        )
        .await
        .map_err(|_| timeout())??;
        let entry = Arc::new(Entry {
            _slot: slot,
            process: Arc::new(process),
            cursor: Mutex::new(None),
            newly_created: AtomicBool::new(false),
            active: AtomicBool::new(false),
        });
        self.entries.lock().await.insert(binding, entry.clone());
        Ok(entry)
    }
    fn validate_context(
        &self,
        request: &ModelRequest,
        context: &ProviderRequestContext,
    ) -> Result<AiProviderRunBinding, ProviderError> {
        if !self.is_admitted() {
            return Err(ProviderError::Unsupported);
        }
        self.registration.validate_request(request)?;
        context.validate_request(&ProviderKind::LocalHarness, request)?;
        context.validate_input_token_reservation(self.registration.maximum_input_tokens)?;
        context.validate_provider_profile(
            &ProviderKind::LocalHarness,
            request,
            &self.registration.profile_id,
        )?;
        let binding = context.run_binding().ok_or_else(rejected)?;
        let session = context.provider_session().ok_or_else(rejected)?;
        let claim = session.claim();
        if !self.registration.matches(claim.descriptor())
            || session.cursor().kind() != CURSOR_KIND
            || claim.session_id() != binding.session_id()
            || claim.run_id() != binding.run_id()
            || claim.attempt_id() != binding.attempt_id()
            || claim.run_lease_generation() != binding.lease_generation()
            || !binding.matches_principal_reference(&claim.principal_reference)
        {
            return Err(rejected());
        }
        Ok(binding)
    }
    async fn activate(
        &self,
        binding: AiProviderRunBinding,
        session: &AiOpenedProviderSession,
    ) -> Result<TurnGuard, ProviderError> {
        let entry = self.entry(binding).await?;
        if entry.active.swap(true, Ordering::AcqRel) {
            return Err(rejected());
        }
        let guard = TurnGuard {
            entry: entry.clone(),
            complete: false,
        };
        let result = async {
            let mut cursor = entry.cursor.lock().await;
            match session.activation() {
                crate::AiProviderSessionActivation::NewlyBoundEmpty => {
                    if cursor.as_ref() != Some(session.cursor())
                        || !entry.newly_created.swap(false, Ordering::AcqRel)
                    {
                        return Err(rejected());
                    }
                }
                crate::AiProviderSessionActivation::ExistingRetained => {
                    if cursor.is_some() {
                        return Err(rejected());
                    }
                    entry
                        .process
                        .process
                        .resume_session(session.cursor())
                        .await?;
                    *cursor = Some(session.cursor().clone());
                }
            }
            Ok(())
        };
        if let Err(error) = tokio::time::timeout(self.startup_timeout, result)
            .await
            .map_err(|_| timeout())
            .and_then(|r| r)
        {
            entry.process.terminate();
            entry.active.store(false, Ordering::Release);
            return Err(error);
        }
        Ok(guard)
    }
    /// Creates the provider-neutral retained-session maintenance adapter.
    pub fn deletion_service(self: &Arc<Self>, clock: Arc<dyn Clock>) -> AiGrokAcpDeletionService {
        AiGrokAcpDeletionService {
            provider: self.clone(),
            clock,
        }
    }
}

#[async_trait]
impl AiProvider for AiGrokAcpProvider {
    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::LocalHarness
    }
    fn capabilities(&self) -> ProviderCapabilities {
        if !self.is_admitted() {
            return ProviderCapabilities::default();
        }
        ProviderCapabilities {
            capability_delivery_modes: [AiCapabilityDeliveryMode::FixedBroker]
                .into_iter()
                .collect(),
            streaming: true,
            custom_tools: true,
            parallel_tool_calls: true,
            provider_retained_continuation: true,
            local: true,
            maximum_context_tokens: Some(self.registration.maximum_input_tokens),
            maximum_output_tokens: Some(self.registration.maximum_output_tokens),
            reasoning_effort_profiles: vec![self.registration.effort_profile.clone()],
            ..Default::default()
        }
    }
    async fn prepare_dispatch(
        &self,
        request: &ModelRequest,
        context: &ProviderRequestContext,
    ) -> Result<(), ProviderError> {
        self.validate_context(request, context).map(|_| ())
    }
    async fn stream(
        &self,
        _request: ModelRequest,
        _context: ProviderRequestContext,
    ) -> Result<ProviderEventStream, ProviderError> {
        Err(ProviderError::Unsupported)
    }
    async fn stream_with_dynamic_tools(
        &self,
        request: ModelRequest,
        context: ProviderRequestContext,
        responder: Arc<dyn ProviderDynamicToolResponder>,
    ) -> Result<ProviderEventStream, ProviderError> {
        let binding = self.validate_context(&request, &context)?;
        let session = context.provider_session().ok_or_else(rejected)?;
        let guard = self.activate(binding, session).await?;
        let entry = guard.entry.clone();
        let input = request
            .input
            .into_iter()
            .map(|block| match block {
                ModelInputBlock::Text { text } => text,
                ModelInputBlock::Json { value } => value.to_string(),
                _ => unreachable!("validated text only"),
            })
            .collect();
        let deadline = tokio::time::Instant::now() + self.turn_timeout;
        let mut stream =
            tokio::time::timeout_at(deadline, entry.process.process.prompt(input, responder))
                .await
                .map_err(|_| timeout())??;
        let limits = self.registration.clone();
        Ok(Box::pin(async_stream::try_stream! {
            let mut guard=guard;
            let mut started=false; let mut usage=false; let mut completed=false; let mut bytes=0usize;
            while let Some(event)=tokio::time::timeout_at(deadline,stream.next()).await.map_err(|_|timeout())? {
                let event=event?;
                if completed {Err(rejected())?;}
                match &event {
                    ProviderEvent::ResponseStarted{..} if !started=>started=true,
                    ProviderEvent::TextDelta{text} if started&&!usage=>{bytes=bytes.checked_add(text.len()).ok_or_else(rejected)?;if bytes>16*1024*1024{Err(rejected())?;}},
                    ProviderEvent::Usage{input_tokens,output_tokens,cached_input_tokens} if started&&!usage=>{if *input_tokens>limits.maximum_input_tokens||*output_tokens>limits.maximum_output_tokens||cached_input_tokens>input_tokens{Err(rejected())?;}usage=true;},
                    ProviderEvent::ResponseCompleted{..} if usage=>completed=true,
                    _=>Err(rejected())?,
                }
                yield event;
            }
            if !completed {Err(rejected())?;}
            guard.complete=true;
        }))
    }
    async fn interrupt_run(
        &self,
        binding: &AiProviderRunBinding,
    ) -> Result<AiProviderRunInterruptOutcome, ProviderError> {
        let entry = self.entries.lock().await.get(binding).cloned();
        let Some(entry) = entry else {
            return Ok(AiProviderRunInterruptOutcome::NotActive);
        };
        let result =
            tokio::time::timeout(Duration::from_secs(5), entry.process.process.cancel()).await;
        entry.process.terminate();
        result.map_err(|_| timeout())??;
        Ok(AiProviderRunInterruptOutcome::Requested)
    }
    async fn close_run(
        &self,
        binding: &AiProviderRunBinding,
        _reason: AiProviderRunCloseReason,
    ) -> Result<AiProviderRunCloseOutcome, ProviderError> {
        match self.entries.lock().await.remove(binding) {
            Some(entry) => {
                entry.process.terminate();
                Ok(AiProviderRunCloseOutcome::Closed)
            }
            None => Ok(AiProviderRunCloseOutcome::NotActive),
        }
    }
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    async fn create_empty_session(
        &self,
        binding: &AiProviderRunBinding,
        descriptor: &AiProviderSessionDescriptor,
        request: &ModelRequest,
    ) -> Result<AiProviderSessionCursor, ProviderError> {
        self.registration.validate_request(request)?;
        if !self.registration.matches(descriptor) {
            return Err(rejected());
        }
        let entry = self.entry(*binding).await?;
        let mut guard = TurnGuard {
            entry: entry.clone(),
            complete: false,
        };
        let mut cursor = entry.cursor.lock().await;
        if cursor.is_some() {
            return Err(rejected());
        }
        let created = tokio::time::timeout(
            self.startup_timeout,
            entry.process.process.create_empty_session(),
        )
        .await
        .map_err(|_| timeout())
        .and_then(|result| result);
        let created = match created {
            Ok(value) => value,
            Err(error) => {
                entry.process.terminate();
                return Err(error);
            }
        };
        if created.kind() != CURSOR_KIND {
            entry.process.terminate();
            return Err(rejected());
        }
        *cursor = Some(created.clone());
        entry.newly_created.store(true, Ordering::Release);
        guard.complete = true;
        Ok(created)
    }
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    async fn discard_empty_session(
        &self,
        binding: &AiProviderRunBinding,
        descriptor: &AiProviderSessionDescriptor,
        cursor: &AiProviderSessionCursor,
    ) -> Result<(), ProviderError> {
        if !self.registration.matches(descriptor) {
            return Err(rejected());
        }
        let entry = self
            .entries
            .lock()
            .await
            .remove(binding)
            .ok_or_else(rejected)?;
        if entry.cursor.lock().await.as_ref() != Some(cursor)
            || !entry.newly_created.load(Ordering::Acquire)
        {
            entry.process.terminate();
            return Err(rejected());
        }
        let result = tokio::time::timeout(
            self.startup_timeout,
            entry.process.process.delete_session(cursor),
        )
        .await
        .map_err(|_| timeout());
        entry.process.terminate();
        result?
    }
}

/// Detached cleanup using the same reviewed profile and stable cursor namespace.
/// Successful transport alone is not absence; the process contract must prove it.
pub struct AiGrokAcpDeletionService {
    provider: Arc<AiGrokAcpProvider>,
    clock: Arc<dyn Clock>,
}
#[async_trait]
impl crate::AiProviderSessionDeletionService for AiGrokAcpDeletionService {
    async fn delete_or_confirm_absent(
        &self,
        request: &crate::AiProviderSessionDeletionRequest,
    ) -> Result<crate::AiProviderSessionAbsenceProof, ProviderError> {
        let registration = &self.provider.registration;
        let descriptor = request.claim().descriptor();
        if !self.provider.is_admitted()
            || descriptor.provider_kind() != &ProviderKind::LocalHarness
            || descriptor.provider_profile_id() != registration.profile_id
            || request.cursor().kind() != CURSOR_KIND
        {
            return Err(rejected());
        }
        let _slot = self
            .provider
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| ProviderError::RateLimited)?;
        let process = tokio::time::timeout(
            self.provider.startup_timeout,
            self.provider.factory.launch(registration.clone()),
        )
        .await
        .map_err(|_| timeout())??;
        tokio::time::timeout(
            self.provider.startup_timeout,
            process.process.delete_session(request.cursor()),
        )
        .await
        .map_err(|_| timeout())??;
        process.terminate();
        Ok(crate::AiProviderSessionAbsenceProof::for_request(
            request,
            self.clock.now(),
        ))
    }
}

#[cfg(all(test, any(feature = "sqlite", feature = "postgres")))]
pub(super) mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    pub(in crate::providers) fn registration() -> AiGrokAcpRegistration {
        AiGrokAcpRegistration::new(
            "grok-test".into(),
            "grok-4.7".into(),
            "a".repeat(64),
            "1.0.40".into(),
            "reviewed-test-only".into(),
            ModelReasoningEffortProfile::new(
                "grok-4.7",
                [ModelReasoningEffort::Low],
                ModelReasoningEffort::Low,
            )
            .unwrap(),
            ModelReasoningEffort::Low,
            "Use only authorized read capabilities.",
            vec![ModelToolDefinition {
                tool_id: "capabilities.discover".into(),
                provider_name: "discover".into(),
                fingerprint: "c".repeat(64),
                description: "Discover read capabilities".into(),
                parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
                strict: true,
                defer_loading: false,
            }],
            16_384,
            2_048,
            8,
        )
        .unwrap()
        .with_usage_model("grok-4.7-build".into())
        .unwrap()
    }
    struct UnreviewedFactory(AtomicUsize);
    #[async_trait]
    impl AiGrokAcpProcessFactory for UnreviewedFactory {
        async fn launch(
            &self,
            _registration: Arc<AiGrokAcpRegistration>,
        ) -> Result<AiGrokAcpLaunchedProcess, ProviderError> {
            self.0.fetch_add(1, Ordering::AcqRel);
            Err(rejected())
        }
    }
    #[tokio::test]
    async fn unreviewed_factory_is_disabled_before_launch() {
        let factory = Arc::new(UnreviewedFactory(AtomicUsize::new(0)));
        let provider = AiGrokAcpProvider::new(
            Arc::new(registration()),
            factory.clone(),
            2,
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(!provider.is_admitted());
        assert!(!provider.capabilities().custom_tools);
        let binding = AiProviderRunBinding::new(
            crate::AiSessionId::new(),
            crate::AiRunId::new(),
            uuid::Uuid::new_v4(),
            1,
            [0; 32],
        )
        .unwrap();
        assert!(matches!(
            provider.entry(binding).await,
            Err(ProviderError::Unsupported)
        ));
        assert_eq!(factory.0.load(Ordering::Acquire), 0);
    }
    #[test]
    fn registration_fences_alias_effort_and_capability_overlay() {
        let registration = registration();
        assert!(
            registration
                .provider_session_fingerprint(ModelReasoningEffort::High)
                .is_err()
        );
        let binding = AiProviderCapabilitySessionBinding::new(
            AiCapabilityDeliveryMode::FixedBroker,
            "b".repeat(64),
            registration
                .tools
                .iter()
                .map(|t| t.fingerprint.clone())
                .collect(),
            "grok-acp-v1",
            registration.model(),
            registration.reasoning_effort(),
            registration.identity(),
        )
        .unwrap();
        let registered = registration
            .clone()
            .with_capability_binding(binding.clone())
            .unwrap();
        assert_eq!(registered.session_fingerprint(), binding.fingerprint());
        let changed = registered
            .with_usage_model("other-usage-model".into())
            .unwrap();
        assert_ne!(changed.identity(), registration.identity());
        assert_eq!(changed.session_fingerprint(), changed.identity());
        assert!(changed.with_capability_binding(binding).is_err());
    }
    #[test]
    fn request_cannot_widen_native_surface_or_reduce_aggregate_output_ceiling() {
        let reg = registration();
        let mut request = ModelRequest {
            model: reg.model.clone(),
            instructions: vec![],
            input: vec![ModelInputBlock::Text {
                text: "fixture".into(),
            }],
            continuation: None,
            continuation_mode: ModelContinuationMode::ProviderRetained,
            tools: reg.tools.clone(),
            builtin_tools: vec![],
            maximum_builtin_tool_calls: None,
            reasoning_summary: crate::ModelReasoningSummaryRequest::Disabled,
            reasoning_effort: reg.effort,
            output_schema: None,
            maximum_output_tokens: Some(2048),
        };
        assert!(reg.validate_request(&request).is_ok());
        request.maximum_output_tokens = Some(2047);
        assert!(reg.validate_request(&request).is_err());
        request.maximum_output_tokens = Some(2048);
        request.instructions.push("untrusted override".into());
        assert!(reg.validate_request(&request).is_err());
        request.instructions.clear();
        request.tools[0].fingerprint = "substitute".into();
        assert!(reg.validate_request(&request).is_err());
    }
}
