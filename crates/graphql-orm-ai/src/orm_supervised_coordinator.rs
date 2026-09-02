//! Bounded top-level coordination for sequential supervised mutations.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use std::sync::Arc;

use agql_auth::Clock;
use async_trait::async_trait;
use time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    AiAdoptedAutomaticMutationBatch, AiAdoptedSupervisedToolBatch, AiAgentCheckpointWriter,
    AiAgentLoopGuard, AiAgentLoopLimits, AiAgentLoopTurn, AiAgentProviderOutputWriter,
    AiAgentProviderTurnExecutor, AiAgentRecoveryPhase, AiAgentRuleResolver, AiAgentRunControl,
    AiApplicationToolCallContext, AiApprovalId, AiError, AiProviderCallPlan,
    AiRequestedConsequentialToolCall, AiResolvedRuleSet, AiRuleRunUsage, AiRunCompletion,
    AiRunLease, AiRunState, AiScope, AiSupervisedResumeOutcome, AiToolCallId,
    AiToolResultEgressRoute, OrmAiApplicationToolCallService, OrmAiConsequentialToolCallService,
    OrmAiCoordinatorCheckpointService, OrmAiSupervisedResumeService,
};

/// Deployment bounds for the sequential supervised coordinator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiSupervisedAgentCoordinatorLimits {
    loop_limits: AiAgentLoopLimits,
    heartbeat_interval: Duration,
    approval_ttl: Duration,
    recent_mfa_required: bool,
}

impl AiSupervisedAgentCoordinatorLimits {
    /// Creates supervised coordinator limits.
    ///
    /// The heartbeat interval must remain comfortably below the run-service
    /// lease TTL. Approval TTL is measured from durable staging, not provider
    /// planning time.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] unless heartbeat is positive
    /// and at most five minutes and approval TTL is positive and at most one
    /// day.
    pub fn new(
        loop_limits: AiAgentLoopLimits,
        heartbeat_interval: Duration,
        approval_ttl: Duration,
        recent_mfa_required: bool,
    ) -> Result<Self, AiError> {
        if !heartbeat_interval.is_positive()
            || heartbeat_interval > Duration::minutes(5)
            || !approval_ttl.is_positive()
            || approval_ttl > Duration::days(1)
        {
            return Err(AiError::InvalidConfiguration(
                "invalid supervised coordinator limits".to_owned(),
            ));
        }
        Ok(Self {
            loop_limits,
            heartbeat_interval,
            approval_ttl,
            recent_mfa_required,
        })
    }
}

/// One host-planned provider turn exposing only classified mutations.
///
/// Construction proves the plan is provider-retained, contains only immutable
/// `AutonomousWrite`/`None` or `SupervisedWrite`/`OneShot` bindings, targets the
/// exact resolved-rule scope, and has a valid server-owned result route. It
/// grants no provider, budget, egress, approval, mutation, or resolver authority.
pub struct AiSupervisedAgentTurnPlan {
    provider_call: AiProviderCallPlan,
    provider_session: Option<crate::AiProviderSessionTurnPlan>,
    result_egress_route: AiToolResultEgressRoute,
    rules: AiResolvedRuleSet,
    uses_byok: bool,
}

impl AiSupervisedAgentTurnPlan {
    /// Creates an exact classified-mutation provider turn.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidInput`] unless the provider plan contains
    /// only supervised one-shot tools, uses provider-retained continuation,
    /// matches the rule target, and the result route is valid.
    pub fn new(
        provider_call: AiProviderCallPlan,
        result_egress_route: AiToolResultEgressRoute,
        rules: AiResolvedRuleSet,
        uses_byok: bool,
    ) -> Result<Self, AiError> {
        if !provider_call.has_only_classified_mutations()
            || !provider_call.uses_provider_retained_continuation()
            || provider_call.scope() != rules.target_scope()
        {
            return Err(AiError::InvalidInput(
                "classified mutation turn is not exactly bound".to_owned(),
            ));
        }
        result_egress_route.validate()?;
        Ok(Self {
            provider_call,
            provider_session: None,
            result_egress_route,
            rules,
            uses_byok,
        })
    }

    /// Binds this turn to one exact durable provider-session contract.
    ///
    /// # Errors
    ///
    /// Returns a conflict unless the provider/model/registration contract is
    /// exact for the already-validated provider call.
    pub fn with_provider_session(
        mut self,
        session: crate::AiProviderSessionTurnPlan,
    ) -> Result<Self, AiError> {
        if !self
            .provider_call
            .matches_provider_session_descriptor(session.descriptor())
        {
            return Err(AiError::Conflict);
        }
        self.provider_session = Some(session);
        Ok(self)
    }

    #[cfg(test)]
    fn test_with_provider_session(mut self, session: crate::AiProviderSessionTurnPlan) -> Self {
        self.provider_session = Some(session);
        self
    }

    fn is_continuation(&self) -> bool {
        self.provider_call.is_continuation()
    }

    fn into_parts(
        self,
    ) -> (
        AiProviderCallPlan,
        Option<crate::AiProviderSessionTurnPlan>,
        AiScope,
        String,
        AiToolResultEgressRoute,
        AiResolvedRuleSet,
        bool,
    ) {
        let scope = self.provider_call.scope().clone();
        let correlation_id = self.provider_call.correlation_id().to_owned();
        (
            self.provider_call,
            self.provider_session,
            scope,
            correlation_id,
            self.result_egress_route,
            self.rules,
            self.uses_byok,
        )
    }
}

fn plan_binding(
    plan: &AiProviderCallPlan,
    result: &crate::AiProviderCallResult,
) -> Option<(crate::ToolMaturity, crate::AiApprovalRule)> {
    let call = result.tool_calls().first()?;
    plan.classified_mutation_binding(call.tool_fingerprint())
}

/// Host-owned construction of supervised initial and continuation turns.
///
/// Implementations select configuration, exact registered definitions,
/// provider/model, current rule evidence, atomic-budget estimate, and every
/// egress manifest. Continuation implementations must use
/// [`AiProviderCallPlan::new_supervised_continuation_with_tools`].
#[async_trait]
pub trait AiSupervisedAgentTurnPlanner: Send + Sync {
    /// Builds the first supervised-only provider turn.
    ///
    /// # Errors
    ///
    /// Returns a safe error when current configuration cannot produce an
    /// exact initial turn.
    async fn initial_plan(&self, lease: &AiRunLease) -> Result<AiSupervisedAgentTurnPlan, AiError>;

    /// Builds the next turn from one opaque approved mutation result.
    ///
    /// # Errors
    ///
    /// Returns a safe error when the exact continuation cannot be bound to a
    /// fresh provider, budget, egress, route, and rule plan.
    async fn continuation_plan(
        &self,
        lease: &AiRunLease,
        provider_turns: u32,
        continuation: crate::AiAgentContinuation,
    ) -> Result<AiSupervisedAgentTurnPlan, AiError>;
}

/// Durable wait created for one provider-requested supervised mutation.
#[derive(Clone, Debug)]
pub struct AiSupervisedApprovalWait {
    approval_id: AiApprovalId,
    tool_call_id: AiToolCallId,
    lease: AiRunLease,
}

impl AiSupervisedApprovalWait {
    fn from_requested(requested: AiRequestedConsequentialToolCall) -> Self {
        Self {
            approval_id: requested.approval_id(),
            tool_call_id: requested.tool_call_id(),
            lease: requested.into_lease(),
        }
    }

    /// Exact pending approval.
    pub const fn approval_id(&self) -> AiApprovalId {
        self.approval_id
    }

    /// Exact staged consequential tool call.
    pub const fn tool_call_id(&self) -> AiToolCallId {
        self.tool_call_id
    }

    /// Durable waiting lease.
    pub fn lease(&self) -> &AiRunLease {
        &self.lease
    }
}

/// Approval-staging boundary used by the coordinator.
#[async_trait]
pub trait AiAgentSupervisedApprovalStager: Send + Sync {
    /// Stages one exact provider-requested mutation for human review.
    ///
    /// # Errors
    ///
    /// Returns a safe error for invalid tool count/binding, preview or current
    /// authorization denial, protection failure, or persistence ambiguity.
    async fn stage(
        &self,
        lease: &AiRunLease,
        result: &crate::AiProviderCallResult,
        context: AiApplicationToolCallContext,
        expires_at: time::OffsetDateTime,
        recent_mfa_required: bool,
    ) -> Result<AiSupervisedApprovalWait, AiError>;
}

/// Fenced execution boundary for one generated automatic mutation.
#[async_trait]
pub trait AiAgentAutomaticMutationExecutor: Send + Sync {
    /// Persists the pre-effect fence, executes once, and converges every
    /// post-effect ambiguity to `RecoveryRequired`.
    async fn execute(
        &self,
        lease: &AiRunLease,
        result: &crate::AiProviderCallResult,
        context: AiApplicationToolCallContext,
        route: AiToolResultEgressRoute,
    ) -> Result<crate::AiConsequentialToolCallOutcome, AiError>;
}

#[async_trait]
impl AiAgentAutomaticMutationExecutor for OrmAiApplicationToolCallService {
    async fn execute(
        &self,
        lease: &AiRunLease,
        result: &crate::AiProviderCallResult,
        context: AiApplicationToolCallContext,
        route: AiToolResultEgressRoute,
    ) -> Result<crate::AiConsequentialToolCallOutcome, AiError> {
        self.execute_automatic_mutation(lease, result, context, route)
            .await
    }
}

#[async_trait]
impl AiAgentSupervisedApprovalStager for OrmAiConsequentialToolCallService {
    async fn stage(
        &self,
        lease: &AiRunLease,
        result: &crate::AiProviderCallResult,
        context: AiApplicationToolCallContext,
        expires_at: time::OffsetDateTime,
        recent_mfa_required: bool,
    ) -> Result<AiSupervisedApprovalWait, AiError> {
        self.request_approval(lease, result, context, expires_at, recent_mfa_required)
            .await
            .map(AiSupervisedApprovalWait::from_requested)
    }
}

/// Exact classified-mutation checkpoint selected from durable checkpoint
/// metadata before protected state is opened.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum AiAdoptedClassifiedMutationBatch {
    /// One approved one-shot mutation result.
    Supervised(AiAdoptedSupervisedToolBatch),
    /// One explicitly automatic mutation result.
    Automatic(AiAdoptedAutomaticMutationBatch),
    /// One bounded replayable-subscription result. The next provider turn
    /// remains subject to the same classified mutation policy as every other
    /// supervised-coordinator turn.
    Subscription(crate::AiAdoptedReadOnlyToolBatch),
}

/// Protected supervised-checkpoint adoption and one-shot consumption.
#[async_trait]
pub trait AiAgentSupervisedCheckpointControl: Send + Sync {
    /// Selects and opens the exact classified checkpoint kind.
    ///
    /// # Errors
    ///
    /// Returns a safe error for a missing, unknown, malformed, or unauthorized
    /// linked checkpoint. Implementations must never reinterpret a failed
    /// proof as another checkpoint kind.
    async fn adopt_classified(
        &self,
        lease: &AiRunLease,
    ) -> Result<Option<AiAdoptedClassifiedMutationBatch>, AiError> {
        if let Some(adopted) = self.adopt(lease).await? {
            return Ok(Some(AiAdoptedClassifiedMutationBatch::Supervised(adopted)));
        }
        if let Some(adopted) = self.adopt_automatic(lease).await? {
            return Ok(Some(AiAdoptedClassifiedMutationBatch::Automatic(adopted)));
        }
        self.adopt_subscription(lease)
            .await
            .map(|adopted| adopted.map(AiAdoptedClassifiedMutationBatch::Subscription))
    }

    /// Reopens the exact linked supervised result under current authority.
    ///
    /// # Errors
    ///
    /// Returns a safe error for malformed, stale, unauthorized, or unprovable
    /// protected checkpoint state.
    async fn adopt(
        &self,
        lease: &AiRunLease,
    ) -> Result<Option<AiAdoptedSupervisedToolBatch>, AiError>;

    /// Consumes the exact adoption proof before provider transport.
    ///
    /// # Errors
    ///
    /// Returns a safe error unless the proof remains linked through the
    /// current row-version fence.
    async fn consume(
        &self,
        lease: &AiRunLease,
        adopted: &AiAdoptedSupervisedToolBatch,
    ) -> Result<AiRunLease, AiError>;

    /// Reopens an exact completed automatic-mutation result under current
    /// authority.
    ///
    /// # Errors
    ///
    /// Returns a safe error for malformed, stale, unauthorized, or unprovable
    /// protected checkpoint state.
    async fn adopt_automatic(
        &self,
        _lease: &AiRunLease,
    ) -> Result<Option<AiAdoptedAutomaticMutationBatch>, AiError> {
        Ok(None)
    }

    /// Consumes an exact automatic-mutation checkpoint before provider
    /// transport.
    ///
    /// # Errors
    ///
    /// Returns a safe error unless the proof remains linked through the
    /// current row-version fence.
    async fn consume_automatic(
        &self,
        _lease: &AiRunLease,
        _adopted: &AiAdoptedAutomaticMutationBatch,
    ) -> Result<AiRunLease, AiError> {
        Err(AiError::Conflict)
    }

