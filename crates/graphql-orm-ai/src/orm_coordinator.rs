//! Fenced top-level coordination for bounded read-only provider/tool loops.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use std::sync::Arc;

use async_trait::async_trait;
use time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    AiAgentContinuation, AiAgentLoopGuard, AiAgentLoopLimits, AiAgentLoopTurn,
    AiAgentRuleResolution, AiApplicationToolCallContext, AiError, AiPersistedApplicationToolCall,
    AiPersistedProviderOutput, AiProviderCallExecutor, AiProviderCallPlan, AiProviderCallResult,
    AiProviderDynamicToolExecution, AiReadOnlyAgentRunOutcome::*, AiResolvedRuleSet,
    AiRuleRunUsage, AiRunCompletion, AiRunLease, AiRunState, AiScope, AiToolResultEgressRoute,
    OrmAiApplicationToolCallService, OrmAiProviderOutputService, OrmAiRunService,
};

/// Deployment-owned bounds for a top-level read-only agent attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiReadOnlyAgentCoordinatorLimits {
    loop_limits: AiAgentLoopLimits,
    heartbeat_interval: Duration,
}

impl AiReadOnlyAgentCoordinatorLimits {
    /// Creates coordinator limits.
    ///
    /// The heartbeat interval must also be comfortably shorter than the lease
    /// TTL configured on the concrete run service. Keeping that relationship
    /// deployment-owned avoids silently changing durable lease policy here.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] unless the heartbeat interval
    /// is positive and no longer than five minutes.
    pub fn new(
        loop_limits: AiAgentLoopLimits,
        heartbeat_interval: Duration,
    ) -> Result<Self, AiError> {
        if !heartbeat_interval.is_positive() || heartbeat_interval > Duration::minutes(5) {
            return Err(AiError::InvalidConfiguration(
                "invalid agent coordinator heartbeat interval".to_owned(),
            ));
        }
        Ok(Self {
            loop_limits,
            heartbeat_interval,
        })
    }
}

/// One host-planned provider turn for the read-only coordinator.
///
/// Construction proves either a truly tool-free initial chat request or custom
/// application tools with one structurally valid result-egress route, plus one
/// exact resolved-rule set. It does not authorize discovery, resolver
/// execution, provider egress, BYOK use, or spend; those decisions remain fresh
/// per turn/call.
pub struct AiReadOnlyAgentTurnPlan {
    provider_call: AiProviderCallPlan,
    mode: AiReadOnlyAgentTurnMode,
    rules: AiResolvedRuleSet,
    uses_byok: bool,
    provider_session: Option<crate::AiProviderSessionTurnPlan>,
    capability_delivery: Option<crate::AiCapabilityDeliveryTurn>,
}

enum AiReadOnlyAgentTurnMode {
    ChatOnly,
    ApplicationTools(AiToolResultEgressRoute),
    ExperimentalDynamicTools(AiToolResultEgressRoute),
}

impl AiReadOnlyAgentTurnPlan {
    /// Binds a provider plan to current rule evidence and the server-selected
    /// route used for every exact application-tool result in that turn.
    ///
    /// `uses_byok` must be derived by the trusted planner from the selected
    /// credential profile. It is checked as a negative rule constraint and is
    /// not itself credential or provider authority.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] if the provider plan has no validated
    /// application tools, its scope differs from the resolved-rule target, or
    /// the route is malformed.
    pub fn new(
        provider_call: AiProviderCallPlan,
        result_egress_route: AiToolResultEgressRoute,
        rules: AiResolvedRuleSet,
        uses_byok: bool,
    ) -> Result<Self, AiError> {
        if !provider_call.has_application_tools() || provider_call.scope() != rules.target_scope() {
            return Err(AiError::InvalidInput(
                "agent turn plan has no tools or exact rule binding".to_owned(),
            ));
        }
        result_egress_route.validate()?;
        Ok(Self {
            provider_call,
            mode: AiReadOnlyAgentTurnMode::ApplicationTools(result_egress_route),
            rules,
            uses_byok,
            provider_session: None,
            capability_delivery: None,
        })
    }

    /// Binds a truly tool-free initial provider call to current rule evidence.
    ///
    /// This mode has no application-tool result route, checkpoint, or
    /// continuation path. `uses_byok` must be derived by the trusted planner
    /// from the selected credential profile and remains only a negative rule
    /// constraint, not provider or credential authority.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] unless the provider plan is an initial
    /// request with no application tools, provider-built-in tools, continuation,
    /// or tool-result input and its scope exactly matches the resolved-rule
    /// target.
    pub fn new_chat(
        provider_call: AiProviderCallPlan,
        rules: AiResolvedRuleSet,
        uses_byok: bool,
    ) -> Result<Self, AiError> {
        if !provider_call.is_tool_free_initial() || provider_call.scope() != rules.target_scope() {
            return Err(AiError::InvalidInput(
                "chat turn plan is not tool-free or exactly rule-bound".to_owned(),
            ));
        }
        Ok(Self {
            provider_call,
            mode: AiReadOnlyAgentTurnMode::ChatOnly,
            rules,
            uses_byok,
            provider_session: None,
            capability_delivery: None,
        })
    }

    /// Binds an initial provider-retained turn to the experimental
    /// coordinator-owned in-flight application-tool bridge.
    ///
    /// This mode is explicit and does not change [`Self::new`]. The selected
    /// provider must independently advertise and enable its reviewed dynamic
    /// tool protocol. The provider receives definitions and disclosure-safe
    /// results only; all execution remains under the ordinary coordinator.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] unless the plan contains at least one
    /// application tool, is an initial provider-retained request without
    /// attachments, result input, reasoning summaries, or an output schema,
    /// is exactly rule-bound, and has a valid result-egress route. Validated
    /// provider built-ins may coexist with the application tools; their
    /// capability, egress, budget, and usage proofs remain independently
    /// enforced by the provider-call plan and executor.
    pub fn new_experimental_dynamic_tools(
        provider_call: AiProviderCallPlan,
        result_egress_route: AiToolResultEgressRoute,
        rules: AiResolvedRuleSet,
        uses_byok: bool,
    ) -> Result<Self, AiError> {
        if !provider_call.is_dynamic_tool_initial() || provider_call.scope() != rules.target_scope()
        {
            return Err(AiError::InvalidInput(
                "experimental dynamic-tool plan is not a closed initial request or exactly rule-bound"
                    .to_owned(),
            ));
        }
        result_egress_route.validate()?;
        Ok(Self {
            provider_call,
            mode: AiReadOnlyAgentTurnMode::ExperimentalDynamicTools(result_egress_route),
            rules,
            uses_byok,
            provider_session: None,
            capability_delivery: None,
        })
    }

    /// Enables exact durable provider-session claim/create/resume for this
    /// turn.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] unless the session descriptor exactly
    /// matches a provider-retained chat-only or experimental dynamic-tool
    /// provider call.
    pub fn with_provider_session(
        mut self,
        session: crate::AiProviderSessionTurnPlan,
    ) -> Result<Self, AiError> {
        if !self
            .provider_call
            .matches_provider_session_descriptor(session.descriptor())
            || !self.provider_call.uses_provider_retained_continuation()
            || !matches!(
                &self.mode,
                AiReadOnlyAgentTurnMode::ChatOnly
                    | AiReadOnlyAgentTurnMode::ExperimentalDynamicTools(_)
            )
            || self.capability_delivery.as_ref().is_some_and(|delivery| {
                session.descriptor().registration_fingerprint()
                    != delivery.session_binding().fingerprint()
            })
        {
            return Err(AiError::InvalidInput(
                "provider-session plan does not match the exact provider call".to_owned(),
            ));
        }
        self.provider_session = Some(session);
        Ok(self)
    }

    /// Binds this turn to one crate-owned capability delivery surface.
    ///
    /// The coordinator refuses the turn unless the offered definitions are
    /// exactly the definitions the crate minted for the current delivery mode
    /// and compact index, so a host installs the surface and never authors it.
    /// Broker calls in the resulting turn are dispatched through the ordinary
    /// durable application-tool broker.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] unless the plan offers exactly the
    /// crate-owned surface and carries an application-tool result route.
    pub fn with_capability_delivery(
        mut self,
        delivery: crate::AiCapabilityDeliveryTurn,
    ) -> Result<Self, AiError> {
        if !delivery.matches_offered_tools(self.provider_call.offered_tools())
            || matches!(&self.mode, AiReadOnlyAgentTurnMode::ChatOnly)
            || self.provider_session.as_ref().is_some_and(|session| {
                session.descriptor().registration_fingerprint()
                    != delivery.session_binding().fingerprint()
            })
        {
            return Err(AiError::InvalidInput(
                "capability delivery surface does not match the exact provider call".to_owned(),
            ));
        }
        self.capability_delivery = Some(delivery);
        Ok(self)
    }

    fn into_parts(
        self,
    ) -> (
        AiProviderCallPlan,
        AiScope,
        String,
        AiReadOnlyAgentTurnMode,
        AiResolvedRuleSet,
        bool,
        Option<crate::AiProviderSessionTurnPlan>,
        Option<crate::AiCapabilityDeliveryTurn>,
    ) {
        let scope = self.provider_call.scope().clone();
        let correlation_id = self.provider_call.correlation_id().to_owned();
        (
            self.provider_call,
            scope,
            correlation_id,
            self.mode,
            self.rules,
            self.uses_byok,
            self.provider_session,
            self.capability_delivery,
        )
    }

    fn is_continuation(&self) -> bool {
        self.provider_call.is_continuation()
    }

    fn rule_fingerprint(&self) -> &str {
        self.rules.fingerprint()
    }
}

/// Host-owned construction of exact initial and continuation provider plans.
///
/// Implementations select configuration, provider profile, model, context,
/// enabled tool definitions, current hierarchical-rule evidence, fresh
/// atomic-budget estimates, and exact egress manifests. Model output must never
/// select these values. Continuations must be consumed through
/// [`AiProviderCallPlan::new_continuation_with_tools`].
#[async_trait]
pub trait AiReadOnlyAgentTurnPlanner: Send + Sync {
    /// Builds the first turn for a newly running fenced attempt.
    ///
    /// # Errors
    ///
    /// Returns a safe library error when current configuration, context,
    /// budget estimates, or egress manifests cannot produce an exact plan.
    async fn initial_plan(&self, lease: &AiRunLease) -> Result<AiReadOnlyAgentTurnPlan, AiError>;

    /// Builds the next provider turn from the exact completed tool batch.
    ///
    /// `provider_turns` is the number of provider results already accepted by
    /// the bounded guard. The opaque continuation binds every call ID, result,
    /// transfer manifest, and prior provider response as one unit.
    ///
    /// # Errors
    ///
    /// Returns a safe library error when fresh configuration or proofs cannot
    /// produce an exactly chained continuation plan.
    async fn continuation_plan(
        &self,
        lease: &AiRunLease,
        provider_turns: u32,
        continuation: AiAgentContinuation,
    ) -> Result<AiReadOnlyAgentTurnPlan, AiError>;

    /// Builds the next provider turn with the crate-owned capability delivery
    /// state that survived the preceding turn.
    ///
    /// Implementations adopting deferred capability delivery should override
    /// this method, call [`crate::AiCapabilityDeliveryTurn::current_surface`]
    /// after any client-deferred installation, and attach a clone of the same
    /// delivery turn to the returned plan. The default preserves existing
    /// planners that do not use capability delivery.
    ///
    /// # Errors
    ///
    /// Returns a safe library error when current configuration, context,
    /// budget estimates, egress manifests, or the updated exact capability
    /// surface cannot produce a continuation plan.
    async fn continuation_plan_with_capability_delivery(
        &self,
        lease: &AiRunLease,
        provider_turns: u32,
        continuation: AiAgentContinuation,
        capability_delivery: Option<&crate::AiCapabilityDeliveryTurn>,
    ) -> Result<AiReadOnlyAgentTurnPlan, AiError> {
        let _ = capability_delivery;
        self.continuation_plan(lease, provider_turns, continuation)
            .await
    }
}

/// Fresh current-principal hierarchical-rule resolution for one run boundary.
///
/// Implementations must rehydrate the lease principal and resolve the complete
/// host-authored hierarchy. A result narrows planning only and never grants
/// provider, tool, resolver, egress, budget, or approval authority.
#[async_trait]
pub trait AiAgentRuleResolver: Send + Sync {
    /// Resolves current rules for the exact fenced lease and target scope.
    ///
    /// # Errors
    ///
    /// Returns a safe error for stale principal state, scope mismatch,
    /// incomplete/unauthorized hierarchy, or persistence ambiguity.
    async fn resolve_rules(
        &self,
        lease: &AiRunLease,
        scope: &AiScope,
    ) -> Result<AiAgentRuleResolution, AiError>;
}

/// Minimal fenced run operations required by the coordinator.
///
/// Alternative implementations must preserve the same attempt, generation,
/// expiry, and row-version checks as [`OrmAiRunService`]. This seam is intended
/// for conformance testing and deployment wrapping, not weaker persistence.
#[async_trait]
pub trait AiAgentRunControl: Send + Sync {
    /// Moves a freshly claimed lease to `Running` and renews its proof.
    ///
    /// # Errors
    ///
    /// Fails closed when the claim is expired, stale, malformed, or cannot be
    /// persisted.
    async fn start(&self, lease: &AiRunLease) -> Result<AiRunLease, AiError>;

    /// Renews one current running proof.
    ///
    /// # Errors
    ///
    /// Fails closed when the fence is expired, stale, malformed, or cannot be
    /// persisted.
    async fn heartbeat(&self, lease: &AiRunLease) -> Result<AiRunLease, AiError>;

    /// Reads an already-terminal owner cancellation for the exact attempt.
    async fn cancellation(
        &self,
        _lease: &AiRunLease,
    ) -> Result<Option<crate::AiRunCancellation>, AiError> {
        Ok(None)
    }

    /// Waits up to a bounded interval for cancellation. Implementations may
    /// accelerate this with process-local notifications, but durable state is
    /// authoritative.
    async fn wait_for_cancellation(
        &self,
        lease: &AiRunLease,
        maximum_wait: std::time::Duration,
    ) -> Result<Option<crate::AiRunCancellation>, AiError> {
        tokio::time::sleep(maximum_wait).await;
        self.cancellation(lease).await
    }

    /// Commits one exact terminal/recovery outcome.
    ///
    /// # Errors
    ///
    /// Fails closed when the fence/outcome is stale, invalid, duplicate, or
    /// cannot be persisted.
    async fn finish(&self, lease: &AiRunLease, completion: AiRunCompletion) -> Result<(), AiError>;

    /// Relinquishes the current fence and schedules a bounded retry.
    ///
    /// The default denies so alternate run-control implementations do not
    /// acquire retry semantics implicitly.
    ///
    /// # Errors
    ///
    /// Fails closed when the delay/code is invalid, the retry ceiling is
    /// exhausted, or the fence cannot be persisted.
    async fn schedule_retry(
        &self,
        _lease: &AiRunLease,
        _delay: time::Duration,
        _error_code: &str,
    ) -> Result<(), AiError> {
        Err(AiError::RuntimeNotReady)
    }
}

#[async_trait]
impl AiAgentRunControl for OrmAiRunService {
    async fn start(&self, lease: &AiRunLease) -> Result<AiRunLease, AiError> {
        OrmAiRunService::start(self, lease).await
    }

    async fn heartbeat(&self, lease: &AiRunLease) -> Result<AiRunLease, AiError> {
        OrmAiRunService::heartbeat(self, lease).await
    }

    async fn cancellation(
        &self,
        lease: &AiRunLease,
    ) -> Result<Option<crate::AiRunCancellation>, AiError> {
        OrmAiRunService::cancellation(self, lease).await
    }

    async fn wait_for_cancellation(
        &self,
        lease: &AiRunLease,
        maximum_wait: std::time::Duration,
    ) -> Result<Option<crate::AiRunCancellation>, AiError> {
        OrmAiRunService::wait_for_cancellation(self, lease, maximum_wait).await
    }

    async fn finish(&self, lease: &AiRunLease, completion: AiRunCompletion) -> Result<(), AiError> {
        OrmAiRunService::finish(self, lease, completion).await
    }

    async fn schedule_retry(
        &self,
        lease: &AiRunLease,
        delay: time::Duration,
        error_code: &str,
    ) -> Result<(), AiError> {
        OrmAiRunService::schedule_retry(self, lease, delay, error_code).await
    }
}

/// One-turn provider execution boundary used by the coordinator.
#[async_trait]
pub trait AiAgentProviderTurnExecutor: Send + Sync {
    /// Executes one exactly planned turn for the current attempt/generation.
    ///
    /// Returning [`AiError::PreTransportBudgetDenied`] is a proof-bearing
    /// contract: the denial must have occurred before provider transport, and
    /// no budget reservation may remain held. Returning
    /// [`AiError::StatelessNativeItemRejected`] is also proof-bearing: the
    /// completed StatelessReplay turn's authoritative usage must already be
    /// committed, no answer or admitted tool effect may exist, and the adapter
    /// must prove the refused native item was contained. Every other error
    /// after transport might have occurred must be [`AiError::ProviderFailed`]
    /// so the coordinator preserves uncertainty.
    ///
    /// # Errors
    ///
    /// Returns a safe library error for any authorization, budget, egress,
    /// provider, normalization, usage, fence, or persistence failure.
    async fn execute_turn(
        &self,
        lease: &AiRunLease,
        plan: AiProviderCallPlan,
    ) -> Result<AiProviderCallResult, AiError>;

    /// Executes one explicitly planned experimental in-flight dynamic-tool
    /// turn using a lease shared with coordinator heartbeats.
    ///
    /// The default remains unavailable so existing executors and providers do
    /// not gain this capability implicitly.
    async fn execute_dynamic_turn(
        &self,
        _lease: Arc<Mutex<AiRunLease>>,
        _plan: AiProviderCallPlan,
        _execution: Arc<dyn AiProviderDynamicToolExecution>,
    ) -> Result<AiProviderCallResult, AiError> {
        Err(AiError::RuntimeNotReady)
    }

    /// Executes one turn through an exact durable provider-session binding.
    async fn execute_retained_turn(
        &self,
        _lease: Arc<Mutex<AiRunLease>>,
        _plan: AiProviderCallPlan,
        _session_plan: crate::AiProviderSessionTurnPlan,
        _session_service: Arc<dyn crate::AiProviderSessionService>,
        _execution: Option<Arc<dyn AiProviderDynamicToolExecution>>,
    ) -> Result<AiProviderCallResult, AiError> {
        Err(AiError::RuntimeNotReady)
    }

    /// Interrupts an active run-scoped provider resource after durable
    /// cancellation or lease loss has already been observed.
    ///
    /// The returned settlement reports what the interruption proved. A default
    /// executor proves nothing, so it reports no live resource rather than
    /// implying the retained thread may be kept.
    async fn interrupt_run(
        &self,
        _lease: &AiRunLease,
    ) -> Result<crate::AiRunInterruptSettlement, AiError> {
        Ok(crate::AiRunInterruptSettlement::NotActive)
    }

    /// Closes all provider resources belonging to one exact run fence.
    async fn close_run(
        &self,
        _lease: &AiRunLease,
        _reason: crate::AiProviderRunCloseReason,
    ) -> Result<(), AiError> {
        Ok(())
    }
}

#[async_trait]
impl AiAgentProviderTurnExecutor for AiProviderCallExecutor {
    async fn execute_turn(
        &self,
        lease: &AiRunLease,
        plan: AiProviderCallPlan,
    ) -> Result<AiProviderCallResult, AiError> {
        self.execute(lease, plan).await
    }

    async fn execute_dynamic_turn(
        &self,
        lease: Arc<Mutex<AiRunLease>>,
        plan: AiProviderCallPlan,
        execution: Arc<dyn AiProviderDynamicToolExecution>,
    ) -> Result<AiProviderCallResult, AiError> {
        self.execute_with_dynamic_tools(lease, plan, execution)
            .await
    }

    async fn execute_retained_turn(
        &self,
        lease: Arc<Mutex<AiRunLease>>,
        plan: AiProviderCallPlan,
        session_plan: crate::AiProviderSessionTurnPlan,
        session_service: Arc<dyn crate::AiProviderSessionService>,
        execution: Option<Arc<dyn AiProviderDynamicToolExecution>>,
    ) -> Result<AiProviderCallResult, AiError> {
        self.execute_with_provider_session(lease, plan, session_plan, session_service, execution)
            .await
    }

    async fn interrupt_run(
        &self,
        lease: &AiRunLease,
    ) -> Result<crate::AiRunInterruptSettlement, AiError> {
        AiProviderCallExecutor::interrupt_run(self, lease).await
    }

    async fn close_run(
        &self,
        lease: &AiRunLease,
        reason: crate::AiProviderRunCloseReason,
    ) -> Result<(), AiError> {
        AiProviderCallExecutor::close_run(self, lease, reason).await
    }
}

/// Protected read-only application-tool execution boundary.
#[async_trait]
pub trait AiAgentReadOnlyToolExecutor: Send + Sync {
    /// Executes and durably resolves one exact provider-requested query.
    ///
    /// # Errors
    ///
    /// Returns a safe library error for stale binding/fence, authorization,
    /// disclosure, protection, execution, egress, or persistence failure.
    async fn execute_tool(
        &self,
        lease: &AiRunLease,
        provider_result: &AiProviderCallResult,
        context: AiApplicationToolCallContext,
        route: AiToolResultEgressRoute,
    ) -> Result<AiPersistedApplicationToolCall, AiError>;

