//! Protected generated-ORM persistence for bounded subscription waits.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use std::sync::Arc;

use agql_auth::{Clock, PrincipalReference, ResolvedPrincipal};
use async_trait::async_trait;
use futures::StreamExt;
use graphql_orm::db::Database;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::filters::{StringFilter, UuidFilter};
use graphql_orm::graphql::orm::{ConditionalUpdateOutcome, DefaultWriteBackend, TransactionMode};
use serde::Deserialize;
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::orm_runs::{
    append_terminal_run_event, canonical_second, coordinator_checkpoint_hash, exact_state,
    load_and_validate_active_lease,
};
use crate::orm_sessions::{
    content_context, map_orm, map_protection, map_transaction, principal_identity, record_scope,
};
use crate::orm_tools::{canonical_json_hash, provider_call_key};
use crate::persistence::*;
use crate::{
    AiAgentRuleResolution, AiContentProtectionPolicy, AiDataSourceRef, AiDisclosureSchema,
    AiEgressDecisionAudit, AiEgressManifest, AiError, AiGraphqlSubscriptionCondition,
    AiProviderCallResult, AiReplayableSubscriptionEvent, AiReplayableSubscriptionOpenRequest,
    AiReplayableSubscriptionSourceItem, AiReplayableSubscriptionSourceRegistry,
    AiRulePolicyService, AiRuleRunUsage, AiRunLease, AiRunState, AiScope, AiSessionAction,
    AiSessionId, AiSourceTrust, AiSubscriptionReplayPosition, AiSubscriptionWaiterId, AiToolCallId,
    AiToolDescriptor, AiToolId, AiToolOperationDomain, AiToolOperationKind,
    AiToolResultEgressRoute, ContentProtectionContext, DataClassification,
    GraphqlInvocationContext, ModelContinuation, OrmAiRunService, ProtectedContentEnvelope,
    ToolGraphqlRequest, ToolMaturity, ai_scope_key,
};

const MAXIMUM_SAFE_FINGERPRINT_BYTES: usize = 64;

/// Deployment-owned persistence and worker ceilings for durable waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiSubscriptionWaitLimits {
    claim_ttl: Duration,
    maximum_principal_age: Duration,
    maximum_candidate_scan: usize,
    maximum_transaction_retries: usize,
    maximum_protected_request_bytes: usize,
    maximum_event_bytes: usize,
}

impl AiSubscriptionWaitLimits {
    /// Creates validated waiter limits.
    ///
    /// # Errors
    ///
    /// Returns an error unless durations are positive and at most one hour,
    /// candidate scans are `1..=256`, transaction retries are at most 16 and
    /// protected request/event ceilings are `1..=64 MiB`.
    pub fn new(
        claim_ttl: Duration,
        maximum_principal_age: Duration,
        maximum_candidate_scan: usize,
        maximum_transaction_retries: usize,
        maximum_protected_request_bytes: usize,
        maximum_event_bytes: usize,
    ) -> Result<Self, AiError> {
        const MAXIMUM_BYTES: usize = 64 * 1024 * 1024;
        if !claim_ttl.is_positive()
            || claim_ttl > Duration::hours(1)
            || !maximum_principal_age.is_positive()
            || maximum_principal_age > Duration::hours(1)
            || !(1..=256).contains(&maximum_candidate_scan)
            || maximum_transaction_retries > 16
            || !(1..=MAXIMUM_BYTES).contains(&maximum_protected_request_bytes)
            || !(1..=MAXIMUM_BYTES).contains(&maximum_event_bytes)
        {
            return Err(AiError::InvalidConfiguration(
                "invalid durable subscription-wait limits".to_owned(),
            ));
        }
        Ok(Self {
            claim_ttl,
            maximum_principal_age,
            maximum_candidate_scan,
            maximum_transaction_retries,
            maximum_protected_request_bytes,
            maximum_event_bytes,
        })
    }

    /// Waiter-worker lease duration. This is never a run/provider lease.
    pub const fn claim_ttl(self) -> Duration {
        self.claim_ttl
    }

    /// Maximum waiter rows considered by one claim pass.
    pub const fn maximum_candidate_scan(self) -> usize {
        self.maximum_candidate_scan
    }
}

impl Default for AiSubscriptionWaitLimits {
    fn default() -> Self {
        Self {
            claim_ttl: Duration::minutes(2),
            maximum_principal_age: Duration::minutes(5),
            maximum_candidate_scan: 50,
            maximum_transaction_retries: 4,
            maximum_protected_request_bytes: 512 * 1024,
            maximum_event_bytes: 512 * 1024,
        }
    }
}

/// Server-owned provider/checkpoint context needed to resume one wait result.
#[derive(Clone, Debug)]
pub struct AiSubscriptionWaitRegistrationContext {
    scope: AiScope,
    correlation_id: String,
    result_egress_route: AiToolResultEgressRoute,
    rules: AiAgentRuleResolution,
    rule_usage: AiRuleRunUsage,
    provider_turns: u32,
    total_tool_calls: u32,
}

impl AiSubscriptionWaitRegistrationContext {
    /// Creates the exact continuation context observed after a provider turn.
    ///
    /// # Errors
    ///
    /// Returns an error for empty/oversized correlation, mismatched scope or
    /// zero provider/tool counts.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scope: AiScope,
        correlation_id: impl Into<String>,
        result_egress_route: AiToolResultEgressRoute,
        rules: AiAgentRuleResolution,
        rule_usage: AiRuleRunUsage,
        provider_turns: u32,
        total_tool_calls: u32,
    ) -> Result<Self, AiError> {
        let correlation_id = correlation_id.into();
        if scope.kind.trim().is_empty()
            || scope.id.trim().is_empty()
            || rules.rules().target_scope() != &scope
            || provider_turns == 0
            || total_tool_calls == 0
            || correlation_id.is_empty()
            || correlation_id.len() > 1_024
            || correlation_id.chars().any(char::is_control)
            || rule_usage.provider_calls() != u64::from(provider_turns)
        {
            return Err(AiError::InvalidInput(
                "invalid subscription-wait continuation context".to_owned(),
            ));
        }
        Ok(Self {
            scope,
            correlation_id,
            result_egress_route,
            rules,
            rule_usage,
            provider_turns,
            total_tool_calls,
        })
    }
}

/// Durable proof that a run released all coordinator/provider resources and
/// entered `WaitingSubscription`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiRegisteredSubscriptionWait {
    waiter_id: AiSubscriptionWaiterId,
    tool_call_id: AiToolCallId,
    run_id: crate::AiRunId,
    expires_at: OffsetDateTime,
    waiter_fingerprint: String,
}

impl AiRegisteredSubscriptionWait {
    /// Durable waiter ID.
    pub const fn waiter_id(&self) -> AiSubscriptionWaiterId {
        self.waiter_id
    }

    /// Provider tool call completed by the eventual outcome.
    pub const fn tool_call_id(&self) -> AiToolCallId {
        self.tool_call_id
    }

    /// Suspended run ID.
    pub const fn run_id(&self) -> crate::AiRunId {
        self.run_id
    }

    /// Exclusive timeout.
    pub const fn expires_at(&self) -> OffsetDateTime {
        self.expires_at
    }

    /// Complete immutable waiter binding fingerprint.
    pub fn waiter_fingerprint(&self) -> &str {
        &self.waiter_fingerprint
    }
}

/// Fenced claim held by one bounded subscription-source worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiSubscriptionWaitClaim {
    waiter_id: AiSubscriptionWaiterId,
    run_id: crate::AiRunId,
    claim_generation: i64,
    claim_expires_at: OffsetDateTime,
    row_version: i64,
    worker_id: String,
}

impl AiSubscriptionWaitClaim {
    /// Claimed waiter.
    pub const fn waiter_id(&self) -> AiSubscriptionWaiterId {
        self.waiter_id
    }

    /// Suspended run.
    pub const fn run_id(&self) -> crate::AiRunId {
        self.run_id
    }

    /// Monotonic waiter-worker fence.
    pub const fn claim_generation(&self) -> i64 {
        self.claim_generation
    }

    /// Exclusive waiter-worker claim expiry.
    pub const fn claim_expires_at(&self) -> OffsetDateTime {
        self.claim_expires_at
    }
}