    /// Reopens one exact bounded subscription-wait result.
    async fn adopt_subscription(
        &self,
        _lease: &AiRunLease,
    ) -> Result<Option<crate::AiAdoptedReadOnlyToolBatch>, AiError> {
        Ok(None)
    }

    /// Consumes one subscription adoption before provider transport.
    async fn consume_subscription(
        &self,
        _lease: &AiRunLease,
        _adopted: &crate::AiAdoptedReadOnlyToolBatch,
    ) -> Result<AiRunLease, AiError> {
        Err(AiError::Conflict)
    }
}

#[async_trait]
impl AiAgentSupervisedCheckpointControl for OrmAiCoordinatorCheckpointService {
    async fn adopt_classified(
        &self,
        lease: &AiRunLease,
    ) -> Result<Option<AiAdoptedClassifiedMutationBatch>, AiError> {
        self.adopt_classified_mutation_batch(lease).await
    }

    async fn adopt(
        &self,
        lease: &AiRunLease,
    ) -> Result<Option<AiAdoptedSupervisedToolBatch>, AiError> {
        self.adopt_supervised_tool_batch(lease).await
    }

    async fn consume(
        &self,
        lease: &AiRunLease,
        adopted: &AiAdoptedSupervisedToolBatch,
    ) -> Result<AiRunLease, AiError> {
        self.consume_supervised_before_provider(lease, adopted)
            .await
    }

    async fn adopt_automatic(
        &self,
        lease: &AiRunLease,
    ) -> Result<Option<AiAdoptedAutomaticMutationBatch>, AiError> {
        self.adopt_automatic_mutation_batch(lease).await
    }

    async fn consume_automatic(
        &self,
        lease: &AiRunLease,
        adopted: &AiAdoptedAutomaticMutationBatch,
    ) -> Result<AiRunLease, AiError> {
        self.consume_automatic_mutation_before_provider(lease, adopted)
            .await
    }
}

/// Approved-wait execution boundary used by the top-level coordinator.
#[async_trait]
pub trait AiAgentSupervisedResumeExecutor: Send + Sync {
    /// Executes and protects one exact approved claim without provider I/O.
    ///
    /// # Errors
    ///
    /// Returns a safe error before resolver ambiguity for stale or denied
    /// evidence; consequential ambiguity is returned durably in the outcome.
    async fn resume(
        &self,
        claim: &crate::AiApprovedRunClaim,
    ) -> Result<AiSupervisedResumeOutcome, AiError>;
}

#[async_trait]
impl AiAgentSupervisedResumeExecutor for OrmAiSupervisedResumeService {
    async fn resume(
        &self,
        claim: &crate::AiApprovedRunClaim,
    ) -> Result<AiSupervisedResumeOutcome, AiError> {
        self.execute_claimed(claim).await
    }
}

/// Durable result of one supervised coordinator invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiSupervisedAgentRunOutcome {
    /// Final assistant output and terminal run completion are durable.
    Completed {
        /// Persisted assistant message.
        message_id: Uuid,
        /// Accepted provider-turn count.
        provider_turns: u32,
        /// Completed/requested application-tool count.
        total_tool_calls: u32,
    },
    /// One exact mutation is durably waiting for a human decision.
    WaitingApproval {
        /// Pending approval.
        approval_id: AiApprovalId,
        /// Staged consequential tool call.
        tool_call_id: AiToolCallId,
        /// Accepted provider-turn count.
        provider_turns: u32,
        /// Requested application-tool count.
        total_tool_calls: u32,
    },
    /// A proof/configuration failure was durably classified as safe failure.
    Failed {
        /// Accepted provider-turn count.
        provider_turns: u32,
        /// Completed/requested application-tool count.
        total_tool_calls: u32,
    },
    /// The owner cancellation fence won and no later provider continuation
    /// was admitted.
    Cancelled {
        /// Accepted provider-turn count before cancellation.
        provider_turns: u32,
        /// Completed/requested application-tool count before cancellation.
        total_tool_calls: u32,
    },
    /// An external boundary was durably closed for privileged recovery.
    RecoveryRequired {
        /// Ambiguous phase.
        phase: AiAgentRecoveryPhase,
        /// Accepted provider-turn count.
        provider_turns: u32,
        /// Completed/requested application-tool count.
        total_tool_calls: u32,
    },
}

/// Top-level coordinator for sequential, human-approved mutations.
///
/// Each provider turn may finish normally or request exactly one supervised
/// mutation. The coordinator checkpoints the provider turn before staging a
/// server-previewed approval and returns without heartbeating the human wait.
/// A later approved claim executes through [`OrmAiSupervisedResumeService`],
/// adopts the exact protected result, consumes it once before transport, and
/// continues. Read-only, mixed, parallel, stateless, autonomous, and
/// model-authored GraphQL tool paths remain closed.
pub struct AiSupervisedAgentCoordinator {
    run_control: Arc<dyn AiAgentRunControl>,
    provider_executor: Arc<dyn AiAgentProviderTurnExecutor>,
    output_writer: Arc<dyn AiAgentProviderOutputWriter>,
    checkpoint_writer: Arc<dyn AiAgentCheckpointWriter>,
    checkpoint_control: Arc<dyn AiAgentSupervisedCheckpointControl>,
    approval_stager: Arc<dyn AiAgentSupervisedApprovalStager>,
    automatic_executor: Arc<dyn AiAgentAutomaticMutationExecutor>,
    resume_executor: Arc<dyn AiAgentSupervisedResumeExecutor>,
    rule_resolver: Arc<dyn AiAgentRuleResolver>,
    planner: Arc<dyn AiSupervisedAgentTurnPlanner>,
    clock: Arc<dyn Clock>,
    limits: AiSupervisedAgentCoordinatorLimits,
    provider_session_service: Option<Arc<dyn crate::AiProviderSessionService>>,
}