    /// Persists one deterministic safe rejection through the ordinary durable
    /// tool broker.
    async fn persist_safe_failure(
        &self,
        _lease: &AiRunLease,
        _provider_result: &AiProviderCallResult,
        _context: AiApplicationToolCallContext,
        _route: AiToolResultEgressRoute,
        _code: crate::AiApplicationToolFailureCode,
    ) -> Result<AiPersistedApplicationToolCall, AiError> {
        Err(AiError::InvalidConfiguration(
            "durable safe tool failures are not implemented by this executor".to_owned(),
        ))
    }

    /// Executes one frozen capability-broker call through the ordinary durable
    /// tool broker.
    ///
    /// Discovery and describe return bounded authority-neutral metadata; only
    /// execute reaches a resolver, through the exact loaded binding. The
    /// default remains unavailable so an existing executor does not gain the
    /// broker implicitly.
    ///
    /// # Errors
    ///
    /// Returns a safe library error for a stale binding/fence, a non-broker or
    /// unoffered tool, authorization, disclosure, protection, execution,
    /// egress, or persistence failure.
    async fn execute_capability_broker(
        &self,
        _lease: &AiRunLease,
        _provider_result: &AiProviderCallResult,
        _context: AiApplicationToolCallContext,
        _route: AiToolResultEgressRoute,
        _delivery: &crate::AiCapabilityDeliveryTurn,
    ) -> Result<AiPersistedApplicationToolCall, AiError> {
        Err(AiError::InvalidConfiguration(
            "capability broker dispatch is not implemented by this executor".to_owned(),
        ))
    }

    /// Projects the exact registered definitions for the capabilities a
    /// client-deferred broker run has already loaded.
    ///
    /// # Errors
    ///
    /// Returns a safe library error when a loaded capability is no longer a
    /// registered generated read capability.
    async fn loaded_capability_definitions(
        &self,
        _lease: &AiRunLease,
        _provider_kind: &crate::ProviderKind,
        _delivery: &crate::AiCapabilityDeliveryTurn,
    ) -> Result<Vec<crate::ModelToolDefinition>, AiError> {
        Err(AiError::InvalidConfiguration(
            "deferred capability projection is not implemented by this executor".to_owned(),
        ))
    }
}

#[async_trait]
impl AiAgentReadOnlyToolExecutor for OrmAiApplicationToolCallService {
    async fn execute_tool(
        &self,
        lease: &AiRunLease,
        provider_result: &AiProviderCallResult,
        context: AiApplicationToolCallContext,
        route: AiToolResultEgressRoute,
    ) -> Result<AiPersistedApplicationToolCall, AiError> {
        self.execute_read_only(lease, provider_result, context, route)
            .await
    }

    async fn persist_safe_failure(
        &self,
        lease: &AiRunLease,
        provider_result: &AiProviderCallResult,
        context: AiApplicationToolCallContext,
        route: AiToolResultEgressRoute,
        code: crate::AiApplicationToolFailureCode,
    ) -> Result<AiPersistedApplicationToolCall, AiError> {
        self.persist_safe_read_failure(lease, provider_result, context, route, code)
            .await
    }

    async fn execute_capability_broker(
        &self,
        lease: &AiRunLease,
        provider_result: &AiProviderCallResult,
        context: AiApplicationToolCallContext,
        route: AiToolResultEgressRoute,
        delivery: &crate::AiCapabilityDeliveryTurn,
    ) -> Result<AiPersistedApplicationToolCall, AiError> {
        self.execute_capability_broker_call(lease, provider_result, context, route, delivery)
            .await
    }

    async fn loaded_capability_definitions(
        &self,
        lease: &AiRunLease,
        provider_kind: &crate::ProviderKind,
        delivery: &crate::AiCapabilityDeliveryTurn,
    ) -> Result<Vec<crate::ModelToolDefinition>, AiError> {
        OrmAiApplicationToolCallService::loaded_capability_definitions(
            self,
            lease,
            provider_kind,
            delivery,
        )
        .await
    }
}

struct DynamicToolExecutionState {
    rule_usage: AiRuleRunUsage,
    accepted_calls: u32,
}

struct ReadOnlyDynamicToolExecution {
    run_control: Arc<dyn AiAgentRunControl>,
    tool_executor: Arc<dyn AiAgentReadOnlyToolExecutor>,
    rule_resolver: Arc<dyn AiAgentRuleResolver>,
    scope: AiScope,
    correlation_id: String,
    route: AiToolResultEgressRoute,
    rule_fingerprint: String,
    provider_turn_index: u32,
    maximum_calls: u32,
    capability_delivery: Option<crate::AiCapabilityDeliveryTurn>,
    state: Mutex<DynamicToolExecutionState>,
}

impl ReadOnlyDynamicToolExecution {
    #[allow(clippy::too_many_arguments)]
    fn new(
        run_control: Arc<dyn AiAgentRunControl>,
        tool_executor: Arc<dyn AiAgentReadOnlyToolExecutor>,
        rule_resolver: Arc<dyn AiAgentRuleResolver>,
        scope: AiScope,
        correlation_id: String,
        route: AiToolResultEgressRoute,
        rule_fingerprint: String,
        provider_turn_index: u32,
        maximum_calls: u32,
        rule_usage: AiRuleRunUsage,
        capability_delivery: Option<crate::AiCapabilityDeliveryTurn>,
    ) -> Self {
        Self {
            run_control,
            tool_executor,
            rule_resolver,
            scope,
            correlation_id,
            route,
            rule_fingerprint,
            provider_turn_index,
            maximum_calls,
            capability_delivery,
            state: Mutex::new(DynamicToolExecutionState {
                rule_usage,
                accepted_calls: 0,
            }),
        }
    }

    async fn rule_usage(&self) -> AiRuleRunUsage {
        self.state.lock().await.rule_usage
    }
}

#[async_trait]
impl AiProviderDynamicToolExecution for ReadOnlyDynamicToolExecution {
    async fn execute_dynamic_tool(
        &self,
        lease: &AiRunLease,
        provider_result: &AiProviderCallResult,
        tool_call_index: usize,
    ) -> Result<AiPersistedApplicationToolCall, AiError> {
        if self.run_control.cancellation(lease).await?.is_some() {
            return Err(AiError::Conflict);
        }
        let current_rules = self.rule_resolver.resolve_rules(lease, &self.scope).await?;
        let call = provider_result
            .tool_calls()
            .get(tool_call_index)
            .ok_or(AiError::Conflict)?;
        if current_rules.rules().fingerprint() != self.rule_fingerprint
            || current_rules.rules().constrain_tool(
                call.tool_fingerprint(),
                crate::ToolMaturity::ReadOnly,
                crate::AiApprovalRule::None,
            ) != Some(crate::AiApprovalRule::None)
        {
            return Err(AiError::Forbidden);
        }
        {
            let mut state = self.state.lock().await;
            state.accepted_calls = state
                .accepted_calls
                .checked_add(1)
                .filter(|calls| *calls <= self.maximum_calls)
                .ok_or(AiError::BudgetDenied)?;
            state.rule_usage = state.rule_usage.accept_tool_calls(1, &current_rules)?;
        }
        let context = AiApplicationToolCallContext::new(
            self.provider_turn_index,
            tool_call_index,
            self.scope.clone(),
            self.correlation_id.clone(),
            provider_result.budget_reservation_id().0.to_string(),
        )?;
        let persisted = match (
            crate::AiCapabilityBrokerOperation::from_tool_id(call.tool_id()),
            self.capability_delivery.as_ref(),
        ) {
            (None, _) => {
                self.tool_executor
                    .execute_tool(lease, provider_result, context, self.route.clone())
                    .await?
            }
            (Some(_), Some(delivery)) => {
                self.tool_executor
                    .execute_capability_broker(
                        lease,
                        provider_result,
                        context,
                        self.route.clone(),
                        delivery,
                    )
                    .await?
            }
            (Some(_), None) => return Err(AiError::Forbidden),
        };
        if self
            .run_control
            .cancellation(persisted.lease())
            .await?
            .is_some()
        {
            return Err(AiError::Conflict);
        }
        Ok(persisted)
    }

    async fn persist_dynamic_failure(
        &self,
        lease: &AiRunLease,
        provider_result: &AiProviderCallResult,
        tool_call_index: usize,
        code: crate::AiApplicationToolFailureCode,
    ) -> Result<AiPersistedApplicationToolCall, AiError> {
        let context = AiApplicationToolCallContext::new(
            self.provider_turn_index,
            tool_call_index,
            self.scope.clone(),
            self.correlation_id.clone(),
            provider_result.budget_reservation_id().0.to_string(),
        )?;
        self.tool_executor
            .persist_safe_failure(lease, provider_result, context, self.route.clone(), code)
            .await
    }
}

/// Protected terminal assistant-output persistence boundary.
#[async_trait]
pub trait AiAgentProviderOutputWriter: Send + Sync {
    /// Persists one tool-free successful final provider result.
    ///
    /// # Errors
    ///
    /// Returns a safe library error for stale binding/fence, current-access or
    /// protection denial, malformed output, or persistence failure.
    async fn persist_output(
        &self,
        lease: &AiRunLease,
        result: &AiProviderCallResult,
    ) -> Result<AiPersistedProviderOutput, AiError>;
}

/// Protected fenced checkpoint persistence for replay-safe coordinator phases.
///
/// A successful write proves only that an exact provider result or completed
/// read-only tool batch, current rule fingerprint, and cumulative rule-budget
/// counters are durably protected under the current attempt fence. It does not
/// itself authorize generation adoption or external replay.
#[async_trait]
pub trait AiAgentCheckpointWriter: Send + Sync {
    /// Persists one normalized provider turn before output or tools consume it.
    ///
    /// # Errors
    ///
    /// Returns a safe error for stale fencing, unsettled budget usage, current
    /// access/protection denial, malformed state, or persistence failure.
    #[allow(clippy::too_many_arguments)]
    async fn persist_provider_turn(
        &self,
        lease: &AiRunLease,
        result: &AiProviderCallResult,
        scope: &AiScope,
        correlation_id: &str,
        route: &AiToolResultEgressRoute,
        rules: &AiResolvedRuleSet,
        rule_usage: AiRuleRunUsage,
        provider_turns: u32,
        total_tool_calls: u32,
    ) -> Result<AiRunLease, AiError>;

    /// Persists one exact complete model-visible read-only tool batch.
    ///
    /// # Errors
    ///
    /// Returns a safe error unless every result is protected, separately
    /// egress-authorized, durably complete, and bound to the current fence and
    /// preceding provider response or exact stateless conversation chain.
    #[allow(clippy::too_many_arguments)]
    async fn persist_tool_batch(
        &self,
        lease: &AiRunLease,
        result: &AiProviderCallResult,
        completed_tools: &[AiPersistedApplicationToolCall],
        continuation: &AiAgentContinuation,
        scope: &AiScope,
        correlation_id: &str,
        route: &AiToolResultEgressRoute,
        rules: &AiResolvedRuleSet,
        rule_usage: AiRuleRunUsage,
        provider_turns: u32,
        total_tool_calls: u32,
    ) -> Result<AiRunLease, AiError>;

    /// Persists one exact completed automatic mutation before any provider
    /// continuation can observe its result.
    ///
    /// # Errors
    ///
    /// Returns a safe error unless the exact non-idempotent result is already
    /// durably complete, protected, egress-authorized, and bound to the
    /// current provider response, lease, rules, and target authority.
    #[allow(clippy::too_many_arguments)]
    async fn persist_automatic_mutation_batch(
        &self,
        _lease: &AiRunLease,
        _result: &AiProviderCallResult,
        _completed_tool: &AiPersistedApplicationToolCall,
        _continuation: &AiAgentContinuation,
        _scope: &AiScope,
        _correlation_id: &str,
        _route: &AiToolResultEgressRoute,
        _rules: &AiResolvedRuleSet,
        _rule_usage: AiRuleRunUsage,
        _provider_turns: u32,
        _total_tool_calls: u32,
    ) -> Result<AiRunLease, AiError> {
        Err(AiError::Conflict)
    }
}

/// Protected state recovered from one exact completed read-only tool batch.
///
/// Fields are private so a host cannot manufacture resume counters, response
/// chaining, rule usage, or model-visible tool results. Adoption applies only
/// to exact completed provider-retained or bounded stateless read-only tool
/// batches. It proves the prior batch's durable integrity under current
/// access, rule, and protection policy; it does not authorize the next
/// provider request, budget, or egress decision.
#[derive(Clone, Debug)]
pub struct AiAdoptedReadOnlyToolBatch {
    checkpoint_id: Uuid,
    provider_turns: u32,
    total_tool_calls: u32,
    scope: AiScope,
    continuation: AiAgentContinuation,
    rule_fingerprint: String,
    rule_usage: AiRuleRunUsage,
}

impl AiAdoptedReadOnlyToolBatch {
    pub(crate) fn new(
        checkpoint_id: Uuid,
        provider_turns: u32,
        total_tool_calls: u32,
        scope: AiScope,
        continuation: AiAgentContinuation,
        rule_fingerprint: String,
        rule_usage: AiRuleRunUsage,
    ) -> Self {
        Self {
            checkpoint_id,
            provider_turns,
            total_tool_calls,
            scope,
            continuation,
            rule_fingerprint,
            rule_usage,
        }
    }

    /// Immutable checkpoint selected for one-shot adoption.
    pub const fn checkpoint_id(&self) -> Uuid {
        self.checkpoint_id
    }

    /// Number of accepted provider turns preceding the adopted batch.
    pub const fn provider_turns(&self) -> u32 {
        self.provider_turns
    }

    /// Number of resolved application-tool calls preceding the adopted batch.
    pub const fn total_tool_calls(&self) -> u32 {
        self.total_tool_calls
    }

    /// Application-defined scope reauthorized during adoption.
    pub fn scope(&self) -> &AiScope {
        &self.scope
    }

    /// Exact current hierarchical-rule fingerprint revalidated at adoption.
    pub fn rule_fingerprint(&self) -> &str {
        &self.rule_fingerprint
    }

    /// Cumulative authoritative rule-budget usage through this checkpoint.
    pub const fn rule_usage(&self) -> AiRuleRunUsage {
        self.rule_usage
    }

    pub(crate) fn continuation(&self) -> &AiAgentContinuation {
        &self.continuation
    }
}

/// Current-authority proof for one completed automatic mutation result.
///
/// This is deliberately distinct from a read-only tool-batch proof. It binds
/// one exact non-idempotent result that was durably checkpointed after the
/// effect, and permits only one checkpoint consumption before continuation.
/// It never authorizes executing or replaying the mutation.
#[derive(Clone, Debug)]
pub struct AiAdoptedAutomaticMutationBatch {
    inner: AiAdoptedReadOnlyToolBatch,
}

impl AiAdoptedAutomaticMutationBatch {
    pub(crate) const fn new(inner: AiAdoptedReadOnlyToolBatch) -> Self {
        Self { inner }
    }

    /// Immutable automatic-mutation checkpoint selected for adoption.
    pub const fn checkpoint_id(&self) -> Uuid {
        self.inner.checkpoint_id()
    }

    /// Accepted provider turns preceding the adopted result.
    pub const fn provider_turns(&self) -> u32 {
        self.inner.provider_turns()
    }

    /// Completed tool-call count through the adopted result.
    pub const fn total_tool_calls(&self) -> u32 {
        self.inner.total_tool_calls()
    }

    /// Application scope reauthorized during adoption.
    pub fn scope(&self) -> &AiScope {
        self.inner.scope()
    }

    /// Exact current hierarchical-rule fingerprint.
    pub fn rule_fingerprint(&self) -> &str {
        self.inner.rule_fingerprint()
    }

    /// Cumulative authoritative rule usage through this checkpoint.
    pub const fn rule_usage(&self) -> AiRuleRunUsage {
        self.inner.rule_usage()
    }

    pub(crate) fn continuation(&self) -> &AiAgentContinuation {
        self.inner.continuation()
    }
}

/// Current-authority adoption and one-shot consumption of protected tool
/// checkpoints, including bounded stateless histories whose every original
/// tool, budget, protected payload, and egress row remains exact.
#[async_trait]
pub trait AiAgentCheckpointAdopter: Send + Sync {
    /// Opens and validates the linked completed tool batch, when present.
    ///
    /// # Errors
    ///
    /// Returns a safe error for stale fencing, current access/protection
    /// denial, malformed protected state, or any mismatch with durable budget,
    /// tool, step, disclosure, or egress records.
    async fn adopt_tool_batch(
        &self,
        lease: &AiRunLease,
    ) -> Result<Option<AiAdoptedReadOnlyToolBatch>, AiError>;

    /// Atomically consumes one validated checkpoint before provider transport.
    ///
    /// # Errors
    ///
    /// Returns a safe error unless the checkpoint remains linked to the exact
    /// current lease and can be cleared through its row-version fence.
    async fn consume_before_provider(
        &self,
        lease: &AiRunLease,
        checkpoint_id: Uuid,
    ) -> Result<AiRunLease, AiError>;
}

#[async_trait]
impl AiAgentProviderOutputWriter for OrmAiProviderOutputService {
    async fn persist_output(
        &self,
        lease: &AiRunLease,
        result: &AiProviderCallResult,
    ) -> Result<AiPersistedProviderOutput, AiError> {
        self.persist(lease, result).await
    }
}

/// External phase whose incomplete durable handoff requires reconciliation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiAgentRecoveryPhase {
    /// A provider request or completed provider result could not be handed to
    /// the next exact durable phase.
    ProviderTurn,
    /// A protected application-tool call could have crossed resolver execution.
    ApplicationTool,
    /// Final provider output could not be proven persisted and finalized.
    ProviderOutput,
}

/// Durable result of one coordinator-owned claimed attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiReadOnlyAgentRunOutcome {
    /// Final assistant output was persisted and the run committed `Completed`.
    Completed {
        /// Persisted assistant message identifier.
        message_id: Uuid,
        /// Accepted provider-turn count.
        provider_turns: u32,
        /// Durably resolved application-tool call count.
        total_tool_calls: u32,
    },
    /// A proof/configuration failure was durably classified as safe failure.
    Failed {
        /// Accepted provider-turn count.
        provider_turns: u32,
        /// Durably resolved application-tool call count.
        total_tool_calls: u32,
    },
    /// The owner cancellation fence won and no later tool, output, or
    /// continuation was persisted.
    Cancelled {
        /// Accepted provider-turn count before cancellation.
        provider_turns: u32,
        /// Durably resolved application-tool call count before cancellation.
        total_tool_calls: u32,
    },
    /// An external boundary remained ambiguous and the run was durably closed
    /// for privileged reconciliation instead of being replayed.
    RecoveryRequired {
        /// Phase whose completion could not be proven.
        phase: AiAgentRecoveryPhase,
        /// Accepted provider-turn count.
        provider_turns: u32,
        /// Durably resolved application-tool call count.
        total_tool_calls: u32,
    },
    /// The retained provider session is cleaning up or waiting to rebind.
    ///
    /// The failed run is not terminal. The application session remains
    /// readable and a later run may continue after exact provider absence.
    Deferred {
        /// Closed reason the provider session cannot be used yet.
        reason: AiProviderSessionDeferralReason,
        /// Bounded delay before the same run may be retried.
        retry_after: std::time::Duration,
        /// Accepted provider-turn count.
        provider_turns: u32,
        /// Durably resolved application-tool call count.
        total_tool_calls: u32,
    },
}

/// Why a retained-provider turn was deferred instead of failing terminally.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiProviderSessionDeferralReason {
    /// The previous cursor is quarantined and has not yet been proven absent.
    CleanupPending,
}

/// Security-ordered coordinator for one claimed read-only agent attempt.
///
/// The coordinator starts and heartbeats the exact lease, asks a trusted host
/// planner for each turn, executes provider calls, resolves every custom query
/// through the protected ORM tool service, constructs exact continuations,
/// persists final output, and commits a terminal outcome. It can adopt only an
/// opaque, freshly validated complete read-only tool batch with a
/// provider-retained or bounded stateless continuation and consumes that
/// checkpoint before provider transport. It re-resolves the exact rule
/// fingerprint before provider egress and every application tool. Stateless
/// adoption revalidates every
/// historical durable result and never reruns a resolver. Any ambiguous
/// provider/tool/output handoff is similarly closed; the coordinator never
/// reconstructs or silently replays uncertain state.
pub struct AiReadOnlyAgentCoordinator {
    run_control: Arc<dyn AiAgentRunControl>,
    provider_executor: Arc<dyn AiAgentProviderTurnExecutor>,
    tool_executor: Arc<dyn AiAgentReadOnlyToolExecutor>,
    output_writer: Arc<dyn AiAgentProviderOutputWriter>,
    checkpoint_writer: Arc<dyn AiAgentCheckpointWriter>,
    checkpoint_adopter: Arc<dyn AiAgentCheckpointAdopter>,
    rule_resolver: Arc<dyn AiAgentRuleResolver>,
    planner: Arc<dyn AiReadOnlyAgentTurnPlanner>,
    limits: AiReadOnlyAgentCoordinatorLimits,
    provider_session_service: Option<Arc<dyn crate::AiProviderSessionService>>,
}