/// Result of one bounded waiter-worker pass.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiSubscriptionWaitWorkerOutcome {
    /// No eligible waiter exists.
    Idle,
    /// The source ended or became temporarily unavailable before a safe
    /// outcome; the exact cursor remains durable and the waiter is reclaimable.
    Waiting {
        /// Waiter left eligible for a later replay-then-live pass.
        waiter_id: AiSubscriptionWaiterId,
    },
    /// One exact event/timeout/event-limit outcome was atomically adopted and
    /// the existing run was placed on its ordinary queue.
    Queued {
        /// Adopted waiter.
        waiter_id: AiSubscriptionWaiterId,
        /// Existing run now eligible for a fresh ordinary claim.
        run_id: crate::AiRunId,
    },
    /// Retention reset or indeterminate protected state closed the run through
    /// the canonical recovery-required fence.
    RecoveryRequired {
        /// Closed waiter.
        waiter_id: AiSubscriptionWaiterId,
        /// Closed run.
        run_id: crate::AiRunId,
    },
    /// Cancellation, session deletion or current-authority loss closed the
    /// waiter without a provider continuation.
    Closed {
        /// Closed waiter.
        waiter_id: AiSubscriptionWaiterId,
        /// Closed run.
        run_id: crate::AiRunId,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProtectedWaitRequest {
    format_version: u32,
    descriptor: AiToolDescriptor,
    disclosure_schema: AiDisclosureSchema,
    variables: Value,
    condition: Option<AiGraphqlSubscriptionCondition>,
    scope: AiScope,
    correlation_id: String,
    result_egress_route: Value,
    rule_fingerprint: String,
    rule_usage: AiRuleRunUsage,
    provider_turns: u32,
    total_tool_calls: u32,
    provider_result: Value,
    pending_continuation: ModelContinuation,
    replay_tool_transfers: Vec<AiEgressManifest>,
    source_id: String,
    source_registration_fingerprint: String,
    capability_fingerprint: String,
    plan_fingerprint: String,
}

struct OpenedWait {
    waiter: AiSubscriptionWaiterRecord,
    principal_reference: PrincipalReference,
    descriptor: AiToolDescriptor,
    disclosure_schema: AiDisclosureSchema,
    condition: Option<AiGraphqlSubscriptionCondition>,
    request: ToolGraphqlRequest,
    route: AiToolResultEgressRoute,
    continuation: ModelContinuation,
    replay_transfers: Vec<AiEgressManifest>,
    provider_kind: String,
    provider_model: String,
    budget_reservation_id: Uuid,
    provider_call_id: String,
    tool_id: String,
    rule_fingerprint: String,
    rule_usage: AiRuleRunUsage,
    provider_turns: u32,
    total_tool_calls: u32,
    position: AiSubscriptionReplayPosition,
}

struct PreparedWaitAdoption {
    id: Uuid,
    outcome_kind: String,
    source_event_fingerprint: Option<String>,
    cursor_fingerprint: String,
    protected_cursor: Value,
    events_examined: i64,
    outcome_fingerprint: String,
    protected_outcome: Value,
    checkpoint_fingerprint: String,
    protected_checkpoint: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProtectedProviderResult {
    format_version: u32,
    session_id: Uuid,
    run_id: Uuid,
    attempt_id: Uuid,
    lease_generation: i64,
    provider_kind: crate::ProviderKind,
    provider_model: String,
    provider_response_id: Option<String>,
    budget_reservation_id: Uuid,
    previous_response_id: Option<String>,
    tool_calls: Vec<ProtectedProviderToolCall>,
    #[serde(flatten)]
    remaining: std::collections::BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProtectedProviderToolCall {
    call_id: String,
    tool_id: String,
    provider_name: String,
    tool_fingerprint: String,
    arguments: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProtectedWaitOutcome {
    format_version: u32,
    waiter_id: AiSubscriptionWaiterId,
    outcome_kind: String,
    source_event_fingerprint: Option<String>,
    source_event: Option<Value>,
    output: Value,
    egress_manifest: AiEgressManifest,
    egress_decision_id: crate::AiEgressDecisionId,
    pending_continuation: ModelContinuation,
    replay_tool_transfers: Vec<AiEgressManifest>,
    provider_call_id: String,
    tool_id: String,
    provider_turns: u32,
    total_tool_calls: u32,
}

/// Routes subscription-adoption checkpoints to the durable waiter service and
/// every other checkpoint to the existing canonical adopter.
pub struct AiSubscriptionCheckpointAdopter {
    waits: Arc<OrmAiSubscriptionWaitService>,
    fallback: Arc<dyn crate::AiAgentCheckpointAdopter>,
}

impl AiSubscriptionCheckpointAdopter {
    /// Creates the single coordinator adopter used when durable waits are enabled.
    pub fn new(
        waits: Arc<OrmAiSubscriptionWaitService>,
        fallback: Arc<dyn crate::AiAgentCheckpointAdopter>,
    ) -> Self {
        Self { waits, fallback }
    }
}

/// Generated-ORM durable subscription-wait service.
///
/// Registration captures replay position before atomically ending the active
/// run attempt. The run keeps no worker/provider/coordinator lease while the
/// separately fenced waiter worker is active.
pub struct OrmAiSubscriptionWaitService {
    run_service: OrmAiRunService,
    runtime: Arc<crate::AiRuntime>,
    sources: Arc<AiReplayableSubscriptionSourceRegistry>,
    rule_service: Arc<dyn AiRulePolicyService>,
    egress_audit: Arc<dyn AiEgressDecisionAudit>,
    clock: Arc<dyn Clock>,
    limits: AiSubscriptionWaitLimits,
}

impl OrmAiSubscriptionWaitService {
    /// Creates a durable wait service.
    pub fn new(
        run_service: OrmAiRunService,
        runtime: Arc<crate::AiRuntime>,
        sources: Arc<AiReplayableSubscriptionSourceRegistry>,
        rule_service: Arc<dyn AiRulePolicyService>,
        egress_audit: Arc<dyn AiEgressDecisionAudit>,
        clock: Arc<dyn Clock>,
        limits: AiSubscriptionWaitLimits,
    ) -> Self {
        Self {
            run_service,
            runtime,
            sources,
            rule_service,
            egress_audit,
            clock,
            limits,
        }
    }

    /// Returns the underlying generated-ORM database.
    pub fn database(&self) -> &Database<DefaultWriteBackend> {
        self.run_service.database()
    }

    /// Compiles, freshly authorizes and durably suspends one exact provider
    /// subscription-capability call.
    ///
    /// Exactly one subscription call must be present in the completed provider
    /// turn. The compiled document/variables/selection/condition are protected;
    /// only fixed hashes and logical IDs remain in safe columns. A semantic
    /// ReplayThenLive descriptor without a matching source registration fails
    /// closed.
    ///
    /// # Errors
    ///
    /// Returns an error for stale capability/source/schema/lease/rule/session
    /// bindings, denied current principal/access/tool/source authorization,
    /// unavailable protection, unsafe cursor capture or persistence conflict.
    pub async fn register_wait(
        &self,
        lease: &AiRunLease,
        provider_result: &AiProviderCallResult,
        context: AiSubscriptionWaitRegistrationContext,
    ) -> Result<AiRegisteredSubscriptionWait, AiError> {
        if lease.state() != AiRunState::Running
            || provider_result.session_id() != lease.session_id()
            || provider_result.run_id() != lease.run_id()
            || provider_result.attempt_id() != lease.attempt_id()
            || provider_result.lease_generation() != lease.lease_generation()
            || provider_result.tool_calls().len() != 1
            || context.rules.rules().target_scope() != &context.scope
            || context.rule_usage.validate(&context.rules).is_err()
        {
            return Err(AiError::Conflict);
        }
        context.result_egress_route.validate()?;
        let provider_call = &provider_result.tool_calls()[0];
        let compiled = self
            .runtime
            .tool_catalog()
            .compile_subscription_capability(
                provider_call.tool_id(),
                provider_call.tool_fingerprint(),
                provider_call.arguments().clone(),
            )?;
        let (
            descriptor,
            disclosure_schema,
            variables,
            condition,
            timeout_seconds,
            maximum_events,
            capability_fingerprint,
            plan_fingerprint,
        ) = compiled.into_parts();
        if descriptor.operation_kind != AiToolOperationKind::Subscription
            || descriptor.operation_domain != AiToolOperationDomain::Application
            || descriptor.maturity != ToolMaturity::ReadOnly
            || descriptor.id != *provider_call.tool_id()
            || capability_fingerprint != provider_call.tool_fingerprint()
            || !valid_fingerprint(&plan_fingerprint)
            || maximum_events == 0
            || timeout_seconds == 0
        {
            return Err(AiError::Forbidden);
        }
        let resolved_source = self.sources.resolve_compiled(&descriptor)?;
        let contract = descriptor
            .graphql_contract
            .clone()
            .ok_or(AiError::Forbidden)?;
        let semantic = contract
            .semantic_operation()
            .cloned()
            .ok_or(AiError::Forbidden)?;
        let waiter_id = AiSubscriptionWaiterId::new();
        let tool_call_id = AiToolCallId::new();
        let request = ToolGraphqlRequest {
            document: descriptor.document.clone(),
            operation_name: contract.operation_name.clone(),
            contract: contract.clone(),
            variables: variables.clone(),
            invocation: GraphqlInvocationContext {
                run_id: lease.run_id(),
                tool_call_id,
                scope: context.scope.clone(),
                correlation_id: context.correlation_id.clone(),
                causation_id: provider_result.budget_reservation_id().0.to_string(),
                delegation_reference: None,
                idempotency_key: None,
            },
        };
        let preauthorization = self
            .runtime
            .preauthorize_compiled_subscription(lease.principal_reference(), &descriptor, &request)
            .await?;
        self.ensure_fresh(preauthorization.principal(), lease.principal_reference())?;
        let session = AiSessionRecord::find_by_id(self.database(), &lease.session_id().0)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        self.authorize_session(
            preauthorization.principal(),
            &session,
            lease,
            &context.scope,
        )
        .await?;
        self.authorize_rules(
            preauthorization.principal(),
            &context.scope,
            context.rules.rules().fingerprint(),
            context.rule_usage,
            descriptor.fingerprint.as_str(),
        )
        .await?;
        let protection_policy = self
            .current_protection_policy(preauthorization.principal(), &context.scope)
            .await?;
        let position = resolved_source
            .source
            .capture_position(preauthorization.principal(), &request)
            .await
            .map_err(map_source)?;
        if !position.has_valid_fingerprint() {
            return Err(AiError::PersistenceFailed);
        }
        let now = canonical_second(self.clock.now());
        let expires_at = now
            .checked_add(Duration::seconds(i64::from(timeout_seconds)))
            .ok_or(AiError::PersistenceFailed)?;
        let principal_reference = serde_json::to_value(lease.principal_reference())
            .map_err(|_| AiError::PersistenceFailed)?;
        let principal_reference_fingerprint = canonical_json_hash(&principal_reference)?;
        let variables_fingerprint = canonical_json_hash(&variables)?;
        let condition_value =
            serde_json::to_value(&condition).map_err(|_| AiError::PersistenceFailed)?;
        let condition_fingerprint = canonical_json_hash(&condition_value)?;
        let provider_result_value = provider_result.checkpoint_value();
        let continuation = provider_result.next_continuation()?;
        let protected_request_plaintext = json!({
            "formatVersion": 1,
            "descriptor": descriptor,
            "disclosureSchema": disclosure_schema,
            "variables": variables,
            "condition": condition,
            "scope": context.scope,
            "correlationId": context.correlation_id,
            "resultEgressRoute": context.result_egress_route.checkpoint_value(),
            "ruleFingerprint": context.rules.rules().fingerprint(),
            "ruleUsage": context.rule_usage,
            "providerTurns": context.provider_turns,
            "totalToolCalls": context.total_tool_calls,
            "providerResult": provider_result_value,
            "pendingContinuation": continuation,
            "replayToolTransfers": provider_result.replay_tool_transfers(),
            "sourceId": resolved_source.descriptor.source_id(),
            "sourceRegistrationFingerprint": resolved_source.descriptor.fingerprint(),
            "capabilityFingerprint": capability_fingerprint,
            "planFingerprint": plan_fingerprint,
        });
        enforce_size(
            &protected_request_plaintext,
            self.limits.maximum_protected_request_bytes,
        )?;
        let protected_request_fingerprint = canonical_json_hash(&protected_request_plaintext)?;
        let protected_request = self
            .protect(
                &protection_policy,
                content_context(
                    "graphql_orm_ai_subscription_waiters",
                    waiter_id.0,
                    "protected_request",
                    &record_scope(&session),
                ),
                protected_request_plaintext,
            )
            .await?;
        let protected_cursor = self
            .protect(
                &protection_policy,
                content_context(
                    "graphql_orm_ai_subscription_waiters",
                    waiter_id.0,
                    "protected_cursor",
                    &record_scope(&session),
                ),
                serde_json::to_value(&position).map_err(|_| AiError::PersistenceFailed)?,
            )
            .await?;
        let protected_arguments = self
            .protect(
                &protection_policy,
                content_context(
                    "graphql_orm_ai_tool_calls",
                    tool_call_id.0,
                    "protected_arguments",
                    &record_scope(&session),
                ),
                provider_call.arguments().clone(),
            )
            .await?;
        let source_checkpoint_id = lease.latest_checkpoint_id().ok_or(AiError::Conflict)?;
        let source_checkpoint =
            AiRunCheckpointRecord::find_by_id(self.database(), &source_checkpoint_id)
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
                .ok_or(AiError::Conflict)?;
        if source_checkpoint.run_id != lease.run_id().0
            || source_checkpoint.attempt_id != lease.attempt_id()
            || source_checkpoint.lease_generation != lease.lease_generation()
            || source_checkpoint.checkpoint_kind != "provider_turn_persisted"
            || source_checkpoint.provider_response_id
                != provider_result.provider_response_id().map(str::to_owned)
            || source_checkpoint.budget_reservation_id
                != Some(provider_result.budget_reservation_id().0)
            || !valid_fingerprint(&source_checkpoint.checkpoint_hash)
        {
            return Err(AiError::Conflict);
        }
        let parked_checkpoint_id = Uuid::new_v4();
        let parked_checkpoint_plaintext = json!({
            "formatVersion": 1,
            "kind": "subscription_wait_parked",
            "sourceCheckpointId": source_checkpoint.id,
            "sourceCheckpointFingerprint": source_checkpoint.checkpoint_hash,
            "waiterId": waiter_id.0,
            "toolCallId": tool_call_id.0,
            "protectedRequestFingerprint": protected_request_fingerprint,
            "cursorFingerprint": position.fingerprint(),
            "capabilityFingerprint": capability_fingerprint,
            "planFingerprint": plan_fingerprint,
        });
        let protected_parked_checkpoint = self
            .protect(
                &protection_policy,
                content_context(
                    "graphql_orm_ai_run_checkpoints",
                    parked_checkpoint_id,
                    "protected_state",
                    &record_scope(&session),
                ),
                parked_checkpoint_plaintext,
            )
            .await?;
        let parked_checkpoint_fingerprint = coordinator_checkpoint_hash(
            lease.run_id(),
            lease.attempt_id(),
            lease.lease_generation(),
            parked_checkpoint_id,
            "subscription_wait_parked",
            provider_result.provider_kind().as_str(),
            provider_result.provider_model(),
            provider_result.provider_response_id(),
            provider_result.budget_reservation_id().0,
            &protected_parked_checkpoint,
        )?;

        let current = self
            .runtime
            .resolve_current_principal(lease.principal_reference())
            .await?;
        self.ensure_fresh(&current, lease.principal_reference())?;
        self.authorize_session(&current, &session, lease, &record_scope(&session))
            .await?;
        self.authorize_rules(
            &current,
            &record_scope(&session),
            context.rules.rules().fingerprint(),
            context.rule_usage,
            descriptor.fingerprint.as_str(),
        )
        .await?;
        let current_policy = self
            .current_protection_policy(&current, &record_scope(&session))
            .await?;
        if current_policy != protection_policy {
            return Err(AiError::ReauthorizationFailed);
        }
        let compiled_descriptor_fingerprint = descriptor.fingerprint.clone();
        let waiter_fingerprint = canonical_json_hash(&json!({
            "format": "graphql-orm-ai/subscription-waiter/v1",
            "id": waiter_id.0,
            "runId": lease.run_id().0,
            "sessionId": lease.session_id().0,
            "toolCallId": tool_call_id.0,
            "sourceAttemptId": lease.attempt_id(),
            "sourceLeaseGeneration": lease.lease_generation(),
            "sourceCheckpointId": source_checkpoint.id,
            "sourceCheckpointFingerprint": source_checkpoint.checkpoint_hash,
            "parkedCheckpointId": parked_checkpoint_id,
            "parkedCheckpointFingerprint": parked_checkpoint_fingerprint,
            "principalReferenceFingerprint": principal_reference_fingerprint,
            "scopeKey": ai_scope_key(&record_scope(&session)),
            "targetId": contract.target_id.as_str(),
            "sourceRegistrationFingerprint": resolved_source.descriptor.fingerprint(),
            "semanticCatalogFingerprint": semantic.catalog_fingerprint(),
            "operationFingerprint": semantic.operation_fingerprint(),
            "schemaFingerprint": contract.schema_fingerprint,
            "capabilityFingerprint": capability_fingerprint,
            "planFingerprint": plan_fingerprint,
            "compiledDescriptorFingerprint": compiled_descriptor_fingerprint,
            "variablesFingerprint": variables_fingerprint,
            "conditionFingerprint": condition_fingerprint,
            "cursorFingerprint": position.fingerprint(),
            "expiresAt": expires_at.unix_timestamp(),
            "maximumEvents": maximum_events,
        }))?;
        let prepared = PreparedWaitRegistration {
            waiter_id: waiter_id.0,
            tool_call_id: tool_call_id.0,
            source_id: resolved_source.descriptor.source_id().to_owned(),
            source_registration_fingerprint: resolved_source.descriptor.fingerprint().to_owned(),
            principal_reference,
            principal_reference_fingerprint,
            source_checkpoint_id: source_checkpoint.id,
            source_checkpoint_fingerprint: source_checkpoint.checkpoint_hash,
            parked_checkpoint_id,
            parked_checkpoint_fingerprint,
            protected_parked_checkpoint,
            scope: record_scope(&session),
            target_id: contract.target_id.as_str().to_owned(),
            semantic_catalog_fingerprint: semantic.catalog_fingerprint().to_owned(),
            operation_fingerprint: semantic.operation_fingerprint().to_owned(),
            target_schema_fingerprint: contract.schema_fingerprint,
            capability_fingerprint,
            plan_fingerprint,
            compiled_descriptor_fingerprint,
            operation_name: contract.operation_name,
            operation_document_hash: contract.document_hash,
            result_projection_fingerprint: contract.result_projection_fingerprint,
            disclosure_schema_fingerprint: contract.disclosure_schema_fingerprint,
            variables_fingerprint,
            condition_fingerprint,
            waiter_fingerprint: waiter_fingerprint.clone(),
            protected_request,
            cursor_fingerprint: position.fingerprint().to_owned(),
            protected_cursor,
            maximum_events: i64::from(maximum_events),
            expires_at: expires_at.unix_timestamp(),
            provider_call_key: provider_call_key(lease, provider_call.call_id()),
            provider_call_id: provider_call.call_id().to_owned(),
            provider_kind: provider_result.provider_kind().as_str().to_owned(),
            provider_model: provider_result.provider_model().to_owned(),
            provider_response_id: provider_result.provider_response_id().map(str::to_owned),
            budget_reservation_id: provider_result.budget_reservation_id().0,
            tool_id: provider_call.tool_id().as_str().to_owned(),
            tool_fingerprint: provider_call.tool_fingerprint().to_owned(),
            protected_arguments,
            argument_hash: canonical_json_hash(provider_call.arguments())?,
            correlation_id: request.invocation.correlation_id,
            causation_id: request.invocation.causation_id,
            authorization_policy_version: preauthorization.policy_version().to_owned(),
            authorization_state_digest: preauthorization.authorization_state_digest().to_owned(),
            provider_turn_index: i64::from(
                context
                    .provider_turns
                    .checked_sub(1)
                    .ok_or(AiError::Conflict)?,
            ),
            tool_step_index: i64::from(
                context
                    .provider_turns
                    .checked_sub(1)
                    .and_then(|turn| turn.checked_mul(64))
                    .ok_or(AiError::Conflict)?,
            ),
            expected_owner_principal_kind: session.owner_principal_kind,
            expected_owner_subject: session.owner_subject,
        };
        self.persist_registration(lease, prepared, now).await?;
        Ok(AiRegisteredSubscriptionWait {
            waiter_id,
            tool_call_id,
            run_id: lease.run_id(),
            expires_at,
            waiter_fingerprint,
        })
    }

    /// Claims one eligible waiter through its own short-lived worker fence.
    /// This never acquires a run, coordinator or provider lease.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe worker ID, malformed waiter graph or
    /// persistence failure.
    pub async fn claim_next(
        &self,
        worker_id: impl Into<String>,
    ) -> Result<Option<AiSubscriptionWaitClaim>, AiError> {
        let worker_id = worker_id.into();
        crate::orm_runs::validate_worker_id(&worker_id)?;
        let now = canonical_second(self.clock.now());
        let claim_expires_at = now
            .checked_add(self.limits.claim_ttl)
            .ok_or(AiError::PersistenceFailed)?;
        let maximum = i64::try_from(self.limits.maximum_candidate_scan)
            .map_err(|_| AiError::InvalidConfiguration("invalid waiter scan bound".to_owned()))?;
        self.database()
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let candidates = tx
                        .query::<AiSubscriptionWaiterRecord>()
                        .filter(AiSubscriptionWaiterRecordWhereInput {
                            state: Some(StringFilter {
                                in_list: Some(vec!["waiting".to_owned(), "claimed".to_owned()]),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .default_order()
                        .limit(maximum)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    for waiter in candidates {
                        if waiter.state == "claimed"
                            && waiter
                                .claim_expires_at
                                .is_some_and(|expires| expires > now.unix_timestamp())
                        {
                            continue;
                        }
                        if waiter.events_examined < 0
                            || waiter.maximum_events <= 0
                            || waiter.events_examined > waiter.maximum_events
                            || waiter.claim_generation < 0
                            || waiter.protected_request.is_none()
                            || waiter.protected_cursor.is_none()
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::InternalError));
                        }
                        let run = tx
                            .find_by_id::<AiRunRecord>(&waiter.run_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        let session = tx
                            .find_by_id::<AiSessionRecord>(&waiter.session_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        let call = tx
                            .find_by_id::<AiToolCallRecord>(&waiter.tool_call_id)
                            .await
                            .map_err(OrmPublicError::from)?
                            .ok_or_else(OrmPublicError::not_found)?;
                        let adoptions = tx
                            .query::<AiSubscriptionWaitAdoptionRecord>()
                            .filter(AiSubscriptionWaitAdoptionRecordWhereInput {
                                waiter_id: Some(UuidFilter {
                                    eq: Some(waiter.id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .limit(1)
                            .fetch_all()
                            .await
                            .map_err(OrmPublicError::from)?;
                        if run.session_id != waiter.session_id
                            || run.state != AiRunState::WaitingSubscription.as_str()
                            || run.attempt_id.is_some()
                            || run.lease_owner.is_some()
                            || run.lease_expires_at.is_some()
                            || run.latest_checkpoint_id != Some(waiter.parked_checkpoint_id)
                            || session.id != waiter.session_id
                            || session.state != "active"
                            || session.deleted_at.is_some()
                            || session.owner_principal_kind != waiter.owner_principal_kind
                            || session.owner_subject != waiter.owner_subject
                            || call.run_id != waiter.run_id
                            || call.state != "waiting_subscription"
                            || call.completed_at.is_some()
                            || !adoptions.is_empty()
                        {
                            return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                        }
                        let generation = waiter
                            .claim_generation
                            .checked_add(1)
                            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                        let updated = tx
                            .compare_and_swap::<AiSubscriptionWaiterRecord>(
                                &waiter.id,
                                waiter.row_version,
                                AiSubscriptionWaiterRecordWhereInput::default(),
                                UpdateAiSubscriptionWaiterRecordInput {
                                    state: Some("claimed".to_owned()),
                                    claim_owner: Some(Some(worker_id.clone())),
                                    claim_generation: Some(generation),
                                    claim_expires_at: Some(Some(claim_expires_at.unix_timestamp())),
                                    ..Default::default()
                                },
                            )
                            .await
                            .map_err(OrmPublicError::from)?;
                        if let ConditionalUpdateOutcome::Updated(updated) = updated {
                            return Ok(Some(AiSubscriptionWaitClaim {
                                waiter_id: AiSubscriptionWaiterId(updated.id),
                                run_id: crate::AiRunId(updated.run_id),
                                claim_generation: updated.claim_generation,
                                claim_expires_at,
                                row_version: updated.row_version,
                                worker_id,
                            }));
                        }
                    }
                    Ok(None)
                })
            })
            .await
            .map_err(map_transaction)
    }

    /// Processes at most one durable waiter through replay-then-live.
    ///
    /// One pass opens the replay source and consumes at most one item. The
    /// source timeout is strictly shorter than the waiter-worker claim, so a
    /// stream is always dropped before another worker can reclaim the row.
    /// The suspended run owns no run/coordinator/provider lease.
    ///
    /// # Errors
    ///
    /// Returns only safe service errors. Source unavailability releases the
    /// waiter for replay; retention reset is durably recovery-required.
    pub async fn process_next(
        &self,
        worker_id: impl Into<String>,
    ) -> Result<AiSubscriptionWaitWorkerOutcome, AiError> {
        let Some(claim) = self.claim_next(worker_id).await? else {
            return Ok(AiSubscriptionWaitWorkerOutcome::Idle);
        };
        self.process_claim(claim).await
    }

    async fn process_claim(
        &self,
        claim: AiSubscriptionWaitClaim,
    ) -> Result<AiSubscriptionWaitWorkerOutcome, AiError> {
        let opened = match self.open_claim(&claim).await {
            Ok(opened) => opened,
            Err(AiError::Forbidden | AiError::NotFound | AiError::ReauthorizationFailed) => {
                self.close_claim(&claim, AiRunState::Failed, "subscription_authority_revoked")
                    .await?;
                return Ok(AiSubscriptionWaitWorkerOutcome::Closed {
                    waiter_id: claim.waiter_id,
                    run_id: claim.run_id,
                });
            }
            Err(error) => return Err(error),
        };
        if opened.waiter.expires_at <= self.clock.now().unix_timestamp() {
            self.queue_limit_outcome(&claim, &opened, "timeout", None)
                .await?;
            return Ok(AiSubscriptionWaitWorkerOutcome::Queued {
                waiter_id: claim.waiter_id,
                run_id: claim.run_id,
            });
        }
        let principal = self
            .runtime
            .resolve_current_principal(&opened.principal_reference)
            .await?;
        self.ensure_fresh(&principal, &opened.principal_reference)?;
        let resolved = self.sources.resolve_compiled(&opened.descriptor)?;
        let remaining_claim_seconds = claim
            .claim_expires_at
            .unix_timestamp()
            .saturating_sub(self.clock.now().unix_timestamp())
            .saturating_sub(1);
        if remaining_claim_seconds <= 0 {
            self.release_claim(&claim).await?;
            return Ok(AiSubscriptionWaitWorkerOutcome::Waiting {
                waiter_id: claim.waiter_id,
            });
        }
        let open_timeout = std::time::Duration::from_secs(
            u64::try_from(remaining_claim_seconds).map_err(|_| AiError::Conflict)?,
        );
        let stream = tokio::time::timeout(
            open_timeout,
            resolved.source.open(
                &principal,
                AiReplayableSubscriptionOpenRequest::new(
                    opened.request.clone(),
                    opened.position.clone(),
                ),
            ),
        )
        .await;
        let mut stream = match stream {
            Err(_) => {
                self.release_claim(&claim).await?;
                return Ok(AiSubscriptionWaitWorkerOutcome::Waiting {
                    waiter_id: claim.waiter_id,
                });
            }
            Ok(Ok(stream)) => stream,
            Ok(Err(crate::AiSubscriptionSourceError::Authorization)) => {
                self.close_claim(&claim, AiRunState::Failed, "subscription_authority_revoked")
                    .await?;
                return Ok(AiSubscriptionWaitWorkerOutcome::Closed {
                    waiter_id: claim.waiter_id,
                    run_id: claim.run_id,
                });
            }
            Ok(Err(crate::AiSubscriptionSourceError::ResetRequired)) => {
                self.close_claim(
                    &claim,
                    AiRunState::RecoveryRequired,
                    "subscription_reset_required",
                )
                .await?;
                return Ok(AiSubscriptionWaitWorkerOutcome::RecoveryRequired {
                    waiter_id: claim.waiter_id,
                    run_id: claim.run_id,
                });
            }
            Ok(Err(crate::AiSubscriptionSourceError::Unavailable)) => {
                self.release_claim(&claim).await?;
                return Ok(AiSubscriptionWaitWorkerOutcome::Waiting {
                    waiter_id: claim.waiter_id,
                });
            }
            Ok(Err(error)) => return Err(map_source(error)),
        };
        let waiter_remaining = opened
            .waiter
            .expires_at
            .saturating_sub(self.clock.now().unix_timestamp());
        let claim_remaining = claim
            .claim_expires_at
            .unix_timestamp()
            .saturating_sub(self.clock.now().unix_timestamp())
            .saturating_sub(1);
        let item_seconds = waiter_remaining.min(claim_remaining);
        if item_seconds <= 0 {
            drop(stream);
            if waiter_remaining <= 0 {
                self.queue_limit_outcome(&claim, &opened, "timeout", None)
                    .await?;
                return Ok(AiSubscriptionWaitWorkerOutcome::Queued {
                    waiter_id: claim.waiter_id,
                    run_id: claim.run_id,
                });
            }
            self.release_claim(&claim).await?;
            return Ok(AiSubscriptionWaitWorkerOutcome::Waiting {
                waiter_id: claim.waiter_id,
            });
        }
        let next = tokio::time::timeout(
            std::time::Duration::from_secs(
                u64::try_from(item_seconds).map_err(|_| AiError::Conflict)?,
            ),
            stream.next(),
        )
        .await;
        let event = match next {
            Err(_) => {
                drop(stream);
                if self.clock.now().unix_timestamp() >= opened.waiter.expires_at {
                    self.queue_limit_outcome(&claim, &opened, "timeout", None)
                        .await?;
                    return Ok(AiSubscriptionWaitWorkerOutcome::Queued {
                        waiter_id: claim.waiter_id,
                        run_id: claim.run_id,
                    });
                }
                self.release_claim(&claim).await?;
                return Ok(AiSubscriptionWaitWorkerOutcome::Waiting {
                    waiter_id: claim.waiter_id,
                });
            }
            Ok(None) | Ok(Some(Err(crate::AiSubscriptionSourceError::Unavailable))) => {
                self.release_claim(&claim).await?;
                return Ok(AiSubscriptionWaitWorkerOutcome::Waiting {
                    waiter_id: claim.waiter_id,
                });
            }
            Ok(Some(Ok(AiReplayableSubscriptionSourceItem::ResetRequired)))
            | Ok(Some(Err(crate::AiSubscriptionSourceError::ResetRequired))) => {
                self.close_claim(
                    &claim,
                    AiRunState::RecoveryRequired,
                    "subscription_reset_required",
                )
                .await?;
                return Ok(AiSubscriptionWaitWorkerOutcome::RecoveryRequired {
                    waiter_id: claim.waiter_id,
                    run_id: claim.run_id,
                });
            }
            Ok(Some(Err(crate::AiSubscriptionSourceError::Authorization))) => {
                self.close_claim(&claim, AiRunState::Failed, "subscription_authority_revoked")
                    .await?;
                return Ok(AiSubscriptionWaitWorkerOutcome::Closed {
                    waiter_id: claim.waiter_id,
                    run_id: claim.run_id,
                });
            }
            Ok(Some(Err(error))) => return Err(map_source(error)),
            Ok(Some(Ok(AiReplayableSubscriptionSourceItem::Event(event)))) => event,
        };
        drop(stream);
        let principal = self
            .runtime
            .resolve_current_principal(&opened.principal_reference)
            .await?;
        self.ensure_fresh(&principal, &opened.principal_reference)?;
        let preauthorization = self
            .runtime
            .preauthorize_compiled_subscription(
                &opened.principal_reference,
                &opened.descriptor,
                &opened.request,
            )
            .await?;
        self.ensure_fresh(preauthorization.principal(), &opened.principal_reference)?;
        let response = resolved
            .source
            .authorize_event(preauthorization.principal(), &opened.request, &event)
            .await;
        let response = match response {
            Ok(response) => response,
            Err(crate::AiSubscriptionSourceError::Authorization) => {
                self.close_claim(&claim, AiRunState::Failed, "subscription_authority_revoked")
                    .await?;
                return Ok(AiSubscriptionWaitWorkerOutcome::Closed {
                    waiter_id: claim.waiter_id,
                    run_id: claim.run_id,
                });
            }
            Err(error) => return Err(map_source(error)),
        };
        let validated = self.runtime.validate_compiled_subscription_event(
            &opened.descriptor,
            &opened.disclosure_schema,
            response,
            &preauthorization,
        )?;
        let model_output = validated.model_output();
        enforce_size(&model_output, self.limits.maximum_event_bytes)?;
        let matches = condition_matches(
            opened
                .request
                .contract
                .semantic_operation()
                .ok_or(AiError::Conflict)?
                .field_name(),
            opened.condition.as_ref(),
            &validated.response().data,
        )?;
        let examined = opened
            .waiter
            .events_examined
            .checked_add(1)
            .ok_or(AiError::PersistenceFailed)?;
        if matches {
            self.queue_event_outcome(&claim, &opened, &event, validated)
                .await?;
            return Ok(AiSubscriptionWaitWorkerOutcome::Queued {
                waiter_id: claim.waiter_id,
                run_id: claim.run_id,
            });
        }
        if examined >= opened.waiter.maximum_events {
            self.queue_limit_outcome(&claim, &opened, "event_limit", Some(&event))
                .await?;
            return Ok(AiSubscriptionWaitWorkerOutcome::Queued {
                waiter_id: claim.waiter_id,
                run_id: claim.run_id,
            });
        }
        self.advance_cursor_and_release(&claim, &opened, &event)
            .await?;
        Ok(AiSubscriptionWaitWorkerOutcome::Waiting {
            waiter_id: claim.waiter_id,
        })
    }

    async fn release_claim(&self, claim: &AiSubscriptionWaitClaim) -> Result<(), AiError> {
        let now = canonical_second(self.clock.now());
        let claim = claim.clone();
        self.database()
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let waiter = tx
                        .find_by_id::<AiSubscriptionWaiterRecord>(&claim.waiter_id.0)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    validate_claimed_waiter(&waiter, &claim, now)?;
                    if !matches!(
                        tx.compare_and_swap::<AiSubscriptionWaiterRecord>(
                            &waiter.id,
                            waiter.row_version,
                            waiter_exact_state(&waiter.state),
                            UpdateAiSubscriptionWaiterRecordInput {
                                state: Some("waiting".to_owned()),
                                claim_owner: Some(None),
                                claim_expires_at: Some(None),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?,
                        ConditionalUpdateOutcome::Updated(_)
                    ) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    Ok(())
                })
            })
            .await
            .map_err(map_transaction)
    }

    async fn advance_cursor_and_release(
        &self,
        claim: &AiSubscriptionWaitClaim,
        opened: &OpenedWait,
        event: &AiReplayableSubscriptionEvent,
    ) -> Result<(), AiError> {
        let principal = self
            .runtime
            .resolve_current_principal(&opened.principal_reference)
            .await?;
        self.ensure_fresh(&principal, &opened.principal_reference)?;
        let scope = AiScope {
            kind: opened.waiter.scope_kind.clone(),
            id: opened.waiter.scope_id.clone(),
            tenant_id: opened.waiter.tenant_id.clone(),
        };
        let policy = self.current_protection_policy(&principal, &scope).await?;
        let cursor =
            serde_json::to_value(event.position()).map_err(|_| AiError::PersistenceFailed)?;
        let protected_cursor = self
            .protect(
                &policy,
                content_context(
                    "graphql_orm_ai_subscription_waiters",
                    claim.waiter_id.0,
                    "protected_cursor",
                    &scope,
                ),
                cursor,
            )
            .await?;
        let cursor_fingerprint = event.position().fingerprint().to_owned();
        let events_examined = opened
            .waiter
            .events_examined
            .checked_add(1)
            .ok_or(AiError::PersistenceFailed)?;
        let now = canonical_second(self.clock.now());
        let claim = claim.clone();
        self.database()
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let waiter = tx
                        .find_by_id::<AiSubscriptionWaiterRecord>(&claim.waiter_id.0)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    validate_claimed_waiter(&waiter, &claim, now)?;
                    if waiter.events_examined.checked_add(1) != Some(events_examined)
                        || events_examined >= waiter.maximum_events
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    if !matches!(
                        tx.compare_and_swap::<AiSubscriptionWaiterRecord>(
                            &waiter.id,
                            waiter.row_version,
                            waiter_exact_state(&waiter.state),
                            UpdateAiSubscriptionWaiterRecordInput {
                                cursor_fingerprint: Some(cursor_fingerprint),
                                protected_cursor: Some(Some(protected_cursor)),
                                events_examined: Some(events_examined),
                                state: Some("waiting".to_owned()),
                                claim_owner: Some(None),
                                claim_expires_at: Some(None),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?,
                        ConditionalUpdateOutcome::Updated(_)
                    ) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    Ok(())
                })
            })
            .await
            .map_err(map_transaction)
    }

    async fn queue_event_outcome(
        &self,
        claim: &AiSubscriptionWaitClaim,
        opened: &OpenedWait,
        event: &AiReplayableSubscriptionEvent,
        validated: crate::AiToolExecutionResult,
    ) -> Result<(), AiError> {
        let output = validated.model_output();
        let event_fingerprint = canonical_json_hash(&json!({
            "eventId": event.event_id(),
            "positionFingerprint": event.position().fingerprint(),
            "output": output,
        }))?;
        let source = AiDataSourceRef {
            kind: "application_subscription_event".to_owned(),
            reference: format!("v1:{}:{event_fingerprint}", claim.waiter_id.0),
            classification: validated.disclosure().maximum_classification,
            trust: AiSourceTrust::ResolverResult,
        };
        self.queue_outcome(
            claim,
            opened,
            "matched",
            output,
            Some(event),
            Some(event_fingerprint),
            source,
        )
        .await
    }

    async fn queue_limit_outcome(
        &self,
        claim: &AiSubscriptionWaitClaim,
        opened: &OpenedWait,
        outcome_kind: &str,
        event: Option<&AiReplayableSubscriptionEvent>,
    ) -> Result<(), AiError> {
        let error_code = match outcome_kind {
            "timeout" => "AI_SUBSCRIPTION_WAIT_TIMEOUT",
            "event_limit" => "AI_SUBSCRIPTION_WAIT_EVENT_LIMIT",
            _ => return Err(AiError::Conflict),
        };
        let source = AiDataSourceRef {
            kind: "subscription_wait_outcome".to_owned(),
            reference: format!("v1:{}:{outcome_kind}", claim.waiter_id.0),
            classification: DataClassification::Public,
            trust: AiSourceTrust::TrustedRuntime,
        };
        self.queue_outcome(
            claim,
            opened,
            outcome_kind,
            json!({"data": Value::Null, "errorCodes": [error_code]}),
            event,
            None,
            source,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn queue_outcome(
        &self,
        claim: &AiSubscriptionWaitClaim,
        opened: &OpenedWait,
        outcome_kind: &str,
        output: Value,
        event: Option<&AiReplayableSubscriptionEvent>,
        source_event_fingerprint: Option<String>,
        source: AiDataSourceRef,
    ) -> Result<(), AiError> {
        enforce_size(&output, self.limits.maximum_event_bytes)?;
        let principal = self
            .runtime
            .resolve_current_principal(&opened.principal_reference)
            .await?;
        self.ensure_fresh(&principal, &opened.principal_reference)?;
        let preauthorization = self
            .runtime
            .preauthorize_compiled_subscription(
                &opened.principal_reference,
                &opened.descriptor,
                &opened.request,
            )
            .await?;
        self.ensure_fresh(preauthorization.principal(), &opened.principal_reference)?;
        let scope = AiScope {
            kind: opened.waiter.scope_kind.clone(),
            id: opened.waiter.scope_id.clone(),
            tenant_id: opened.waiter.tenant_id.clone(),
        };
        self.authorize_rules(
            preauthorization.principal(),
            &scope,
            &opened.rule_fingerprint,
            opened.rule_usage,
            &opened.descriptor.fingerprint,
        )
        .await?;
        let bytes = serde_json::to_vec(&output)
            .map_err(|_| AiError::PersistenceFailed)?
            .len();
        let manifest = opened.route.subscription_wait_manifest(
            scope.clone(),
            AiSessionId(opened.waiter.session_id),
            claim.run_id,
            opened.provider_kind.clone(),
            opened.provider_model.clone(),
            source,
            u64::try_from(bytes).map_err(|_| AiError::PersistenceFailed)?,
        );
        let decision = self
            .runtime
            .authorize_egress(&opened.principal_reference, &manifest)
            .await?;
        self.egress_audit.record(&manifest, &decision).await?;
        decision.authorize(&manifest)?;
        let policy = self.current_protection_policy(&principal, &scope).await?;
        let position =
            event.map_or_else(|| opened.position.clone(), |event| event.position().clone());
        let cursor_fingerprint = position.fingerprint().to_owned();
        let protected_cursor = self
            .protect(
                &policy,
                content_context(
                    "graphql_orm_ai_subscription_waiters",
                    claim.waiter_id.0,
                    "protected_cursor",
                    &scope,
                ),
                serde_json::to_value(&position).map_err(|_| AiError::PersistenceFailed)?,
            )
            .await?;
        let event_increment = if event.is_some() { 1 } else { 0 };
        let events_examined = opened
            .waiter
            .events_examined
            .checked_add(event_increment)
            .ok_or(AiError::PersistenceFailed)?;
        let id = Uuid::new_v4();
        let outcome_plaintext = json!({
            "formatVersion": 1,
            "waiterId": claim.waiter_id,
            "outcomeKind": outcome_kind,
            "sourceEventFingerprint": source_event_fingerprint,
            "sourceEvent": event.map(AiReplayableSubscriptionEvent::checkpoint_value),
            "output": output,
            "egressManifest": manifest,
            "egressDecisionId": decision.id,
            "pendingContinuation": opened.continuation,
            "replayToolTransfers": opened.replay_transfers,
            "providerCallId": opened.provider_call_id,
            "toolId": opened.tool_id,
            "providerTurns": opened.provider_turns,
            "totalToolCalls": opened.total_tool_calls,
        });
        let outcome_fingerprint = canonical_json_hash(&outcome_plaintext)?;
        let protected_outcome = self
            .protect(
                &policy,
                content_context(
                    "graphql_orm_ai_subscription_wait_adoptions",
                    id,
                    "protected_outcome",
                    &scope,
                ),
                outcome_plaintext,
            )
            .await?;
        let checkpoint_plaintext = json!({
            "formatVersion": 1,
            "kind": "subscription_wait_adopted",
            "waiterId": claim.waiter_id,
            "parkedCheckpointId": opened.waiter.parked_checkpoint_id,
            "parkedCheckpointFingerprint": opened.waiter.parked_checkpoint_fingerprint,
            "outcomeFingerprint": outcome_fingerprint,
            "cursorFingerprint": cursor_fingerprint,
        });
        let protected_checkpoint = self
            .protect(
                &policy,
                content_context(
                    "graphql_orm_ai_run_checkpoints",
                    id,
                    "protected_state",
                    &scope,
                ),
                checkpoint_plaintext,
            )
            .await?;
        let checkpoint_fingerprint = coordinator_checkpoint_hash(
            claim.run_id,
            opened.waiter.source_attempt_id,
            opened.waiter.source_lease_generation,
            id,
            "subscription_wait_adopted",
            &opened.provider_kind,
            &opened.provider_model,
            None,
            opened.budget_reservation_id,
            &protected_checkpoint,
        )?;
        self.persist_adoption(
            claim,
            PreparedWaitAdoption {
                id,
                outcome_kind: outcome_kind.to_owned(),
                source_event_fingerprint,
                cursor_fingerprint,
                protected_cursor,
                events_examined,
                outcome_fingerprint,
                protected_outcome,
                checkpoint_fingerprint,
                protected_checkpoint,
            },
        )
        .await
    }

    async fn persist_adoption(
        &self,
        claim: &AiSubscriptionWaitClaim,
        adoption: PreparedWaitAdoption,
    ) -> Result<(), AiError> {
        let claim = claim.clone();
        let now = canonical_second(self.clock.now());
        self.database()
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let waiter = tx
                        .find_by_id::<AiSubscriptionWaiterRecord>(&claim.waiter_id.0)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    validate_claimed_waiter(&waiter, &claim, now)?;
                    let run = tx
                        .find_by_id::<AiRunRecord>(&waiter.run_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    let call = tx
                        .find_by_id::<AiToolCallRecord>(&waiter.tool_call_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if run.state != AiRunState::WaitingSubscription.as_str()
                        || run.attempt_id.is_some()
                        || run.lease_owner.is_some()
                        || run.latest_checkpoint_id != Some(waiter.parked_checkpoint_id)
                        || call.run_id != run.id
                        || call.state != "waiting_subscription"
                        || call.completed_at.is_some()
                        || adoption.events_examined > waiter.maximum_events
                        || adoption.events_examined
                            != waiter
                                .events_examined
                                .checked_add(if adoption.outcome_kind == "timeout" {
                                    0
                                } else {
                                    1
                                })
                                .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let existing = tx
                        .query::<AiSubscriptionWaitAdoptionRecord>()
                        .filter(AiSubscriptionWaitAdoptionRecordWhereInput {
                            waiter_id: Some(UuidFilter {
                                eq: Some(waiter.id),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .limit(1)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?;
                    if !existing.is_empty() {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    if !matches!(
                        tx.compare_and_swap::<AiSubscriptionWaiterRecord>(
                            &waiter.id,
                            waiter.row_version,
                            waiter_exact_state(&waiter.state),
                            UpdateAiSubscriptionWaiterRecordInput {
                                cursor_fingerprint: Some(adoption.cursor_fingerprint.clone()),
                                protected_cursor: Some(Some(adoption.protected_cursor.clone())),
                                events_examined: Some(adoption.events_examined),
                                state: Some("adopted".to_owned()),
                                claim_owner: Some(None),
                                claim_expires_at: Some(None),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?,
                        ConditionalUpdateOutcome::Updated(_)
                    ) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    tx.insert::<AiSubscriptionWaitAdoptionRecord>(
                        CreateAiSubscriptionWaitAdoptionRecordInput {
                            id: adoption.id,
                            waiter_id: waiter.id,
                            run_id: run.id,
                            tool_call_id: waiter.tool_call_id,
                            outcome_kind: adoption.outcome_kind,
                            source_event_fingerprint: adoption.source_event_fingerprint,
                            cursor_fingerprint: adoption.cursor_fingerprint,
                            outcome_fingerprint: adoption.outcome_fingerprint,
                            checkpoint_fingerprint: adoption.checkpoint_fingerprint.clone(),
                            protected_outcome: Some(adoption.protected_outcome),
                            state: "queued".to_owned(),
                            queued_at: now.unix_timestamp(),
                            claimed_attempt_id: None,
                            claimed_lease_generation: None,
                            consumed_at: None,
                        },
                    )
                    .await
                    .map_err(OrmPublicError::from)?;
                    tx.insert::<AiRunCheckpointRecord>(CreateAiRunCheckpointRecordInput {
                        id: adoption.id,
                        run_id: run.id,
                        attempt_id: waiter.source_attempt_id,
                        lease_generation: waiter.source_lease_generation,
                        checkpoint_kind: "subscription_wait_adopted".to_owned(),
                        provider_response_id: None,
                        budget_reservation_id: call.budget_reservation_id,
                        assistant_message_id: None,
                        protected_state: Some(adoption.protected_checkpoint),
                        checkpoint_hash: adoption.checkpoint_fingerprint,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    if !matches!(
                        tx.compare_and_swap::<AiRunRecord>(
                            &run.id,
                            run.row_version,
                            exact_state(&run.state),
                            UpdateAiRunRecordInput {
                                state: Some(AiRunState::Queued.as_str().to_owned()),
                                next_attempt_at: Some(Some(now.unix_timestamp())),
                                latest_checkpoint_id: Some(Some(adoption.id)),
                                error_code: Some(None),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?,
                        ConditionalUpdateOutcome::Updated(_)
                    ) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
                        actor_principal_kind: waiter.owner_principal_kind,
                        actor_subject: waiter.owner_subject,
                        action: "ai.subscription_wait.adopt".to_owned(),
                        resource_kind: "ai_subscription_waiter".to_owned(),
                        resource_reference: waiter.id.to_string(),
                        outcome: "queued".to_owned(),
                        reason_code: "bounded_outcome_adopted".to_owned(),
                        correlation_id: call
                            .correlation_id
                            .unwrap_or_else(|| waiter.id.to_string()),
                        causation_id: Some(adoption.id.to_string()),
                        policy_version: None,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    Ok(())
                })
            })
            .await
            .map_err(map_transaction)
    }

    async fn close_claim(
        &self,
        claim: &AiSubscriptionWaitClaim,
        final_state: AiRunState,
        reason_code: &str,
    ) -> Result<(), AiError> {
        if !matches!(
            final_state,
            AiRunState::Failed | AiRunState::Cancelled | AiRunState::RecoveryRequired
        ) {
            return Err(AiError::Conflict);
        }
        let claim = claim.clone();
        let reason_code = reason_code.to_owned();
        let now = canonical_second(self.clock.now());
        self.database()
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let waiter = tx
                        .find_by_id::<AiSubscriptionWaiterRecord>(&claim.waiter_id.0)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    validate_claimed_waiter(&waiter, &claim, now)?;
                    let run = tx
                        .find_by_id::<AiRunRecord>(&waiter.run_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    let call = tx
                        .find_by_id::<AiToolCallRecord>(&waiter.tool_call_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    let step = tx
                        .find_by_id::<AiRunStepRecord>(&waiter.tool_call_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if run.state != AiRunState::WaitingSubscription.as_str()
                        || !AiRunState::WaitingSubscription.can_transition_to(final_state)
                        || call.state != "waiting_subscription"
                        || step.state != "waiting_subscription"
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let waiter_state = match final_state {
                        AiRunState::Cancelled => "cancelled",
                        AiRunState::RecoveryRequired => "recovery_required",
                        _ => "failed",
                    };
                    if !matches!(
                        tx.compare_and_swap::<AiSubscriptionWaiterRecord>(
                            &waiter.id,
                            waiter.row_version,
                            waiter_exact_state(&waiter.state),
                            UpdateAiSubscriptionWaiterRecordInput {
                                state: Some(waiter_state.to_owned()),
                                claim_owner: Some(None),
                                claim_expires_at: Some(None),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?,
                        ConditionalUpdateOutcome::Updated(_)
                    ) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    if !matches!(
                        tx.compare_and_swap::<AiToolCallRecord>(
                            &call.id,
                            call.row_version,
                            tool_call_exact_state(&call.state),
                            UpdateAiToolCallRecordInput {
                                state: Some(waiter_state.to_owned()),
                                authorization_code: Some(Some(reason_code.clone())),
                                completed_at: Some(Some(now.unix_timestamp())),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?,
                        ConditionalUpdateOutcome::Updated(_)
                    ) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    if !matches!(
                        tx.compare_and_swap::<AiRunStepRecord>(
                            &step.id,
                            step.row_version,
                            run_step_row_version_only(),
                            UpdateAiRunStepRecordInput {
                                state: Some(waiter_state.to_owned()),
                                finished_at: Some(Some(now.unix_timestamp())),
                                error_code: Some(Some(reason_code.clone())),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?,
                        ConditionalUpdateOutcome::Updated(_)
                    ) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    if !matches!(
                        tx.compare_and_swap::<AiRunRecord>(
                            &run.id,
                            run.row_version,
                            exact_state(&run.state),
                            UpdateAiRunRecordInput {
                                state: Some(final_state.as_str().to_owned()),
                                next_attempt_at: Some(None),
                                error_code: Some(Some(reason_code)),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?,
                        ConditionalUpdateOutcome::Updated(_)
                    ) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    append_terminal_run_event(tx, &run, final_state, now).await
                })
            })
            .await
            .map_err(map_transaction)
    }

    async fn adopt_wait(
        &self,
        lease: &AiRunLease,
        checkpoint_id: Uuid,
    ) -> Result<Option<crate::AiAdoptedReadOnlyToolBatch>, AiError> {
        let checkpoint = AiRunCheckpointRecord::find_by_id(self.database(), &checkpoint_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        if checkpoint.checkpoint_kind != "subscription_wait_adopted" {
            return Ok(None);
        }
        let adoption =
            AiSubscriptionWaitAdoptionRecord::find_by_id(self.database(), &checkpoint_id)
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
                .ok_or(AiError::NotFound)?;
        let waiter = AiSubscriptionWaiterRecord::find_by_id(self.database(), &adoption.waiter_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        let session = AiSessionRecord::find_by_id(self.database(), &lease.session_id().0)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        if lease.latest_checkpoint_id() != Some(checkpoint_id)
            || waiter.state != "adopted"
            || waiter.run_id != lease.run_id().0
            || waiter.session_id != lease.session_id().0
            || adoption.waiter_id != waiter.id
            || adoption.run_id != waiter.run_id
            || adoption.tool_call_id != waiter.tool_call_id
            || adoption.state != "queued"
            || adoption.claimed_attempt_id.is_some()
            || adoption.claimed_lease_generation.is_some()
            || adoption.consumed_at.is_some()
            || adoption.checkpoint_fingerprint != checkpoint.checkpoint_hash
            || checkpoint.run_id != waiter.run_id
            || checkpoint.attempt_id != waiter.source_attempt_id
            || checkpoint.lease_generation != waiter.source_lease_generation
            || checkpoint.protected_state.is_none()
            || adoption.protected_outcome.is_none()
        {
            return Err(AiError::Conflict);
        }
        let principal_reference: PrincipalReference =
            serde_json::from_value(waiter.principal_reference.clone())
                .map_err(|_| AiError::PersistenceFailed)?;
        if &principal_reference != lease.principal_reference()
            || canonical_json_hash(&waiter.principal_reference)?
                != waiter.principal_reference_fingerprint
        {
            return Err(AiError::Conflict);
        }
        let principal = self
            .runtime
            .resolve_current_principal(&principal_reference)
            .await?;
        self.ensure_fresh(&principal, &principal_reference)?;
        let scope = record_scope(&session);
        self.authorize_session(&principal, &session, lease, &scope)
            .await?;
        let policy = self.current_protection_policy(&principal, &scope).await?;
        let request_value = self
            .open(
                &policy,
                content_context(
                    "graphql_orm_ai_subscription_waiters",
                    waiter.id,
                    "protected_request",
                    &scope,
                ),
                waiter
                    .protected_request
                    .as_ref()
                    .ok_or(AiError::PersistenceFailed)?,
            )
            .await?;
        let request_payload: ProtectedWaitRequest =
            serde_json::from_value(request_value).map_err(|_| AiError::PersistenceFailed)?;
        let outcome_value = self
            .open(
                &policy,
                content_context(
                    "graphql_orm_ai_subscription_wait_adoptions",
                    adoption.id,
                    "protected_outcome",
                    &scope,
                ),
                adoption
                    .protected_outcome
                    .as_ref()
                    .ok_or(AiError::PersistenceFailed)?,
            )
            .await?;
        if canonical_json_hash(&outcome_value)? != adoption.outcome_fingerprint {
            return Err(AiError::Conflict);
        }
        let outcome: ProtectedWaitOutcome =
            serde_json::from_value(outcome_value).map_err(|_| AiError::PersistenceFailed)?;
        let provider: ProtectedProviderResult =
            serde_json::from_value(request_payload.provider_result.clone())
                .map_err(|_| AiError::PersistenceFailed)?;
        let provider_tool = provider.tool_calls.first().ok_or(AiError::Conflict)?;
        let tool_id = AiToolId::parse(provider_tool.tool_id.clone())?;
        let compiled = self
            .runtime
            .tool_catalog()
            .compile_subscription_capability(
                &tool_id,
                &request_payload.capability_fingerprint,
                provider_tool.arguments.clone(),
            )?;
        let (
            descriptor,
            disclosure_schema,
            variables,
            condition,
            _,
            maximum_events,
            capability_fingerprint,
            plan_fingerprint,
        ) = compiled.into_parts();
        if outcome.format_version != 1
            || outcome.waiter_id.0 != waiter.id
            || outcome.outcome_kind != adoption.outcome_kind
            || outcome.source_event_fingerprint != adoption.source_event_fingerprint
            || outcome.egress_decision_id.0.is_nil()
            || outcome.provider_call_id != provider_tool.call_id
            || outcome.tool_id != provider_tool.tool_id
            || outcome.provider_turns != request_payload.provider_turns
            || outcome.total_tool_calls != request_payload.total_tool_calls
            || descriptor != request_payload.descriptor
            || disclosure_schema != request_payload.disclosure_schema
            || variables != request_payload.variables
            || condition != request_payload.condition
            || i64::from(maximum_events) != waiter.maximum_events
            || capability_fingerprint != waiter.capability_fingerprint
            || plan_fingerprint != waiter.plan_fingerprint
            || descriptor.fingerprint != waiter.compiled_descriptor_fingerprint
        {
            return Err(AiError::Conflict);
        }
        let contract = descriptor
            .graphql_contract
            .clone()
            .ok_or(AiError::Conflict)?;
        let request = ToolGraphqlRequest {
            document: descriptor.document.clone(),
            operation_name: contract.operation_name.clone(),
            contract,
            variables,
            invocation: GraphqlInvocationContext {
                run_id: lease.run_id(),
                tool_call_id: AiToolCallId(waiter.tool_call_id),
                scope: scope.clone(),
                correlation_id: request_payload.correlation_id,
                causation_id: provider.budget_reservation_id.to_string(),
                delegation_reference: None,
                idempotency_key: None,
            },
        };
        let preauthorization = self
            .runtime
            .preauthorize_compiled_subscription(&principal_reference, &descriptor, &request)
            .await?;
        self.ensure_fresh(preauthorization.principal(), &principal_reference)?;
        self.authorize_rules(
            preauthorization.principal(),
            &scope,
            &request_payload.rule_fingerprint,
            request_payload.rule_usage,
            &descriptor.fingerprint,
        )
        .await?;
        let resolved = self.sources.resolve_compiled(&descriptor)?;
        if resolved.descriptor.source_id() != waiter.source_id
            || resolved.descriptor.fingerprint() != waiter.source_registration_fingerprint
        {
            return Err(AiError::Conflict);
        }
        if let Some(event_value) = outcome.source_event {
            let event = AiReplayableSubscriptionEvent::from_checkpoint_value(event_value)
                .map_err(map_source)?;
            let event_response = resolved
                .source
                .authorize_event(preauthorization.principal(), &request, &event)
                .await
                .map_err(map_source)?;
            let validated = self.runtime.validate_compiled_subscription_event(
                &descriptor,
                &disclosure_schema,
                event_response,
                &preauthorization,
            )?;
            let expected_event_fingerprint = canonical_json_hash(&json!({
                "eventId": event.event_id(),
                "positionFingerprint": event.position().fingerprint(),
                "output": validated.model_output(),
            }))?;
            if validated.model_output() != outcome.output
                || outcome.source_event_fingerprint.as_deref()
                    != Some(expected_event_fingerprint.as_str())
            {
                return Err(AiError::Conflict);
            }
        } else if outcome.source_event_fingerprint.is_some() || outcome.outcome_kind == "matched" {
            return Err(AiError::Conflict);
        }
        let route =
            AiToolResultEgressRoute::from_checkpoint_value(request_payload.result_egress_route)?;
        if !route.matches_manifest(
            &outcome.egress_manifest,
            lease,
            &scope,
            provider.provider_kind.as_str(),
            &provider.provider_model,
        ) {
            return Err(AiError::Conflict);
        }
        let decision = self
            .runtime
            .authorize_egress(&principal_reference, &outcome.egress_manifest)
            .await?;
        self.egress_audit
            .record(&outcome.egress_manifest, &decision)
            .await?;
        decision.authorize(&outcome.egress_manifest)?;
        let continuation = crate::AiAgentContinuation::from_subscription_result(
            outcome.pending_continuation,
            outcome.provider_call_id,
            outcome.tool_id,
            outcome.output,
            outcome.egress_manifest,
            outcome.replay_tool_transfers,
        )?;
        self.claim_adoption(lease, &adoption).await?;
        Ok(Some(crate::AiAdoptedReadOnlyToolBatch::new(
            adoption.id,
            outcome.provider_turns,
            outcome.total_tool_calls,
            scope,
            continuation,
            request_payload.rule_fingerprint,
            request_payload.rule_usage,
        )))
    }

    async fn claim_adoption(
        &self,
        lease: &AiRunLease,
        adoption: &AiSubscriptionWaitAdoptionRecord,
    ) -> Result<(), AiError> {
        let lease = lease.clone();
        let adoption = adoption.clone();
        let now = canonical_second(self.clock.now());
        self.database()
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    load_and_validate_active_lease(tx, &lease, now).await?;
                    if !matches!(
                        tx.compare_and_swap::<AiSubscriptionWaitAdoptionRecord>(
                            &adoption.id,
                            adoption.row_version,
                            AiSubscriptionWaitAdoptionRecordWhereInput {
                                state: Some(StringFilter {
                                    eq: Some("queued".to_owned()),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            },
                            UpdateAiSubscriptionWaitAdoptionRecordInput {
                                state: Some("claimed".to_owned()),
                                claimed_attempt_id: Some(Some(lease.attempt_id())),
                                claimed_lease_generation: Some(Some(lease.lease_generation())),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?,
                        ConditionalUpdateOutcome::Updated(_)
                    ) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    Ok(())
                })
            })
            .await
            .map_err(map_transaction)
    }

    async fn consume_wait_adoption(
        &self,
        lease: &AiRunLease,
        checkpoint_id: Uuid,
    ) -> Result<AiRunLease, AiError> {
        let lease = lease.clone();
        let now = canonical_second(self.clock.now());
        let ttl = self.run_service.lease_ttl();
        let updated = self
            .database()
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let run = load_and_validate_active_lease(tx, &lease, now).await?;
                    let adoption = tx
                        .find_by_id::<AiSubscriptionWaitAdoptionRecord>(&checkpoint_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    let waiter = tx
                        .find_by_id::<AiSubscriptionWaiterRecord>(&adoption.waiter_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    let call = tx
                        .find_by_id::<AiToolCallRecord>(&adoption.tool_call_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    let step = tx
                        .find_by_id::<AiRunStepRecord>(&adoption.tool_call_id)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if run.latest_checkpoint_id != Some(checkpoint_id)
                        || adoption.state != "claimed"
                        || adoption.claimed_attempt_id != Some(lease.attempt_id())
                        || adoption.claimed_lease_generation != Some(lease.lease_generation())
                        || adoption.consumed_at.is_some()
                        || waiter.state != "adopted"
                        || call.state != "waiting_subscription"
                        || step.state != "waiting_subscription"
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    if !matches!(
                        tx.compare_and_swap::<AiSubscriptionWaitAdoptionRecord>(
                            &adoption.id,
                            adoption.row_version,
                            AiSubscriptionWaitAdoptionRecordWhereInput::default(),
                            UpdateAiSubscriptionWaitAdoptionRecordInput {
                                state: Some("consumed".to_owned()),
                                consumed_at: Some(Some(now.unix_timestamp())),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?,
                        ConditionalUpdateOutcome::Updated(_)
                    ) || !matches!(
                        tx.compare_and_swap::<AiToolCallRecord>(
                            &call.id,
                            call.row_version,
                            tool_call_exact_state(&call.state),
                            UpdateAiToolCallRecordInput {
                                state: Some("completed".to_owned()),
                                authorization_code: Some(Some(
                                    "subscription_wait_adopted".to_owned()
                                )),
                                completed_at: Some(Some(now.unix_timestamp())),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?,
                        ConditionalUpdateOutcome::Updated(_)
                    ) || !matches!(
                        tx.compare_and_swap::<AiRunStepRecord>(
                            &step.id,
                            step.row_version,
                            run_step_row_version_only(),
                            UpdateAiRunStepRecordInput {
                                state: Some("completed".to_owned()),
                                finished_at: Some(Some(now.unix_timestamp())),
                                error_code: Some(None),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?,
                        ConditionalUpdateOutcome::Updated(_)
                    ) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let expires = now
                        .checked_add(ttl)
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::InternalError))?;
                    match tx
                        .compare_and_swap::<AiRunRecord>(
                            &run.id,
                            run.row_version,
                            exact_state(&run.state),
                            UpdateAiRunRecordInput {
                                latest_checkpoint_id: Some(None),
                                lease_expires_at: Some(Some(expires.unix_timestamp())),
                                lease_heartbeat_at: Some(Some(now.unix_timestamp())),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?
                    {
                        ConditionalUpdateOutcome::Updated(run) => Ok(run),
                        _ => Err(OrmPublicError::new(OrmErrorCode::Conflict)),
                    }
                })
            })
            .await
            .map_err(map_transaction)?;
        crate::orm_runs::lease_from_record(&updated).map_err(map_orm)
    }

    async fn open_claim(&self, claim: &AiSubscriptionWaitClaim) -> Result<OpenedWait, AiError> {
        let waiter = AiSubscriptionWaiterRecord::find_by_id(self.database(), &claim.waiter_id.0)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        let now = self.clock.now().unix_timestamp();
        if waiter.run_id != claim.run_id.0
            || waiter.state != "claimed"
            || waiter.claim_owner.as_deref() != Some(claim.worker_id.as_str())
            || waiter.claim_generation != claim.claim_generation
            || waiter.claim_expires_at != Some(claim.claim_expires_at.unix_timestamp())
            || waiter.claim_expires_at.is_none_or(|expires| expires <= now)
            || waiter.row_version != claim.row_version
        {
            return Err(AiError::Conflict);
        }
        let session = AiSessionRecord::find_by_id(self.database(), &waiter.session_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        let run = AiRunRecord::find_by_id(self.database(), &waiter.run_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        let call = AiToolCallRecord::find_by_id(self.database(), &waiter.tool_call_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        let source_checkpoint =
            AiRunCheckpointRecord::find_by_id(self.database(), &waiter.source_checkpoint_id)
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
                .ok_or(AiError::NotFound)?;
        let parked_checkpoint =
            AiRunCheckpointRecord::find_by_id(self.database(), &waiter.parked_checkpoint_id)
                .await
                .map_err(|error| map_orm(OrmPublicError::from(error)))?
                .ok_or(AiError::NotFound)?;
        if run.session_id != waiter.session_id
            || run.state != AiRunState::WaitingSubscription.as_str()
            || run.attempt_id.is_some()
            || run.lease_owner.is_some()
            || run.latest_checkpoint_id != Some(waiter.parked_checkpoint_id)
            || call.run_id != run.id
            || call.state != "waiting_subscription"
            || call.id != waiter.tool_call_id
            || call.completed_at.is_some()
            || source_checkpoint.id != waiter.source_checkpoint_id
            || source_checkpoint.run_id != waiter.run_id
            || source_checkpoint.attempt_id != waiter.source_attempt_id
            || source_checkpoint.lease_generation != waiter.source_lease_generation
            || source_checkpoint.checkpoint_kind != "provider_turn_persisted"
            || source_checkpoint.checkpoint_hash != waiter.source_checkpoint_fingerprint
            || parked_checkpoint.id != waiter.parked_checkpoint_id
            || parked_checkpoint.run_id != waiter.run_id
            || parked_checkpoint.attempt_id != waiter.source_attempt_id
            || parked_checkpoint.lease_generation != waiter.source_lease_generation
            || parked_checkpoint.checkpoint_kind != "subscription_wait_parked"
            || parked_checkpoint.checkpoint_hash != waiter.parked_checkpoint_fingerprint
            || parked_checkpoint.protected_state.is_none()
        {
            return Err(AiError::Conflict);
        }
        let principal_reference: PrincipalReference =
            serde_json::from_value(waiter.principal_reference.clone())
                .map_err(|_| AiError::PersistenceFailed)?;
        if canonical_json_hash(&waiter.principal_reference)?
            != waiter.principal_reference_fingerprint
            || principal_reference.subject != waiter.owner_subject
        {
            return Err(AiError::Conflict);
        }
        let principal = self
            .runtime
            .resolve_current_principal(&principal_reference)
            .await?;
        self.ensure_fresh(&principal, &principal_reference)?;
        let scope = record_scope(&session);
        self.authorize_waiter_session(&principal, &session, &waiter, &scope)
            .await?;
        let policy = self.current_protection_policy(&principal, &scope).await?;
        let protected_request = waiter
            .protected_request
            .as_ref()
            .ok_or(AiError::PersistenceFailed)?;
        let request_value = self
            .open(
                &policy,
                content_context(
                    "graphql_orm_ai_subscription_waiters",
                    waiter.id,
                    "protected_request",
                    &scope,
                ),
                protected_request,
            )
            .await?;
        enforce_size(&request_value, self.limits.maximum_protected_request_bytes)?;
        let payload: ProtectedWaitRequest = serde_json::from_value(request_value.clone())
            .map_err(|_| AiError::PersistenceFailed)?;
        let cursor_value = self
            .open(
                &policy,
                content_context(
                    "graphql_orm_ai_subscription_waiters",
                    waiter.id,
                    "protected_cursor",
                    &scope,
                ),
                waiter
                    .protected_cursor
                    .as_ref()
                    .ok_or(AiError::PersistenceFailed)?,
            )
            .await?;
        let position: AiSubscriptionReplayPosition =
            serde_json::from_value(cursor_value).map_err(|_| AiError::PersistenceFailed)?;
        if !position.has_valid_fingerprint() || position.fingerprint() != waiter.cursor_fingerprint
        {
            return Err(AiError::Conflict);
        }
        let provider: ProtectedProviderResult =
            serde_json::from_value(payload.provider_result.clone())
                .map_err(|_| AiError::PersistenceFailed)?;
        let provider_tool = provider.tool_calls.first().ok_or(AiError::Conflict)?;
        if payload.format_version != 1
            || payload.scope != scope
            || payload.source_id != waiter.source_id
            || payload.source_registration_fingerprint != waiter.source_registration_fingerprint
            || payload.capability_fingerprint != waiter.capability_fingerprint
            || payload.plan_fingerprint != waiter.plan_fingerprint
            || payload.provider_turns == 0
            || payload.total_tool_calls == 0
            || provider.format_version != 1
            || provider.session_id != waiter.session_id
            || provider.run_id != waiter.run_id
            || provider.attempt_id != waiter.source_attempt_id
            || provider.lease_generation != waiter.source_lease_generation
            || provider.tool_calls.len() != 1
            || provider.budget_reservation_id
                != call.budget_reservation_id.ok_or(AiError::Conflict)?
            || provider.provider_response_id != call.provider_response_id
            || provider.provider_kind.as_str()
                != call.provider_kind.as_deref().ok_or(AiError::Conflict)?
            || provider.provider_model != call.provider_model.as_deref().ok_or(AiError::Conflict)?
            || provider_tool.call_id != call.provider_call_id
            || provider_tool.tool_id != call.tool_id
            || provider_tool.tool_fingerprint != call.tool_fingerprint
            || canonical_json_hash(&provider_tool.arguments)? != call.argument_hash
            || canonical_json_hash(&payload.variables)? != waiter.variables_fingerprint
            || canonical_json_hash(
                &serde_json::to_value(&payload.condition)
                    .map_err(|_| AiError::PersistenceFailed)?,
            )? != waiter.condition_fingerprint
        {
            return Err(AiError::Conflict);
        }
        let tool_id = AiToolId::parse(provider_tool.tool_id.clone())?;
        let compiled = self
            .runtime
            .tool_catalog()
            .compile_subscription_capability(
                &tool_id,
                &payload.capability_fingerprint,
                provider_tool.arguments.clone(),
            )?;
        let (
            descriptor,
            disclosure_schema,
            variables,
            condition,
            _timeout_seconds,
            maximum_events,
            capability_fingerprint,
            plan_fingerprint,
        ) = compiled.into_parts();
        if descriptor != payload.descriptor
            || disclosure_schema != payload.disclosure_schema
            || variables != payload.variables
            || condition != payload.condition
            || i64::from(maximum_events) != waiter.maximum_events
            || capability_fingerprint != waiter.capability_fingerprint
            || plan_fingerprint != waiter.plan_fingerprint
            || descriptor.fingerprint != waiter.compiled_descriptor_fingerprint
        {
            return Err(AiError::Conflict);
        }
        let contract = descriptor
            .graphql_contract
            .clone()
            .ok_or(AiError::Conflict)?;
        let semantic = contract.semantic_operation().ok_or(AiError::Conflict)?;
        if contract.target_id.as_str() != waiter.target_id
            || contract.schema_fingerprint != waiter.target_schema_fingerprint
            || contract.operation_name != waiter.operation_name
            || contract.document_hash != waiter.operation_document_hash
            || contract.result_projection_fingerprint != waiter.result_projection_fingerprint
            || contract.disclosure_schema_fingerprint != waiter.disclosure_schema_fingerprint
            || semantic.catalog_fingerprint() != waiter.semantic_catalog_fingerprint
            || semantic.operation_fingerprint() != waiter.operation_fingerprint
        {
            return Err(AiError::Conflict);
        }
        let request = ToolGraphqlRequest {
            document: descriptor.document.clone(),
            operation_name: contract.operation_name.clone(),
            contract,
            variables,
            invocation: GraphqlInvocationContext {
                run_id: crate::AiRunId(waiter.run_id),
                tool_call_id: AiToolCallId(waiter.tool_call_id),
                scope: scope.clone(),
                correlation_id: payload.correlation_id.clone(),
                causation_id: provider.budget_reservation_id.to_string(),
                delegation_reference: None,
                idempotency_key: None,
            },
        };
        let preauthorization = self
            .runtime
            .preauthorize_compiled_subscription(&principal_reference, &descriptor, &request)
            .await?;
        self.ensure_fresh(preauthorization.principal(), &principal_reference)?;
        self.authorize_rules(
            preauthorization.principal(),
            &scope,
            &payload.rule_fingerprint,
            payload.rule_usage,
            &descriptor.fingerprint,
        )
        .await?;
        let source = self.sources.resolve_compiled(&descriptor)?;
        if source.descriptor.source_id() != waiter.source_id
            || source.descriptor.fingerprint() != waiter.source_registration_fingerprint
        {
            return Err(AiError::Conflict);
        }
        let route = AiToolResultEgressRoute::from_checkpoint_value(payload.result_egress_route)?;
        Ok(OpenedWait {
            waiter,
            principal_reference,
            descriptor,
            disclosure_schema,
            condition,
            request,
            route,
            continuation: payload.pending_continuation,
            replay_transfers: payload.replay_tool_transfers,
            provider_kind: provider.provider_kind.as_str().to_owned(),
            provider_model: provider.provider_model,
            budget_reservation_id: provider.budget_reservation_id,
            provider_call_id: provider_tool.call_id.clone(),
            tool_id: provider_tool.tool_id.clone(),
            rule_fingerprint: payload.rule_fingerprint,
            rule_usage: payload.rule_usage,
            provider_turns: payload.provider_turns,
            total_tool_calls: payload.total_tool_calls,
            position,
        })
    }

    fn ensure_fresh(
        &self,
        resolved: &ResolvedPrincipal,
        expected: &PrincipalReference,
    ) -> Result<(), AiError> {
        let now = self.clock.now();
        if resolved.reference() != expected
            || resolved.resolved_at() > now
            || now - resolved.resolved_at() >= self.limits.maximum_principal_age
            || expected
                .expires_at
                .is_some_and(|expires_at| expires_at <= now)
        {
            return Err(AiError::ReauthorizationFailed);
        }
        Ok(())
    }

    async fn authorize_session(
        &self,
        principal: &ResolvedPrincipal,
        session: &AiSessionRecord,
        lease: &AiRunLease,
        scope: &AiScope,
    ) -> Result<(), AiError> {
        let (kind, subject) = principal_identity(principal.principal());
        if session.id != lease.session_id().0
            || session.owner_principal_kind != kind
            || session.owner_subject != subject
            || session.state != "active"
            || session.deleted_at.is_some()
            || record_scope(session) != *scope
            || !self
                .runtime
                .access_policy()
                .can_access_session(
                    principal.principal(),
                    AiSessionId(session.id),
                    AiSessionAction::Write,
                )
                .await
                .is_allowed()
            || !self
                .runtime
                .access_policy()
                .can_access_scope(principal.principal(), scope, AiSessionAction::Write)
                .await
                .is_allowed()
        {
            return Err(AiError::Forbidden);
        }
        Ok(())
    }

    async fn authorize_waiter_session(
        &self,
        principal: &ResolvedPrincipal,
        session: &AiSessionRecord,
        waiter: &AiSubscriptionWaiterRecord,
        scope: &AiScope,
    ) -> Result<(), AiError> {
        let (kind, subject) = principal_identity(principal.principal());
        if session.id != waiter.session_id
            || waiter.run_id.is_nil()
            || waiter.owner_principal_kind != kind
            || waiter.owner_subject != subject
            || session.owner_principal_kind != kind
            || session.owner_subject != subject
            || session.state != "active"
            || session.deleted_at.is_some()
            || record_scope(session) != *scope
            || waiter.scope_key != ai_scope_key(scope)
            || waiter.scope_kind != scope.kind
            || waiter.scope_id != scope.id
            || waiter.tenant_id != scope.tenant_id
            || !self
                .runtime
                .access_policy()
                .can_access_session(
                    principal.principal(),
                    AiSessionId(session.id),
                    AiSessionAction::Write,
                )
                .await
                .is_allowed()
            || !self
                .runtime
                .access_policy()
                .can_access_scope(principal.principal(), scope, AiSessionAction::Write)
                .await
                .is_allowed()
        {
            return Err(AiError::Forbidden);
        }
        Ok(())
    }

    async fn current_protection_policy(
        &self,
        principal: &ResolvedPrincipal,
        scope: &AiScope,
    ) -> Result<AiContentProtectionPolicy, AiError> {
        let policy = self
            .runtime
            .content_protection_policy_resolver()
            .resolve(principal.principal(), scope)
            .await?;
        if !policy.ready || policy.scope != *scope {
            return Err(AiError::RuntimeNotReady);
        }
        Ok(policy)
    }

    async fn authorize_rules(
        &self,
        principal: &ResolvedPrincipal,
        scope: &AiScope,
        expected_fingerprint: &str,
        usage: AiRuleRunUsage,
        tool_fingerprint: &str,
    ) -> Result<AiAgentRuleResolution, AiError> {
        let rules = self
            .rule_service
            .resolve_for_run(principal.principal(), scope.clone())
            .await?;
        if rules.target_scope() != scope
            || rules.fingerprint() != expected_fingerprint
            || rules.constrain_tool(
                tool_fingerprint,
                ToolMaturity::ReadOnly,
                crate::AiApprovalRule::None,
            ) != Some(crate::AiApprovalRule::None)
        {
            return Err(AiError::Forbidden);
        }
        let resolution = AiAgentRuleResolution::new(rules, self.clock.now())?;
        usage.validate(&resolution)?;
        Ok(resolution)
    }

    async fn protect(
        &self,
        policy: &AiContentProtectionPolicy,
        context: ContentProtectionContext,
        value: serde_json::Value,
    ) -> Result<serde_json::Value, AiError> {
        let envelope = self
            .runtime
            .content_protector()
            .protect(policy, &context, value)
            .await
            .map_err(map_protection)?;
        serde_json::to_value(envelope).map_err(|_| AiError::PersistenceFailed)
    }

    async fn open(
        &self,
        policy: &AiContentProtectionPolicy,
        context: ContentProtectionContext,
        value: &Value,
    ) -> Result<Value, AiError> {
        let envelope: ProtectedContentEnvelope =
            serde_json::from_value(value.clone()).map_err(|_| AiError::PersistenceFailed)?;
        self.runtime
            .content_protector()
            .open(policy, &context, &envelope)
            .await
            .map_err(map_protection)
    }

    async fn persist_registration(
        &self,
        lease: &AiRunLease,
        registration: PreparedWaitRegistration,
        now: OffsetDateTime,
    ) -> Result<(), AiError> {
        let lease = lease.clone();
        self.database()
            .transaction(TransactionMode::StateMachine, move |tx| {
                Box::pin(async move {
                    let run = load_and_validate_active_lease(tx, &lease, now).await?;
                    if run.state != AiRunState::Running.as_str() {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let session = tx
                        .find_by_id::<AiSessionRecord>(&lease.session_id().0)
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if session.state != "active"
                        || session.deleted_at.is_some()
                        || session.owner_principal_kind
                            != registration.expected_owner_principal_kind
                        || session.owner_subject != registration.expected_owner_subject
                        || record_scope(&session) != registration.scope
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let checkpoint_id = run
                        .latest_checkpoint_id
                        .ok_or_else(|| OrmPublicError::new(OrmErrorCode::Conflict))?;
                    let checkpoint = tx
                        .query::<AiRunCheckpointRecord>()
                        .filter(AiRunCheckpointRecordWhereInput {
                            id: Some(UuidFilter {
                                eq: Some(checkpoint_id),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .limit(1)
                        .fetch_one()
                        .await
                        .map_err(OrmPublicError::from)?
                        .ok_or_else(OrmPublicError::not_found)?;
                    if checkpoint.id != registration.source_checkpoint_id
                        || checkpoint.checkpoint_hash != registration.source_checkpoint_fingerprint
                        || checkpoint.run_id != run.id
                        || checkpoint.attempt_id != lease.attempt_id()
                        || checkpoint.lease_generation != lease.lease_generation()
                        || checkpoint.checkpoint_kind != "provider_turn_persisted"
                        || checkpoint.provider_response_id != registration.provider_response_id
                        || checkpoint.budget_reservation_id
                            != Some(registration.budget_reservation_id)
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    let expected_parked_checkpoint_fingerprint = coordinator_checkpoint_hash(
                        lease.run_id(),
                        lease.attempt_id(),
                        lease.lease_generation(),
                        registration.parked_checkpoint_id,
                        "subscription_wait_parked",
                        &registration.provider_kind,
                        &registration.provider_model,
                        registration.provider_response_id.as_deref(),
                        registration.budget_reservation_id,
                        &registration.protected_parked_checkpoint,
                    )
                    .map_err(|_| OrmPublicError::new(OrmErrorCode::Conflict))?;
                    if expected_parked_checkpoint_fingerprint
                        != registration.parked_checkpoint_fingerprint
                    {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    if !matches!(
                        tx.compare_and_swap::<AiRunRecord>(
                            &run.id,
                            run.row_version,
                            exact_state(AiRunState::Running.as_str()),
                            UpdateAiRunRecordInput {
                                state: Some(AiRunState::WaitingSubscription.as_str().to_owned()),
                                attempt_id: Some(None),
                                lease_owner: Some(None),
                                lease_expires_at: Some(None),
                                lease_heartbeat_at: Some(None),
                                next_attempt_at: Some(None),
                                latest_checkpoint_id: Some(
                                    Some(registration.parked_checkpoint_id,)
                                ),
                                error_code: Some(Some("subscription_wait_registered".to_owned())),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(OrmPublicError::from)?,
                        ConditionalUpdateOutcome::Updated(_)
                    ) {
                        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
                    }
                    tx.insert::<AiRunCheckpointRecord>(CreateAiRunCheckpointRecordInput {
                        id: registration.parked_checkpoint_id,
                        run_id: run.id,
                        attempt_id: lease.attempt_id(),
                        lease_generation: lease.lease_generation(),
                        checkpoint_kind: "subscription_wait_parked".to_owned(),
                        provider_response_id: registration.provider_response_id.clone(),
                        budget_reservation_id: Some(registration.budget_reservation_id),
                        assistant_message_id: None,
                        protected_state: Some(registration.protected_parked_checkpoint),
                        checkpoint_hash: registration.parked_checkpoint_fingerprint.clone(),
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    tx.insert::<AiRunStepRecord>(CreateAiRunStepRecordInput {
                        id: registration.tool_call_id,
                        run_id: run.id,
                        step_index: registration.tool_step_index,
                        step_kind: "subscription_wait".to_owned(),
                        state: "waiting_subscription".to_owned(),
                        lease_generation: lease.lease_generation(),
                        started_at: Some(now.unix_timestamp()),
                        finished_at: None,
                        error_code: None,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    tx.insert::<AiToolCallRecord>(CreateAiToolCallRecordInput {
                        id: registration.tool_call_id,
                        run_id: run.id,
                        provider_call_key: registration.provider_call_key,
                        provider_call_id: registration.provider_call_id,
                        provider_kind: Some(registration.provider_kind),
                        provider_model: Some(registration.provider_model),
                        provider_response_id: registration.provider_response_id,
                        budget_reservation_id: Some(registration.budget_reservation_id),
                        provider_turn_index: registration.provider_turn_index,
                        tool_call_index: 0,
                        tool_id: registration.tool_id,
                        tool_fingerprint: registration.tool_fingerprint,
                        protected_arguments: Some(registration.protected_arguments),
                        argument_hash: registration.argument_hash,
                        protected_result: None,
                        payload_purged_at: None,
                        risk: "read_only".to_owned(),
                        authorization_code: Some("waiting_subscription".to_owned()),
                        authorization_policy_version: Some(
                            registration.authorization_policy_version,
                        ),
                        authorization_state_digest: Some(registration.authorization_state_digest),
                        disclosure_schema_fingerprint: Some(
                            registration.disclosure_schema_fingerprint.clone(),
                        ),
                        result_classification: None,
                        result_egress_decision_id: None,
                        result_egress_manifest_hash: None,
                        application_audit_ref: None,
                        approval_id: None,
                        idempotency_key: None,
                        correlation_id: Some(registration.correlation_id.clone()),
                        causation_id: Some(registration.causation_id),
                        delegation_reference: None,
                        lease_generation: lease.lease_generation(),
                        state: "waiting_subscription".to_owned(),
                        completed_at: None,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    tx.insert::<AiSubscriptionWaiterRecord>(
                        CreateAiSubscriptionWaiterRecordInput {
                            id: registration.waiter_id,
                            run_id: run.id,
                            session_id: session.id,
                            tool_call_id: registration.tool_call_id,
                            source_attempt_id: lease.attempt_id(),
                            source_lease_generation: lease.lease_generation(),
                            source_checkpoint_id: registration.source_checkpoint_id,
                            source_checkpoint_fingerprint: registration
                                .source_checkpoint_fingerprint,
                            parked_checkpoint_id: registration.parked_checkpoint_id,
                            parked_checkpoint_fingerprint: registration
                                .parked_checkpoint_fingerprint,
                            owner_principal_kind: registration
                                .expected_owner_principal_kind
                                .clone(),
                            owner_subject: registration.expected_owner_subject.clone(),
                            principal_reference: registration.principal_reference,
                            principal_reference_fingerprint: registration
                                .principal_reference_fingerprint,
                            scope_key: ai_scope_key(&registration.scope),
                            scope_kind: registration.scope.kind,
                            scope_id: registration.scope.id,
                            tenant_id: registration.scope.tenant_id,
                            target_id: registration.target_id,
                            source_id: registration.source_id,
                            source_registration_fingerprint: registration
                                .source_registration_fingerprint,
                            semantic_catalog_fingerprint: registration.semantic_catalog_fingerprint,
                            operation_fingerprint: registration.operation_fingerprint,
                            target_schema_fingerprint: registration.target_schema_fingerprint,
                            capability_fingerprint: registration.capability_fingerprint,
                            plan_fingerprint: registration.plan_fingerprint,
                            compiled_descriptor_fingerprint: registration
                                .compiled_descriptor_fingerprint,
                            operation_name: registration.operation_name,
                            operation_document_hash: registration.operation_document_hash,
                            result_projection_fingerprint: registration
                                .result_projection_fingerprint,
                            disclosure_schema_fingerprint: registration
                                .disclosure_schema_fingerprint,
                            variables_fingerprint: registration.variables_fingerprint,
                            condition_fingerprint: registration.condition_fingerprint,
                            waiter_fingerprint: registration.waiter_fingerprint,
                            protected_request: Some(registration.protected_request),
                            cursor_fingerprint: registration.cursor_fingerprint,
                            protected_cursor: Some(registration.protected_cursor),
                            events_examined: 0,
                            maximum_events: registration.maximum_events,
                            expires_at: registration.expires_at,
                            state: "waiting".to_owned(),
                            claim_owner: None,
                            claim_generation: 0,
                            claim_expires_at: None,
                        },
                    )
                    .await
                    .map_err(OrmPublicError::from)?;
                    tx.insert::<AiRunAttemptOutcomeRecord>(CreateAiRunAttemptOutcomeRecordInput {
                        attempt_id: lease.attempt_id(),
                        run_id: run.id,
                        lease_generation: lease.lease_generation(),
                        worker_id: lease.worker_id().to_owned(),
                        final_state: AiRunState::WaitingSubscription.as_str().to_owned(),
                        outcome_code: "subscription_wait_registered".to_owned(),
                        provider_response_id: checkpoint.provider_response_id,
                        finished_at: now.unix_timestamp(),
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    tx.insert::<AiAuditEventRecord>(CreateAiAuditEventRecordInput {
                        actor_principal_kind: registration.expected_owner_principal_kind,
                        actor_subject: registration.expected_owner_subject,
                        action: "ai.subscription_wait.register".to_owned(),
                        resource_kind: "ai_subscription_waiter".to_owned(),
                        resource_reference: registration.waiter_id.to_string(),
                        outcome: "waiting".to_owned(),
                        reason_code: "replay_then_live_registered".to_owned(),
                        correlation_id: registration.correlation_id,
                        causation_id: Some(registration.tool_call_id.to_string()),
                        policy_version: None,
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    Ok(())
                })
            })
            .await
            .map_err(map_transaction)
    }
}

struct PreparedWaitRegistration {
    waiter_id: Uuid,
    tool_call_id: Uuid,
    source_id: String,
    source_registration_fingerprint: String,
    principal_reference: serde_json::Value,
    principal_reference_fingerprint: String,
    source_checkpoint_id: Uuid,
    source_checkpoint_fingerprint: String,
    parked_checkpoint_id: Uuid,
    parked_checkpoint_fingerprint: String,
    protected_parked_checkpoint: serde_json::Value,
    scope: AiScope,
    target_id: String,
    semantic_catalog_fingerprint: String,
    operation_fingerprint: String,
    target_schema_fingerprint: String,
    capability_fingerprint: String,
    plan_fingerprint: String,
    compiled_descriptor_fingerprint: String,
    operation_name: String,
    operation_document_hash: String,
    result_projection_fingerprint: String,
    disclosure_schema_fingerprint: String,
    variables_fingerprint: String,
    condition_fingerprint: String,
    waiter_fingerprint: String,
    protected_request: serde_json::Value,
    cursor_fingerprint: String,
    protected_cursor: serde_json::Value,
    maximum_events: i64,
    expires_at: i64,
    provider_call_key: String,
    provider_call_id: String,
    provider_kind: String,
    provider_model: String,
    provider_response_id: Option<String>,
    budget_reservation_id: Uuid,
    tool_id: String,
    tool_fingerprint: String,
    protected_arguments: serde_json::Value,
    argument_hash: String,
    correlation_id: String,
    causation_id: String,
    authorization_policy_version: String,
    authorization_state_digest: String,
    provider_turn_index: i64,
    tool_step_index: i64,
    expected_owner_principal_kind: String,
    expected_owner_subject: String,
}

#[async_trait]
impl crate::AiAgentCheckpointAdopter for AiSubscriptionCheckpointAdopter {
    async fn adopt_tool_batch(
        &self,
        lease: &AiRunLease,
    ) -> Result<Option<crate::AiAdoptedReadOnlyToolBatch>, AiError> {
        let Some(checkpoint_id) = lease.latest_checkpoint_id() else {
            return Ok(None);
        };
        match self.waits.adopt_wait(lease, checkpoint_id).await? {
            Some(adopted) => Ok(Some(adopted)),
            None => self.fallback.adopt_tool_batch(lease).await,
        }
    }

    async fn consume_before_provider(
        &self,
        lease: &AiRunLease,
        checkpoint_id: Uuid,
    ) -> Result<AiRunLease, AiError> {
        let checkpoint = AiRunCheckpointRecord::find_by_id(self.waits.database(), &checkpoint_id)
            .await
            .map_err(|error| map_orm(OrmPublicError::from(error)))?
            .ok_or(AiError::NotFound)?;
        if checkpoint.checkpoint_kind == "subscription_wait_adopted" {
            self.waits.consume_wait_adoption(lease, checkpoint_id).await
        } else {
            self.fallback
                .consume_before_provider(lease, checkpoint_id)
                .await
        }
    }
}

fn enforce_size(value: &serde_json::Value, maximum: usize) -> Result<(), AiError> {
    if serde_json::to_vec(value)
        .map_err(|_| AiError::PersistenceFailed)?
        .len()
        > maximum
    {
        return Err(AiError::InvalidInput(
            "subscription wait protected state exceeds deployment limit".to_owned(),
        ));
    }
    Ok(())
}

fn validate_claimed_waiter(
    waiter: &AiSubscriptionWaiterRecord,
    claim: &AiSubscriptionWaitClaim,
    now: OffsetDateTime,
) -> Result<(), OrmPublicError> {
    if waiter.id != claim.waiter_id.0
        || waiter.run_id != claim.run_id.0
        || waiter.state != "claimed"
        || waiter.claim_owner.as_deref() != Some(claim.worker_id.as_str())
        || waiter.claim_generation != claim.claim_generation
        || waiter.claim_expires_at != Some(claim.claim_expires_at.unix_timestamp())
        || waiter
            .claim_expires_at
            .is_none_or(|expires| expires <= now.unix_timestamp())
        || waiter.row_version != claim.row_version
    {
        return Err(OrmPublicError::new(OrmErrorCode::Conflict));
    }
    Ok(())
}

fn waiter_exact_state(state: &str) -> AiSubscriptionWaiterRecordWhereInput {
    AiSubscriptionWaiterRecordWhereInput {
        state: Some(StringFilter {
            eq: Some(state.to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn tool_call_exact_state(state: &str) -> AiToolCallRecordWhereInput {
    AiToolCallRecordWhereInput {
        state: Some(StringFilter {
            eq: Some(state.to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn run_step_row_version_only() -> AiRunStepRecordWhereInput {
    AiRunStepRecordWhereInput::default()
}

fn condition_matches(
    root_field: &str,
    condition: Option<&AiGraphqlSubscriptionCondition>,
    data: &Value,
) -> Result<bool, AiError> {
    let Some(condition) = condition else {
        return Ok(true);
    };
    let root = data
        .as_object()
        .and_then(|object| object.get(root_field))
        .and_then(Value::as_object)
        .ok_or(AiError::ToolExecutionFailed)?;
    let actual = root
        .get(&condition.field)
        .ok_or(AiError::ToolExecutionFailed)?;
    let operator = serde_json::to_value(condition.operator)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or(AiError::ToolExecutionFailed)?;
    match operator.as_str() {
        "equal" => Ok(actual == &condition.value),
        "not_equal" => Ok(actual != &condition.value),
        "less_than" => ordered_json(actual, &condition.value).map(|order| order.is_lt()),
        "less_than_or_equal" => ordered_json(actual, &condition.value).map(|order| !order.is_gt()),
        "greater_than" => ordered_json(actual, &condition.value).map(|order| order.is_gt()),
        "greater_than_or_equal" => {
            ordered_json(actual, &condition.value).map(|order| !order.is_lt())
        }
        _ => Err(AiError::ToolExecutionFailed),
    }
}

fn ordered_json(left: &Value, right: &Value) -> Result<std::cmp::Ordering, AiError> {
    match (left, right) {
        (Value::String(left), Value::String(right)) => Ok(left.cmp(right)),
        (Value::Number(left), Value::Number(right)) => {
            let left = left.as_f64().ok_or(AiError::ToolExecutionFailed)?;
            let right = right.as_f64().ok_or(AiError::ToolExecutionFailed)?;
            left.partial_cmp(&right).ok_or(AiError::ToolExecutionFailed)
        }
        _ => Err(AiError::ToolExecutionFailed),
    }
}

fn valid_fingerprint(value: &str) -> bool {
    value.len() == MAXIMUM_SAFE_FINGERPRINT_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn map_source(error: crate::AiSubscriptionSourceError) -> AiError {
    match error {
        crate::AiSubscriptionSourceError::Authorization => AiError::Forbidden,
        crate::AiSubscriptionSourceError::ResetRequired => AiError::ToolExecutionFailed,
        crate::AiSubscriptionSourceError::InvalidPosition
        | crate::AiSubscriptionSourceError::InvalidEvent
        | crate::AiSubscriptionSourceError::LimitExceeded => {
            AiError::InvalidInput("subscription source returned invalid bounded state".to_owned())
        }
        crate::AiSubscriptionSourceError::Unavailable => AiError::ToolExecutionFailed,
    }
}