impl AiSupervisedAgentCoordinator {
    /// Creates the supervised coordinator from proof-preserving boundaries.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_control: Arc<dyn AiAgentRunControl>,
        provider_executor: Arc<dyn AiAgentProviderTurnExecutor>,
        output_writer: Arc<dyn AiAgentProviderOutputWriter>,
        checkpoint_writer: Arc<dyn AiAgentCheckpointWriter>,
        checkpoint_control: Arc<dyn AiAgentSupervisedCheckpointControl>,
        approval_stager: Arc<dyn AiAgentSupervisedApprovalStager>,
        automatic_executor: Arc<dyn AiAgentAutomaticMutationExecutor>,
        resume_executor: Arc<dyn AiAgentSupervisedResumeExecutor>,
        rule_resolver: Arc<dyn AiAgentRuleResolver>,
        planner: Arc<dyn AiSupervisedAgentTurnPlanner>,
        clock: Arc<dyn Clock>,
        limits: AiSupervisedAgentCoordinatorLimits,
    ) -> Self {
        Self {
            run_control,
            provider_executor,
            output_writer,
            checkpoint_writer,
            checkpoint_control,
            approval_stager,
            automatic_executor,
            resume_executor,
            rule_resolver,
            planner,
            clock,
            limits,
            provider_session_service: None,
        }
    }

    /// Enables retained-session execution, wait reclaim, and terminal commit.
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
        result: &crate::AiProviderCallResult,
        reason: &str,
    ) {
        if let (Some(service), Some(claim)) = (
            &self.provider_session_service,
            result.provider_session_claim(),
        ) {
            let _ = service.require_cleanup(claim, reason).await;
        }
    }

    /// Starts a fresh or checkpoint-requeued claim and advances one turn.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale claim, lost fence, or failure to durably
    /// classify an otherwise safe failure/recovery outcome.
    pub async fn execute_claimed(
        &self,
        claimed: &AiRunLease,
    ) -> Result<AiSupervisedAgentRunOutcome, AiError> {
        let lease = self.run_control.start(claimed).await?;
        if self.run_control.cancellation(&lease).await?.is_some() {
            return Ok(AiSupervisedAgentRunOutcome::Cancelled {
                provider_turns: 0,
                total_tool_calls: 0,
            });
        }
        if lease.latest_checkpoint_id().is_some() {
            match self.checkpoint_control.adopt_classified(&lease).await {
                Ok(Some(AiAdoptedClassifiedMutationBatch::Supervised(adopted))) => {
                    self.continue_from_adopted(lease, adopted).await
                }
                Ok(Some(AiAdoptedClassifiedMutationBatch::Automatic(adopted))) => {
                    self.continue_from_automatic(lease, adopted).await
                }
                Ok(Some(AiAdoptedClassifiedMutationBatch::Subscription(adopted))) => {
                    self.continue_from_subscription(lease, adopted).await
                }
                Ok(None) | Err(_) => {
                    let guard = AiAgentLoopGuard::new(&lease, self.limits.loop_limits);
                    self.finish_recovery(
                        &lease,
                        &guard,
                        AiAgentRecoveryPhase::ApplicationTool,
                        "classified_mutation_checkpoint_adoption_failed",
                        None,
                    )
                    .await
                }
            }
        } else {
            let guard = AiAgentLoopGuard::new(&lease, self.limits.loop_limits);
            let plan = match self.planner.initial_plan(&lease).await {
                Ok(plan) if !plan.is_continuation() => plan,
                _ => {
                    return self
                        .finish_failed(&lease, &guard, "supervised_initial_plan_invalid")
                        .await;
                }
            };
            self.execute_turn(
                lease,
                guard,
                plan,
                AiRuleRunUsage::default(),
                None,
                None,
                None,
            )
            .await
        }
    }

    /// Executes one exact approved claim and advances its next provider turn.
    ///
    /// The mutation is executed and checkpointed before any provider call. A
    /// recovery-required mutation outcome is returned without replay.
    ///
    /// # Errors
    ///
    /// Returns a safe error for stale/denied pre-execution evidence, lost
    /// fencing, or failure to persist a terminal classification.
    pub async fn execute_approved_claim(
        &self,
        claim: &crate::AiApprovedRunClaim,
    ) -> Result<AiSupervisedAgentRunOutcome, AiError> {
        let resumed = self.resume_executor.resume(claim).await?;
        if let AiSupervisedResumeOutcome::RecoveryRequired {
            provider_turns,
            total_tool_calls,
            ..
        } = resumed
        {
            return Ok(AiSupervisedAgentRunOutcome::RecoveryRequired {
                phase: AiAgentRecoveryPhase::ApplicationTool,
                provider_turns,
                total_tool_calls,
            });
        }
        let checkpointed = resumed.checkpointed().ok_or(AiError::Conflict)?;
        let lease = checkpointed.lease().clone();
        let adopted = match self.checkpoint_control.adopt(&lease).await {
            Ok(Some(adopted)) => adopted,
            Ok(None) | Err(_) => {
                return self
                    .finish_recovery_counts(
                        &lease,
                        AiAgentRecoveryPhase::ApplicationTool,
                        "supervised_checkpoint_adoption_failed",
                        None,
                        checkpointed.provider_turns(),
                        checkpointed.total_tool_calls(),
                    )
                    .await;
            }
        };
        self.continue_from_adopted(lease, adopted).await
    }

    async fn continue_from_adopted(
        &self,
        lease: AiRunLease,
        adopted: AiAdoptedSupervisedToolBatch,
    ) -> Result<AiSupervisedAgentRunOutcome, AiError> {
        let reference = adopted.continuation().chain_reference()?;
        let guard = AiAgentLoopGuard::resume_after_tool_batch(
            &lease,
            self.limits.loop_limits,
            adopted.provider_turns(),
            adopted.total_tool_calls(),
            &reference,
        )?;
        let plan = match self
            .planner
            .continuation_plan(
                &lease,
                adopted.provider_turns(),
                adopted.continuation().clone(),
            )
            .await
        {
            Ok(plan)
                if plan.is_continuation()
                    && plan.provider_call.scope() == adopted.scope()
                    && plan.rules.fingerprint() == adopted.rule_fingerprint() =>
            {
                plan
            }
            _ => {
                return self
                    .finish_failed(&lease, &guard, "supervised_continuation_plan_invalid")
                    .await;
            }
        };
        let usage = adopted.rule_usage();
        self.execute_turn(lease, guard, plan, usage, Some(&adopted), None, None)
            .await
    }

    async fn continue_from_automatic(
        &self,
        lease: AiRunLease,
        adopted: AiAdoptedAutomaticMutationBatch,
    ) -> Result<AiSupervisedAgentRunOutcome, AiError> {
        let reference = adopted.continuation().chain_reference()?;
        let guard = AiAgentLoopGuard::resume_after_tool_batch(
            &lease,
            self.limits.loop_limits,
            adopted.provider_turns(),
            adopted.total_tool_calls(),
            &reference,
        )?;
        let plan = match self
            .planner
            .continuation_plan(
                &lease,
                adopted.provider_turns(),
                adopted.continuation().clone(),
            )
            .await
        {
            Ok(plan)
                if plan.is_continuation()
                    && plan.provider_call.scope() == adopted.scope()
                    && plan.rules.fingerprint() == adopted.rule_fingerprint() =>
            {
                plan
            }
            _ => {
                return self
                    .finish_failed(&lease, &guard, "automatic_continuation_plan_invalid")
                    .await;
            }
        };
        let usage = adopted.rule_usage();
        self.execute_turn(lease, guard, plan, usage, None, Some(&adopted), None)
            .await
    }

    async fn continue_from_subscription(
        &self,
        lease: AiRunLease,
        adopted: crate::AiAdoptedReadOnlyToolBatch,
    ) -> Result<AiSupervisedAgentRunOutcome, AiError> {
        let reference = adopted.continuation().chain_reference()?;
        let guard = AiAgentLoopGuard::resume_after_tool_batch(
            &lease,
            self.limits.loop_limits,
            adopted.provider_turns(),
            adopted.total_tool_calls(),
            &reference,
        )?;
        let plan = match self
            .planner
            .continuation_plan(
                &lease,
                adopted.provider_turns(),
                adopted.continuation().clone(),
            )
            .await
        {
            Ok(plan)
                if plan.is_continuation()
                    && plan.provider_call.scope() == adopted.scope()
                    && plan.rules.fingerprint() == adopted.rule_fingerprint() =>
            {
                plan
            }
            _ => {
                return self
                    .finish_failed(&lease, &guard, "subscription_continuation_plan_invalid")
                    .await;
            }
        };
        let usage = adopted.rule_usage();
        self.execute_turn(lease, guard, plan, usage, None, None, Some(&adopted))
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_turn(
        &self,
        mut lease: AiRunLease,
        mut guard: AiAgentLoopGuard,
        plan: AiSupervisedAgentTurnPlan,
        mut rule_usage: AiRuleRunUsage,
        adopted: Option<&AiAdoptedSupervisedToolBatch>,
        adopted_automatic: Option<&AiAdoptedAutomaticMutationBatch>,
        adopted_subscription: Option<&crate::AiAdoptedReadOnlyToolBatch>,
    ) -> Result<AiSupervisedAgentRunOutcome, AiError> {
        if !guard.can_begin_provider_turn() {
            return self
                .finish_failed(&lease, &guard, "supervised_provider_turn_limit_reached")
                .await;
        }
        let (
            provider_plan,
            provider_session,
            scope,
            correlation_id,
            route,
            planned_rules,
            uses_byok,
        ) = plan.into_parts();
        let resolution = match self.rule_resolver.resolve_rules(&lease, &scope).await {
            Ok(resolution) if resolution.rules().fingerprint() == planned_rules.fingerprint() => {
                resolution
            }
            _ => {
                return self
                    .finish_failed(&lease, &guard, "supervised_rule_plan_stale")
                    .await;
            }
        };
        let started_usage = match rule_usage.validate(&resolution) {
            Ok(usage) => usage,
            Err(_) => {
                return self
                    .finish_failed(&lease, &guard, "supervised_rule_duration_exceeded")
                    .await;
            }
        };
        if provider_plan
            .project_supervised_rule_usage(&resolution, started_usage, uses_byok)
            .is_err()
        {
            return self
                .finish_failed(&lease, &guard, "supervised_rule_plan_denied")
                .await;
        }
        rule_usage = started_usage;
        if let Some(adopted) = adopted {
            lease = match self.checkpoint_control.consume(&lease, adopted).await {
                Ok(renewed) => renewed,
                Err(_) => {
                    return self
                        .finish_recovery(
                            &lease,
                            &guard,
                            AiAgentRecoveryPhase::ApplicationTool,
                            "supervised_checkpoint_consumption_failed",
                            None,
                        )
                        .await;
                }
            };
            let final_rules = match self.rule_resolver.resolve_rules(&lease, &scope).await {
                Ok(current)
                    if current.rules().fingerprint() == planned_rules.fingerprint()
                        && rule_usage.validate(&current).is_ok() =>
                {
                    current
                }
                _ => {
                    return self
                        .finish_failed(&lease, &guard, "supervised_rule_changed_after_consume")
                        .await;
                }
            };
            if provider_plan
                .project_supervised_rule_usage(&final_rules, rule_usage, uses_byok)
                .is_err()
            {
                return self
                    .finish_failed(&lease, &guard, "supervised_rule_denied_after_consume")
                    .await;
            }
        } else if let Some(adopted) = adopted_automatic {
            lease = match self
                .checkpoint_control
                .consume_automatic(&lease, adopted)
                .await
            {
                Ok(renewed) => renewed,
                Err(_) => {
                    return self
                        .finish_recovery(
                            &lease,
                            &guard,
                            AiAgentRecoveryPhase::ApplicationTool,
                            "automatic_checkpoint_consumption_failed",
                            None,
                        )
                        .await;
                }
            };
            let final_rules = match self.rule_resolver.resolve_rules(&lease, &scope).await {
                Ok(current)
                    if current.rules().fingerprint() == planned_rules.fingerprint()
                        && rule_usage.validate(&current).is_ok() =>
                {
                    current
                }
                _ => {
                    return self
                        .finish_failed(&lease, &guard, "automatic_rule_changed_after_consume")
                        .await;
                }
            };
            if provider_plan
                .project_supervised_rule_usage(&final_rules, rule_usage, uses_byok)
                .is_err()
            {
                return self
                    .finish_failed(&lease, &guard, "automatic_rule_denied_after_consume")
                    .await;
            }
        } else if let Some(adopted) = adopted_subscription {
            lease = match self
                .checkpoint_control
                .consume_subscription(&lease, adopted)
                .await
            {
                Ok(renewed) => renewed,
                Err(_) => {
                    return self
                        .finish_recovery(
                            &lease,
                            &guard,
                            AiAgentRecoveryPhase::ApplicationTool,
                            "subscription_checkpoint_consumption_failed",
                            None,
                        )
                        .await;
                }
            };
            let final_rules = match self.rule_resolver.resolve_rules(&lease, &scope).await {
                Ok(current)
                    if current.rules().fingerprint() == planned_rules.fingerprint()
                        && rule_usage.validate(&current).is_ok() =>
                {
                    current
                }
                _ => {
                    return self
                        .finish_failed(&lease, &guard, "subscription_rule_changed_after_consume")
                        .await;
                }
            };
            if provider_plan
                .project_supervised_rule_usage(&final_rules, rule_usage, uses_byok)
                .is_err()
            {
                return self
                    .finish_failed(&lease, &guard, "subscription_rule_denied_after_consume")
                    .await;
            }
        }
        let classified_bindings = provider_plan.clone();
        let resumed_checkpoint =
            adopted.is_some() || adopted_automatic.is_some() || adopted_subscription.is_some();
        let reclaimed = if resumed_checkpoint && provider_session.is_some() {
            let Some(service) = &self.provider_session_service else {
                return self
                    .finish_recovery(
                        &lease,
                        &guard,
                        AiAgentRecoveryPhase::ProviderTurn,
                        "provider_session_reclaim_service_unavailable",
                        None,
                    )
                    .await;
            };
            match service.reclaim_after_wait(&lease).await {
                Ok(claim) => Some((service.clone(), claim)),
                Err(_) => {
                    return self
                        .finish_recovery(
                            &lease,
                            &guard,
                            AiAgentRecoveryPhase::ProviderTurn,
                            "provider_session_wait_reclaim_failed",
                            None,
                        )
                        .await;
                }
            }
        } else {
            None
        };
        let provider_result = if let Some(session_plan) = provider_session {
            let Some(service) = &self.provider_session_service else {
                return self
                    .finish_recovery(
                        &lease,
                        &guard,
                        AiAgentRecoveryPhase::ProviderTurn,
                        "provider_session_service_unavailable",
                        None,
                    )
                    .await;
            };
            self.execute_retained_provider_with_heartbeats(
                &mut lease,
                provider_plan,
                session_plan,
                service.clone(),
            )
            .await
        } else {
            self.execute_provider_with_heartbeats(&mut lease, provider_plan)
                .await
        };
        let result = match provider_result {
            Ok(result) => result,
            Err(SupervisedProviderTurnFailure::Provider) => {
                if let Some((service, claim)) = &reclaimed {
                    let _ = service
                        .require_cleanup(claim, "provider_session_reclaimed_handoff_failed")
                        .await;
                }
                if self.run_control.cancellation(&lease).await?.is_some() {
                    return Ok(AiSupervisedAgentRunOutcome::Cancelled {
                        provider_turns: guard.provider_turns(),
                        total_tool_calls: guard.total_tool_calls(),
                    });
                }
                return self
                    .finish_recovery(
                        &lease,
                        &guard,
                        AiAgentRecoveryPhase::ProviderTurn,
                        "supervised_provider_turn_uncertain",
                        None,
                    )
                    .await;
            }
            Err(SupervisedProviderTurnFailure::BudgetDenied) => {
                if let Some((service, claim)) = &reclaimed {
                    let _ = service
                        .require_cleanup(claim, "provider_session_reclaimed_handoff_failed")
                        .await;
                }
                // A pre-transport reservation denial is certain and local: it
                // consumed no provider turn and left no reservation held.
                return self
                    .finish_failed(&lease, &guard, "provider_budget_denied")
                    .await;
            }
            Err(SupervisedProviderTurnFailure::PreTransportProvider) => {
                if let Some((service, claim)) = &reclaimed {
                    let _ = service
                        .require_cleanup(claim, "provider_session_reclaimed_handoff_failed")
                        .await;
                }
                // The adapter proved that no business turn crossed the
                // provider boundary. The reservation has already been
                // released and retained-session cleanup, when applicable,
                // has been durably fenced by the call executor.
                return self
                    .finish_failed(&lease, &guard, "provider_pre_transport_failed")
                    .await;
            }
            Err(SupervisedProviderTurnFailure::StatelessNativeItemRejected) => {
                if let Some((service, claim)) = &reclaimed {
                    let _ = service
                        .require_cleanup(claim, "provider_session_reclaimed_handoff_failed")
                        .await;
                }
                return self
                    .finish_failed(&lease, &guard, "provider_native_item_rejected")
                    .await;
            }
            Err(SupervisedProviderTurnFailure::LeaseLost(error)) => return Err(error),
        };
        if self.run_control.cancellation(&lease).await?.is_some() {
            self.invalidate_result_provider_session(
                &result,
                "provider_session_cancelled_after_turn",
            )
            .await;
            return Ok(AiSupervisedAgentRunOutcome::Cancelled {
                provider_turns: guard.provider_turns(),
                total_tool_calls: guard.total_tool_calls(),
            });
        }
        let observed = match guard.observe_provider_turn(&result) {
            Ok(observed) => observed,
            Err(_) => {
                self.invalidate_result_provider_session(
                    &result,
                    "provider_session_turn_binding_failed",
                )
                .await;
                return self
                    .finish_recovery(
                        &lease,
                        &guard,
                        AiAgentRecoveryPhase::ProviderTurn,
                        "supervised_provider_turn_binding_failed",
                        result.provider_response_id(),
                    )
                    .await;
            }
        };
        let current_rules = match self.rule_resolver.resolve_rules(&lease, &scope).await {
            Ok(current) if current.rules().fingerprint() == planned_rules.fingerprint() => current,
            _ => {
                self.invalidate_result_provider_session(
                    &result,
                    "provider_session_rule_changed_after_provider",
                )
                .await;
                return self
                    .finish_recovery(
                        &lease,
                        &guard,
                        AiAgentRecoveryPhase::ProviderTurn,
                        "supervised_rule_changed_after_provider",
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
                    "provider_session_rule_budget_exceeded",
                )
                .await;
                return self
                    .finish_recovery(
                        &lease,
                        &guard,
                        AiAgentRecoveryPhase::ProviderTurn,
                        "supervised_rule_budget_exceeded",
                        result.provider_response_id(),
                    )
                    .await;
            }
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
                self.invalidate_result_provider_session(
                    &result,
                    "provider_session_checkpoint_uncertain",
                )
                .await;
                return self
                    .finish_recovery(
                        &lease,
                        &guard,
                        AiAgentRecoveryPhase::ProviderTurn,
                        "supervised_provider_checkpoint_uncertain",
                        result.provider_response_id(),
                    )
                    .await;
            }
        };
        match observed {
            AiAgentLoopTurn::Completed => {
                let persisted = match self.output_writer.persist_output(&lease, &result).await {
                    Ok(persisted) => persisted,
                    Err(_) => {
                        if let (Some(service), Some(claim)) = (
                            &self.provider_session_service,
                            result.provider_session_claim(),
                        ) {
                            let _ = service
                                .require_cleanup(claim, "provider_session_output_uncertain")
                                .await;
                        }
                        return self
                            .finish_recovery(
                                &lease,
                                &guard,
                                AiAgentRecoveryPhase::ProviderOutput,
                                "supervised_output_uncertain",
                                result.provider_response_id(),
                            )
                            .await;
                    }
                };
                let message_id = persisted.message_id();
                lease = persisted.into_lease();
                if self.run_control.cancellation(&lease).await?.is_some() {
                    self.invalidate_result_provider_session(
                        &result,
                        "provider_session_cancelled_after_output",
                    )
                    .await;
                    return Ok(AiSupervisedAgentRunOutcome::Cancelled {
                        provider_turns: guard.provider_turns(),
                        total_tool_calls: guard.total_tool_calls(),
                    });
                }
                let provider_session_commit = match result.provider_session_commit(message_id) {
                    Ok(Some(commit)) => {
                        let Some(service) = &self.provider_session_service else {
                            self.invalidate_result_provider_session(
                                &result,
                                "provider_session_commit_service_unavailable",
                            )
                            .await;
                            return self
                                .finish_recovery(
                                    &lease,
                                    &guard,
                                    AiAgentRecoveryPhase::ProviderOutput,
                                    "provider_session_commit_service_unavailable",
                                    result.provider_response_id(),
                                )
                                .await;
                        };
                        let claim = result.provider_session_claim().ok_or(AiError::Conflict)?;
                        Some((service.clone(), claim.clone(), commit))
                    }
                    Ok(None) => None,
                    Err(_) => {
                        if let (Some(service), Some(claim)) = (
                            &self.provider_session_service,
                            result.provider_session_claim(),
                        ) {
                            let _ = service
                                .require_cleanup(claim, "provider_session_commit_proof_invalid")
                                .await;
                        }
                        return self
                            .finish_recovery(
                                &lease,
                                &guard,
                                AiAgentRecoveryPhase::ProviderOutput,
                                "provider_session_commit_proof_invalid",
                                result.provider_response_id(),
                            )
                            .await;
                    }
                };
                let completion = AiRunCompletion::new(
                    AiRunState::Completed,
                    "supervised_agent_completed",
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
                    let _ = service
                        .require_cleanup(&claim, "provider_session_commit_uncertain")
                        .await;
                }
                Ok(AiSupervisedAgentRunOutcome::Completed {
                    message_id,
                    provider_turns: guard.provider_turns(),
                    total_tool_calls: guard.total_tool_calls(),
                })
            }
            AiAgentLoopTurn::ToolCalls {
                provider_turn_index,
                call_count,
            } => {
                if call_count != 1 || result.provider_response_id().is_none() {
                    self.invalidate_result_provider_session(
                        &result,
                        "provider_session_tool_batch_unsupported",
                    )
                    .await;
                    return self
                        .finish_failed(&lease, &guard, "supervised_tool_batch_unsupported")
                        .await;
                }
                if !guard.has_provider_turn_capacity() {
                    self.invalidate_result_provider_session(
                        &result,
                        "provider_session_continuation_limit_reached",
                    )
                    .await;
                    return self
                        .finish_failed(&lease, &guard, "supervised_continuation_limit_reached")
                        .await;
                }
                let binding = match plan_binding(&classified_bindings, &result) {
                    Some(binding) => binding,
                    None => {
                        self.invalidate_result_provider_session(
                            &result,
                            "provider_session_classified_binding_missing",
                        )
                        .await;
                        return self
                            .finish_failed(&lease, &guard, "classified_mutation_binding_missing")
                            .await;
                    }
                };
                let rules = match self.rule_resolver.resolve_rules(&lease, &scope).await {
                    Ok(current)
                        if current.rules().fingerprint() == planned_rules.fingerprint()
                            && current.rules().constrain_tool(
                                result.tool_calls()[0].tool_fingerprint(),
                                binding.0,
                                binding.1,
                            ) == Some(binding.1) =>
                    {
                        current
                    }
                    _ => {
                        self.invalidate_result_provider_session(
                            &result,
                            "provider_session_tool_rule_denied",
                        )
                        .await;
                        return self
                            .finish_failed(&lease, &guard, "supervised_tool_rule_denied")
                            .await;
                    }
                };
                let completed_rule_usage = match rule_usage.accept_tool_calls(1, &rules) {
                    Ok(usage) => usage,
                    Err(_) => {
                        self.invalidate_result_provider_session(
                            &result,
                            "provider_session_rule_steps_exceeded",
                        )
                        .await;
                        return self
                            .finish_failed(&lease, &guard, "supervised_rule_steps_exceeded")
                            .await;
                    }
                };
                let context = AiApplicationToolCallContext::new(
                    provider_turn_index,
                    0,
                    scope.clone(),
                    correlation_id.clone(),
                    result.budget_reservation_id().0.to_string(),
                )?;
                if binding
                    == (
                        crate::ToolMaturity::AutonomousWrite,
                        crate::AiApprovalRule::None,
                    )
                {
                    let outcome = match self
                        .automatic_executor
                        .execute(&lease, &result, context, route.clone())
                        .await
                    {
                        Ok(outcome) => outcome,
                        Err(_) => {
                            self.invalidate_result_provider_session(
                                &result,
                                "provider_session_automatic_mutation_uncertain",
                            )
                            .await;
                            return self
                                .finish_recovery(
                                    &lease,
                                    &guard,
                                    AiAgentRecoveryPhase::ApplicationTool,
                                    "automatic_mutation_uncertain",
                                    result.provider_response_id(),
                                )
                                .await;
                        }
                    };
                    let Some(persisted) = outcome.persisted() else {
                        self.invalidate_result_provider_session(
                            &result,
                            "provider_session_automatic_mutation_recovery",
                        )
                        .await;
                        return Ok(AiSupervisedAgentRunOutcome::RecoveryRequired {
                            phase: AiAgentRecoveryPhase::ApplicationTool,
                            provider_turns: guard.provider_turns(),
                            total_tool_calls: guard.total_tool_calls(),
                        });
                    };
                    if guard.observe_tool_result(persisted).is_err() {
                        self.invalidate_result_provider_session(
                            &result,
                            "provider_session_automatic_result_invalid",
                        )
                        .await;
                        return self
                            .finish_recovery(
                                persisted.lease(),
                                &guard,
                                AiAgentRecoveryPhase::ApplicationTool,
                                "automatic_mutation_result_invalid",
                                result.provider_response_id(),
                            )
                            .await;
                    }
                    if self
                        .run_control
                        .cancellation(persisted.lease())
                        .await?
                        .is_some()
                    {
                        self.invalidate_result_provider_session(
                            &result,
                            "provider_session_automatic_cancelled_after_effect",
                        )
                        .await;
                        return self
                            .finish_recovery(
                                persisted.lease(),
                                &guard,
                                AiAgentRecoveryPhase::ApplicationTool,
                                "automatic_mutation_cancelled_after_effect",
                                result.provider_response_id(),
                            )
                            .await;
                    }
                    let continuation = match guard.continuation() {
                        Ok(continuation) => continuation,
                        Err(_) => {
                            self.invalidate_result_provider_session(
                                &result,
                                "provider_session_automatic_continuation_invalid",
                            )
                            .await;
                            return self
                                .finish_recovery(
                                    persisted.lease(),
                                    &guard,
                                    AiAgentRecoveryPhase::ApplicationTool,
                                    "automatic_mutation_continuation_invalid",
                                    result.provider_response_id(),
                                )
                                .await;
                        }
                    };
                    lease = match self
                        .checkpoint_writer
                        .persist_automatic_mutation_batch(
                            persisted.lease(),
                            &result,
                            persisted,
                            &continuation,
                            &scope,
                            &correlation_id,
                            &route,
                            &planned_rules,
                            completed_rule_usage,
                            guard.provider_turns(),
                            guard.total_tool_calls(),
                        )
                        .await
                    {
                        Ok(renewed) => renewed,
                        Err(_) => {
                            self.invalidate_result_provider_session(
                                &result,
                                "provider_session_automatic_checkpoint_uncertain",
                            )
                            .await;
                            return self
                                .finish_recovery(
                                    persisted.lease(),
                                    &guard,
                                    AiAgentRecoveryPhase::ApplicationTool,
                                    "automatic_mutation_checkpoint_uncertain",
                                    result.provider_response_id(),
                                )
                                .await;
                        }
                    };
                    let adopted = match self.checkpoint_control.adopt_automatic(&lease).await {
                        Ok(Some(adopted)) => adopted,
                        Ok(None) | Err(_) => {
                            self.invalidate_result_provider_session(
                                &result,
                                "provider_session_automatic_adoption_failed",
                            )
                            .await;
                            return self
                                .finish_recovery(
                                    &lease,
                                    &guard,
                                    AiAgentRecoveryPhase::ApplicationTool,
                                    "automatic_mutation_checkpoint_adoption_failed",
                                    result.provider_response_id(),
                                )
                                .await;
                        }
                    };
                    return Box::pin(self.continue_from_automatic(lease, adopted)).await;
                }
                let expires_at = self
                    .clock
                    .now()
                    .checked_add(self.limits.approval_ttl)
                    .ok_or(AiError::PersistenceFailed)?;
                let wait = match self
                    .approval_stager
                    .stage(
                        &lease,
                        &result,
                        context,
                        expires_at,
                        self.limits.recent_mfa_required,
                    )
                    .await
                {
                    Ok(wait) => wait,
                    Err(_) => {
                        return self
                            .finish_recovery(
                                &lease,
                                &guard,
                                AiAgentRecoveryPhase::ApplicationTool,
                                "supervised_approval_staging_uncertain",
                                result.provider_response_id(),
                            )
                            .await;
                    }
                };
                if wait.lease().state() != AiRunState::WaitingApproval {
                    return Err(AiError::Conflict);
                }
                Ok(AiSupervisedAgentRunOutcome::WaitingApproval {
                    approval_id: wait.approval_id(),
                    tool_call_id: wait.tool_call_id(),
                    provider_turns: guard.provider_turns(),
                    total_tool_calls: guard.total_tool_calls(),
                })
            }
        }
    }

    async fn execute_provider_with_heartbeats(
        &self,
        lease: &mut AiRunLease,
        plan: AiProviderCallPlan,
    ) -> Result<crate::AiProviderCallResult, SupervisedProviderTurnFailure> {
        let provider_lease = lease.clone();
        let provider = self.provider_executor.execute_turn(&provider_lease, plan);
        tokio::pin!(provider);
        let delay = self.limits.heartbeat_interval.unsigned_abs();
        loop {
            let heartbeat = tokio::time::sleep(delay);
            tokio::pin!(heartbeat);
            tokio::select! {
                result = &mut provider => {
                    return result.map_err(|error| classify_supervised_turn_failure(&error));
                }
                () = &mut heartbeat => {
                    *lease = self
                        .run_control
                        .heartbeat(lease)
                        .await
                        .map_err(SupervisedProviderTurnFailure::LeaseLost)?;
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
    ) -> Result<crate::AiProviderCallResult, SupervisedProviderTurnFailure> {
        let lease_state = Arc::new(Mutex::new(lease.clone()));
        let provider = self.provider_executor.execute_retained_turn(
            lease_state.clone(),
            plan,
            session_plan,
            session_service,
            None,
        );
        tokio::pin!(provider);
        let delay = self.limits.heartbeat_interval.unsigned_abs();
        loop {
            let heartbeat = tokio::time::sleep(delay);
            tokio::pin!(heartbeat);
            tokio::select! {
                result = &mut provider => {
                    *lease = lease_state.lock().await.clone();
                    return result.map_err(|error| classify_supervised_turn_failure(&error));
                }
                () = &mut heartbeat => {
                    let current = lease_state.lock().await.clone();
                    let renewed = self
                        .run_control
                        .heartbeat(&current)
                        .await
                        .map_err(SupervisedProviderTurnFailure::LeaseLost)?;
                    *lease = renewed.clone();
                    *lease_state.lock().await = renewed;
                }
            }
        }
    }

    async fn finish_failed(
        &self,
        lease: &AiRunLease,
        guard: &AiAgentLoopGuard,
        code: &str,
    ) -> Result<AiSupervisedAgentRunOutcome, AiError> {
        let completion =
            AiRunCompletion::new(AiRunState::Failed, code, Some(code.to_owned()), None)?;
        self.run_control.finish(lease, completion).await?;
        Ok(AiSupervisedAgentRunOutcome::Failed {
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
    ) -> Result<AiSupervisedAgentRunOutcome, AiError> {
        self.finish_recovery_counts(
            lease,
            phase,
            code,
            provider_response_id,
            guard.provider_turns(),
            guard.total_tool_calls(),
        )
        .await
    }

    async fn finish_recovery_counts(
        &self,
        lease: &AiRunLease,
        phase: AiAgentRecoveryPhase,
        code: &str,
        provider_response_id: Option<&str>,
        provider_turns: u32,
        total_tool_calls: u32,
    ) -> Result<AiSupervisedAgentRunOutcome, AiError> {
        let completion = AiRunCompletion::new(
            AiRunState::RecoveryRequired,
            code,
            Some(code.to_owned()),
            provider_response_id.map(str::to_owned),
        )?;
        self.run_control.finish(lease, completion).await?;
        Ok(AiSupervisedAgentRunOutcome::RecoveryRequired {
            phase,
            provider_turns,
            total_tool_calls,
        })
    }
}

enum SupervisedProviderTurnFailure {
    Provider,
    BudgetDenied,
    PreTransportProvider,
    StatelessNativeItemRejected,
    LeaseLost(AiError),
}

/// Separates proof-bearing refusals from an uncertain turn.
///
/// See the read-only coordinator for the full argument. Budget denial, typed
/// pre-dispatch rejection, and a provider-session cleanup deferral all prove
/// that no business turn crossed the provider boundary.
const fn classify_supervised_turn_failure(error: &AiError) -> SupervisedProviderTurnFailure {
    match error {
        AiError::PreTransportBudgetDenied => SupervisedProviderTurnFailure::BudgetDenied,
        AiError::PreTransportProviderFailed | AiError::ProviderSessionDeferred => {
            SupervisedProviderTurnFailure::PreTransportProvider
        }
        AiError::StatelessNativeItemRejected => {
            SupervisedProviderTurnFailure::StatelessNativeItemRejected
        }
        _ => SupervisedProviderTurnFailure::Provider,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use agql_auth::{
        AccessTokenMetadata, AuthPrincipal, AuthUser, FixedClock, PrincipalReference,
        SessionContext,
    };
    use serde_json::json;

    use super::*;
    use crate::{
        AiAgentContinuation, AiAgentRuleResolution, AiDataSourceRef, AiDestinationTrust,
        AiEgressCapability, AiEgressManifest, AiPersistedApplicationToolCall,
        AiPersistedProviderOutput, AiRuleApprovalRequirement, AiRuleBudgetCeilings,
        AiRuleConstraints, AiSourceTrust, DataClassification, ModelContinuation, ToolMaturity,
    };

    struct TestRunControl {
        finishes: Mutex<Vec<AiRunState>>,
    }

    impl TestRunControl {
        fn new() -> Self {
            Self {
                finishes: Mutex::new(Vec::new()),
            }
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
            Ok(lease.clone())
        }

        async fn finish(
            &self,
            _lease: &AiRunLease,
            completion: AiRunCompletion,
        ) -> Result<(), AiError> {
            self.finishes
                .lock()
                .expect("test finish lock should not be poisoned")
                .push(completion.final_state());
            Ok(())
        }
    }

    struct TestProviderExecutor {
        responses: Mutex<VecDeque<Result<crate::AiProviderCallResult, AiError>>>,
        require_checkpoint_cleared: bool,
        calls: AtomicUsize,
    }

    impl TestProviderExecutor {
        fn remaining_responses(&self) -> usize {
            self.responses
                .lock()
                .expect("test provider lock should not be poisoned")
                .len()
        }
    }

    #[async_trait]
    impl AiAgentProviderTurnExecutor for TestProviderExecutor {
        async fn execute_turn(
            &self,
            lease: &AiRunLease,
            _plan: AiProviderCallPlan,
        ) -> Result<crate::AiProviderCallResult, AiError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.require_checkpoint_cleared && lease.latest_checkpoint_id().is_some() {
                return Err(AiError::Conflict);
            }
            self.responses
                .lock()
                .expect("test provider lock should not be poisoned")
                .pop_front()
                .expect("test provider response should exist")
        }
    }

    struct RetainedSupervisedProviderExecutor {
        result: Mutex<Option<crate::AiProviderCallResult>>,
        claim: crate::AiProviderSessionClaim,
        reclaimed: Arc<AtomicBool>,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl AiAgentProviderTurnExecutor for RetainedSupervisedProviderExecutor {
        async fn execute_turn(
            &self,
            _lease: &AiRunLease,
            _plan: AiProviderCallPlan,
        ) -> Result<crate::AiProviderCallResult, AiError> {
            Err(AiError::Conflict)
        }

        async fn execute_retained_turn(
            &self,
            lease: Arc<tokio::sync::Mutex<AiRunLease>>,
            _plan: AiProviderCallPlan,
            _session_plan: crate::AiProviderSessionTurnPlan,
            _session_service: Arc<dyn crate::AiProviderSessionService>,
            _execution: Option<Arc<dyn crate::AiProviderDynamicToolExecution>>,
        ) -> Result<crate::AiProviderCallResult, AiError> {
            if lease.lock().await.latest_checkpoint_id().is_some()
                || !self.reclaimed.load(Ordering::SeqCst)
            {
                return Err(AiError::Conflict);
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result
                .lock()
                .expect("retained result lock should not be poisoned")
                .take()
                .ok_or(AiError::Conflict)
                .map(|result| result.test_with_provider_session_claim(self.claim.clone()))
        }
    }

    struct RetainedSupervisedSessionService {
        claim: Mutex<Option<crate::AiProviderSessionClaim>>,
        run: Arc<TestRunControl>,
        reclaimed: Arc<AtomicBool>,
        commits: AtomicUsize,
        cleanups: AtomicUsize,
    }

    #[async_trait]
    impl crate::AiProviderSessionService for RetainedSupervisedSessionService {
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
            claim: &crate::AiProviderSessionClaim,
        ) -> Result<crate::AiProviderSessionClaim, AiError> {
            Ok(claim.clone())
        }

        async fn reclaim_after_wait(
            &self,
            lease: &AiRunLease,
        ) -> Result<crate::AiProviderSessionClaim, AiError> {
            let claim = self
                .claim
                .lock()
                .expect("retained claim lock should not be poisoned")
                .take()
                .ok_or(AiError::Conflict)?;
            if claim.run_id() != lease.run_id()
                || claim.attempt_id() != lease.attempt_id()
                || claim.run_lease_generation() != lease.lease_generation()
                || lease.latest_checkpoint_id().is_some()
            {
                return Err(AiError::Conflict);
            }
            self.reclaimed.store(true, Ordering::SeqCst);
            Ok(claim)
        }

        async fn commit_turn(
            &self,
            _lease: &AiRunLease,
            claim: &crate::AiProviderSessionClaim,
            commit: crate::AiProviderSessionCommit,
        ) -> Result<crate::AiProviderSessionBindingView, AiError> {
            if self.run.final_states() != [AiRunState::Completed]
                || !self.reclaimed.load(Ordering::SeqCst)
            {
                return Err(AiError::Conflict);
            }
            self.commits.fetch_add(1, Ordering::SeqCst);
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

    struct RetainedSupervisedPlanner {
        scope: AiScope,
        route: AiToolResultEgressRoute,
        provider_session: crate::AiProviderSessionTurnPlan,
        continuation_count: AtomicUsize,
    }

    #[async_trait]
    impl AiSupervisedAgentTurnPlanner for RetainedSupervisedPlanner {
        async fn initial_plan(
            &self,
            lease: &AiRunLease,
        ) -> Result<AiSupervisedAgentTurnPlan, AiError> {
            Ok(AiSupervisedAgentTurnPlan::new(
                AiProviderCallPlan::test_supervised_plan(lease, self.scope.clone(), false),
                self.route.clone(),
                test_rules(self.scope.clone()),
                false,
            )?
            .test_with_provider_session(self.provider_session.clone()))
        }

        async fn continuation_plan(
            &self,
            lease: &AiRunLease,
            _provider_turns: u32,
            _continuation: AiAgentContinuation,
        ) -> Result<AiSupervisedAgentTurnPlan, AiError> {
            self.continuation_count.fetch_add(1, Ordering::SeqCst);
            Ok(AiSupervisedAgentTurnPlan::new(
                AiProviderCallPlan::test_supervised_plan(lease, self.scope.clone(), true),
                self.route.clone(),
                test_rules(self.scope.clone()),
                false,
            )?
            .test_with_provider_session(self.provider_session.clone()))
        }
    }

    #[async_trait]
    impl AiSupervisedAgentTurnPlanner for TestPlanner {
        async fn initial_plan(
            &self,
            lease: &AiRunLease,
        ) -> Result<AiSupervisedAgentTurnPlan, AiError> {
            AiSupervisedAgentTurnPlan::new(
                AiProviderCallPlan::test_supervised_plan(lease, self.scope.clone(), false),
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
        ) -> Result<AiSupervisedAgentTurnPlan, AiError> {
            self.continuation_count.fetch_add(1, Ordering::SeqCst);
            AiSupervisedAgentTurnPlan::new(
                AiProviderCallPlan::test_supervised_plan(lease, self.scope.clone(), true),
                self.route.clone(),
                test_rules(self.scope.clone()),
                false,
            )
        }
    }

    struct TestOutputWriter;

    #[async_trait]
    impl AiAgentProviderOutputWriter for TestOutputWriter {
        async fn persist_output(
            &self,
            lease: &AiRunLease,
            _result: &crate::AiProviderCallResult,
        ) -> Result<AiPersistedProviderOutput, AiError> {
            Ok(AiPersistedProviderOutput::test_output(lease.clone()))
        }
    }

    struct TestCheckpointWriter {
        provider_checkpoints: AtomicUsize,
    }

    #[async_trait]
    impl AiAgentCheckpointWriter for TestCheckpointWriter {
        async fn persist_provider_turn(
            &self,
            lease: &AiRunLease,
            result: &crate::AiProviderCallResult,
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
            self.provider_checkpoints.fetch_add(1, Ordering::SeqCst);
            Ok(lease.test_with_checkpoint(Uuid::new_v4()))
        }

        async fn persist_tool_batch(
            &self,
            _lease: &AiRunLease,
            _result: &crate::AiProviderCallResult,
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

    struct TestCheckpointControl {
        adopted: Mutex<Option<AiAdoptedSupervisedToolBatch>>,
        consumed: AtomicBool,
    }

    #[async_trait]
    impl AiAgentSupervisedCheckpointControl for TestCheckpointControl {
        async fn adopt(
            &self,
            lease: &AiRunLease,
        ) -> Result<Option<AiAdoptedSupervisedToolBatch>, AiError> {
            let adopted = self
                .adopted
                .lock()
                .expect("test adoption lock should not be poisoned")
                .take();
            if adopted
                .as_ref()
                .map(AiAdoptedSupervisedToolBatch::checkpoint_id)
                != lease.latest_checkpoint_id()
            {
                return Err(AiError::Conflict);
            }
            Ok(adopted)
        }

        async fn consume(
            &self,
            lease: &AiRunLease,
            adopted: &AiAdoptedSupervisedToolBatch,
        ) -> Result<AiRunLease, AiError> {
            if lease.latest_checkpoint_id() != Some(adopted.checkpoint_id())
                || self.consumed.swap(true, Ordering::SeqCst)
            {
                return Err(AiError::Conflict);
            }
            Ok(lease.test_without_checkpoint())
        }
    }

    struct TestSubscriptionCheckpointControl {
        adopted: Mutex<Option<crate::AiAdoptedReadOnlyToolBatch>>,
        consumed: AtomicBool,
    }

    #[async_trait]
    impl AiAgentSupervisedCheckpointControl for TestSubscriptionCheckpointControl {
        async fn adopt(
            &self,
            _lease: &AiRunLease,
        ) -> Result<Option<AiAdoptedSupervisedToolBatch>, AiError> {
            Ok(None)
        }

        async fn consume(
            &self,
            _lease: &AiRunLease,
            _adopted: &AiAdoptedSupervisedToolBatch,
        ) -> Result<AiRunLease, AiError> {
            Err(AiError::Conflict)
        }

        async fn adopt_subscription(
            &self,
            lease: &AiRunLease,
        ) -> Result<Option<crate::AiAdoptedReadOnlyToolBatch>, AiError> {
            let adopted = self
                .adopted
                .lock()
                .expect("test subscription adoption lock should not be poisoned")
                .take();
            if adopted
                .as_ref()
                .map(crate::AiAdoptedReadOnlyToolBatch::checkpoint_id)
                != lease.latest_checkpoint_id()
            {
                return Err(AiError::Conflict);
            }
            Ok(adopted)
        }

        async fn consume_subscription(
            &self,
            lease: &AiRunLease,
            adopted: &crate::AiAdoptedReadOnlyToolBatch,
        ) -> Result<AiRunLease, AiError> {
            if lease.latest_checkpoint_id() != Some(adopted.checkpoint_id())
                || self.consumed.swap(true, Ordering::SeqCst)
            {
                return Err(AiError::Conflict);
            }
            Ok(lease.test_without_checkpoint())
        }
    }

    struct TestApprovalStager {
        calls: AtomicUsize,
        saw_checkpoint: AtomicBool,
    }

    struct TestAutomaticExecutor;

    #[async_trait]
    impl AiAgentAutomaticMutationExecutor for TestAutomaticExecutor {
        async fn execute(
            &self,
            _lease: &AiRunLease,
            _result: &crate::AiProviderCallResult,
            _context: AiApplicationToolCallContext,
            _route: AiToolResultEgressRoute,
        ) -> Result<crate::AiConsequentialToolCallOutcome, AiError> {
            Err(AiError::Conflict)
        }
    }

    fn unused_automatic() -> Arc<TestAutomaticExecutor> {
        Arc::new(TestAutomaticExecutor)
    }

    struct TestSuccessfulAutomaticExecutor {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl AiAgentAutomaticMutationExecutor for TestSuccessfulAutomaticExecutor {
        async fn execute(
            &self,
            lease: &AiRunLease,
            result: &crate::AiProviderCallResult,
            _context: AiApplicationToolCallContext,
            _route: AiToolResultEgressRoute,
        ) -> Result<crate::AiConsequentialToolCallOutcome, AiError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let call = result.tool_calls().first().ok_or(AiError::Conflict)?;
            Ok(crate::AiConsequentialToolCallOutcome::Persisted(Box::new(
                AiPersistedApplicationToolCall::test_completed(
                    lease.clone(),
                    call.call_id(),
                    call.tool_id().as_str(),
                    Some(json!({"updated": true})),
                    Some(test_manifest(lease)),
                ),
            )))
        }
    }

    struct TestAutomaticCheckpointWriter {
        provider_checkpoints: AtomicUsize,
        automatic_checkpoints: AtomicUsize,
    }

    #[async_trait]
    impl AiAgentCheckpointWriter for TestAutomaticCheckpointWriter {
        async fn persist_provider_turn(
            &self,
            lease: &AiRunLease,
            _result: &crate::AiProviderCallResult,
            _scope: &AiScope,
            _correlation_id: &str,
            _route: &AiToolResultEgressRoute,
            _rules: &AiResolvedRuleSet,
            _rule_usage: AiRuleRunUsage,
            _provider_turns: u32,
            _total_tool_calls: u32,
        ) -> Result<AiRunLease, AiError> {
            self.provider_checkpoints.fetch_add(1, Ordering::SeqCst);
            Ok(lease.test_with_checkpoint(Uuid::new_v4()))
        }

        async fn persist_tool_batch(
            &self,
            _lease: &AiRunLease,
            _result: &crate::AiProviderCallResult,
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

        async fn persist_automatic_mutation_batch(
            &self,
            lease: &AiRunLease,
            result: &crate::AiProviderCallResult,
            completed_tool: &AiPersistedApplicationToolCall,
            _continuation: &AiAgentContinuation,
            _scope: &AiScope,
            _correlation_id: &str,
            _route: &AiToolResultEgressRoute,
            _rules: &AiResolvedRuleSet,
            _rule_usage: AiRuleRunUsage,
            _provider_turns: u32,
            _total_tool_calls: u32,
        ) -> Result<AiRunLease, AiError> {
            if result.tool_calls().len() != 1
                || completed_tool.provider_call_id() != result.tool_calls()[0].call_id()
            {
                return Err(AiError::Conflict);
            }
            self.automatic_checkpoints.fetch_add(1, Ordering::SeqCst);
            Ok(lease.test_with_checkpoint(Uuid::new_v4()))
        }
    }

    struct TestAutomaticCheckpointControl {
        consumed: AtomicBool,
    }

    #[async_trait]
    impl AiAgentSupervisedCheckpointControl for TestAutomaticCheckpointControl {
        async fn adopt(
            &self,
            _lease: &AiRunLease,
        ) -> Result<Option<AiAdoptedSupervisedToolBatch>, AiError> {
            Ok(None)
        }

        async fn consume(
            &self,
            _lease: &AiRunLease,
            _adopted: &AiAdoptedSupervisedToolBatch,
        ) -> Result<AiRunLease, AiError> {
            Err(AiError::Conflict)
        }

        async fn adopt_automatic(
            &self,
            lease: &AiRunLease,
        ) -> Result<Option<AiAdoptedAutomaticMutationBatch>, AiError> {
            let checkpoint_id = lease.latest_checkpoint_id().ok_or(AiError::Conflict)?;
            let result = crate::AiProviderCallResult::test_result(
                lease,
                None,
                "automatic-response",
                vec![("automatic-call", "test.write", json!({"value": 7}))],
            );
            let usage = AiRuleRunUsage::default()
                .accept_provider_with_web_searches(
                    result.usage(),
                    0,
                    &AiAgentRuleResolution::new(
                        test_rules(test_scope()),
                        time::OffsetDateTime::now_utc(),
                    )?,
                )
                .and_then(|usage| {
                    usage.accept_tool_calls(
                        1,
                        &AiAgentRuleResolution::new(
                            test_rules(test_scope()),
                            time::OffsetDateTime::now_utc(),
                        )?,
                    )
                })?;
            let completed = AiPersistedApplicationToolCall::test_completed(
                lease.clone(),
                "automatic-call",
                "test.write",
                Some(json!({"updated": true})),
                Some(test_manifest(lease)),
            );
            let continuation = AiAgentContinuation::from_persisted_results(
                ModelContinuation::ProviderResponse {
                    response_id: "automatic-response".to_owned(),
                },
                crate::ModelReasoningEffort::Unspecified,
                &[completed],
                Vec::new(),
            )?;
            Ok(Some(AiAdoptedAutomaticMutationBatch::new(
                crate::AiAdoptedReadOnlyToolBatch::new(
                    checkpoint_id,
                    1,
                    1,
                    test_scope(),
                    continuation,
                    test_rules(test_scope()).fingerprint().to_owned(),
                    usage,
                ),
            )))
        }

        async fn consume_automatic(
            &self,
            lease: &AiRunLease,
            adopted: &AiAdoptedAutomaticMutationBatch,
        ) -> Result<AiRunLease, AiError> {
            if lease.latest_checkpoint_id() != Some(adopted.checkpoint_id())
                || self.consumed.swap(true, Ordering::SeqCst)
            {
                return Err(AiError::Conflict);
            }
            Ok(lease.test_without_checkpoint())
        }
    }

    struct TestAutomaticPlanner {
        scope: AiScope,
        route: AiToolResultEgressRoute,
        continuation_count: AtomicUsize,
    }

    #[async_trait]
    impl AiSupervisedAgentTurnPlanner for TestAutomaticPlanner {
        async fn initial_plan(
            &self,
            lease: &AiRunLease,
        ) -> Result<AiSupervisedAgentTurnPlan, AiError> {
            AiSupervisedAgentTurnPlan::new(
                AiProviderCallPlan::test_automatic_mutation_plan(lease, self.scope.clone(), false),
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
        ) -> Result<AiSupervisedAgentTurnPlan, AiError> {
            self.continuation_count.fetch_add(1, Ordering::SeqCst);
            AiSupervisedAgentTurnPlan::new(
                AiProviderCallPlan::test_automatic_mutation_plan(lease, self.scope.clone(), true),
                self.route.clone(),
                test_rules(self.scope.clone()),
                false,
            )
        }
    }

    #[async_trait]
    impl AiAgentSupervisedApprovalStager for TestApprovalStager {
        async fn stage(
            &self,
            lease: &AiRunLease,
            result: &crate::AiProviderCallResult,
            _context: AiApplicationToolCallContext,
            _expires_at: time::OffsetDateTime,
            _recent_mfa_required: bool,
        ) -> Result<AiSupervisedApprovalWait, AiError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if lease.latest_checkpoint_id().is_none() || result.tool_calls().len() != 1 {
                return Err(AiError::Conflict);
            }
            self.saw_checkpoint.store(true, Ordering::SeqCst);
            Ok(AiSupervisedApprovalWait {
                approval_id: AiApprovalId::new(),
                tool_call_id: AiToolCallId::new(),
                lease: lease.test_with_state(AiRunState::WaitingApproval),
            })
        }
    }

    struct TestResumeExecutor {
        outcome: Mutex<Option<AiSupervisedResumeOutcome>>,
    }

    #[async_trait]
    impl AiAgentSupervisedResumeExecutor for TestResumeExecutor {
        async fn resume(
            &self,
            _claim: &crate::AiApprovedRunClaim,
        ) -> Result<AiSupervisedResumeOutcome, AiError> {
            self.outcome
                .lock()
                .expect("test resume lock should not be poisoned")
                .take()
                .ok_or(AiError::Conflict)
        }
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

    fn principal_reference() -> PrincipalReference {
        AuthPrincipal::User(AuthUser {
            user_id: "supervised-coordinator-user".to_owned(),
            session_id: Uuid::new_v4(),
            roles: Vec::new(),
            scopes: Vec::new(),
            session: SessionContext::default(),
            token_claims: AccessTokenMetadata {
                tenant_id: Some("supervised-coordinator-tenant".to_owned()),
                ..AccessTokenMetadata::default()
            },
        })
        .reference()
    }

    fn test_scope() -> AiScope {
        AiScope::new("test", "supervised-coordinator-scope")
            .with_tenant_id("supervised-coordinator-tenant")
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
            AiRuleConstraints {
                enabled: true,
                maximum_classification: DataClassification::Restricted,
                maximum_tool_maturity: ToolMaturity::AutonomousWrite,
                approval_requirement: AiRuleApprovalRequirement::DescriptorPolicy,
                allowed_tool_fingerprints: None,
                allowed_provider_kinds: None,
                allowed_provider_capabilities: None,
                allow_provider_retention: true,
                allow_byok: true,
                budget: AiRuleBudgetCeilings {
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

    fn test_route() -> AiToolResultEgressRoute {
        AiToolResultEgressRoute::new(
            "test-provider-profile",
            "test-provider-boundary",
            AiDestinationTrust::ExternalProcessor,
            "supervised-agent-test",
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

    fn test_manifest(lease: &AiRunLease) -> AiEgressManifest {
        AiEgressManifest {
            provider_profile_id: "test-provider-profile".to_owned(),
            provider_kind: "openai".to_owned(),
            model: "coordinator-test-model".to_owned(),
            destination: "test-provider-boundary".to_owned(),
            destination_trust: AiDestinationTrust::ExternalProcessor,
            capability: AiEgressCapability::ToolResult,
            scope: test_scope(),
            session_id: Some(lease.session_id()),
            run_id: Some(lease.run_id()),
            sources: vec![AiDataSourceRef {
                kind: "application_tool_result".to_owned(),
                reference: Uuid::new_v4().to_string(),
                classification: DataClassification::Public,
                trust: AiSourceTrust::ResolverResult,
            }],
            estimated_bytes: 64,
            estimated_tokens: 0,
            attachment_count: 0,
            purpose: "supervised-agent-test".to_owned(),
            retention: "none".to_owned(),
            residency: None,
            policy_version: "egress-v1".to_owned(),
            consent_reference: None,
        }
    }

    fn adopted_checkpoint(lease: &AiRunLease, checkpoint_id: Uuid) -> AiAdoptedSupervisedToolBatch {
        let old_result = crate::AiProviderCallResult::test_result(
            lease,
            None,
            "test-previous-response",
            vec![("test-call", "test.write", json!({"value": "approved"}))],
        );
        let resolution =
            AiAgentRuleResolution::new(test_rules(test_scope()), time::OffsetDateTime::now_utc())
                .expect("test rules should resolve");
        let usage = AiRuleRunUsage::default()
            .accept_provider_with_web_searches(old_result.usage(), 0, &resolution)
            .and_then(|usage| usage.accept_tool_calls(1, &resolution))
            .expect("adopted usage should fit test rules");
        let completed = AiPersistedApplicationToolCall::test_completed(
            lease.clone(),
            "test-call",
            "test.write",
            Some(json!({"updated": true})),
            Some(test_manifest(lease)),
        );
        let continuation = AiAgentContinuation::from_persisted_results(
            ModelContinuation::ProviderResponse {
                response_id: "test-previous-response".to_owned(),
            },
            crate::ModelReasoningEffort::Unspecified,
            &[completed],
            Vec::new(),
        )
        .expect("test continuation should bind");
        AiAdoptedSupervisedToolBatch::test_adopted(
            checkpoint_id,
            1,
            1,
            test_scope(),
            continuation,
            resolution.rules().fingerprint().to_owned(),
            usage,
        )
    }

    fn adopted_subscription_checkpoint(
        lease: &AiRunLease,
        checkpoint_id: Uuid,
    ) -> crate::AiAdoptedReadOnlyToolBatch {
        let old_result = crate::AiProviderCallResult::test_result(
            lease,
            None,
            "test-previous-response",
            vec![(
                "subscription-call",
                "test.subscription",
                json!({"maximumEvents": 1}),
            )],
        );
        let resolution =
            AiAgentRuleResolution::new(test_rules(test_scope()), time::OffsetDateTime::now_utc())
                .expect("test rules should resolve");
        let usage = AiRuleRunUsage::default()
            .accept_provider_with_web_searches(old_result.usage(), 0, &resolution)
            .and_then(|usage| usage.accept_tool_calls(1, &resolution))
            .expect("subscription usage should fit test rules");
        let completed = AiPersistedApplicationToolCall::test_completed(
            lease.clone(),
            "subscription-call",
            "test.subscription",
            Some(json!({"event": {"id": "wanted"}})),
            Some(test_manifest(lease)),
        );
        let continuation = AiAgentContinuation::from_persisted_results(
            ModelContinuation::ProviderResponse {
                response_id: "test-previous-response".to_owned(),
            },
            crate::ModelReasoningEffort::Unspecified,
            &[completed],
            Vec::new(),
        )
        .expect("subscription continuation should bind");
        crate::AiAdoptedReadOnlyToolBatch::new(
            checkpoint_id,
            1,
            1,
            test_scope(),
            continuation,
            resolution.rules().fingerprint().to_owned(),
            usage,
        )
    }

    fn limits() -> AiSupervisedAgentCoordinatorLimits {
        limits_with_provider_turns(4)
    }

    fn limits_with_provider_turns(
        maximum_provider_turns: u32,
    ) -> AiSupervisedAgentCoordinatorLimits {
        AiSupervisedAgentCoordinatorLimits::new(
            AiAgentLoopLimits::new(maximum_provider_turns, 8)
                .expect("test loop limits should validate"),
            Duration::milliseconds(50),
            Duration::minutes(10),
            true,
        )
        .expect("test supervised limits should validate")
    }

    fn unused_resume() -> Arc<TestResumeExecutor> {
        Arc::new(TestResumeExecutor {
            outcome: Mutex::new(Some(AiSupervisedResumeOutcome::RecoveryRequired {
                tool_call_id: AiToolCallId::new(),
                provider_turns: 0,
                total_tool_calls: 0,
            })),
        })
    }

    #[tokio::test]
    async fn approved_retained_continuation_reclaims_then_commits_terminal_turn() {
        let base = AiRunLease::test_running(principal_reference());
        let checkpoint_id = Uuid::new_v4();
        let claimed = base.test_with_checkpoint(checkpoint_id);
        let control = Arc::new(TestCheckpointControl {
            adopted: Mutex::new(Some(adopted_checkpoint(&base, checkpoint_id))),
            consumed: AtomicBool::new(false),
        });
        let descriptor = retained_descriptor();
        let claim = retained_claim(&claimed, descriptor.clone());
        let reclaimed = Arc::new(AtomicBool::new(false));
        let provider = Arc::new(RetainedSupervisedProviderExecutor {
            result: Mutex::new(Some(crate::AiProviderCallResult::test_result(
                &claimed,
                Some("test-previous-response".to_owned()),
                "supervised-retained-final",
                Vec::new(),
            ))),
            claim: claim.clone(),
            reclaimed: reclaimed.clone(),
            calls: AtomicUsize::new(0),
        });
        let run = Arc::new(TestRunControl::new());
        let sessions = Arc::new(RetainedSupervisedSessionService {
            claim: Mutex::new(Some(claim)),
            run: run.clone(),
            reclaimed,
            commits: AtomicUsize::new(0),
            cleanups: AtomicUsize::new(0),
        });
        let planner = Arc::new(RetainedSupervisedPlanner {
            scope: test_scope(),
            route: test_route(),
            provider_session: crate::AiProviderSessionTurnPlan::new(descriptor, "c".repeat(64))
                .expect("retained plan should validate"),
            continuation_count: AtomicUsize::new(0),
        });
        let coordinator = AiSupervisedAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            Arc::new(TestOutputWriter),
            Arc::new(TestCheckpointWriter {
                provider_checkpoints: AtomicUsize::new(0),
            }),
            control.clone(),
            Arc::new(TestApprovalStager {
                calls: AtomicUsize::new(0),
                saw_checkpoint: AtomicBool::new(false),
            }),
            unused_automatic(),
            unused_resume(),
            Arc::new(TestRuleResolver),
            planner.clone(),
            Arc::new(FixedClock::new(time::OffsetDateTime::now_utc())),
            limits(),
        )
        .with_provider_session_service(sessions.clone());

        let outcome = coordinator
            .execute_claimed(&claimed)
            .await
            .expect("retained supervised continuation should complete");

        assert!(
            matches!(
                outcome,
                AiSupervisedAgentRunOutcome::Completed {
                    provider_turns: 2,
                    total_tool_calls: 1,
                    ..
                }
            ),
            "unexpected outcome: {outcome:?}; provider calls {}; reclaimed {}; final states {:?}",
            provider.calls.load(Ordering::SeqCst),
            sessions.reclaimed.load(Ordering::SeqCst),
            run.final_states(),
        );
        assert!(control.consumed.load(Ordering::SeqCst));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(planner.continuation_count.load(Ordering::SeqCst), 1);
        assert_eq!(sessions.commits.load(Ordering::SeqCst), 1);
        assert_eq!(sessions.cleanups.load(Ordering::SeqCst), 0);
        assert_eq!(run.final_states(), vec![AiRunState::Completed]);
    }

    #[tokio::test]
    async fn automatic_mutation_checkpoints_once_and_continues_to_final_output() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([
                Ok(crate::AiProviderCallResult::test_result(
                    &lease,
                    None,
                    "automatic-response",
                    vec![("automatic-call", "test.write", json!({"value": 7}))],
                )),
                Ok(crate::AiProviderCallResult::test_result(
                    &lease,
                    Some("automatic-response".to_owned()),
                    "automatic-completed-response",
                    Vec::new(),
                )),
            ])),
            require_checkpoint_cleared: true,
            calls: AtomicUsize::new(0),
        });
        let checkpoints = Arc::new(TestAutomaticCheckpointWriter {
            provider_checkpoints: AtomicUsize::new(0),
            automatic_checkpoints: AtomicUsize::new(0),
        });
        let automatic = Arc::new(TestSuccessfulAutomaticExecutor {
            calls: AtomicUsize::new(0),
        });
        let planner = Arc::new(TestAutomaticPlanner {
            scope: test_scope(),
            route: test_route(),
            continuation_count: AtomicUsize::new(0),
        });
        let planned = planner
            .initial_plan(&lease)
            .await
            .expect("automatic initial plan should validate");
        let resolution =
            AiAgentRuleResolution::new(test_rules(test_scope()), time::OffsetDateTime::now_utc())
                .expect("automatic test rules should resolve");
        planned
            .provider_call
            .project_supervised_rule_usage(&resolution, AiRuleRunUsage::default(), false)
            .expect("automatic initial plan should satisfy rules");
        let coordinator = AiSupervisedAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            Arc::new(TestOutputWriter),
            checkpoints.clone(),
            Arc::new(TestAutomaticCheckpointControl {
                consumed: AtomicBool::new(false),
            }),
            Arc::new(TestApprovalStager {
                calls: AtomicUsize::new(0),
                saw_checkpoint: AtomicBool::new(false),
            }),
            automatic.clone(),
            unused_resume(),
            Arc::new(TestRuleResolver),
            planner.clone(),
            Arc::new(FixedClock::new(time::OffsetDateTime::now_utc())),
            limits(),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("automatic mutation should continue to final output");

        assert!(matches!(
            outcome,
            AiSupervisedAgentRunOutcome::Completed {
                provider_turns: 2,
                total_tool_calls: 1,
                ..
            }
        ));
        assert_eq!(automatic.calls.load(Ordering::SeqCst), 1);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
        assert_eq!(checkpoints.provider_checkpoints.load(Ordering::SeqCst), 2);
        assert_eq!(checkpoints.automatic_checkpoints.load(Ordering::SeqCst), 1);
        assert_eq!(planner.continuation_count.load(Ordering::SeqCst), 1);
        assert_eq!(run.final_states(), vec![AiRunState::Completed]);
    }

    #[tokio::test]
    async fn pre_transport_budget_denial_is_a_certain_supervised_failure() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Err(AiError::PreTransportBudgetDenied)])),
            require_checkpoint_cleared: false,
            calls: AtomicUsize::new(0),
        });
        let coordinator = AiSupervisedAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            Arc::new(TestOutputWriter),
            Arc::new(TestCheckpointWriter {
                provider_checkpoints: AtomicUsize::new(0),
            }),
            Arc::new(TestCheckpointControl {
                adopted: Mutex::new(None),
                consumed: AtomicBool::new(false),
            }),
            Arc::new(TestApprovalStager {
                calls: AtomicUsize::new(0),
                saw_checkpoint: AtomicBool::new(false),
            }),
            unused_automatic(),
            unused_resume(),
            Arc::new(TestRuleResolver),
            Arc::new(TestPlanner {
                scope: test_scope(),
                route: test_route(),
                continuation_count: AtomicUsize::new(0),
            }),
            Arc::new(FixedClock::new(time::OffsetDateTime::now_utc())),
            limits(),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("a supervised budget denial is a clean terminal failure");

        assert_eq!(
            outcome,
            AiSupervisedAgentRunOutcome::Failed {
                provider_turns: 0,
                total_tool_calls: 0,
            }
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(provider.remaining_responses(), 0);
        assert_eq!(run.final_states(), vec![AiRunState::Failed]);
        assert_eq!(
            crate::classify_run_retry(
                crate::AiRunRetryEvidence {
                    terminal: crate::AiRunTerminalEvent::Failed,
                    produced_assistant_output: false,
                    provider_dispatch_possible: false,
                },
                Some("provider_budget_denied"),
            ),
            crate::AiRunRetryAdmission::Allowed
        );
    }

    #[tokio::test]
    async fn pre_transport_provider_rejection_is_a_certain_supervised_failure() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Err(AiError::PreTransportProviderFailed)])),
            require_checkpoint_cleared: false,
            calls: AtomicUsize::new(0),
        });
        let coordinator = AiSupervisedAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            Arc::new(TestOutputWriter),
            Arc::new(TestCheckpointWriter {
                provider_checkpoints: AtomicUsize::new(0),
            }),
            Arc::new(TestCheckpointControl {
                adopted: Mutex::new(None),
                consumed: AtomicBool::new(false),
            }),
            Arc::new(TestApprovalStager {
                calls: AtomicUsize::new(0),
                saw_checkpoint: AtomicBool::new(false),
            }),
            unused_automatic(),
            unused_resume(),
            Arc::new(TestRuleResolver),
            Arc::new(TestPlanner {
                scope: test_scope(),
                route: test_route(),
                continuation_count: AtomicUsize::new(0),
            }),
            Arc::new(FixedClock::new(time::OffsetDateTime::now_utc())),
            limits(),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("a proven supervised pre-transport rejection should fail cleanly");

        assert_eq!(
            outcome,
            AiSupervisedAgentRunOutcome::Failed {
                provider_turns: 0,
                total_tool_calls: 0,
            }
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(provider.remaining_responses(), 0);
        assert_eq!(run.final_states(), vec![AiRunState::Failed]);
        assert_eq!(
            crate::classify_run_retry(
                crate::AiRunRetryEvidence {
                    terminal: crate::AiRunTerminalEvent::Failed,
                    produced_assistant_output: false,
                    provider_dispatch_possible: false,
                },
                Some("provider_pre_transport_failed"),
            ),
            crate::AiRunRetryAdmission::Allowed
        );
    }

    #[test]
    fn retained_cleanup_deferral_keeps_the_supervised_turn_out_of_recovery() {
        assert!(matches!(
            classify_supervised_turn_failure(&AiError::ProviderSessionDeferred),
            SupervisedProviderTurnFailure::PreTransportProvider
        ));
    }

    #[tokio::test]
    async fn provider_turn_is_checkpointed_before_one_approval_is_staged() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(
                crate::AiProviderCallResult::test_result(
                    &lease,
                    None,
                    "supervised-response",
                    vec![("write-call", "test.write", json!({"value": 7}))],
                ),
            )])),
            require_checkpoint_cleared: false,
            calls: AtomicUsize::new(0),
        });
        let checkpoints = Arc::new(TestCheckpointWriter {
            provider_checkpoints: AtomicUsize::new(0),
        });
        let stager = Arc::new(TestApprovalStager {
            calls: AtomicUsize::new(0),
            saw_checkpoint: AtomicBool::new(false),
        });
        let coordinator = AiSupervisedAgentCoordinator::new(
            run.clone(),
            provider,
            Arc::new(TestOutputWriter),
            checkpoints.clone(),
            Arc::new(TestCheckpointControl {
                adopted: Mutex::new(None),
                consumed: AtomicBool::new(false),
            }),
            stager.clone(),
            unused_automatic(),
            unused_resume(),
            Arc::new(TestRuleResolver),
            Arc::new(TestPlanner {
                scope: test_scope(),
                route: test_route(),
                continuation_count: AtomicUsize::new(0),
            }),
            Arc::new(FixedClock::new(time::OffsetDateTime::now_utc())),
            limits(),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("supervised turn should wait for approval");

        assert!(matches!(
            outcome,
            AiSupervisedAgentRunOutcome::WaitingApproval {
                provider_turns: 1,
                total_tool_calls: 1,
                ..
            }
        ));
        assert_eq!(checkpoints.provider_checkpoints.load(Ordering::SeqCst), 1);
        assert_eq!(stager.calls.load(Ordering::SeqCst), 1);
        assert!(stager.saw_checkpoint.load(Ordering::SeqCst));
        assert!(run.final_states().is_empty());
    }

    #[tokio::test]
    async fn parallel_supervised_calls_fail_without_staging_any_approval() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(
                crate::AiProviderCallResult::test_result(
                    &lease,
                    None,
                    "parallel-response",
                    vec![
                        ("write-call-1", "test.write", json!({"value": 1})),
                        ("write-call-2", "test.write", json!({"value": 2})),
                    ],
                ),
            )])),
            require_checkpoint_cleared: false,
            calls: AtomicUsize::new(0),
        });
        let stager = Arc::new(TestApprovalStager {
            calls: AtomicUsize::new(0),
            saw_checkpoint: AtomicBool::new(false),
        });
        let coordinator = AiSupervisedAgentCoordinator::new(
            run.clone(),
            provider,
            Arc::new(TestOutputWriter),
            Arc::new(TestCheckpointWriter {
                provider_checkpoints: AtomicUsize::new(0),
            }),
            Arc::new(TestCheckpointControl {
                adopted: Mutex::new(None),
                consumed: AtomicBool::new(false),
            }),
            stager.clone(),
            unused_automatic(),
            unused_resume(),
            Arc::new(TestRuleResolver),
            Arc::new(TestPlanner {
                scope: test_scope(),
                route: test_route(),
                continuation_count: AtomicUsize::new(0),
            }),
            Arc::new(FixedClock::new(time::OffsetDateTime::now_utc())),
            limits(),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("parallel mutation request should close safely");

        assert_eq!(
            outcome,
            AiSupervisedAgentRunOutcome::Failed {
                provider_turns: 1,
                total_tool_calls: 2,
            }
        );
        assert_eq!(stager.calls.load(Ordering::SeqCst), 0);
        assert_eq!(run.final_states(), vec![AiRunState::Failed]);
    }

    #[tokio::test]
    async fn final_allowed_provider_turn_cannot_stage_an_unfinishable_mutation() {
        let lease = AiRunLease::test_running(principal_reference());
        let run = Arc::new(TestRunControl::new());
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(
                crate::AiProviderCallResult::test_result(
                    &lease,
                    None,
                    "last-allowed-response",
                    vec![("write-call", "test.write", json!({"value": 7}))],
                ),
            )])),
            require_checkpoint_cleared: false,
            calls: AtomicUsize::new(0),
        });
        let stager = Arc::new(TestApprovalStager {
            calls: AtomicUsize::new(0),
            saw_checkpoint: AtomicBool::new(false),
        });
        let coordinator = AiSupervisedAgentCoordinator::new(
            run.clone(),
            provider,
            Arc::new(TestOutputWriter),
            Arc::new(TestCheckpointWriter {
                provider_checkpoints: AtomicUsize::new(0),
            }),
            Arc::new(TestCheckpointControl {
                adopted: Mutex::new(None),
                consumed: AtomicBool::new(false),
            }),
            stager.clone(),
            unused_automatic(),
            unused_resume(),
            Arc::new(TestRuleResolver),
            Arc::new(TestPlanner {
                scope: test_scope(),
                route: test_route(),
                continuation_count: AtomicUsize::new(0),
            }),
            Arc::new(FixedClock::new(time::OffsetDateTime::now_utc())),
            limits_with_provider_turns(1),
        );

        let outcome = coordinator
            .execute_claimed(&lease)
            .await
            .expect("unfinishable mutation should close without human staging");

        assert_eq!(
            outcome,
            AiSupervisedAgentRunOutcome::Failed {
                provider_turns: 1,
                total_tool_calls: 1,
            }
        );
        assert_eq!(stager.calls.load(Ordering::SeqCst), 0);
        assert_eq!(run.final_states(), vec![AiRunState::Failed]);
    }

    #[tokio::test]
    async fn adopted_mutation_result_is_consumed_before_the_next_provider_turn() {
        let base = AiRunLease::test_running(principal_reference());
        let checkpoint_id = Uuid::new_v4();
        let claimed = base.test_with_checkpoint(checkpoint_id);
        let control = Arc::new(TestCheckpointControl {
            adopted: Mutex::new(Some(adopted_checkpoint(&base, checkpoint_id))),
            consumed: AtomicBool::new(false),
        });
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(
                crate::AiProviderCallResult::test_result(
                    &claimed,
                    Some("test-previous-response".to_owned()),
                    "supervised-final-response",
                    Vec::new(),
                ),
            )])),
            require_checkpoint_cleared: true,
            calls: AtomicUsize::new(0),
        });
        let run = Arc::new(TestRunControl::new());
        let planner = Arc::new(TestPlanner {
            scope: test_scope(),
            route: test_route(),
            continuation_count: AtomicUsize::new(0),
        });
        let coordinator = AiSupervisedAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            Arc::new(TestOutputWriter),
            Arc::new(TestCheckpointWriter {
                provider_checkpoints: AtomicUsize::new(0),
            }),
            control.clone(),
            Arc::new(TestApprovalStager {
                calls: AtomicUsize::new(0),
                saw_checkpoint: AtomicBool::new(false),
            }),
            unused_automatic(),
            unused_resume(),
            Arc::new(TestRuleResolver),
            planner.clone(),
            Arc::new(FixedClock::new(time::OffsetDateTime::now_utc())),
            limits(),
        );

        let outcome = coordinator
            .execute_claimed(&claimed)
            .await
            .expect("adopted supervised result should continue once");

        assert!(matches!(
            outcome,
            AiSupervisedAgentRunOutcome::Completed {
                provider_turns: 2,
                total_tool_calls: 1,
                ..
            }
        ));
        assert!(control.consumed.load(Ordering::SeqCst));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(planner.continuation_count.load(Ordering::SeqCst), 1);
        assert_eq!(run.final_states(), vec![AiRunState::Completed]);
    }

    #[tokio::test]
    async fn resumed_subscription_cannot_bypass_approval_required_mutation_policy() {
        let base = AiRunLease::test_running(principal_reference());
        let checkpoint_id = Uuid::new_v4();
        let claimed = base.test_with_checkpoint(checkpoint_id);
        let control = Arc::new(TestSubscriptionCheckpointControl {
            adopted: Mutex::new(Some(adopted_subscription_checkpoint(&base, checkpoint_id))),
            consumed: AtomicBool::new(false),
        });
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(
                crate::AiProviderCallResult::test_result(
                    &claimed,
                    Some("test-previous-response".to_owned()),
                    "subscription-resume-response",
                    vec![("write-after-event", "test.write", json!({"value": 7}))],
                ),
            )])),
            require_checkpoint_cleared: true,
            calls: AtomicUsize::new(0),
        });
        let run = Arc::new(TestRunControl::new());
        let planner = Arc::new(TestPlanner {
            scope: test_scope(),
            route: test_route(),
            continuation_count: AtomicUsize::new(0),
        });
        let stager = Arc::new(TestApprovalStager {
            calls: AtomicUsize::new(0),
            saw_checkpoint: AtomicBool::new(false),
        });
        let coordinator = AiSupervisedAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            Arc::new(TestOutputWriter),
            Arc::new(TestCheckpointWriter {
                provider_checkpoints: AtomicUsize::new(0),
            }),
            control.clone(),
            stager.clone(),
            unused_automatic(),
            unused_resume(),
            Arc::new(TestRuleResolver),
            planner.clone(),
            Arc::new(FixedClock::new(time::OffsetDateTime::now_utc())),
            limits(),
        );

        let outcome = coordinator
            .execute_claimed(&claimed)
            .await
            .expect("resumed subscription should retain mutation classification");

        assert!(matches!(
            outcome,
            AiSupervisedAgentRunOutcome::WaitingApproval {
                provider_turns: 2,
                total_tool_calls: 2,
                ..
            }
        ));
        assert!(control.consumed.load(Ordering::SeqCst));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(stager.calls.load(Ordering::SeqCst), 1);
        assert!(stager.saw_checkpoint.load(Ordering::SeqCst));
        assert_eq!(planner.continuation_count.load(Ordering::SeqCst), 1);
        assert!(run.final_states().is_empty());
    }

    #[tokio::test]
    async fn rule_change_after_checkpoint_consumption_blocks_provider_transport() {
        let base = AiRunLease::test_running(principal_reference());
        let checkpoint_id = Uuid::new_v4();
        let claimed = base.test_with_checkpoint(checkpoint_id);
        let control = Arc::new(TestCheckpointControl {
            adopted: Mutex::new(Some(adopted_checkpoint(&base, checkpoint_id))),
            consumed: AtomicBool::new(false),
        });
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(
                crate::AiProviderCallResult::test_result(
                    &claimed,
                    Some("test-previous-response".to_owned()),
                    "must-not-run",
                    Vec::new(),
                ),
            )])),
            require_checkpoint_cleared: true,
            calls: AtomicUsize::new(0),
        });
        let run = Arc::new(TestRunControl::new());
        let coordinator = AiSupervisedAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            Arc::new(TestOutputWriter),
            Arc::new(TestCheckpointWriter {
                provider_checkpoints: AtomicUsize::new(0),
            }),
            control.clone(),
            Arc::new(TestApprovalStager {
                calls: AtomicUsize::new(0),
                saw_checkpoint: AtomicBool::new(false),
            }),
            unused_automatic(),
            unused_resume(),
            Arc::new(ChangingRuleResolver(AtomicUsize::new(0))),
            Arc::new(TestPlanner {
                scope: test_scope(),
                route: test_route(),
                continuation_count: AtomicUsize::new(0),
            }),
            Arc::new(FixedClock::new(time::OffsetDateTime::now_utc())),
            limits(),
        );

        let outcome = coordinator
            .execute_claimed(&claimed)
            .await
            .expect("changed rule should close without provider transport");

        assert_eq!(
            outcome,
            AiSupervisedAgentRunOutcome::Failed {
                provider_turns: 1,
                total_tool_calls: 1,
            }
        );
        assert!(control.consumed.load(Ordering::SeqCst));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        assert_eq!(provider.remaining_responses(), 1);
        assert_eq!(run.final_states(), vec![AiRunState::Failed]);
    }

    #[tokio::test]
    async fn exhausted_turn_limit_preserves_the_unconsumed_checkpoint() {
        let base = AiRunLease::test_running(principal_reference());
        let checkpoint_id = Uuid::new_v4();
        let claimed = base.test_with_checkpoint(checkpoint_id);
        let control = Arc::new(TestCheckpointControl {
            adopted: Mutex::new(Some(adopted_checkpoint(&base, checkpoint_id))),
            consumed: AtomicBool::new(false),
        });
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(
                crate::AiProviderCallResult::test_result(
                    &claimed,
                    Some("test-previous-response".to_owned()),
                    "must-not-run",
                    Vec::new(),
                ),
            )])),
            require_checkpoint_cleared: true,
            calls: AtomicUsize::new(0),
        });
        let run = Arc::new(TestRunControl::new());
        let coordinator = AiSupervisedAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            Arc::new(TestOutputWriter),
            Arc::new(TestCheckpointWriter {
                provider_checkpoints: AtomicUsize::new(0),
            }),
            control.clone(),
            Arc::new(TestApprovalStager {
                calls: AtomicUsize::new(0),
                saw_checkpoint: AtomicBool::new(false),
            }),
            unused_automatic(),
            unused_resume(),
            Arc::new(TestRuleResolver),
            Arc::new(TestPlanner {
                scope: test_scope(),
                route: test_route(),
                continuation_count: AtomicUsize::new(0),
            }),
            Arc::new(FixedClock::new(time::OffsetDateTime::now_utc())),
            limits_with_provider_turns(1),
        );

        let outcome = coordinator
            .execute_claimed(&claimed)
            .await
            .expect("exhausted loop should close without consuming the checkpoint");

        assert_eq!(
            outcome,
            AiSupervisedAgentRunOutcome::Failed {
                provider_turns: 1,
                total_tool_calls: 1,
            }
        );
        assert!(!control.consumed.load(Ordering::SeqCst));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        assert_eq!(provider.remaining_responses(), 1);
        assert_eq!(run.final_states(), vec![AiRunState::Failed]);
    }

    #[tokio::test]
    async fn ambiguous_approved_mutation_never_reenters_the_provider() {
        let lease = AiRunLease::test_running(principal_reference());
        let claim =
            crate::AiApprovedRunClaim::test_claim(lease.test_with_state(AiRunState::WaitingTool));
        let provider = Arc::new(TestProviderExecutor {
            responses: Mutex::new(VecDeque::from([Ok(
                crate::AiProviderCallResult::test_result(&lease, None, "must-not-run", Vec::new()),
            )])),
            require_checkpoint_cleared: false,
            calls: AtomicUsize::new(0),
        });
        let run = Arc::new(TestRunControl::new());
        let coordinator = AiSupervisedAgentCoordinator::new(
            run.clone(),
            provider.clone(),
            Arc::new(TestOutputWriter),
            Arc::new(TestCheckpointWriter {
                provider_checkpoints: AtomicUsize::new(0),
            }),
            Arc::new(TestCheckpointControl {
                adopted: Mutex::new(None),
                consumed: AtomicBool::new(false),
            }),
            Arc::new(TestApprovalStager {
                calls: AtomicUsize::new(0),
                saw_checkpoint: AtomicBool::new(false),
            }),
            unused_automatic(),
            Arc::new(TestResumeExecutor {
                outcome: Mutex::new(Some(AiSupervisedResumeOutcome::RecoveryRequired {
                    tool_call_id: claim.tool_call_id(),
                    provider_turns: 3,
                    total_tool_calls: 2,
                })),
            }),
            Arc::new(TestRuleResolver),
            Arc::new(TestPlanner {
                scope: test_scope(),
                route: test_route(),
                continuation_count: AtomicUsize::new(0),
            }),
            Arc::new(FixedClock::new(time::OffsetDateTime::now_utc())),
            limits(),
        );

        let outcome = coordinator
            .execute_approved_claim(&claim)
            .await
            .expect("ambiguous mutation should already be durably closed");

        assert_eq!(
            outcome,
            AiSupervisedAgentRunOutcome::RecoveryRequired {
                phase: AiAgentRecoveryPhase::ApplicationTool,
                provider_turns: 3,
                total_tool_calls: 2,
            }
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        assert_eq!(provider.remaining_responses(), 1);
        assert!(run.final_states().is_empty());
    }

    #[test]
    fn supervised_turn_rejects_a_read_only_provider_plan() {
        let lease = AiRunLease::test_running(principal_reference());
        let result = AiSupervisedAgentTurnPlan::new(
            AiProviderCallPlan::test_plan(&lease, test_scope(), false),
            test_route(),
            test_rules(test_scope()),
            false,
        );

        assert!(matches!(result, Err(AiError::InvalidInput(_))));
    }

    #[test]
    fn limits_reject_unbounded_approval_waits() {
        let result = AiSupervisedAgentCoordinatorLimits::new(
            AiAgentLoopLimits::new(4, 8).expect("test loop limits should validate"),
            Duration::seconds(1),
            Duration::days(2),
            false,
        );

        assert!(matches!(result, Err(AiError::InvalidConfiguration(_))));
    }
}