impl AiReadOnlyAgentCoordinator {
    /// Creates a coordinator from proof-preserving service boundaries.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_control: Arc<dyn AiAgentRunControl>,
        provider_executor: Arc<dyn AiAgentProviderTurnExecutor>,
        tool_executor: Arc<dyn AiAgentReadOnlyToolExecutor>,
        output_writer: Arc<dyn AiAgentProviderOutputWriter>,
        checkpoint_writer: Arc<dyn AiAgentCheckpointWriter>,
        checkpoint_adopter: Arc<dyn AiAgentCheckpointAdopter>,
        rule_resolver: Arc<dyn AiAgentRuleResolver>,
        planner: Arc<dyn AiReadOnlyAgentTurnPlanner>,
        limits: AiReadOnlyAgentCoordinatorLimits,
    ) -> Self {
        Self {
            run_control,
            provider_executor,
            tool_executor,
            output_writer,
            checkpoint_writer,
            checkpoint_adopter,
            rule_resolver,
            planner,
            limits,
            provider_session_service: None,
        }
    }

    /// Enables durable provider-session claim/create/resume for plans that
    /// explicitly carry [`crate::AiProviderSessionTurnPlan`].
    #[must_use]
    pub fn with_provider_session_service(
        mut self,
        service: Arc<dyn crate::AiProviderSessionService>,
    ) -> Self {
        self.provider_session_service = Some(service);
        self
    }

    async fn invalidate_result_provider_session(
        &self,
        result: &AiProviderCallResult,
        reason: &str,
    ) {
        if let (Some(service), Some(claim)) = (
            &self.provider_session_service,
            result.provider_session_claim(),
        ) {
            let _ = service.require_cleanup(claim, reason).await;
        }
    }

    /// Applies an interrupt settlement to the retained provider session of a
    /// cancelled run.
    ///
    /// A settled interrupt keeps the binding: the provider proved it discarded
    /// the interrupted partial turn, and the durable evidence below proves the
    /// turn persisted nothing, so the retained thread holds exactly the
    /// interrupted prompt with no reply — which is what the durable transcript
    /// records too. Everything else invalidates the binding through the same
    /// disclosed cleanup funnel as any other reset, so the user learns the
    /// model's context was lost.
    ///
    /// The durable leg is deliberately conservative. Settlement needs a run
    /// that never observed a completed provider turn, never executed a tool,
    /// and holds no checkpoint; a later turn of the same run has already put
    /// tool traffic into the thread that the message transcript does not
    /// reproduce on its own.
    async fn settle_interrupted_provider_session(
        &self,
        lease: &AiRunLease,
        guard: &AiAgentLoopGuard,
        settlement: crate::AiRunInterruptSettlement,
    ) {
        let Some(service) = &self.provider_session_service else {
            return;
        };
        let no_uncertain_persisted_output = guard.provider_turns() == 0
            && guard.total_tool_calls() == 0
            && lease.latest_checkpoint_id().is_none();
        let settlement = settlement.with_durable_turn_evidence(no_uncertain_persisted_output);
        if settlement.retains_thread()
            && service
                .settle_interrupted_turn(lease, settlement)
                .await
                .is_ok()
        {
            return;
        }
        let _ = service
            .require_cleanup_for_run(lease, "provider_session_interrupted_unsettled")
            .await;
    }

    /// Executes one freshly claimed lease through a bounded terminal outcome.
    ///
    /// A successful return means the corresponding terminal or
    /// `RecoveryRequired` state was durably committed. An error means that even
    /// that final write could not be proven; normal expired-lease/restore
    /// reconciliation must remain authoritative.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale/invalid claim, failed terminal write, lost
    /// heartbeat fence, or malformed service result. Provider/tool/output
    /// ambiguity is converted to a durable [`RecoveryRequired`] outcome when
    /// the current fence can still commit it.
    pub async fn execute_claimed(
        &self,
        claimed: &AiRunLease,
    ) -> Result<AiReadOnlyAgentRunOutcome, AiError> {
        let result = self.execute_claimed_inner(claimed).await;
        let reason = match &result {
            Ok(AiReadOnlyAgentRunOutcome::Completed { .. }) => {
                crate::AiProviderRunCloseReason::Completed
            }
            Ok(AiReadOnlyAgentRunOutcome::Cancelled { .. }) => {
                crate::AiProviderRunCloseReason::Cancelled
            }
            Ok(AiReadOnlyAgentRunOutcome::RecoveryRequired { .. }) => {
                crate::AiProviderRunCloseReason::RecoveryRequired
            }
            Ok(AiReadOnlyAgentRunOutcome::Deferred { .. }) => {
                crate::AiProviderRunCloseReason::Cancelled
            }
            Ok(AiReadOnlyAgentRunOutcome::Failed { .. }) => crate::AiProviderRunCloseReason::Failed,
            Err(_) => crate::AiProviderRunCloseReason::LeaseLost,
        };
        // The durable outcome remains authoritative even if graceful provider
        // cleanup reports an error. Stateful adapters must remove the exact
        // resource from admission and retain a synchronous kill-on-drop
        // fallback before returning that error.
        let _ = self.provider_executor.close_run(claimed, reason).await;
        result
    }

    async fn execute_claimed_inner(
        &self,
        claimed: &AiRunLease,
    ) -> Result<AiReadOnlyAgentRunOutcome, AiError> {
        let mut lease = match self.run_control.start(claimed).await {
            Ok(lease) => lease,
            Err(error) => {
                if self.run_control.cancellation(claimed).await?.is_some() {
                    return Ok(Cancelled {
                        provider_turns: 0,
                        total_tool_calls: 0,
                    });
                }
                return Err(error);
            }
        };
        if self.run_control.cancellation(&lease).await?.is_some() {
            return Ok(Cancelled {
                provider_turns: 0,
                total_tool_calls: 0,
            });
        }
        let (mut guard, mut turn_plan, mut rule_usage) = if lease.latest_checkpoint_id().is_some() {
            let adopted = match self.checkpoint_adopter.adopt_tool_batch(&lease).await {
                Ok(Some(adopted)) => adopted,
                Ok(None) | Err(_) => {
                    let guard = AiAgentLoopGuard::new(&lease, self.limits.loop_limits);
                    return self
                        .finish_recovery(
                            &lease,
                            &guard,
                            AiAgentRecoveryPhase::ApplicationTool,
                            "checkpoint_adoption_failed",
                            None,
                        )
                        .await;
                }
            };
            let continuation_reference = match adopted.continuation.chain_reference() {
                Ok(reference) => reference,
                Err(_) => {
                    let guard = AiAgentLoopGuard::new(&lease, self.limits.loop_limits);
                    return self
                        .finish_recovery(
                            &lease,
                            &guard,
                            AiAgentRecoveryPhase::ApplicationTool,
                            "checkpoint_adoption_reference_invalid",
                            None,
                        )
                        .await;
                }
            };
            let guard = match AiAgentLoopGuard::resume_after_tool_batch(
                &lease,
                self.limits.loop_limits,
                adopted.provider_turns,
                adopted.total_tool_calls,
                &continuation_reference,
            ) {
                Ok(guard) => guard,
                Err(_) => {
                    let guard = AiAgentLoopGuard::new(&lease, self.limits.loop_limits);
                    return self
                        .finish_recovery(
                            &lease,
                            &guard,
                            AiAgentRecoveryPhase::ApplicationTool,
                            "checkpoint_adoption_limits_invalid",
                            None,
                        )
                        .await;
                }
            };
            let adopted_rule_fingerprint = adopted.rule_fingerprint.clone();
            let adopted_usage = adopted.rule_usage;
            if !guard.can_begin_provider_turn() {
                return self
                    .finish_failed(&lease, &guard, "adopted_provider_turn_limit_reached")
                    .await;
            }
            let plan = match self
                .planner
                .continuation_plan(&lease, adopted.provider_turns, adopted.continuation)
                .await
            {
                Ok(plan)
                    if plan.is_continuation()
                        && plan.provider_call.scope() == &adopted.scope
                        && plan.rule_fingerprint() == adopted_rule_fingerprint =>
                {
                    plan
                }
                Err(_) => {
                    return self
                        .finish_failed(&lease, &guard, "adopted_continuation_plan_failed")
                        .await;
                }
                Ok(_) => {
                    return self
                        .finish_failed(&lease, &guard, "adopted_continuation_plan_invalid")
                        .await;
                }
            };
            lease = match self
                .checkpoint_adopter
                .consume_before_provider(&lease, adopted.checkpoint_id)
                .await
            {
                Ok(renewed) => renewed,
                Err(_) => {
                    return self
                        .finish_recovery(
                            &lease,
                            &guard,
                            AiAgentRecoveryPhase::ApplicationTool,
                            "checkpoint_consumption_failed",
                            None,
                        )
                        .await;
                }
            };
            (guard, plan, adopted_usage)
        } else {
            let guard = AiAgentLoopGuard::new(&lease, self.limits.loop_limits);
            let plan = match self.planner.initial_plan(&lease).await {
                Ok(plan) if !plan.is_continuation() => plan,
                Err(_) => {
                    return self
                        .finish_failed(&lease, &guard, "initial_plan_failed")
                        .await;
                }
                Ok(_) => {
                    return self
                        .finish_failed(&lease, &guard, "initial_plan_phase_invalid")
                        .await;
                }
            };
            (guard, plan, AiRuleRunUsage::default())
        };
        let mut capability_delivery: Option<crate::AiCapabilityDeliveryTurn> = None;
        let mut capability_delivery_initialized = false;

        loop {
            if self.run_control.cancellation(&lease).await?.is_some() {
                return Ok(Cancelled {
                    provider_turns: guard.provider_turns(),
                    total_tool_calls: guard.total_tool_calls(),
                });
            }
            if !guard.can_begin_provider_turn() {
                return self
                    .finish_failed(&lease, &guard, "provider_turn_limit_reached")
                    .await;
            }
            let (
                provider_plan,
                scope,
                correlation_id,
                mode,
                planned_rules,
                uses_byok,
                provider_session_plan,
                turn_capability_delivery,
            ) = turn_plan.into_parts();
            match (
                capability_delivery_initialized,
                &capability_delivery,
                &turn_capability_delivery,
            ) {
                (false, _, None) => {
                    capability_delivery_initialized = true;
                }
                (false, _, Some(delivery))
                    if delivery.matches_offered_tools(provider_plan.offered_tools()) =>
                {
                    capability_delivery = turn_capability_delivery.clone();
                    capability_delivery_initialized = true;
                }
                (true, None, None) => {}
                (true, Some(existing), Some(delivery))
                    if delivery.matches_offered_tools(provider_plan.offered_tools())
                        && existing.shares_run_state(delivery) =>
                {
                    capability_delivery = turn_capability_delivery.clone();
                }
                _ => {
                    return self
                        .finish_failed(&lease, &guard, "capability_delivery_surface_invalid")
                        .await;
                }
            }
            let resolution = match self.rule_resolver.resolve_rules(&lease, &scope).await {
                Ok(resolution)
                    if resolution.rules().fingerprint() == planned_rules.fingerprint() =>
                {
                    resolution
                }
                _ => {
                    return self
                        .finish_failed(&lease, &guard, "agent_rule_plan_stale")
                        .await;
                }
            };
            let started_rule_usage = match rule_usage.validate(&resolution) {
                Ok(usage) => usage,
                Err(_) => {
                    return self
                        .finish_failed(&lease, &guard, "agent_rule_duration_exceeded")
                        .await;
                }
            };
            if provider_plan
                .project_rule_usage(&resolution, started_rule_usage, uses_byok)
                .is_err()
            {
                return self
                    .finish_failed(&lease, &guard, "agent_rule_plan_denied")
                    .await;
            }
            rule_usage = started_rule_usage;
            let dynamic_execution = match &mode {
                AiReadOnlyAgentTurnMode::ExperimentalDynamicTools(route) => {
                    Some(Arc::new(ReadOnlyDynamicToolExecution::new(
                        self.run_control.clone(),
                        self.tool_executor.clone(),
                        self.rule_resolver.clone(),
                        scope.clone(),
                        correlation_id.clone(),
                        route.clone(),
                        planned_rules.fingerprint().to_owned(),
                        guard.provider_turns(),
                        guard.remaining_tool_capacity(),
                        rule_usage,
                        capability_delivery.clone(),
                    )))
                }
                AiReadOnlyAgentTurnMode::ChatOnly
                | AiReadOnlyAgentTurnMode::ApplicationTools(_) => None,
            };
            let provider_result = if let Some(session_plan) = provider_session_plan {
                let Some(session_service) = &self.provider_session_service else {
                    return self
                        .finish_failed(&lease, &guard, "provider_session_service_unavailable")
                        .await;
                };
                let execution_for_provider = dynamic_execution
                    .as_ref()
                    .map(|execution| execution.clone() as Arc<dyn AiProviderDynamicToolExecution>);
                self.execute_retained_provider_with_heartbeats(
                    &mut lease,
                    provider_plan,
                    session_plan,
                    session_service.clone(),
                    execution_for_provider,
                )
                .await
            } else if let Some(execution) = &dynamic_execution {
                let execution_for_provider: Arc<dyn AiProviderDynamicToolExecution> =
                    execution.clone();
                self.execute_dynamic_provider_with_heartbeats(
                    &mut lease,
                    provider_plan,
                    execution_for_provider,
                )
                .await
            } else {
                self.execute_provider_with_heartbeats(&mut lease, provider_plan)
                    .await
            };
            let result = match provider_result {
                Ok(result) => result,
                Err(ProviderTurnFailure::Deferred) => {
                    const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(5);
                    // The durable retry is what makes a message accepted during
                    // provider-session cleanup converge without an operator: the
                    // run becomes retry-scheduled and is reclaimed after the
                    // deadline, across a host restart. The `Deferred` outcome is
                    // a report, not the mechanism.
                    match self
                        .run_control
                        .schedule_retry(
                            &lease,
                            time::Duration::seconds(5),
                            "provider_session_cleanup_pending",
                        )
                        .await
                    {
                        Ok(()) => {
                            return Ok(Deferred {
                                reason: AiProviderSessionDeferralReason::CleanupPending,
                                retry_after: RETRY_AFTER,
                                provider_turns: guard.provider_turns(),
                                total_tool_calls: guard.total_tool_calls(),
                            });
                        }
                        Err(AiError::Conflict) => {
                            // The retry ceiling ran out while cleanup stayed
                            // pending. Nothing was executed on this attempt: no
                            // provider call, no tool, no persisted output. Close
                            // the run as a clean visible failure rather than
                            // letting the lease expire into `RecoveryRequired`,
                            // which would be both misclassified and stuck until
                            // an operator looked at it. A stale fence makes the
                            // terminal write fail too, so ordinary expired-lease
                            // reconciliation still owns that case.
                            return self
                                .finish_failed(
                                    &lease,
                                    &guard,
                                    "provider_session_cleanup_unavailable",
                                )
                                .await;
                        }
                        Err(error) => return Err(error),
                    }
                }
                Err(ProviderTurnFailure::Provider) => {
                    if self.run_control.cancellation(&lease).await?.is_some() {
                        return Ok(Cancelled {
                            provider_turns: guard.provider_turns(),
                            total_tool_calls: guard.total_tool_calls(),
                        });
                    }
                    return self
                        .finish_recovery(
                            &lease,
                            &guard,
                            AiAgentRecoveryPhase::ProviderTurn,
                            "provider_turn_uncertain",
                            None,
                        )
                        .await;
                }
                Err(ProviderTurnFailure::BudgetDenied) => {
                    // The atomic reservation is taken before the transport
                    // boundary, so a denial is a certain, local, pre-transport
                    // refusal: no provider turn was consumed and the
                    // reservation transaction held nothing. Closing the run
                    // for recovery here would report a proven refusal as
                    // unprovable provider uncertainty and permanently refuse
                    // retry admission for a run that is safe to author again
                    // once capacity exists.
                    return self
                        .finish_failed(&lease, &guard, "provider_budget_denied")
                        .await;
                }
                Err(ProviderTurnFailure::PreTransportProvider) => {
                    return self
                        .finish_failed(&lease, &guard, "provider_pre_transport_failed")
                        .await;
                }
                Err(ProviderTurnFailure::StatelessNativeItemRejected) => {
                    return self
                        .finish_failed(&lease, &guard, "provider_native_item_rejected")
                        .await;
                }
                Err(ProviderTurnFailure::LeaseLost(error)) => return Err(error),
                Err(ProviderTurnFailure::Cancelled(settlement)) => {
                    self.settle_interrupted_provider_session(&lease, &guard, settlement)
                        .await;
                    return Ok(Cancelled {
                        provider_turns: guard.provider_turns(),
                        total_tool_calls: guard.total_tool_calls(),
                    });
                }
            };
            if let Some(execution) = &dynamic_execution {
                rule_usage = execution.rule_usage().await;
            }

            if self.run_control.cancellation(&lease).await?.is_some() {
                self.invalidate_result_provider_session(
                    &result,
                    "provider_session_cancelled_after_turn",
                )
                .await;
                return Ok(Cancelled {
                    provider_turns: guard.provider_turns(),
                    total_tool_calls: guard.total_tool_calls(),
                });
            }

            let observed = match guard.observe_provider_turn(&result) {
                Ok(observed) => observed,
                Err(_) => {
                    self.invalidate_result_provider_session(
                        &result,
                        "provider_session_result_binding_failed",
                    )
                    .await;
                    return self
                        .finish_recovery(
                            &lease,
                            &guard,
                            AiAgentRecoveryPhase::ProviderTurn,
                            "provider_turn_binding_failed",
                            result.provider_response_id(),
                        )
                        .await;
                }
            };
            let current_rules = match self.rule_resolver.resolve_rules(&lease, &scope).await {
                Ok(current) if current.rules().fingerprint() == planned_rules.fingerprint() => {
                    current
                }
                _ => {
                    self.invalidate_result_provider_session(
                        &result,
                        "provider_session_rule_changed",
                    )
                    .await;
                    return self
                        .finish_recovery(
                            &lease,
                            &guard,
                            AiAgentRecoveryPhase::ProviderTurn,
                            "agent_rule_changed_after_provider",
                            result.provider_response_id(),
                        )
                        .await;
                }
            };
            rule_usage = match rule_usage.accept_provider_with_web_searches(
                result.usage(),
                result.builtin_usage().web_search_calls(),
                &current_rules,
            ) {
                Ok(usage) => usage,
                Err(_) => {
                    self.invalidate_result_provider_session(
                        &result,
                        "provider_session_budget_exceeded",
                    )
                    .await;
                    return self
                        .finish_recovery(
                            &lease,
                            &guard,
                            AiAgentRecoveryPhase::ProviderTurn,
                            "agent_rule_budget_exceeded",
                            result.provider_response_id(),
                        )
                        .await;
                }
            };
            let route = match mode {
                AiReadOnlyAgentTurnMode::ChatOnly => match observed {
                    AiAgentLoopTurn::Completed => {
                        return self
                            .finish_completed_provider_turn(&lease, &guard, &result)
                            .await;
                    }
                    AiAgentLoopTurn::ToolCalls { .. } => {
                        self.invalidate_result_provider_session(
                            &result,
                            "provider_session_unexpected_tool_batch",
                        )
                        .await;
                        return self
                            .finish_recovery(
                                &lease,
                                &guard,
                                AiAgentRecoveryPhase::ProviderTurn,
                                "chat_turn_returned_application_tools",
                                result.provider_response_id(),
                            )
                            .await;
                    }
                },
                AiReadOnlyAgentTurnMode::ApplicationTools(route) => route,
                AiReadOnlyAgentTurnMode::ExperimentalDynamicTools(_) => match observed {
                    AiAgentLoopTurn::Completed => {
                        return self
                            .finish_completed_provider_turn(&lease, &guard, &result)
                            .await;
                    }
                    AiAgentLoopTurn::ToolCalls { .. } => {
                        self.invalidate_result_provider_session(
                            &result,
                            "provider_session_dynamic_turn_incomplete",
                        )
                        .await;
                        return self
                            .finish_recovery(
                                &lease,
                                &guard,
                                AiAgentRecoveryPhase::ProviderTurn,
                                "experimental_dynamic_tool_turn_incomplete",
                                result.provider_response_id(),
                            )
                            .await;
                    }
                },
            };
            lease = match self
                .checkpoint_writer
                .persist_provider_turn(
                    &lease,
                    &result,
                    &scope,
                    &correlation_id,
                    &route,
                    &planned_rules,
                    rule_usage,
                    guard.provider_turns(),
                    guard.total_tool_calls(),
                )
                .await
            {
                Ok(renewed) => renewed,
                Err(_) => {
                    return self
                        .finish_recovery(
                            &lease,
                            &guard,
                            AiAgentRecoveryPhase::ProviderTurn,
                            "provider_turn_checkpoint_uncertain",
                            result.provider_response_id(),
                        )
                        .await;
                }
            };
            match observed {
                AiAgentLoopTurn::Completed => {
                    return self
                        .finish_completed_provider_turn(&lease, &guard, &result)
                        .await;
                }
                AiAgentLoopTurn::ToolCalls {
                    provider_turn_index,
                    call_count,
                } => {
                    let tool_rules = match self.rule_resolver.resolve_rules(&lease, &scope).await {
                        Ok(current)
                            if current.rules().fingerprint() == planned_rules.fingerprint() =>
                        {
                            current
                        }
                        _ => {
                            return self
                                .finish_failed(&lease, &guard, "agent_rule_changed_before_tools")
                                .await;
                        }
                    };
                    let completed_rule_usage =
                        match rule_usage.accept_tool_calls(call_count, &tool_rules) {
                            Ok(usage) => usage,
                            Err(_) => {
                                return self
                                    .finish_failed(&lease, &guard, "agent_rule_steps_exceeded")
                                    .await;
                            }
                        };
                    let mut completed_tools = Vec::with_capacity(call_count);
                    for tool_call_index in 0..call_count {
                        if self.run_control.cancellation(&lease).await?.is_some() {
                            return Ok(Cancelled {
                                provider_turns: guard.provider_turns(),
                                total_tool_calls: guard.total_tool_calls(),
                            });
                        }
                        match self.rule_resolver.resolve_rules(&lease, &scope).await {
                            Ok(current)
                                if current.rules().fingerprint() == planned_rules.fingerprint() =>
                            {
                                if current.rules().constrain_tool(
                                    result.tool_calls()[tool_call_index].tool_fingerprint(),
                                    crate::ToolMaturity::ReadOnly,
                                    crate::AiApprovalRule::None,
                                ) != Some(crate::AiApprovalRule::None)
                                {
                                    return self
                                        .finish_failed(&lease, &guard, "agent_rule_tool_denied")
                                        .await;
                                }
                            }
                            _ => {
                                return self
                                    .finish_failed(&lease, &guard, "agent_rule_changed_before_tool")
                                    .await;
                            }
                        }
                        let context = AiApplicationToolCallContext::new(
                            provider_turn_index,
                            tool_call_index,
                            scope.clone(),
                            correlation_id.clone(),
                            result.budget_reservation_id().0.to_string(),
                        )?;
                        let failure_context = context.clone();
                        let broker_operation = crate::AiCapabilityBrokerOperation::from_tool_id(
                            result.tool_calls()[tool_call_index].tool_id(),
                        );
                        let dispatch = match (broker_operation, capability_delivery.as_ref()) {
                            (None, _) => {
                                self.tool_executor
                                    .execute_tool(&lease, &result, context, route.clone())
                                    .await
                            }
                            (Some(_), Some(delivery)) => {
                                self.tool_executor
                                    .execute_capability_broker(
                                        &lease,
                                        &result,
                                        context,
                                        route.clone(),
                                        delivery,
                                    )
                                    .await
                            }
                            (Some(_), None) => Err(AiError::Forbidden),
                        };
                        let persisted = match dispatch {
                            Ok(persisted) => persisted,
                            Err(error) => {
                                if self.run_control.cancellation(&lease).await?.is_some() {
                                    return Ok(Cancelled {
                                        provider_turns: guard.provider_turns(),
                                        total_tool_calls: guard.total_tool_calls(),
                                    });
                                }
                                if let Some(code) =
                                    crate::classify_safe_application_tool_error(&error)
                                {
                                    match self
                                        .tool_executor
                                        .persist_safe_failure(
                                            &lease,
                                            &result,
                                            failure_context,
                                            route.clone(),
                                            code,
                                        )
                                        .await
                                    {
                                        Ok(persisted) => persisted,
                                        Err(_) => {
                                            return self
                                                .finish_recovery(
                                                    &lease,
                                                    &guard,
                                                    AiAgentRecoveryPhase::ApplicationTool,
                                                    "application_tool_persistence_uncertain",
                                                    result.provider_response_id(),
                                                )
                                                .await;
                                        }
                                    }
                                } else {
                                    return self
                                        .finish_recovery(
                                            &lease,
                                            &guard,
                                            AiAgentRecoveryPhase::ApplicationTool,
                                            "application_tool_uncertain",
                                            result.provider_response_id(),
                                        )
                                        .await;
                                }
                            }
                        };
                        let renewed = persisted.lease().clone();
                        if guard.observe_tool_result(&persisted).is_err() {
                            return self
                                .finish_failed(
                                    &renewed,
                                    &guard,
                                    "application_tool_result_unavailable",
                                )
                                .await;
                        }
                        lease = renewed;
                        if self.run_control.cancellation(&lease).await?.is_some() {
                            return Ok(Cancelled {
                                provider_turns: guard.provider_turns(),
                                total_tool_calls: guard.total_tool_calls(),
                            });
                        }
                        completed_tools.push(persisted);
                    }
                    let continuation = match guard.continuation() {
                        Ok(continuation) => continuation,
                        Err(_) => {
                            return self
                                .finish_recovery(
                                    &lease,
                                    &guard,
                                    AiAgentRecoveryPhase::ApplicationTool,
                                    "application_tool_batch_invalid",
                                    result.provider_response_id(),
                                )
                                .await;
                        }
                    };
                    rule_usage = completed_rule_usage;
                    lease = match self
                        .checkpoint_writer
                        .persist_tool_batch(
                            &lease,
                            &result,
                            &completed_tools,
                            &continuation,
                            &scope,
                            &correlation_id,
                            &route,
                            &planned_rules,
                            rule_usage,
                            guard.provider_turns(),
                            guard.total_tool_calls(),
                        )
                        .await
                    {
                        Ok(renewed) => renewed,
                        Err(_) => {
                            return self
                                .finish_recovery(
                                    &lease,
                                    &guard,
                                    AiAgentRecoveryPhase::ApplicationTool,
                                    "application_tool_batch_checkpoint_uncertain",
                                    result.provider_response_id(),
                                )
                                .await;
                        }
                    };
                    if let Some(delivery) = capability_delivery.as_ref()
                        && delivery.requires_deferred_installation()
                    {
                        let installed = self
                            .tool_executor
                            .loaded_capability_definitions(&lease, result.provider_kind(), delivery)
                            .await
                            .and_then(|definitions| {
                                delivery.install_deferred_definitions(definitions)
                            });
                        if installed.is_err() {
                            return self
                                .finish_failed(&lease, &guard, "capability_delivery_install_failed")
                                .await;
                        }
                    }
                    turn_plan = match self
                        .planner
                        .continuation_plan_with_capability_delivery(
                            &lease,
                            guard.provider_turns(),
                            continuation,
                            capability_delivery.as_ref(),
                        )
                        .await
                    {
                        Ok(plan) if plan.is_continuation() => plan,
                        Err(_) => {
                            return self
                                .finish_failed(&lease, &guard, "continuation_plan_failed")
                                .await;
                        }
                        Ok(_) => {
                            return self
                                .finish_failed(&lease, &guard, "continuation_plan_phase_invalid")
                                .await;
                        }
                    };
                    if !guard.can_begin_provider_turn() {
                        return self
                            .finish_failed(&lease, &guard, "provider_turn_limit_reached")
                            .await;
                    }
                    let checkpoint_id = match lease.latest_checkpoint_id() {
                        Some(checkpoint_id) => checkpoint_id,
                        None => {
                            return self
                                .finish_recovery(
                                    &lease,
                                    &guard,
                                    AiAgentRecoveryPhase::ApplicationTool,
                                    "application_tool_checkpoint_missing",
                                    result.provider_response_id(),
                                )
                                .await;
                        }
                    };
                    lease = match self
                        .checkpoint_adopter
                        .consume_before_provider(&lease, checkpoint_id)
                        .await
                    {
                        Ok(renewed) => renewed,
                        Err(_) => {
                            return self
                                .finish_recovery(
                                    &lease,
                                    &guard,
                                    AiAgentRecoveryPhase::ApplicationTool,
                                    "application_tool_checkpoint_consumption_failed",
                                    result.provider_response_id(),
                                )
                                .await;
                        }
                    };
                }
            }
        }
    }

    async fn execute_provider_with_heartbeats(
        &self,
        lease: &mut AiRunLease,
        plan: AiProviderCallPlan,
    ) -> Result<AiProviderCallResult, ProviderTurnFailure> {
        let provider_lease = lease.clone();
        let provider = self.provider_executor.execute_turn(&provider_lease, plan);
        tokio::pin!(provider);
        let heartbeat_delay = self.limits.heartbeat_interval.unsigned_abs();
        loop {
            let cancellation_lease = lease.clone();
            let cancellation = self
                .run_control
                .wait_for_cancellation(&cancellation_lease, heartbeat_delay);
            tokio::pin!(cancellation);
            tokio::select! {
                result = &mut provider => {
                    return result.map_err(|error| classify_provider_turn_failure(&error));
                }
                result = &mut cancellation => {
                    match result.map_err(ProviderTurnFailure::LeaseLost)? {
                        Some(_) => {
                            let settlement = self
                                .provider_executor
                                .interrupt_run(lease)
                                .await
                                .unwrap_or(crate::AiRunInterruptSettlement::RequestedUnsettled);
                            return Err(ProviderTurnFailure::Cancelled(settlement));
                        }
                        None => {
                            match self.run_control.heartbeat(lease).await {
                                Ok(renewed) => *lease = renewed,
                                Err(error) => {
                                    let _ = self.provider_executor.interrupt_run(lease).await;
                                    return Err(ProviderTurnFailure::LeaseLost(error));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    async fn execute_dynamic_provider_with_heartbeats(
        &self,
        lease: &mut AiRunLease,
        plan: AiProviderCallPlan,
        execution: Arc<dyn AiProviderDynamicToolExecution>,
    ) -> Result<AiProviderCallResult, ProviderTurnFailure> {
        let lease_state = Arc::new(Mutex::new(lease.clone()));
        let provider =
            self.provider_executor
                .execute_dynamic_turn(lease_state.clone(), plan, execution);
        tokio::pin!(provider);
        let heartbeat_delay = self.limits.heartbeat_interval.unsigned_abs();
        loop {
            let cancellation_lease = lease_state.lock().await.clone();
            let cancellation = self
                .run_control
                .wait_for_cancellation(&cancellation_lease, heartbeat_delay);
            tokio::pin!(cancellation);
            tokio::select! {
                result = &mut provider => {
                    *lease = lease_state.lock().await.clone();
                    return result.map_err(|error| classify_provider_turn_failure(&error));
                }
                result = &mut cancellation => {
                    match result.map_err(ProviderTurnFailure::LeaseLost)? {
                        Some(_) => {
                            let current = lease_state.lock().await.clone();
                            // A failed or unrecognized interrupt proves nothing,
                            // so it fails closed into invalidation.
                            let settlement = self
                                .provider_executor
                                .interrupt_run(&current)
                                .await
                                .unwrap_or(crate::AiRunInterruptSettlement::RequestedUnsettled);
                            *lease = current;
                            return Err(ProviderTurnFailure::Cancelled(settlement));
                        }
                        None => {
                            let mut current = lease_state.lock().await;
                            match self.run_control.heartbeat(&current).await {
                                Ok(renewed) => {
                                    *current = renewed.clone();
                                    *lease = renewed;
                                }
                                Err(error) => {
                                    let lost = current.clone();
                                    drop(current);
                                    let _ = self.provider_executor.interrupt_run(&lost).await;
                                    *lease = lost;
                                    return Err(ProviderTurnFailure::LeaseLost(error));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    async fn execute_retained_provider_with_heartbeats(
        &self,
        lease: &mut AiRunLease,
        plan: AiProviderCallPlan,
        session_plan: crate::AiProviderSessionTurnPlan,
        session_service: Arc<dyn crate::AiProviderSessionService>,
        execution: Option<Arc<dyn AiProviderDynamicToolExecution>>,
    ) -> Result<AiProviderCallResult, ProviderTurnFailure> {
        let lease_state = Arc::new(Mutex::new(lease.clone()));
        let provider = self.provider_executor.execute_retained_turn(
            lease_state.clone(),
            plan,
            session_plan,
            session_service,
            execution,
        );
        tokio::pin!(provider);
        let heartbeat_delay = self.limits.heartbeat_interval.unsigned_abs();
        loop {
            let cancellation_lease = lease_state.lock().await.clone();
            let cancellation = self
                .run_control
                .wait_for_cancellation(&cancellation_lease, heartbeat_delay);
            tokio::pin!(cancellation);
            tokio::select! {
                result = &mut provider => {
                    *lease = lease_state.lock().await.clone();
                    return match result {
                        Ok(value) => Ok(value),
                        Err(AiError::ProviderSessionDeferred) => {
                            Err(ProviderTurnFailure::Deferred)
                        }
                        Err(error) => Err(classify_provider_turn_failure(&error)),
                    };
                }
                result = &mut cancellation => {
                    match result.map_err(ProviderTurnFailure::LeaseLost)? {
                        Some(_) => {
                            let current = lease_state.lock().await.clone();
                            // A failed or unrecognized interrupt proves nothing,
                            // so it fails closed into invalidation.
                            let settlement = self
                                .provider_executor
                                .interrupt_run(&current)
                                .await
                                .unwrap_or(crate::AiRunInterruptSettlement::RequestedUnsettled);
                            *lease = current;
                            return Err(ProviderTurnFailure::Cancelled(settlement));
                        }
                        None => {
                            let mut current = lease_state.lock().await;
                            match self.run_control.heartbeat(&current).await {
                                Ok(renewed) => {
                                    *current = renewed.clone();
                                    *lease = renewed;
                                }
                                Err(error) => {
                                    let lost = current.clone();
                                    drop(current);
                                    let _ = self.provider_executor.interrupt_run(&lost).await;
                                    *lease = lost;
                                    return Err(ProviderTurnFailure::LeaseLost(error));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    async fn finish_completed_provider_turn(
        &self,
        lease: &AiRunLease,
        guard: &AiAgentLoopGuard,
        result: &AiProviderCallResult,
    ) -> Result<AiReadOnlyAgentRunOutcome, AiError> {
        if self.run_control.cancellation(lease).await?.is_some() {
            self.invalidate_result_provider_session(
                result,
                "provider_session_cancelled_before_output",
            )
            .await;
            return Ok(Cancelled {
                provider_turns: guard.provider_turns(),
                total_tool_calls: guard.total_tool_calls(),
            });
        }
        let persisted = match self.output_writer.persist_output(lease, result).await {
            Ok(persisted) => persisted,
            Err(_) => {
                self.invalidate_result_provider_session(
                    result,
                    "provider_session_output_uncertain",
                )
                .await;
                return self
                    .finish_recovery(
                        lease,
                        guard,
                        AiAgentRecoveryPhase::ProviderOutput,
                        "provider_output_uncertain",
                        result.provider_response_id(),
                    )
                    .await;
            }
        };
        let message_id = persisted.message_id();
        let lease = persisted.into_lease();
        if self.run_control.cancellation(&lease).await?.is_some() {
            if let (Some(service), Some(claim)) = (
                &self.provider_session_service,
                result.provider_session_claim(),
            ) {
                let _ = service
                    .require_cleanup(claim, "provider_session_cancelled_after_output")
                    .await;
            }
            return Ok(Cancelled {
                provider_turns: guard.provider_turns(),
                total_tool_calls: guard.total_tool_calls(),
            });
        }
        let provider_session_commit = match result.provider_session_commit(message_id) {
            Ok(commit) => commit,
            Err(_) => {
                self.invalidate_result_provider_session(
                    result,
                    "provider_session_commit_proof_invalid",
                )
                .await;
                None
            }
        };
        let provider_session_commit = if let Some(commit) = provider_session_commit {
            let Some(service) = &self.provider_session_service else {
                return self
                    .finish_recovery(
                        &lease,
                        guard,
                        AiAgentRecoveryPhase::ProviderOutput,
                        "provider_session_commit_service_unavailable",
                        result.provider_response_id(),
                    )
                    .await;
            };
            let claim = result.provider_session_claim().ok_or(AiError::Conflict)?;
            Some((service.clone(), claim.clone(), commit))
        } else {
            None
        };
        let completion = AiRunCompletion::new(
            AiRunState::Completed,
            "agent_completed",
            None,
            result.provider_response_id().map(str::to_owned),
        )?;
        if let Err(error) = self.run_control.finish(&lease, completion).await {
            if let Some((service, claim, _)) = &provider_session_commit {
                let _ = service
                    .require_cleanup(claim, "provider_session_terminal_write_uncertain")
                    .await;
            }
            return Err(error);
        }
        if let Some((service, claim, commit)) = provider_session_commit
            && service.commit_turn(&lease, &claim, commit).await.is_err()
        {
            // The answer and canonical terminal run already won their fence.
            // Provider retention is an optimization, so quarantine the cursor
            // rather than changing the successful user-visible outcome.
            let _ = service
                .require_cleanup(&claim, "provider_session_commit_uncertain")
                .await;
        }
        Ok(Completed {
            message_id,
            provider_turns: guard.provider_turns(),
            total_tool_calls: guard.total_tool_calls(),
        })
    }

    async fn finish_failed(
        &self,
        lease: &AiRunLease,
        guard: &AiAgentLoopGuard,
        code: &str,
    ) -> Result<AiReadOnlyAgentRunOutcome, AiError> {
        if self.run_control.cancellation(lease).await?.is_some() {
            return Ok(Cancelled {
                provider_turns: guard.provider_turns(),
                total_tool_calls: guard.total_tool_calls(),
            });
        }
        let completion =
            AiRunCompletion::new(AiRunState::Failed, code, Some(code.to_owned()), None)?;
        self.run_control.finish(lease, completion).await?;
        Ok(Failed {
            provider_turns: guard.provider_turns(),
            total_tool_calls: guard.total_tool_calls(),
        })
    }

    async fn finish_recovery(
        &self,
        lease: &AiRunLease,
        guard: &AiAgentLoopGuard,
        phase: AiAgentRecoveryPhase,
        code: &str,
        provider_response_id: Option<&str>,
    ) -> Result<AiReadOnlyAgentRunOutcome, AiError> {
        if self.run_control.cancellation(lease).await?.is_some() {
            return Ok(Cancelled {
                provider_turns: guard.provider_turns(),
                total_tool_calls: guard.total_tool_calls(),
            });
        }
        let completion = AiRunCompletion::new(
            AiRunState::RecoveryRequired,
            code,
            Some(code.to_owned()),
            provider_response_id.map(str::to_owned),
        )?;
        self.run_control.finish(lease, completion).await?;
        Ok(RecoveryRequired {
            phase,
            provider_turns: guard.provider_turns(),
            total_tool_calls: guard.total_tool_calls(),
        })
    }
}

enum ProviderTurnFailure {
    Provider,
    BudgetDenied,
    PreTransportProvider,
    StatelessNativeItemRejected,
    Deferred,
    LeaseLost(AiError),
    /// Owner cancellation won the fence; the value reports what the resulting
    /// interrupt proved about the provider's retained thread.
    Cancelled(crate::AiRunInterruptSettlement),
}

/// Separates proof-bearing refusals from an uncertain provider turn.
///
/// The budget reservation is taken before the transport boundary and inside
/// the same call that later dispatches. A budget denial or a typed adapter
/// pre-dispatch rejection therefore proves that no bytes crossed the provider
/// boundary, that no provider turn was consumed, and that the atomic
/// reservation transaction left nothing held. A stateless
/// native-item refusal is separately proof-bearing only after the
/// executor has committed authoritative usage and proven that no answer or
/// admitted host tool effect exists. Every other executor error keeps the
/// fail-closed uncertain classification.
const fn classify_provider_turn_failure(error: &AiError) -> ProviderTurnFailure {
    match error {
        AiError::PreTransportBudgetDenied => ProviderTurnFailure::BudgetDenied,
        AiError::PreTransportProviderFailed => ProviderTurnFailure::PreTransportProvider,
        AiError::StatelessNativeItemRejected => ProviderTurnFailure::StatelessNativeItemRejected,
        _ => ProviderTurnFailure::Provider,
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use agql_auth::{
        AccessTokenMetadata, AuthPrincipal, AuthUser, PrincipalReference, SessionContext,
    };
    use serde_json::json;

    use super::*;
    use crate::{
        AiDataSourceRef, AiDestinationTrust, AiEgressCapability, AiEgressManifest,
        AiRunCancellation, AiSourceTrust, DataClassification, ModelContinuation,
    };

    struct TestRunControl {
        finishes: Mutex<Vec<AiRunState>>,
        finish_codes: Mutex<Vec<String>>,
        heartbeat_count: AtomicUsize,
        fail_heartbeat: AtomicBool,
        cancelled: AtomicBool,
        scheduled_retries: Mutex<Vec<String>>,
        retry_ceiling_reached: AtomicBool,
    }

    impl TestRunControl {
        fn new() -> Self {
            Self {
                finishes: Mutex::new(Vec::new()),
                finish_codes: Mutex::new(Vec::new()),
                heartbeat_count: AtomicUsize::new(0),
                fail_heartbeat: AtomicBool::new(false),
                cancelled: AtomicBool::new(false),
                scheduled_retries: Mutex::new(Vec::new()),
                retry_ceiling_reached: AtomicBool::new(false),
            }
        }

        fn scheduled_retry_codes(&self) -> Vec<String> {
            self.scheduled_retries
                .lock()
                .expect("test retry lock should not be poisoned")
                .clone()
        }

        fn final_codes(&self) -> Vec<String> {
            self.finish_codes
                .lock()
                .expect("test finish code lock should not be poisoned")
                .clone()
        }

        fn final_states(&self) -> Vec<AiRunState> {
            self.finishes
                .lock()
                .expect("test finish lock should not be poisoned")
                .clone()
        }
    }

    #[async_trait]
    impl AiAgentRunControl for TestRunControl {
        async fn start(&self, lease: &AiRunLease) -> Result<AiRunLease, AiError> {
            Ok(lease.clone())
        }

        async fn heartbeat(&self, lease: &AiRunLease) -> Result<AiRunLease, AiError> {
            self.heartbeat_count.fetch_add(1, Ordering::SeqCst);
            if self.fail_heartbeat.load(Ordering::SeqCst) {
                Err(AiError::Conflict)
            } else {
                Ok(lease.clone())
            }
        }

        async fn cancellation(
            &self,
            lease: &AiRunLease,
        ) -> Result<Option<AiRunCancellation>, AiError> {
            Ok(self.cancelled.load(Ordering::SeqCst).then(|| {
                AiRunCancellation::new(
                    lease.session_id(),
                    lease.run_id(),
                    Uuid::from_u128(91),
                    1_700_000_000,
                )
            }))
        }

        async fn wait_for_cancellation(
            &self,
            lease: &AiRunLease,
            maximum_wait: std::time::Duration,
        ) -> Result<Option<AiRunCancellation>, AiError> {
            tokio::time::sleep(maximum_wait.min(std::time::Duration::from_millis(5))).await;
            self.cancellation(lease).await
        }

        async fn finish(
            &self,
            _lease: &AiRunLease,
            completion: AiRunCompletion,
        ) -> Result<(), AiError> {
            self.finish_codes
                .lock()
                .expect("test finish code lock should not be poisoned")
                .push(completion.outcome_code().to_owned());
            self.finishes
                .lock()
                .expect("test finish lock should not be poisoned")
                .push(completion.final_state());
            Ok(())
        }

        async fn schedule_retry(
            &self,
            _lease: &AiRunLease,
            _delay: time::Duration,
            error_code: &str,
        ) -> Result<(), AiError> {
            if self.retry_ceiling_reached.load(Ordering::SeqCst) {
                // Exactly what the durable service returns once the run has
                // exhausted its bounded retry allowance.
                return Err(AiError::Conflict);
            }
            self.scheduled_retries
                .lock()
                .expect("test retry lock should not be poisoned")
                .push(error_code.to_owned());
            Ok(())
        }
    }

    struct TestProviderExecutor {
        responses: Mutex<VecDeque<Result<AiProviderCallResult, AiError>>>,
        delay: Option<std::time::Duration>,
    }

    #[cfg(feature = "provider-codex-app-server")]
    struct CanonicalDynamicProviderExecutor {
        definition: crate::ModelToolDefinition,
    }

    impl TestProviderExecutor {
        fn remaining_responses(&self) -> usize {
            self.responses
                .lock()
                .expect("test response lock should not be poisoned")
                .len()
        }
    }

    #[async_trait]
    impl AiAgentProviderTurnExecutor for TestProviderExecutor {
        async fn execute_turn(
            &self,
            _lease: &AiRunLease,
            _plan: AiProviderCallPlan,
        ) -> Result<AiProviderCallResult, AiError> {
            if let Some(delay) = self.delay {
                tokio::time::sleep(delay).await;
            }
            self.responses
                .lock()
                .expect("test response lock should not be poisoned")
                .pop_front()
                .expect("test provider response should exist")
        }

        async fn execute_dynamic_turn(
            &self,
            lease: Arc<tokio::sync::Mutex<AiRunLease>>,
            _plan: AiProviderCallPlan,
            execution: Arc<dyn AiProviderDynamicToolExecution>,
        ) -> Result<AiProviderCallResult, AiError> {
            if let Some(delay) = self.delay {
                tokio::time::sleep(delay).await;
            }
            let mut result = self
                .responses
                .lock()
                .expect("test response lock should not be poisoned")
                .pop_front()
                .expect("test provider response should exist")?;
            let mut completed = Vec::new();
            for tool_call_index in 0..result.tool_calls().len() {
                let current = lease.lock().await.clone();
                let persisted = execution
                    .execute_dynamic_tool(&current, &result, tool_call_index)
                    .await?;
                *lease.lock().await = persisted.lease().clone();
                completed.push(persisted);
            }
            result = result.test_with_interactive_tool_results(completed);
            Ok(result)
        }
    }

    #[cfg(feature = "provider-codex-app-server")]
    #[async_trait]
    impl AiAgentProviderTurnExecutor for CanonicalDynamicProviderExecutor {
        async fn execute_turn(
            &self,
            _lease: &AiRunLease,
            _plan: AiProviderCallPlan,
        ) -> Result<AiProviderCallResult, AiError> {
            Err(AiError::Conflict)
        }

        async fn execute_dynamic_turn(
            &self,
            lease: Arc<tokio::sync::Mutex<AiRunLease>>,
            _plan: AiProviderCallPlan,
            execution: Arc<dyn AiProviderDynamicToolExecution>,
        ) -> Result<AiProviderCallResult, AiError> {
            let current = lease.lock().await.clone();
            let mut result = AiProviderCallResult::test_result(
                &current,
                None,
                "canonical-dynamic-response",
                vec![(
                    "canonical-dynamic-call",
                    self.definition.tool_id.as_str(),
                    json!({"Limit": 3}),
                )],
            );
            let persisted = execution.execute_dynamic_tool(&current, &result, 0).await?;
            *lease.lock().await = persisted.lease().clone();
            result = result.test_with_interactive_tool_results(vec![persisted]);
            Ok(result)
        }
    }

    struct RetainedTestProviderExecutor {
        result: Mutex<Option<AiProviderCallResult>>,
        claim: crate::AiProviderSessionClaim,
    }

    #[async_trait]
    impl AiAgentProviderTurnExecutor for RetainedTestProviderExecutor {
        async fn execute_turn(
            &self,
            _lease: &AiRunLease,
            _plan: AiProviderCallPlan,
        ) -> Result<AiProviderCallResult, AiError> {
            Err(AiError::Conflict)
        }

        async fn execute_retained_turn(
            &self,
            _lease: Arc<tokio::sync::Mutex<AiRunLease>>,
            _plan: AiProviderCallPlan,
            _session_plan: crate::AiProviderSessionTurnPlan,
            _session_service: Arc<dyn crate::AiProviderSessionService>,
            _execution: Option<Arc<dyn AiProviderDynamicToolExecution>>,
        ) -> Result<AiProviderCallResult, AiError> {
            self.result
                .lock()
                .expect("retained result lock should not be poisoned")
                .take()
                .ok_or(AiError::Conflict)
                .map(|result| result.test_with_provider_session_claim(self.claim.clone()))
        }
    }

    struct DeferringRetainedProviderExecutor;

    #[async_trait]
    impl AiAgentProviderTurnExecutor for DeferringRetainedProviderExecutor {
        async fn execute_turn(
            &self,
            _lease: &AiRunLease,
            _plan: AiProviderCallPlan,
        ) -> Result<AiProviderCallResult, AiError> {
            Err(AiError::Conflict)
        }

        async fn execute_retained_turn(
            &self,
            _lease: Arc<tokio::sync::Mutex<AiRunLease>>,
            _plan: AiProviderCallPlan,
            _session_plan: crate::AiProviderSessionTurnPlan,
            _session_service: Arc<dyn crate::AiProviderSessionService>,
            _execution: Option<Arc<dyn AiProviderDynamicToolExecution>>,
        ) -> Result<AiProviderCallResult, AiError> {
            Err(AiError::ProviderSessionDeferred)
        }
    }

    struct TestProviderSessionService {
        run: Arc<TestRunControl>,
        commits: AtomicUsize,
        cleanups: AtomicUsize,
        fail_commit: bool,
    }

    #[async_trait]
    impl crate::AiProviderSessionService for TestProviderSessionService {
        async fn inspect_for_run(
            &self,
            _lease: &AiRunLease,
        ) -> Result<Option<crate::AiProviderSessionBindingView>, AiError> {
            Ok(None)
        }

        async fn bind_for_run(
            &self,
            _lease: &AiRunLease,
            _request: crate::AiProviderSessionBindRequest,
        ) -> Result<crate::AiProviderSessionClaim, AiError> {
            Err(AiError::Conflict)
        }

        async fn claim_for_run(
            &self,
            _lease: &AiRunLease,
            _expected: &crate::AiProviderSessionDescriptor,
            _expected_transcript_fingerprint: &str,
        ) -> Result<crate::AiProviderSessionClaim, AiError> {
            Err(AiError::Conflict)
        }

        async fn open_for_run(
            &self,
            _lease: &AiRunLease,
            _claim: &crate::AiProviderSessionClaim,
        ) -> Result<crate::AiOpenedProviderSession, AiError> {
            Err(AiError::Conflict)
        }

        async fn heartbeat(
            &self,
            _lease: &AiRunLease,
            _claim: &crate::AiProviderSessionClaim,
        ) -> Result<crate::AiProviderSessionClaim, AiError> {
            Err(AiError::Conflict)
        }

        async fn commit_turn(
            &self,
            _lease: &AiRunLease,
            claim: &crate::AiProviderSessionClaim,
            commit: crate::AiProviderSessionCommit,
        ) -> Result<crate::AiProviderSessionBindingView, AiError> {
            if self.run.final_states() != [AiRunState::Completed] {
                return Err(AiError::Conflict);
            }
            self.commits.fetch_add(1, Ordering::SeqCst);
            if self.fail_commit {
                return Err(AiError::PersistenceFailed);
            }
            Ok(crate::AiProviderSessionBindingView {
                binding_id: claim.binding_id(),
                session_id: claim.session_id(),
                scope: test_scope(),
                descriptor: claim.descriptor().clone(),
                state: crate::AiProviderSessionState::Active,
                through_message_sequence: commit.through_message_sequence(),
                transcript_fingerprint: commit.transcript_fingerprint().to_owned(),
                provider_expires_at: None,
                idle_expires_at: time::OffsetDateTime::now_utc() + Duration::minutes(5),
                absolute_expires_at: time::OffsetDateTime::now_utc() + Duration::hours(1),
                row_version: 2,
            })
        }

        async fn require_cleanup(
            &self,
            _claim: &crate::AiProviderSessionClaim,
            _reason_code: &str,
        ) -> Result<(), AiError> {
            self.cleanups.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn claim_cleanup(
            &self,
            _worker_id: &str,
        ) -> Result<Option<crate::AiProviderSessionCleanupClaim>, AiError> {
            Ok(None)
        }

        async fn open_for_cleanup(
            &self,
            _claim: &crate::AiProviderSessionCleanupClaim,
            _policy: &crate::AiContentProtectionPolicy,
        ) -> Result<crate::AiProviderSessionDeletionRequest, AiError> {
            Err(AiError::Conflict)
        }

        async fn complete_cleanup(
            &self,
            _claim: &crate::AiProviderSessionCleanupClaim,
            _proof: crate::AiProviderSessionAbsenceProof,
        ) -> Result<(), AiError> {
            Err(AiError::Conflict)
        }

        async fn schedule_cleanup_retry(
            &self,
            _claim: &crate::AiProviderSessionCleanupClaim,
            _delay: Duration,
            _reason_code: &str,
        ) -> Result<(), AiError> {
            Err(AiError::Conflict)
        }
    }

    struct TestPlanner {
        scope: AiScope,
        route: AiToolResultEgressRoute,
        continuation_count: AtomicUsize,
    }

    struct TestChatPlanner {
        scope: AiScope,
        continuation_count: AtomicUsize,
    }

    struct TestDynamicPlanner {
        scope: AiScope,
        route: AiToolResultEgressRoute,
        continuation_count: AtomicUsize,
    }

    #[cfg(feature = "provider-codex-app-server")]
    struct CanonicalDynamicPlanner {
        scope: AiScope,
        route: AiToolResultEgressRoute,
        plan: AiProviderCallPlan,
    }

    struct TestRetainedChatPlanner {
        scope: AiScope,
        provider_session: crate::AiProviderSessionTurnPlan,
    }

    #[async_trait]
    impl AiReadOnlyAgentTurnPlanner for TestPlanner {
        async fn initial_plan(
            &self,
            lease: &AiRunLease,
        ) -> Result<AiReadOnlyAgentTurnPlan, AiError> {
            AiReadOnlyAgentTurnPlan::new(
                AiProviderCallPlan::test_plan(lease, self.scope.clone(), false),
                self.route.clone(),
                test_rules(self.scope.clone()),
                false,
            )
        }

        async fn continuation_plan(
            &self,
            lease: &AiRunLease,
            _provider_turns: u32,
            _continuation: AiAgentContinuation,
        ) -> Result<AiReadOnlyAgentTurnPlan, AiError> {
            self.continuation_count.fetch_add(1, Ordering::SeqCst);
            AiReadOnlyAgentTurnPlan::new(
                AiProviderCallPlan::test_plan(lease, self.scope.clone(), true),
                self.route.clone(),
                test_rules(self.scope.clone()),
                false,
            )
        }
    }

    #[async_trait]
    impl AiReadOnlyAgentTurnPlanner for TestChatPlanner {
        async fn initial_plan(
            &self,
            lease: &AiRunLease,
        ) -> Result<AiReadOnlyAgentTurnPlan, AiError> {
            AiReadOnlyAgentTurnPlan::new_chat(
                AiProviderCallPlan::test_chat_plan(lease, self.scope.clone()),
                test_rules(self.scope.clone()),
                false,
            )
        }

        async fn continuation_plan(
            &self,
            _lease: &AiRunLease,
            _provider_turns: u32,
            _continuation: AiAgentContinuation,
        ) -> Result<AiReadOnlyAgentTurnPlan, AiError> {
            self.continuation_count.fetch_add(1, Ordering::SeqCst);
            Err(AiError::Conflict)
        }
    }

    #[async_trait]
    impl AiReadOnlyAgentTurnPlanner for TestDynamicPlanner {
        async fn initial_plan(
            &self,
            lease: &AiRunLease,
        ) -> Result<AiReadOnlyAgentTurnPlan, AiError> {
            AiReadOnlyAgentTurnPlan::new_experimental_dynamic_tools(
                AiProviderCallPlan::test_plan(lease, self.scope.clone(), false),
                self.route.clone(),
                test_rules(self.scope.clone()),
                false,
            )
        }

        async fn continuation_plan(
            &self,
            _lease: &AiRunLease,
            _provider_turns: u32,
            _continuation: AiAgentContinuation,
        ) -> Result<AiReadOnlyAgentTurnPlan, AiError> {
            self.continuation_count.fetch_add(1, Ordering::SeqCst);
            Err(AiError::Conflict)
        }
    }

    #[cfg(feature = "provider-codex-app-server")]
    #[async_trait]
    impl AiReadOnlyAgentTurnPlanner for CanonicalDynamicPlanner {
        async fn initial_plan(
            &self,
            _lease: &AiRunLease,
        ) -> Result<AiReadOnlyAgentTurnPlan, AiError> {
            AiReadOnlyAgentTurnPlan::new_experimental_dynamic_tools(
                self.plan.clone(),
                self.route.clone(),
                test_rules(self.scope.clone()),
                false,
            )
        }

        async fn continuation_plan(
            &self,
            _lease: &AiRunLease,
            _provider_turns: u32,
            _continuation: AiAgentContinuation,
        ) -> Result<AiReadOnlyAgentTurnPlan, AiError> {
            Err(AiError::Conflict)
        }
    }

    #[async_trait]
    impl AiReadOnlyAgentTurnPlanner for TestRetainedChatPlanner {
        async fn initial_plan(
            &self,
            lease: &AiRunLease,
        ) -> Result<AiReadOnlyAgentTurnPlan, AiError> {
            AiReadOnlyAgentTurnPlan::new_chat(
                AiProviderCallPlan::test_chat_plan(lease, self.scope.clone()),
                test_rules(self.scope.clone()),
                false,
            )?
            .with_provider_session(self.provider_session.clone())
        }

        async fn continuation_plan(
            &self,
            _lease: &AiRunLease,
            _provider_turns: u32,
            _continuation: AiAgentContinuation,
        ) -> Result<AiReadOnlyAgentTurnPlan, AiError> {
            Err(AiError::Conflict)
        }
    }

    struct InvalidContinuationPlanner {
        scope: AiScope,
        route: AiToolResultEgressRoute,
    }

    struct AdoptionOnlyPlanner {
        scope: AiScope,
        route: AiToolResultEgressRoute,
        continuation_count: AtomicUsize,
    }

    #[async_trait]
    impl AiReadOnlyAgentTurnPlanner for AdoptionOnlyPlanner {
        async fn initial_plan(
            &self,
            _lease: &AiRunLease,
        ) -> Result<AiReadOnlyAgentTurnPlan, AiError> {
            Err(AiError::Conflict)
        }

        async fn continuation_plan(
            &self,
            lease: &AiRunLease,
            _provider_turns: u32,
            _continuation: AiAgentContinuation,
        ) -> Result<AiReadOnlyAgentTurnPlan, AiError> {
            self.continuation_count.fetch_add(1, Ordering::SeqCst);
            AiReadOnlyAgentTurnPlan::new(
                AiProviderCallPlan::test_plan(lease, self.scope.clone(), true),
                self.route.clone(),
                test_rules(self.scope.clone()),
                false,
            )
        }
    }

    struct TestCheckpointAdopter {
        adopted: Mutex<Option<AiAdoptedReadOnlyToolBatch>>,
        consumed: AtomicBool,
    }

    #[async_trait]
    impl AiAgentCheckpointAdopter for TestCheckpointAdopter {
        async fn adopt_tool_batch(
            &self,
            lease: &AiRunLease,
        ) -> Result<Option<AiAdoptedReadOnlyToolBatch>, AiError> {
            let adopted = self
                .adopted
                .lock()
                .expect("test adoption lock should not be poisoned")
                .take();
            if adopted
                .as_ref()
                .map(AiAdoptedReadOnlyToolBatch::checkpoint_id)
                != lease.latest_checkpoint_id()
            {
                return Err(AiError::Conflict);
            }
            Ok(adopted)
        }

        async fn consume_before_provider(
            &self,
            lease: &AiRunLease,
            checkpoint_id: Uuid,
        ) -> Result<AiRunLease, AiError> {
            if lease.latest_checkpoint_id() != Some(checkpoint_id)
                || self.consumed.swap(true, Ordering::SeqCst)
            {
                return Err(AiError::Conflict);
            }
            Ok(lease.test_without_checkpoint())
        }
    }

    struct CheckpointClearedProvider {
        response: Mutex<Option<AiProviderCallResult>>,
    }

    #[async_trait]
    impl AiAgentProviderTurnExecutor for CheckpointClearedProvider {
        async fn execute_turn(
            &self,
            lease: &AiRunLease,
            _plan: AiProviderCallPlan,
        ) -> Result<AiProviderCallResult, AiError> {
            if lease.latest_checkpoint_id().is_some() {
                return Err(AiError::Conflict);
            }
            self.response
                .lock()
                .expect("test response lock should not be poisoned")
                .take()
                .ok_or(AiError::Conflict)
        }
    }

    #[async_trait]
    impl AiReadOnlyAgentTurnPlanner for InvalidContinuationPlanner {
        async fn initial_plan(
            &self,
            lease: &AiRunLease,
        ) -> Result<AiReadOnlyAgentTurnPlan, AiError> {
            AiReadOnlyAgentTurnPlan::new(
                AiProviderCallPlan::test_plan(lease, self.scope.clone(), false),
                self.route.clone(),
                test_rules(self.scope.clone()),
                false,
            )
        }

        async fn continuation_plan(
            &self,
            lease: &AiRunLease,
            _provider_turns: u32,
            _continuation: AiAgentContinuation,
        ) -> Result<AiReadOnlyAgentTurnPlan, AiError> {
            AiReadOnlyAgentTurnPlan::new(
                AiProviderCallPlan::test_plan(lease, self.scope.clone(), false),
                self.route.clone(),
                test_rules(self.scope.clone()),
                false,
            )
        }
    }

    struct TestToolExecutor {
        expose_result: bool,
    }

    #[derive(Default)]
    struct ChatForbiddenBoundaries {
        tool_calls: AtomicUsize,
        provider_checkpoints: AtomicUsize,
        tool_batch_checkpoints: AtomicUsize,
    }

    #[async_trait]
    impl AiAgentReadOnlyToolExecutor for ChatForbiddenBoundaries {
        async fn execute_tool(
            &self,
            _lease: &AiRunLease,
            _provider_result: &AiProviderCallResult,
            _context: AiApplicationToolCallContext,
            _route: AiToolResultEgressRoute,
        ) -> Result<AiPersistedApplicationToolCall, AiError> {
            self.tool_calls.fetch_add(1, Ordering::SeqCst);
            Err(AiError::Forbidden)
        }
    }

    #[async_trait]
    impl AiAgentCheckpointWriter for ChatForbiddenBoundaries {
        async fn persist_provider_turn(
            &self,
            _lease: &AiRunLease,
            _result: &AiProviderCallResult,
            _scope: &AiScope,
            _correlation_id: &str,
            _route: &AiToolResultEgressRoute,
            _rules: &AiResolvedRuleSet,
            _rule_usage: AiRuleRunUsage,
            _provider_turns: u32,
            _total_tool_calls: u32,
        ) -> Result<AiRunLease, AiError> {
            self.provider_checkpoints.fetch_add(1, Ordering::SeqCst);
            Err(AiError::Forbidden)
        }

        async fn persist_tool_batch(
            &self,
            _lease: &AiRunLease,
            _result: &AiProviderCallResult,
            _completed_tools: &[AiPersistedApplicationToolCall],
            _continuation: &AiAgentContinuation,
            _scope: &AiScope,
            _correlation_id: &str,
            _route: &AiToolResultEgressRoute,
            _rules: &AiResolvedRuleSet,
            _rule_usage: AiRuleRunUsage,
            _provider_turns: u32,
            _total_tool_calls: u32,
        ) -> Result<AiRunLease, AiError> {
            self.tool_batch_checkpoints.fetch_add(1, Ordering::SeqCst);
            Err(AiError::Forbidden)
        }
    }

    #[async_trait]
    impl AiAgentReadOnlyToolExecutor for TestToolExecutor {
        async fn execute_tool(
            &self,
            lease: &AiRunLease,
            provider_result: &AiProviderCallResult,
            _context: AiApplicationToolCallContext,
            _route: AiToolResultEgressRoute,
        ) -> Result<AiPersistedApplicationToolCall, AiError> {
            let call = provider_result
                .tool_calls()
                .first()
                .expect("test tool call should exist");
            let manifest = self
                .expose_result
                .then(|| test_manifest(lease, AiEgressCapability::ToolResult));
            Ok(AiPersistedApplicationToolCall::test_completed(
                lease.clone(),
                call.call_id(),
                call.tool_id().as_str(),
                self.expose_result.then(|| json!({"record": "safe"})),
                manifest,
            ))
        }
    }

    struct TestOutputWriter;

    #[async_trait]
    impl AiAgentProviderOutputWriter for TestOutputWriter {
        async fn persist_output(
            &self,
            lease: &AiRunLease,
            _result: &AiProviderCallResult,
        ) -> Result<AiPersistedProviderOutput, AiError> {
            Ok(AiPersistedProviderOutput::test_output(lease.clone()))
        }
    }

    struct CancellingOutputWriter {
        run: Arc<TestRunControl>,
    }

    #[async_trait]
    impl AiAgentProviderOutputWriter for CancellingOutputWriter {
        async fn persist_output(
            &self,
            lease: &AiRunLease,
            _result: &AiProviderCallResult,
        ) -> Result<AiPersistedProviderOutput, AiError> {
            self.run.cancelled.store(true, Ordering::SeqCst);
            Ok(AiPersistedProviderOutput::test_output(lease.clone()))
        }
    }

    struct TestCheckpointWriter;

    #[async_trait]
    impl AiAgentCheckpointWriter for TestCheckpointWriter {
        async fn persist_provider_turn(
            &self,
            lease: &AiRunLease,
            result: &AiProviderCallResult,
            _scope: &AiScope,
            _correlation_id: &str,
            _route: &AiToolResultEgressRoute,
            _rules: &AiResolvedRuleSet,
            _rule_usage: AiRuleRunUsage,
            provider_turns: u32,
            _total_tool_calls: u32,
        ) -> Result<AiRunLease, AiError> {
            if provider_turns == 0 || result.run_id() != lease.run_id() {
                return Err(AiError::Conflict);
            }
            Ok(lease.test_with_checkpoint(Uuid::new_v4()))
        }

        async fn persist_tool_batch(
            &self,
            lease: &AiRunLease,
            _result: &AiProviderCallResult,
            completed_tools: &[AiPersistedApplicationToolCall],
            _continuation: &AiAgentContinuation,
            _scope: &AiScope,
            _correlation_id: &str,
            _route: &AiToolResultEgressRoute,
            _rules: &AiResolvedRuleSet,
            _rule_usage: AiRuleRunUsage,
            _provider_turns: u32,
            total_tool_calls: u32,
        ) -> Result<AiRunLease, AiError> {
            if completed_tools.is_empty()
                || usize::try_from(total_tool_calls)
                    .map_or(true, |total| total < completed_tools.len())
            {
                return Err(AiError::Conflict);
            }
            Ok(lease.test_with_checkpoint(Uuid::new_v4()))
        }
    }

    #[async_trait]
    impl AiAgentCheckpointAdopter for TestCheckpointWriter {
        async fn adopt_tool_batch(
            &self,
            _lease: &AiRunLease,
        ) -> Result<Option<AiAdoptedReadOnlyToolBatch>, AiError> {
            Ok(None)
        }

        async fn consume_before_provider(
            &self,
            lease: &AiRunLease,
            checkpoint_id: Uuid,
        ) -> Result<AiRunLease, AiError> {
            if lease.latest_checkpoint_id() != Some(checkpoint_id) {
                return Err(AiError::Conflict);
            }
            Ok(lease.test_without_checkpoint())
        }
    }

    struct RejectProviderCheckpoint;

    #[async_trait]
    impl AiAgentCheckpointWriter for RejectProviderCheckpoint {
        async fn persist_provider_turn(
            &self,
            _lease: &AiRunLease,
            _result: &AiProviderCallResult,
            _scope: &AiScope,
            _correlation_id: &str,
            _route: &AiToolResultEgressRoute,
            _rules: &AiResolvedRuleSet,
            _rule_usage: AiRuleRunUsage,
            _provider_turns: u32,
            _total_tool_calls: u32,
        ) -> Result<AiRunLease, AiError> {
            Err(AiError::PersistenceFailed)
        }

        async fn persist_tool_batch(
            &self,
            _lease: &AiRunLease,
            _result: &AiProviderCallResult,
            _completed_tools: &[AiPersistedApplicationToolCall],
            _continuation: &AiAgentContinuation,
            _scope: &AiScope,
            _correlation_id: &str,
            _route: &AiToolResultEgressRoute,
            _rules: &AiResolvedRuleSet,
            _rule_usage: AiRuleRunUsage,
            _provider_turns: u32,
            _total_tool_calls: u32,
        ) -> Result<AiRunLease, AiError> {
            Err(AiError::Conflict)
        }
    }

    fn principal_reference() -> PrincipalReference {
        AuthPrincipal::User(AuthUser {
            user_id: "coordinator-user".to_owned(),
            session_id: Uuid::new_v4(),
            roles: Vec::new(),
            scopes: Vec::new(),
            session: SessionContext::default(),
            token_claims: AccessTokenMetadata {
                tenant_id: Some("coordinator-tenant".to_owned()),
                ..AccessTokenMetadata::default()
            },
        })
        .reference()
    }

    fn test_scope() -> AiScope {
        AiScope::new("test", "coordinator-scope").with_tenant_id("coordinator-tenant")
    }

    fn test_rules(scope: AiScope) -> AiResolvedRuleSet {
        test_rules_with_fingerprint(scope, '1')
    }

    fn test_rules_with_fingerprint(scope: AiScope, fingerprint: char) -> AiResolvedRuleSet {
        let applied_layers = vec![crate::AiAppliedRuleLayer {
            scope: scope.clone(),
            row_version: i64::from(
                fingerprint
                    .to_digit(16)
                    .expect("test fingerprint marker should be hexadecimal"),
            ),
        }];
        AiResolvedRuleSet::new(
            scope,
            crate::AiRuleConstraints {
                enabled: true,
                maximum_classification: DataClassification::Restricted,
                maximum_tool_maturity: crate::ToolMaturity::ReadOnly,
                approval_requirement: crate::AiRuleApprovalRequirement::DescriptorPolicy,
                allowed_tool_fingerprints: None,
                allowed_provider_kinds: None,
                allowed_provider_capabilities: None,
                allow_provider_retention: true,
                allow_byok: true,
                budget: crate::AiRuleBudgetCeilings {
                    maximum_steps: Some(16),
                    maximum_duration_seconds: Some(3_600),
                    maximum_output_tokens: Some(16_000),
                    maximum_cost_microunits: Some(10_000_000),
                    maximum_provider_calls: Some(8),
                    maximum_tool_units: Some(100),
                    maximum_web_search_calls: Some(4),
                    maximum_image_units: Some(100),
                },
            },
            applied_layers,
        )
        .expect("test rules should validate")
    }

    struct TestRuleResolver;

    #[async_trait]
    impl AiAgentRuleResolver for TestRuleResolver {
        async fn resolve_rules(
            &self,
            _lease: &AiRunLease,
            scope: &AiScope,
        ) -> Result<AiAgentRuleResolution, AiError> {
            AiAgentRuleResolution::new(test_rules(scope.clone()), time::OffsetDateTime::now_utc())
        }
    }

    struct ChangingRuleResolver(AtomicUsize);

    #[async_trait]
    impl AiAgentRuleResolver for ChangingRuleResolver {
        async fn resolve_rules(
            &self,
            _lease: &AiRunLease,
            scope: &AiScope,
        ) -> Result<AiAgentRuleResolution, AiError> {
            let call = self.0.fetch_add(1, Ordering::SeqCst);
            AiAgentRuleResolution::new(
                test_rules_with_fingerprint(scope.clone(), if call == 0 { '1' } else { '4' }),
                time::OffsetDateTime::now_utc(),
            )
        }
    }

    fn test_route() -> AiToolResultEgressRoute {
        AiToolResultEgressRoute::new(
            "test-provider-profile",
            "test-provider-boundary",
            AiDestinationTrust::ExternalProcessor,
            "agent-test",
            "none",
            "egress-v1",
        )
        .expect("test route should validate")
    }

    fn retained_descriptor() -> crate::AiProviderSessionDescriptor {
        crate::AiProviderSessionDescriptor::new(
            crate::ProviderKind::OpenAi,
            "coordinator-test-profile",
            "coordinator-test-model",
            "a".repeat(64),
            "test-provider-retained/v1",
            "b".repeat(64),
        )
        .expect("retained test descriptor should validate")
    }

    fn retained_claim(
        lease: &AiRunLease,
        descriptor: crate::AiProviderSessionDescriptor,
    ) -> crate::AiProviderSessionClaim {
        crate::AiProviderSessionClaim {
            binding_id: Uuid::new_v4(),
            session_id: lease.session_id(),
            run_id: lease.run_id(),
            attempt_id: lease.attempt_id(),
            run_lease_generation: lease.lease_generation(),
            binding_claim_generation: 1,
            binding_row_version: 1,
            claim_expires_at: time::OffsetDateTime::now_utc() + Duration::minutes(5),
            through_message_sequence: 0,
            transcript_fingerprint: "c".repeat(64),
            principal_reference: lease.principal_reference().clone(),
            descriptor,
        }
    }

    fn test_manifest(lease: &AiRunLease, capability: AiEgressCapability) -> AiEgressManifest {
        AiEgressManifest {
            provider_profile_id: "test-provider-profile".to_owned(),
            provider_kind: "openai".to_owned(),
            model: "coordinator-test-model".to_owned(),
            destination: "test-provider-boundary".to_owned(),
            destination_trust: AiDestinationTrust::ExternalProcessor,
            capability,
            scope: test_scope(),
            session_id: Some(lease.session_id()),
            run_id: Some(lease.run_id()),
            sources: vec![AiDataSourceRef {
                kind: "test".to_owned(),
                reference: "test-result".to_owned(),
                classification: DataClassification::Public,
                trust: AiSourceTrust::ResolverResult,
            }],
            estimated_bytes: 64,
            estimated_tokens: 0,
            attachment_count: 0,
            purpose: "agent-test".to_owned(),
            retention: "none".to_owned(),
            residency: None,
            policy_version: "egress-v1".to_owned(),
            consent_reference: None,
        }
    }

    #[cfg(feature = "provider-codex-app-server")]
    fn canonical_dynamic_plan(
        lease: &AiRunLease,
    ) -> (AiProviderCallPlan, crate::ModelToolDefinition) {
        let (catalog, definition) = crate::providers::canonical_dynamic_tool_catalog();
        let tool_id = crate::AiToolId::parse(definition.tool_id.clone())
            .expect("canonical generated tool ID should validate");
        let descriptor = catalog
            .descriptor(&tool_id)
            .expect("canonical generated descriptor should be registered");
        let mut policy = crate::AiToolPolicySet::new(crate::ToolMaturity::ReadOnly);
        policy.bind(crate::AiToolPolicyBinding {
            tool_id,
            fingerprint: descriptor.fingerprint.clone(),
            enabled: true,
        });
        let scope = test_scope();
        let request = crate::ModelRequest {
            model: "model-1".to_owned(),
            instructions: Vec::new(),
            input: vec![crate::ModelInputBlock::Text {
                text: "Use inventory_count with Limit 3 and return the count.".to_owned(),
            }],
            continuation: None,
            continuation_mode: crate::ModelContinuationMode::ProviderRetained,
            tools: vec![definition.clone()],
            builtin_tools: Vec::new(),
            maximum_builtin_tool_calls: None,
            reasoning_summary: crate::ModelReasoningSummaryRequest::Disabled,
            reasoning_effort: crate::ModelReasoningEffort::Unspecified,
            output_schema: None,
            maximum_output_tokens: Some(128),
        };
        let budget = crate::AiBudgetReservationRequest {
            scope: scope.clone(),
            session_id: lease.session_id(),
            run_id: lease.run_id(),
            attempt_id: lease.attempt_id(),
            lease_generation: lease.lease_generation(),
            provider_kind: crate::ProviderKind::LocalHarness,
            model: request.model.clone(),
            reasoning_effort: request.reasoning_effort,
            pricing_policy_version: "canonical-codex-v1".to_owned(),
            estimate: crate::AiBudgetAmounts {
                output_tokens: 128,
                runs: 1,
                tool_units: 1,
                ..crate::AiBudgetAmounts::default()
            },
            idempotency_key: Uuid::new_v4().to_string(),
            expires_at: time::OffsetDateTime::now_utc() + Duration::minutes(5),
        };
        let manifest = AiEgressManifest {
            provider_profile_id: "canonical-codex-profile".to_owned(),
            provider_kind: crate::ProviderKind::LocalHarness.as_str().to_owned(),
            model: request.model.clone(),
            destination: "sandboxed-local-harness".to_owned(),
            destination_trust: AiDestinationTrust::Local,
            capability: AiEgressCapability::ModelInference,
            scope,
            session_id: Some(lease.session_id()),
            run_id: Some(lease.run_id()),
            sources: vec![AiDataSourceRef {
                kind: "user_message".to_owned(),
                reference: "canonical-generated-tool-test".to_owned(),
                classification: DataClassification::Internal,
                trust: AiSourceTrust::UserProvided,
            }],
            estimated_bytes: request.conservative_egress_bytes(),
            estimated_tokens: 64,
            attachment_count: 0,
            purpose: "answer-with-registered-tool".to_owned(),
            retention: "provider-session".to_owned(),
            residency: None,
            policy_version: "canonical-egress-v1".to_owned(),
            consent_reference: None,
        };
        let plan = AiProviderCallPlan::new_with_tools(
            crate::ProviderKind::LocalHarness,
            request,
            budget,
            vec![manifest],
            "canonical-generated-dynamic-turn",
            &catalog,
            &policy,
        )
        .expect("canonical generated dynamic plan should validate");
        (plan, definition)
    }

    fn adopted_read_only_checkpoint(
        lease: &AiRunLease,
        checkpoint_id: Uuid,
    ) -> AiAdoptedReadOnlyToolBatch {
        let result = AiProviderCallResult::test_result(
            lease,
            None,
            "adopted-response",
            vec![("adopted-call", "test.read", json!({}))],
        );
        let persisted = AiPersistedApplicationToolCall::test_completed(
            lease.clone(),
            "adopted-call",
            "test.read",
            Some(json!({"record": "safe"})),
            Some(test_manifest(lease, AiEgressCapability::ToolResult)),
        );
        let continuation = AiAgentContinuation::from_persisted_results(
            ModelContinuation::ProviderResponse {
                response_id: "adopted-response".to_owned(),
            },
            crate::ModelReasoningEffort::Unspecified,
            &[persisted],
            Vec::new(),
        )
        .expect("test continuation should bind");
        let resolution =
            AiAgentRuleResolution::new(test_rules(test_scope()), time::OffsetDateTime::now_utc())
                .expect("test rules should resolve");
        let rule_usage = AiRuleRunUsage::default()
            .accept_provider_with_web_searches(result.usage(), 0, &resolution)
            .and_then(|usage| usage.accept_tool_calls(1, &resolution))
            .expect("adopted usage should fit test rules");
        AiAdoptedReadOnlyToolBatch::new(
            checkpoint_id,
            1,
            1,
            test_scope(),
            continuation,
            resolution.rules().fingerprint().to_owned(),
            rule_usage,
        )
    }

    fn limits(heartbeat_millis: i64) -> AiReadOnlyAgentCoordinatorLimits {
        limits_with_provider_turns(heartbeat_millis, 4)
    }

    fn limits_with_provider_turns(
        heartbeat_millis: i64,
        maximum_provider_turns: u32,
    ) -> AiReadOnlyAgentCoordinatorLimits {
        AiReadOnlyAgentCoordinatorLimits::new(
            AiAgentLoopLimits::new(maximum_provider_turns, 8)
                .expect("test loop limits should validate"),
            Duration::milliseconds(heartbeat_millis),
        )
        .expect("test coordinator limits should validate")
    }

    fn coordinator(
        run: Arc<TestRunControl>,
        provider: Arc<TestProviderExecutor>,
        planner: Arc<TestPlanner>,
        expose_tool_result: bool,
        coordinator_limits: AiReadOnlyAgentCoordinatorLimits,
    ) -> AiReadOnlyAgentCoordinator {
        AiReadOnlyAgentCoordinator::new(
            run,
            provider,
            Arc::new(TestToolExecutor {
                expose_result: expose_tool_result,
            }),
            Arc::new(TestOutputWriter),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            planner,
            coordinator_limits,
        )
    }

    #[test]
    fn chat_turn_factory_accepts_only_exact_initial_tool_free_plans() {
        let lease = AiRunLease::test_running(principal_reference());
        let scope = test_scope();
        let plan = AiReadOnlyAgentTurnPlan::new_chat(
            AiProviderCallPlan::test_chat_plan(&lease, scope.clone()),
            test_rules(scope.clone()),
            false,
        )
        .expect("an exact initial tool-free plan should validate");
        let (_, _, _, mode, _, _, _, _) = plan.into_parts();
        assert!(matches!(mode, AiReadOnlyAgentTurnMode::ChatOnly));

        assert!(matches!(
            AiReadOnlyAgentTurnPlan::new_chat(
                AiProviderCallPlan::test_plan(&lease, scope.clone(), false),
                test_rules(scope.clone()),
                false,
            ),
            Err(AiError::InvalidInput(_))
        ));
        assert!(matches!(
            AiReadOnlyAgentTurnPlan::new_chat(
                AiProviderCallPlan::test_builtin_plan(&lease, scope.clone()),
                test_rules(scope.clone()),
                false,
            ),
            Err(AiError::InvalidInput(_))
        ));
        assert!(matches!(
            AiReadOnlyAgentTurnPlan::new_chat(
                AiProviderCallPlan::test_chat_continuation_plan(&lease, scope.clone()),
                test_rules(scope.clone()),
                false,
            ),
            Err(AiError::InvalidInput(_))
        ));
        assert!(matches!(
            AiReadOnlyAgentTurnPlan::new_chat(
                AiProviderCallPlan::test_chat_plan(&lease, scope),
                test_rules(AiScope::new("test", "different-scope")),
                false,
            ),
            Err(AiError::InvalidInput(_))
        ));
    }

    #[test]
    fn retained_dynamic_tool_factory_accepts_validated_provider_builtins() {
        let lease = AiRunLease::test_running(principal_reference());
        let scope = test_scope();
        let provider_call = AiProviderCallPlan::test_dynamic_builtin_plan(&lease, scope.clone());

        let plan = AiReadOnlyAgentTurnPlan::new_experimental_dynamic_tools(
            provider_call,
            test_route(),
            test_rules(scope),
            false,
        )
        .expect("a validated hosted built-in may coexist with retained dynamic tools");
        let (_, _, _, mode, _, _, _, _) = plan.into_parts();
        assert!(matches!(
            mode,
            AiReadOnlyAgentTurnMode::ExperimentalDynamicTools(_)
        ));
    }

    #[tokio::test]
    async fn pre_transport_budget_denial_fails_cleanly_instead_of_requiring_recovery() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Err(AiError::PreTransportBudgetDenied)])),
            delay: None,
        });
        let planner = Arc::new(TestChatPlanner {
            scope: test_scope(),
            continuation_count: AtomicUsize::new(0),
        });
        let forbidden = Arc::new(ChatForbiddenBoundaries::default());
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            forbidden.clone(),
            Arc::new(TestOutputWriter),
            forbidden.clone(),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            planner,
            limits(50),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("a budget denial is a clean terminal failure");

        assert!(matches!(
            outcome,
            Failed {
                provider_turns: 0,
                total_tool_calls: 0,
            }
        ));
        assert_eq!(run.final_states(), vec![AiRunState::Failed]);
        assert_eq!(run.final_codes(), vec!["provider_budget_denied".to_owned()]);
        assert!(run.scheduled_retry_codes().is_empty());
        assert_eq!(provider.remaining_responses(), 0);
        assert_eq!(forbidden.tool_calls.load(Ordering::SeqCst), 0);
        assert_eq!(forbidden.provider_checkpoints.load(Ordering::SeqCst), 0);
        // A certain pre-transport refusal is safe to author again once
        // capacity exists, so the terminal-event failure record admits retry.
        assert_eq!(
            crate::classify_run_retry(
                crate::AiRunRetryEvidence {
                    terminal: crate::AiRunTerminalEvent::Failed,
                    produced_assistant_output: false,
                },
                Some("provider_budget_denied"),
            ),
            crate::AiRunRetryAdmission::Allowed
        );
    }

    #[tokio::test]
    async fn pre_transport_provider_rejection_fails_cleanly_and_is_retryable() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Err(AiError::PreTransportProviderFailed)])),
            delay: None,
        });
        let planner = Arc::new(TestChatPlanner {
            scope: test_scope(),
            continuation_count: AtomicUsize::new(0),
        });
        let forbidden = Arc::new(ChatForbiddenBoundaries::default());
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            forbidden.clone(),
            Arc::new(TestOutputWriter),
            forbidden.clone(),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            planner,
            limits(50),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("a proven pre-transport rejection is a clean terminal failure");

        assert_eq!(
            outcome,
            Failed {
                provider_turns: 0,
                total_tool_calls: 0,
            }
        );
        assert_eq!(run.final_states(), vec![AiRunState::Failed]);
        assert_eq!(
            run.final_codes(),
            vec!["provider_pre_transport_failed".to_owned()]
        );
        assert!(run.scheduled_retry_codes().is_empty());
        assert_eq!(provider.remaining_responses(), 0);
        assert_eq!(forbidden.tool_calls.load(Ordering::SeqCst), 0);
        assert_eq!(forbidden.provider_checkpoints.load(Ordering::SeqCst), 0);
        assert_eq!(
            crate::classify_run_retry(
                crate::AiRunRetryEvidence {
                    terminal: crate::AiRunTerminalEvent::Failed,
                    produced_assistant_output: false,
                },
                Some("provider_pre_transport_failed"),
            ),
            crate::AiRunRetryAdmission::Allowed
        );
    }

    #[tokio::test]
    async fn metered_stateless_native_item_refusal_is_failed_and_retryable() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Err(AiError::StatelessNativeItemRejected)])),
            delay: None,
        });
        let planner = Arc::new(TestChatPlanner {
            scope: test_scope(),
            continuation_count: AtomicUsize::new(0),
        });
        let forbidden = Arc::new(ChatForbiddenBoundaries::default());
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            forbidden.clone(),
            Arc::new(TestOutputWriter),
            forbidden.clone(),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            planner,
            limits(50),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("a proof-bearing native-item refusal should fail cleanly");

        assert_eq!(
            outcome,
            Failed {
                provider_turns: 0,
                total_tool_calls: 0,
            }
        );
        assert_eq!(run.final_states(), vec![AiRunState::Failed]);
        assert_eq!(
            run.final_codes(),
            vec!["provider_native_item_rejected".to_owned()]
        );
        assert_eq!(provider.remaining_responses(), 0);
        assert_eq!(forbidden.tool_calls.load(Ordering::SeqCst), 0);
        assert_eq!(forbidden.provider_checkpoints.load(Ordering::SeqCst), 0);
        assert_eq!(
            crate::classify_run_retry(
                crate::AiRunRetryEvidence {
                    terminal: crate::AiRunTerminalEvent::Failed,
                    produced_assistant_output: false,
                },
                Some("provider_native_item_rejected"),
            ),
            crate::AiRunRetryAdmission::Allowed
        );
    }

    #[tokio::test]
    async fn generic_budget_denial_cannot_claim_pre_transport_certainty() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Err(AiError::BudgetDenied)])),
            delay: None,
        });
        let planner = Arc::new(TestChatPlanner {
            scope: test_scope(),
            continuation_count: AtomicUsize::new(0),
        });
        let forbidden = Arc::new(ChatForbiddenBoundaries::default());
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider,
            forbidden.clone(),
            Arc::new(TestOutputWriter),
            forbidden.clone(),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            planner,
            limits(50),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("a generic denial must preserve possible transport uncertainty");

        assert!(matches!(
            outcome,
            RecoveryRequired {
                phase: AiAgentRecoveryPhase::ProviderTurn,
                provider_turns: 0,
                total_tool_calls: 0,
            }
        ));
        assert_eq!(run.final_states(), vec![AiRunState::RecoveryRequired]);
        assert_eq!(
            run.final_codes(),
            vec!["provider_turn_uncertain".to_owned()]
        );
    }

    #[tokio::test]
    async fn chat_turn_persists_final_output_without_tool_boundaries() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(AiProviderCallResult::test_result(
                &lease,
                None,
                "response-chat-final",
                Vec::new(),
            ))])),
            delay: None,
        });
        let planner = Arc::new(TestChatPlanner {
            scope: test_scope(),
            continuation_count: AtomicUsize::new(0),
        });
        let forbidden = Arc::new(ChatForbiddenBoundaries::default());
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider,
            forbidden.clone(),
            Arc::new(TestOutputWriter),
            forbidden.clone(),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            planner.clone(),
            limits(50),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("chat-only coordinator turn should complete");

        assert!(matches!(
            outcome,
            Completed {
                provider_turns: 1,
                total_tool_calls: 0,
                ..
            }
        ));
        assert_eq!(forbidden.tool_calls.load(Ordering::SeqCst), 0);
        assert_eq!(forbidden.provider_checkpoints.load(Ordering::SeqCst), 0);
        assert_eq!(forbidden.tool_batch_checkpoints.load(Ordering::SeqCst), 0);
        assert_eq!(planner.continuation_count.load(Ordering::SeqCst), 0);
        assert_eq!(run.final_states(), vec![AiRunState::Completed]);
    }

    #[tokio::test]
    async fn retained_watermark_advances_only_after_terminal_completion_and_failure_quarantines() {
        for fail_commit in [false, true] {
            let lease = AiRunLease::test_running(principal_reference());
            let run = Arc::new(TestRunControl::new());
            let descriptor = retained_descriptor();
            let claim = retained_claim(&lease, descriptor.clone());
            let provider = Arc::new(RetainedTestProviderExecutor {
                result: Mutex::new(Some(AiProviderCallResult::test_result(
                    &lease,
                    None,
                    "response-retained-final",
                    Vec::new(),
                ))),
                claim,
            });
            let planner = Arc::new(TestRetainedChatPlanner {
                scope: test_scope(),
                provider_session: crate::AiProviderSessionTurnPlan::new(descriptor, "c".repeat(64))
                    .expect("retained turn plan should validate"),
            });
            let session_service = Arc::new(TestProviderSessionService {
                run: run.clone(),
                commits: AtomicUsize::new(0),
                cleanups: AtomicUsize::new(0),
                fail_commit,
            });
            let forbidden = Arc::new(ChatForbiddenBoundaries::default());
            let coordinator = AiReadOnlyAgentCoordinator::new(
                run.clone(),
                provider,
                forbidden.clone(),
                Arc::new(TestOutputWriter),
                forbidden.clone(),
                Arc::new(TestCheckpointWriter),
                Arc::new(TestRuleResolver),
                planner,
                limits(50),
            )
            .with_provider_session_service(session_service.clone());

            let outcome = coordinator
                .execute_claimed(&lease)
                .await
                .expect("retained terminal turn should preserve completed output");

            assert!(matches!(outcome, Completed { .. }));
            assert_eq!(run.final_states(), vec![AiRunState::Completed]);
            assert_eq!(session_service.commits.load(Ordering::SeqCst), 1);
            assert_eq!(
                session_service.cleanups.load(Ordering::SeqCst),
                usize::from(fail_commit)
            );
        }
    }

    /// Work item 4.3: a message accepted while provider-session cleanup is
    /// pending must converge without an operator.
    ///
    /// While the run still has retry allowance it becomes durably
    /// retry-scheduled, which is what survives a host restart: `claim_next`
    /// reclaims queued and retry-scheduled runs, so the `Deferred` outcome is a
    /// report rather than the delivery mechanism.
    #[tokio::test]
    async fn cleanup_pending_defers_through_a_durable_retry() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let descriptor = retained_descriptor();
        let planner = Arc::new(TestRetainedChatPlanner {
            scope: test_scope(),
            provider_session: crate::AiProviderSessionTurnPlan::new(descriptor, "c".repeat(64))
                .expect("retained turn plan should validate"),
        });
        let session_service = Arc::new(TestProviderSessionService {
            run: run.clone(),
            commits: AtomicUsize::new(0),
            cleanups: AtomicUsize::new(0),
            fail_commit: false,
        });
        let forbidden = Arc::new(ChatForbiddenBoundaries::default());
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            Arc::new(DeferringRetainedProviderExecutor),
            forbidden.clone(),
            Arc::new(TestOutputWriter),
            forbidden.clone(),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            planner,
            limits(50),
        )
        .with_provider_session_service(session_service);

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("a pending cleanup should defer rather than fail");
        assert!(matches!(
            outcome,
            Deferred {
                reason: AiProviderSessionDeferralReason::CleanupPending,
                ..
            }
        ));
        assert_eq!(
            run.scheduled_retry_codes(),
            vec!["provider_session_cleanup_pending".to_owned()],
            "the durable retry, not the reported outcome, is what converges"
        );
        assert!(
            run.final_states().is_empty(),
            "a deferral must not close the run"
        );
    }

    struct PendingRetainedProviderExecutor {
        settlement: crate::AiRunInterruptSettlement,
        interrupts: AtomicUsize,
        /// Cancellation must land *while* the provider turn is in flight; a run
        /// cancelled before the turn starts never reaches an interrupt.
        run: Arc<TestRunControl>,
    }

    #[async_trait]
    impl AiAgentProviderTurnExecutor for PendingRetainedProviderExecutor {
        async fn execute_turn(
            &self,
            _lease: &AiRunLease,
            _plan: AiProviderCallPlan,
        ) -> Result<AiProviderCallResult, AiError> {
            Err(AiError::Conflict)
        }

        async fn execute_retained_turn(
            &self,
            _lease: Arc<tokio::sync::Mutex<AiRunLease>>,
            _plan: AiProviderCallPlan,
            _session_plan: crate::AiProviderSessionTurnPlan,
            _session_service: Arc<dyn crate::AiProviderSessionService>,
            _execution: Option<Arc<dyn AiProviderDynamicToolExecution>>,
        ) -> Result<AiProviderCallResult, AiError> {
            self.run.cancelled.store(true, Ordering::SeqCst);
            std::future::pending().await
        }

        async fn interrupt_run(
            &self,
            _lease: &AiRunLease,
        ) -> Result<crate::AiRunInterruptSettlement, AiError> {
            self.interrupts.fetch_add(1, Ordering::SeqCst);
            Ok(self.settlement)
        }
    }

    #[derive(Default)]
    struct InterruptSessionService {
        settlements: AtomicUsize,
        run_cleanups: AtomicUsize,
        settlement_fails: bool,
    }

    #[async_trait]
    impl crate::AiProviderSessionService for InterruptSessionService {
        async fn inspect_for_run(
            &self,
            _lease: &AiRunLease,
        ) -> Result<Option<crate::AiProviderSessionBindingView>, AiError> {
            Ok(None)
        }

        async fn bind_for_run(
            &self,
            _lease: &AiRunLease,
            _request: crate::AiProviderSessionBindRequest,
        ) -> Result<crate::AiProviderSessionClaim, AiError> {
            Err(AiError::Conflict)
        }

        async fn claim_for_run(
            &self,
            _lease: &AiRunLease,
            _expected: &crate::AiProviderSessionDescriptor,
            _expected_transcript_fingerprint: &str,
        ) -> Result<crate::AiProviderSessionClaim, AiError> {
            Err(AiError::Conflict)
        }

        async fn open_for_run(
            &self,
            _lease: &AiRunLease,
            _claim: &crate::AiProviderSessionClaim,
        ) -> Result<crate::AiOpenedProviderSession, AiError> {
            Err(AiError::Conflict)
        }

        async fn heartbeat(
            &self,
            _lease: &AiRunLease,
            _claim: &crate::AiProviderSessionClaim,
        ) -> Result<crate::AiProviderSessionClaim, AiError> {
            Err(AiError::Conflict)
        }

        async fn commit_turn(
            &self,
            _lease: &AiRunLease,
            _claim: &crate::AiProviderSessionClaim,
            _commit: crate::AiProviderSessionCommit,
        ) -> Result<crate::AiProviderSessionBindingView, AiError> {
            Err(AiError::Conflict)
        }

        async fn settle_interrupted_turn(
            &self,
            lease: &AiRunLease,
            settlement: crate::AiRunInterruptSettlement,
        ) -> Result<crate::AiProviderSessionBindingView, AiError> {
            assert!(
                settlement.retains_thread(),
                "the durable boundary must never be asked to retain an unsettled thread"
            );
            self.settlements.fetch_add(1, Ordering::SeqCst);
            if self.settlement_fails {
                return Err(AiError::Conflict);
            }
            Ok(crate::AiProviderSessionBindingView {
                binding_id: Uuid::from_u128(7),
                session_id: lease.session_id(),
                scope: test_scope(),
                descriptor: retained_descriptor(),
                state: crate::AiProviderSessionState::Active,
                through_message_sequence: 1,
                transcript_fingerprint: "d".repeat(64),
                provider_expires_at: None,
                idle_expires_at: time::OffsetDateTime::now_utc() + Duration::minutes(5),
                absolute_expires_at: time::OffsetDateTime::now_utc() + Duration::hours(1),
                row_version: 2,
            })
        }

        async fn require_cleanup(
            &self,
            _claim: &crate::AiProviderSessionClaim,
            _reason_code: &str,
        ) -> Result<(), AiError> {
            Err(AiError::Conflict)
        }

        async fn require_cleanup_for_run(
            &self,
            _lease: &AiRunLease,
            reason_code: &str,
        ) -> Result<(), AiError> {
            assert_eq!(reason_code, "provider_session_interrupted_unsettled");
            self.run_cleanups.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn claim_cleanup(
            &self,
            _worker_id: &str,
        ) -> Result<Option<crate::AiProviderSessionCleanupClaim>, AiError> {
            Ok(None)
        }

        async fn open_for_cleanup(
            &self,
            _claim: &crate::AiProviderSessionCleanupClaim,
            _policy: &crate::AiContentProtectionPolicy,
        ) -> Result<crate::AiProviderSessionDeletionRequest, AiError> {
            Err(AiError::Conflict)
        }

        async fn complete_cleanup(
            &self,
            _claim: &crate::AiProviderSessionCleanupClaim,
            _proof: crate::AiProviderSessionAbsenceProof,
        ) -> Result<(), AiError> {
            Err(AiError::Conflict)
        }

        async fn schedule_cleanup_retry(
            &self,
            _claim: &crate::AiProviderSessionCleanupClaim,
            _delay: Duration,
            _reason_code: &str,
        ) -> Result<(), AiError> {
            Err(AiError::Conflict)
        }
    }

    fn interrupted_retained_coordinator(
        settlement: crate::AiRunInterruptSettlement,
        settlement_fails: bool,
    ) -> (
        AiReadOnlyAgentCoordinator,
        Arc<InterruptSessionService>,
        Arc<PendingRetainedProviderExecutor>,
    ) {
        let run = Arc::new(TestRunControl::new());
        let descriptor = retained_descriptor();
        let planner = Arc::new(TestRetainedChatPlanner {
            scope: test_scope(),
            provider_session: crate::AiProviderSessionTurnPlan::new(descriptor, "c".repeat(64))
                .expect("retained turn plan should validate"),
        });
        let provider = Arc::new(PendingRetainedProviderExecutor {
            settlement,
            interrupts: AtomicUsize::new(0),
            run: run.clone(),
        });
        let session_service = Arc::new(InterruptSessionService {
            settlement_fails,
            ..InterruptSessionService::default()
        });
        let forbidden = Arc::new(ChatForbiddenBoundaries::default());
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            forbidden.clone(),
            Arc::new(TestOutputWriter),
            forbidden.clone(),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            planner,
            limits(50),
        )
        .with_provider_session_service(session_service.clone());

        (coordinator, session_service, provider)
    }

    async fn interrupted_retained_run(
        settlement: crate::AiRunInterruptSettlement,
        lease: &AiRunLease,
        settlement_fails: bool,
    ) -> (
        Arc<InterruptSessionService>,
        Arc<PendingRetainedProviderExecutor>,
    ) {
        let (coordinator, session_service, provider) =
            interrupted_retained_coordinator(settlement, settlement_fails);

        let outcome = coordinator
            .execute_claimed(lease)
            .await
            .expect("an interrupted run should close as cancelled");
        assert!(matches!(outcome, Cancelled { .. }));
        assert_eq!(provider.interrupts.load(Ordering::SeqCst), 1);
        (session_service, provider)
    }

    /// A settled interrupt asks the durable boundary to keep the binding and
    /// never routes through the invalidation funnel.
    #[tokio::test]
    async fn settled_interrupt_retains_the_provider_session_binding() {
        let lease = AiRunLease::test_running(principal_reference());
        let (service, _) =
            interrupted_retained_run(crate::AiRunInterruptSettlement::Settled, &lease, false).await;

        assert_eq!(service.settlements.load(Ordering::SeqCst), 1);
        assert_eq!(
            service.run_cleanups.load(Ordering::SeqCst),
            0,
            "a settled interrupt must not invalidate the retained thread"
        );
    }

    /// An acknowledged-but-unsettled interrupt still invalidates through the
    /// disclosed cleanup funnel.
    #[tokio::test]
    async fn unsettled_interrupt_invalidates_the_provider_session_binding() {
        let lease = AiRunLease::test_running(principal_reference());
        let (service, _) = interrupted_retained_run(
            crate::AiRunInterruptSettlement::RequestedUnsettled,
            &lease,
            false,
        )
        .await;

        assert_eq!(service.settlements.load(Ordering::SeqCst), 0);
        assert_eq!(service.run_cleanups.load(Ordering::SeqCst), 1);
    }

    /// Uncertain persisted output demotes an adapter-proven settlement before
    /// the durable boundary is ever asked to retain the binding.
    #[tokio::test]
    async fn uncertain_persisted_output_demotes_a_settled_interrupt() {
        let lease =
            AiRunLease::test_running(principal_reference()).test_with_checkpoint(Uuid::new_v4());
        let (coordinator, service, _) =
            interrupted_retained_coordinator(crate::AiRunInterruptSettlement::Settled, false);
        let guard = AiAgentLoopGuard::new(&lease, limits(50).loop_limits);
        coordinator
            .settle_interrupted_provider_session(
                &lease,
                &guard,
                crate::AiRunInterruptSettlement::Settled,
            )
            .await;

        assert_eq!(service.settlements.load(Ordering::SeqCst), 0);
        assert_eq!(service.run_cleanups.load(Ordering::SeqCst), 1);
    }

    /// A durable boundary that refuses the settlement falls back to the same
    /// disclosed invalidation.
    #[tokio::test]
    async fn refused_durable_settlement_falls_back_to_invalidation() {
        let lease = AiRunLease::test_running(principal_reference());
        let (service, _) =
            interrupted_retained_run(crate::AiRunInterruptSettlement::Settled, &lease, true).await;

        assert_eq!(service.settlements.load(Ordering::SeqCst), 1);
        assert_eq!(service.run_cleanups.load(Ordering::SeqCst), 1);
    }

    /// Work item 4.3: once the bounded retry allowance is exhausted while
    /// cleanup is still pending, the run must close as a visible failure rather
    /// than being left to expire into `RecoveryRequired`, which is both
    /// misclassified and stuck until an operator looks at it.
    #[tokio::test]
    async fn exhausted_cleanup_retries_converge_to_a_clean_visible_failure() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        run.retry_ceiling_reached.store(true, Ordering::SeqCst);
        let descriptor = retained_descriptor();
        let planner = Arc::new(TestRetainedChatPlanner {
            scope: test_scope(),
            provider_session: crate::AiProviderSessionTurnPlan::new(descriptor, "c".repeat(64))
                .expect("retained turn plan should validate"),
        });
        let session_service = Arc::new(TestProviderSessionService {
            run: run.clone(),
            commits: AtomicUsize::new(0),
            cleanups: AtomicUsize::new(0),
            fail_commit: false,
        });
        let forbidden = Arc::new(ChatForbiddenBoundaries::default());
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            Arc::new(DeferringRetainedProviderExecutor),
            forbidden.clone(),
            Arc::new(TestOutputWriter),
            forbidden.clone(),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            planner,
            limits(50),
        )
        .with_provider_session_service(session_service);

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("an exhausted retry allowance should still close the run");
        assert!(matches!(outcome, Failed { .. }));
        assert_eq!(run.final_states(), vec![AiRunState::Failed]);
        assert_eq!(
            run.final_codes(),
            vec!["provider_session_cleanup_unavailable".to_owned()]
        );
        assert!(
            run.scheduled_retry_codes().is_empty(),
            "no retry may be recorded once the allowance is exhausted"
        );
        // Nothing executed on this attempt, so the failure is provably clean
        // and the owner may author a new run for the same message.
        assert_eq!(
            crate::classify_run_retry(
                crate::AiRunRetryEvidence {
                    terminal: crate::AiRunTerminalEvent::Failed,
                    produced_assistant_output: false,
                },
                Some("provider_session_cleanup_unavailable"),
            ),
            crate::AiRunRetryAdmission::Allowed
        );
    }

    #[tokio::test]
    async fn retained_turn_cancelled_after_output_never_advances_and_requires_cleanup() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let descriptor = retained_descriptor();
        let provider = Arc::new(RetainedTestProviderExecutor {
            result: Mutex::new(Some(AiProviderCallResult::test_result(
                &lease,
                None,
                "response-retained-cancelled",
                Vec::new(),
            ))),
            claim: retained_claim(&lease, descriptor.clone()),
        });
        let planner = Arc::new(TestRetainedChatPlanner {
            scope: test_scope(),
            provider_session: crate::AiProviderSessionTurnPlan::new(descriptor, "c".repeat(64))
                .expect("retained turn plan should validate"),
        });
        let session_service = Arc::new(TestProviderSessionService {
            run: run.clone(),
            commits: AtomicUsize::new(0),
            cleanups: AtomicUsize::new(0),
            fail_commit: false,
        });
        let forbidden = Arc::new(ChatForbiddenBoundaries::default());
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider,
            forbidden.clone(),
            Arc::new(CancellingOutputWriter { run: run.clone() }),
            forbidden.clone(),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            planner,
            limits(50),
        )
        .with_provider_session_service(session_service.clone());

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("cancellation should retain its canonical outcome");

        assert!(matches!(outcome, Cancelled { .. }));
        assert!(run.final_states().is_empty());
        assert_eq!(session_service.commits.load(Ordering::SeqCst), 0);
        assert_eq!(session_service.cleanups.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "provider-codex-app-server")]
    #[tokio::test]
    async fn experimental_dynamic_turn_uses_ordinary_tool_boundary_and_no_continuation() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let (plan, definition) = canonical_dynamic_plan(&lease);
        let provider = Arc::new(CanonicalDynamicProviderExecutor { definition });
        let planner = Arc::new(CanonicalDynamicPlanner {
            scope: test_scope(),
            route: AiToolResultEgressRoute::new(
                "canonical-codex-profile",
                "sandboxed-local-harness",
                AiDestinationTrust::Local,
                "answer-with-registered-tool",
                "provider-session",
                "canonical-egress-v1",
            )
            .expect("canonical dynamic route should validate"),
            plan,
        });
        let forbidden_checkpoints = Arc::new(ChatForbiddenBoundaries::default());
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider,
            Arc::new(TestToolExecutor {
                expose_result: true,
            }),
            Arc::new(TestOutputWriter),
            forbidden_checkpoints.clone(),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            planner.clone(),
            limits(50),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("experimental dynamic turn should complete");

        assert!(matches!(
            outcome,
            Completed {
                provider_turns: 1,
                total_tool_calls: 1,
                ..
            }
        ));
        assert_eq!(
            forbidden_checkpoints
                .provider_checkpoints
                .load(Ordering::SeqCst),
            0
        );
        assert_eq!(
            forbidden_checkpoints
                .tool_batch_checkpoints
                .load(Ordering::SeqCst),
            0
        );
        assert_eq!(run.final_states(), vec![AiRunState::Completed]);
    }

    #[tokio::test]
    async fn post_transport_dynamic_tool_limit_remains_provider_uncertainty() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(AiProviderCallResult::test_result(
                &lease,
                None,
                "response-dynamic-tool-limit",
                vec![
                    ("dynamic-call-1", "test.read", json!({})),
                    ("dynamic-call-2", "test.read", json!({})),
                ],
            ))])),
            delay: None,
        });
        let planner = Arc::new(TestDynamicPlanner {
            scope: test_scope(),
            route: test_route(),
            continuation_count: AtomicUsize::new(0),
        });
        let coordinator_limits = AiReadOnlyAgentCoordinatorLimits::new(
            AiAgentLoopLimits::new(4, 1).expect("test loop limits should validate"),
            Duration::milliseconds(50),
        )
        .expect("test coordinator limits should validate");
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider,
            Arc::new(TestToolExecutor {
                expose_result: true,
            }),
            Arc::new(TestOutputWriter),
            Arc::new(ChatForbiddenBoundaries::default()),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            planner,
            coordinator_limits,
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("a limit reached after dispatch must retain uncertainty");

        assert!(matches!(
            outcome,
            RecoveryRequired {
                phase: AiAgentRecoveryPhase::ProviderTurn,
                provider_turns: 0,
                total_tool_calls: 0,
            }
        ));
        assert_eq!(run.final_states(), vec![AiRunState::RecoveryRequired]);
        assert_eq!(
            run.final_codes(),
            vec!["provider_turn_uncertain".to_owned()]
        );
        assert!(
            !run.final_codes()
                .iter()
                .any(|code| code == "provider_budget_denied")
        );
    }

    #[tokio::test]
    async fn experimental_dynamic_turn_denies_rule_change_before_tool_execution() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(AiProviderCallResult::test_result(
                &lease,
                None,
                "response-dynamic-stale-rule",
                vec![("dynamic-call-stale", "test.read", json!({}))],
            ))])),
            delay: None,
        });
        let planner = Arc::new(TestDynamicPlanner {
            scope: test_scope(),
            route: test_route(),
            continuation_count: AtomicUsize::new(0),
        });
        let forbidden = Arc::new(ChatForbiddenBoundaries::default());
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider,
            forbidden.clone(),
            Arc::new(TestOutputWriter),
            forbidden.clone(),
            Arc::new(TestCheckpointWriter),
            Arc::new(ChangingRuleResolver(AtomicUsize::new(0))),
            planner,
            limits(50),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("stale dynamic-tool rules should close for recovery");

        assert!(matches!(
            outcome,
            RecoveryRequired {
                phase: AiAgentRecoveryPhase::ProviderTurn,
                provider_turns: 0,
                total_tool_calls: 0,
            }
        ));
        assert_eq!(forbidden.tool_calls.load(Ordering::SeqCst), 0);
        assert_eq!(run.final_states(), vec![AiRunState::RecoveryRequired]);
    }

    #[tokio::test]
    async fn owner_cancellation_drops_active_provider_and_writes_no_later_output() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(AiProviderCallResult::test_result(
                &lease,
                None,
                "response-cancelled-before-return",
                Vec::new(),
            ))])),
            delay: Some(std::time::Duration::from_secs(5)),
        });
        let planner = Arc::new(TestChatPlanner {
            scope: test_scope(),
            continuation_count: AtomicUsize::new(0),
        });
        let forbidden = Arc::new(ChatForbiddenBoundaries::default());
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            forbidden.clone(),
            Arc::new(TestOutputWriter),
            forbidden.clone(),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            planner.clone(),
            limits(10),
        );
        let cancel_run = run.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(15)).await;
            cancel_run.cancelled.store(true, Ordering::SeqCst);
        });

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("durable cancellation should become the canonical outcome");

        assert!(matches!(
            outcome,
            Cancelled {
                provider_turns: 0,
                total_tool_calls: 0
            }
        ));
        assert_eq!(provider.remaining_responses(), 1);
        assert_eq!(forbidden.tool_calls.load(Ordering::SeqCst), 0);
        assert_eq!(forbidden.provider_checkpoints.load(Ordering::SeqCst), 0);
        assert_eq!(forbidden.tool_batch_checkpoints.load(Ordering::SeqCst), 0);
        assert_eq!(planner.continuation_count.load(Ordering::SeqCst), 0);
        assert!(run.final_states().is_empty());
    }

    #[tokio::test]
    async fn chat_turn_rejects_unoffered_tool_call_without_tool_execution() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(AiProviderCallResult::test_result(
                &lease,
                None,
                "response-chat-tool",
                vec![("unoffered-call", "test.read", json!({}))],
            ))])),
            delay: None,
        });
        let planner = Arc::new(TestChatPlanner {
            scope: test_scope(),
            continuation_count: AtomicUsize::new(0),
        });
        let forbidden = Arc::new(ChatForbiddenBoundaries::default());
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider,
            forbidden.clone(),
            Arc::new(TestOutputWriter),
            forbidden.clone(),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            planner.clone(),
            limits(50),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("unoffered chat tool call should close for recovery");

        assert!(matches!(
            outcome,
            RecoveryRequired {
                phase: AiAgentRecoveryPhase::ProviderTurn,
                provider_turns: 1,
                ..
            }
        ));
        assert_eq!(forbidden.tool_calls.load(Ordering::SeqCst), 0);
        assert_eq!(forbidden.provider_checkpoints.load(Ordering::SeqCst), 0);
        assert_eq!(forbidden.tool_batch_checkpoints.load(Ordering::SeqCst), 0);
        assert_eq!(planner.continuation_count.load(Ordering::SeqCst), 0);
        assert_eq!(run.final_states(), vec![AiRunState::RecoveryRequired]);
    }

    #[tokio::test]
    async fn chat_turn_rechecks_current_rules_after_provider_transport() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(AiProviderCallResult::test_result(
                &lease,
                None,
                "response-chat-stale-rule",
                Vec::new(),
            ))])),
            delay: None,
        });
        let planner = Arc::new(TestChatPlanner {
            scope: test_scope(),
            continuation_count: AtomicUsize::new(0),
        });
        let forbidden = Arc::new(ChatForbiddenBoundaries::default());
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider,
            forbidden.clone(),
            Arc::new(TestOutputWriter),
            forbidden.clone(),
            Arc::new(TestCheckpointWriter),
            Arc::new(ChangingRuleResolver(AtomicUsize::new(0))),
            planner.clone(),
            limits(50),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("changed chat rules should close for recovery");

        assert!(matches!(
            outcome,
            RecoveryRequired {
                phase: AiAgentRecoveryPhase::ProviderTurn,
                provider_turns: 1,
                total_tool_calls: 0,
            }
        ));
        assert_eq!(forbidden.tool_calls.load(Ordering::SeqCst), 0);
        assert_eq!(forbidden.provider_checkpoints.load(Ordering::SeqCst), 0);
        assert_eq!(forbidden.tool_batch_checkpoints.load(Ordering::SeqCst), 0);
        assert_eq!(planner.continuation_count.load(Ordering::SeqCst), 0);
        assert_eq!(run.final_states(), vec![AiRunState::RecoveryRequired]);
    }

    #[tokio::test]
    async fn final_provider_turn_persists_output_before_completion() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(AiProviderCallResult::test_result(
                &lease,
                None,
                "response-final",
                Vec::new(),
            ))])),
            delay: None,
        });
        let planner = Arc::new(TestPlanner {
            scope: test_scope(),
            route: test_route(),
            continuation_count: AtomicUsize::new(0),
        });

        let outcome = coordinator(run.clone(), provider, planner, true, limits(50))
            .execute_claimed(&lease)
            .await
            .expect("coordinator should complete");

        assert!(matches!(
            outcome,
            Completed {
                provider_turns: 1,
                total_tool_calls: 0,
                ..
            }
        ));
        assert_eq!(run.final_states(), vec![AiRunState::Completed]);
    }

    #[tokio::test]
    async fn adopted_tool_batch_is_consumed_before_the_next_provider_call() {
        let base_lease = AiRunLease::test_running(principal_reference());
        let mut old_guard = AiAgentLoopGuard::new(
            &base_lease,
            AiAgentLoopLimits::new(4, 8).expect("test limits should validate"),
        );
        let old_result = AiProviderCallResult::test_result(
            &base_lease,
            None,
            "adopted-response",
            vec![("adopted-call", "test.read", json!({}))],
        );
        assert!(matches!(
            old_guard
                .observe_provider_turn(&old_result)
                .expect("old provider turn should bind"),
            AiAgentLoopTurn::ToolCalls { .. }
        ));
        let persisted = AiPersistedApplicationToolCall::test_completed(
            base_lease.clone(),
            "adopted-call",
            "test.read",
            Some(json!({"record": "safe"})),
            Some(test_manifest(&base_lease, AiEgressCapability::ToolResult)),
        );
        old_guard
            .observe_tool_result(&persisted)
            .expect("old tool result should bind");
        let continuation = old_guard
            .continuation()
            .expect("old complete batch should continue");
        let checkpoint_id = Uuid::new_v4();
        let claimed = base_lease.test_with_checkpoint(checkpoint_id);
        let adopter = Arc::new(TestCheckpointAdopter {
            adopted: {
                let resolution = AiAgentRuleResolution::new(
                    test_rules(test_scope()),
                    time::OffsetDateTime::now_utc(),
                )
                .expect("test rules should resolve");
                let rule_usage = AiRuleRunUsage::default()
                    .accept_provider_with_web_searches(old_result.usage(), 0, &resolution)
                    .and_then(|usage| usage.accept_tool_calls(1, &resolution))
                    .expect("adopted usage should fit test rules");
                Mutex::new(Some(AiAdoptedReadOnlyToolBatch::new(
                    checkpoint_id,
                    old_guard.provider_turns(),
                    old_guard.total_tool_calls(),
                    test_scope(),
                    continuation,
                    resolution.rules().fingerprint().to_owned(),
                    rule_usage,
                )))
            },
            consumed: AtomicBool::new(false),
        });
        let planner = Arc::new(AdoptionOnlyPlanner {
            scope: test_scope(),
            route: test_route(),
            continuation_count: AtomicUsize::new(0),
        });
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(CheckpointClearedProvider {
            response: Mutex::new(Some(AiProviderCallResult::test_result(
                &claimed,
                Some("adopted-response".to_owned()),
                "final-response",
                Vec::new(),
            ))),
        });
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider,
            Arc::new(TestToolExecutor {
                expose_result: true,
            }),
            Arc::new(TestOutputWriter),
            Arc::new(TestCheckpointWriter),
            adopter.clone(),
            Arc::new(TestRuleResolver),
            planner.clone(),
            limits(50),
        );

        let outcome = coordinator
            .execute_claimed(&claimed)
            .await
            .expect("adopted continuation should complete");

        assert!(matches!(
            outcome,
            Completed {
                provider_turns: 2,
                total_tool_calls: 1,
                ..
            }
        ));
        assert!(adopter.consumed.load(Ordering::SeqCst));
        assert_eq!(planner.continuation_count.load(Ordering::SeqCst), 1);
        assert_eq!(run.final_states(), vec![AiRunState::Completed]);
    }

    #[tokio::test]
    async fn exhausted_adopted_turn_limit_preserves_the_unconsumed_checkpoint() {
        let base_lease = AiRunLease::test_running(principal_reference());
        let checkpoint_id = Uuid::new_v4();
        let claimed = base_lease.test_with_checkpoint(checkpoint_id);
        let adopter = Arc::new(TestCheckpointAdopter {
            adopted: Mutex::new(Some(adopted_read_only_checkpoint(
                &base_lease,
                checkpoint_id,
            ))),
            consumed: AtomicBool::new(false),
        });
        let planner = Arc::new(AdoptionOnlyPlanner {
            scope: test_scope(),
            route: test_route(),
            continuation_count: AtomicUsize::new(0),
        });
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(CheckpointClearedProvider {
            response: Mutex::new(Some(AiProviderCallResult::test_result(
                &claimed,
                Some("adopted-response".to_owned()),
                "must-not-run",
                Vec::new(),
            ))),
        });
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            Arc::new(TestToolExecutor {
                expose_result: true,
            }),
            Arc::new(TestOutputWriter),
            Arc::new(TestCheckpointWriter),
            adopter.clone(),
            Arc::new(TestRuleResolver),
            planner.clone(),
            limits_with_provider_turns(50, 1),
        );

        let outcome = coordinator
            .execute_claimed(&claimed)
            .await
            .expect("exhausted loop should close without consuming the checkpoint");

        assert_eq!(
            outcome,
            Failed {
                provider_turns: 1,
                total_tool_calls: 1,
            }
        );
        assert!(!adopter.consumed.load(Ordering::SeqCst));
        assert_eq!(planner.continuation_count.load(Ordering::SeqCst), 0);
        assert!(
            provider
                .response
                .lock()
                .expect("test response lock should not be poisoned")
                .is_some()
        );
        assert_eq!(run.final_states(), vec![AiRunState::Failed]);
    }

    #[tokio::test]
    async fn provider_result_without_a_durable_checkpoint_requires_recovery() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(AiProviderCallResult::test_result(
                &lease,
                None,
                "response-without-checkpoint",
                Vec::new(),
            ))])),
            delay: None,
        });
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider,
            Arc::new(TestToolExecutor {
                expose_result: true,
            }),
            Arc::new(TestOutputWriter),
            Arc::new(RejectProviderCheckpoint),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            Arc::new(TestPlanner {
                scope: test_scope(),
                route: test_route(),
                continuation_count: AtomicUsize::new(0),
            }),
            limits(50),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("missing checkpoint should close for recovery");

        assert!(matches!(
            outcome,
            RecoveryRequired {
                phase: AiAgentRecoveryPhase::ProviderTurn,
                provider_turns: 1,
                total_tool_calls: 0,
            }
        ));
        assert_eq!(run.final_states(), vec![AiRunState::RecoveryRequired]);
    }

    #[tokio::test]
    async fn exact_tool_result_is_chained_into_the_next_turn() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([
                Ok(AiProviderCallResult::test_result(
                    &lease,
                    None,
                    "response-tool",
                    vec![("call-1", "test.read", json!({}))],
                )),
                Ok(AiProviderCallResult::test_result(
                    &lease,
                    Some("response-tool".to_owned()),
                    "response-final",
                    Vec::new(),
                )),
            ])),
            delay: None,
        });
        let planner = Arc::new(TestPlanner {
            scope: test_scope(),
            route: test_route(),
            continuation_count: AtomicUsize::new(0),
        });

        let outcome = coordinator(run.clone(), provider, planner.clone(), true, limits(50))
            .execute_claimed(&lease)
            .await
            .expect("coordinator should complete tool loop");

        assert!(matches!(
            outcome,
            Completed {
                provider_turns: 2,
                total_tool_calls: 1,
                ..
            }
        ));
        assert_eq!(planner.continuation_count.load(Ordering::SeqCst), 1);
        assert_eq!(run.final_states(), vec![AiRunState::Completed]);
    }

    #[tokio::test]
    async fn provider_failure_is_closed_as_recovery_required() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Err(AiError::ProviderFailed)])),
            delay: None,
        });
        let planner = Arc::new(TestPlanner {
            scope: test_scope(),
            route: test_route(),
            continuation_count: AtomicUsize::new(0),
        });

        let outcome = coordinator(run.clone(), provider, planner, true, limits(50))
            .execute_claimed(&lease)
            .await
            .expect("provider ambiguity should be durably classified");

        assert_eq!(
            outcome,
            RecoveryRequired {
                phase: AiAgentRecoveryPhase::ProviderTurn,
                provider_turns: 0,
                total_tool_calls: 0,
            }
        );
        assert_eq!(run.final_states(), vec![AiRunState::RecoveryRequired]);
    }

    #[tokio::test]
    async fn rule_change_after_provider_egress_requires_recovery() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(AiProviderCallResult::test_result(
                &lease,
                None,
                "response-rule-changed",
                Vec::new(),
            ))])),
            delay: None,
        });
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            Arc::new(TestToolExecutor {
                expose_result: true,
            }),
            Arc::new(TestOutputWriter),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestCheckpointWriter),
            Arc::new(ChangingRuleResolver(AtomicUsize::new(0))),
            Arc::new(TestPlanner {
                scope: test_scope(),
                route: test_route(),
                continuation_count: AtomicUsize::new(0),
            }),
            limits(50),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("a changed post-egress rule must close durably");

        assert_eq!(
            outcome,
            RecoveryRequired {
                phase: AiAgentRecoveryPhase::ProviderTurn,
                provider_turns: 1,
                total_tool_calls: 0,
            }
        );
        assert_eq!(provider.remaining_responses(), 0);
        assert_eq!(run.final_states(), vec![AiRunState::RecoveryRequired]);
    }

    #[tokio::test]
    async fn unavailable_tool_egress_fails_without_a_second_provider_call() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(AiProviderCallResult::test_result(
                &lease,
                None,
                "response-tool",
                vec![("call-1", "test.read", json!({}))],
            ))])),
            delay: None,
        });
        let planner = Arc::new(TestPlanner {
            scope: test_scope(),
            route: test_route(),
            continuation_count: AtomicUsize::new(0),
        });

        let outcome = coordinator(run.clone(), provider, planner, false, limits(50))
            .execute_claimed(&lease)
            .await
            .expect("denied result should become a durable safe failure");

        assert!(matches!(
            outcome,
            Failed {
                provider_turns: 1,
                total_tool_calls: 1,
            }
        ));
        assert_eq!(run.final_states(), vec![AiRunState::Failed]);
    }

    #[tokio::test]
    async fn continuation_planner_cannot_drop_the_opaque_continuation() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([
                Ok(AiProviderCallResult::test_result(
                    &lease,
                    None,
                    "response-tool",
                    vec![("call-1", "test.read", json!({}))],
                )),
                Ok(AiProviderCallResult::test_result(
                    &lease,
                    None,
                    "must-not-run",
                    Vec::new(),
                )),
            ])),
            delay: None,
        });
        let invalid_planner = Arc::new(InvalidContinuationPlanner {
            scope: test_scope(),
            route: test_route(),
        });
        let coordinator = AiReadOnlyAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            Arc::new(TestToolExecutor {
                expose_result: true,
            }),
            Arc::new(TestOutputWriter),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestCheckpointWriter),
            Arc::new(TestRuleResolver),
            invalid_planner,
            limits(50),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("invalid continuation phase should be a safe failure");

        assert!(matches!(
            outcome,
            Failed {
                provider_turns: 1,
                total_tool_calls: 1,
            }
        ));
        assert_eq!(provider.remaining_responses(), 1);
        assert_eq!(run.final_states(), vec![AiRunState::Failed]);
    }

    #[tokio::test]
    async fn lost_heartbeat_fence_stops_without_terminal_write() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        run.fail_heartbeat.store(true, Ordering::SeqCst);
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(AiProviderCallResult::test_result(
                &lease,
                None,
                "late-response",
                Vec::new(),
            ))])),
            delay: Some(std::time::Duration::from_millis(50)),
        });
        let planner = Arc::new(TestPlanner {
            scope: test_scope(),
            route: test_route(),
            continuation_count: AtomicUsize::new(0),
        });

        let error = coordinator(run.clone(), provider, planner, true, limits(1))
            .execute_claimed(&lease)
            .await
            .expect_err("lost fence must stop the attempt");

        assert!(matches!(error, AiError::Conflict));
        assert!(run.heartbeat_count.load(Ordering::SeqCst) >= 1);
        assert!(run.final_states().is_empty());
    }
}
