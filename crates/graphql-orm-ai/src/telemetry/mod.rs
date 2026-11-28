//! Exporter-neutral, content-free operational telemetry contracts.
//!
//! The observations in this module deliberately contain no prompts, model
//! output, tool arguments/results, GraphQL documents, principal references,
//! provider response IDs, model/profile names, endpoint URLs, secret
//! references, durable resource IDs, or arbitrary error text. They are not an
//! audit log and do not grant authorization or prove durable state.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use crate::{AiRunState, AiToolOperationDomain, AiToolOperationKind, ProviderKind, ToolMaturity};

// Keep type definitions here to preserve their qualified names.
mod calls;
mod lifecycle;

/// Stable schema version for the typed observation vocabulary.
pub const AI_OPERATIONAL_TELEMETRY_SCHEMA_VERSION: u16 = 1;

/// Random correlation for one telemetry operation.
///
/// This ID is created solely for telemetry and is not derived from a session,
/// run, attempt, principal, tool, or provider reference. It is still
/// high-cardinality and must be used only for trace/event correlation, never as
/// a metric attribute.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AiTelemetryOperationId(Uuid);

impl AiTelemetryOperationId {
    /// Creates a fresh random telemetry-only operation ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Returns the random telemetry-only UUID for exporter correlation.
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for AiTelemetryOperationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for AiTelemetryOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AiTelemetryOperationId([REDACTED])")
    }
}

/// Content-free completion classification shared by operational observations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiTelemetryOutcome {
    /// The operation completed and its authoritative work succeeded.
    Succeeded,
    /// The operation was rejected before an external side effect could occur.
    Rejected,
    /// The operation failed with known side-effect certainty.
    Failed,
    /// An external effect may have occurred and automated replay is unsafe.
    Uncertain,
    /// Work was safely retained because a proof, policy, or dependency blocked it.
    Blocked,
}

/// Provider-call observation phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiProviderTelemetryPhase {
    /// Provider orchestration is about to begin.
    Started,
    /// Provider orchestration has finished with a classified outcome.
    Finished,
}

/// Content-free observation for one provider call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiProviderCallTelemetry {
    operation_id: AiTelemetryOperationId,
    provider_kind: ProviderKind,
    phase: AiProviderTelemetryPhase,
    outcome: Option<AiTelemetryOutcome>,
    duration: Option<Duration>,
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
}

/// Content-free durable run-state transition observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRunTransitionTelemetry {
    operation_id: AiTelemetryOperationId,
    from: AiRunState,
    to: AiRunState,
}

/// Tool-call observation phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiToolTelemetryPhase {
    /// Fresh authorization is beginning for an exact registered call.
    Started,
    /// The exact registered call has finished with a classified outcome.
    Finished,
}

/// Content-free application/internal tool-call observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiToolCallTelemetry {
    operation_id: AiTelemetryOperationId,
    operation_kind: AiToolOperationKind,
    operation_domain: AiToolOperationDomain,
    maturity: ToolMaturity,
    phase: AiToolTelemetryPhase,
    outcome: Option<AiTelemetryOutcome>,
    duration: Option<Duration>,
}

/// Content-free aggregate for one expired-run recovery pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRunRecoveryTelemetry {
    operation_id: AiTelemetryOperationId,
    duration: Duration,
    requeued: u64,
    checkpoint_requeued: u64,
    recovery_required: u64,
    failed: u64,
    completed: u64,
    quarantined: u64,
    skipped: u64,
    budget_reservations_settled: u64,
}

/// Content-free aggregate for one bounded session-retention pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRetentionTelemetry {
    operation_id: AiTelemetryOperationId,
    duration: Duration,
    sessions_scanned: u64,
    sessions_changed: u64,
    sessions_finalized: u64,
    rows_deleted: u64,
    payloads_tombstoned: u64,
    messages_scrubbed: u64,
    cleanups_requested: u64,
    blocked: u64,
    conflicted: u64,
    failed: u64,
    has_more: bool,
}

/// Content-free aggregate for one dry-run restore plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRestorePlanTelemetry {
    operation_id: AiTelemetryOperationId,
    duration: Duration,
    preserved_terminal: u64,
    requeued: u64,
    recovery_required: u64,
    approvals_to_revalidate: u64,
    consents_to_revalidate: u64,
    fatal_issues: u64,
    warning_issues: u64,
}

/// Content-free runtime start-gate evaluation after restore application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRestoreReadinessTelemetry {
    operation_id: AiTelemetryOperationId,
    executor_bound: bool,
    restore_reconciled: bool,
    fatal_issue_count: u64,
    non_fingerprint_requirements_ready: bool,
}

/// Typed content-free event accepted by an operational telemetry sink.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiOperationalTelemetryEvent {
    /// Provider call lifecycle.
    ProviderCall(AiProviderCallTelemetry),
    /// Authoritative durable run transition.
    RunTransition(AiRunTransitionTelemetry),
    /// Application/internal tool call lifecycle.
    ToolCall(AiToolCallTelemetry),
    /// Completed expired-run recovery pass.
    RunRecovery(AiRunRecoveryTelemetry),
    /// Completed bounded retention pass.
    Retention(AiRetentionTelemetry),
    /// Completed dry-run restore plan.
    RestorePlan(AiRestorePlanTelemetry),
    /// Restore/start-gate readiness evaluation.
    RestoreReadiness(AiRestoreReadinessTelemetry),
}

impl AiOperationalTelemetryEvent {
    /// Stable low-cardinality event name for exporter adapters.
    pub const fn event_name(&self) -> &'static str {
        match self {
            Self::ProviderCall(_) => "graphql_orm_ai.provider.call",
            Self::RunTransition(_) => "graphql_orm_ai.run.transition",
            Self::ToolCall(_) => "graphql_orm_ai.tool.call",
            Self::RunRecovery(_) => "graphql_orm_ai.run.recovery",
            Self::Retention(_) => "graphql_orm_ai.retention.pass",
            Self::RestorePlan(_) => "graphql_orm_ai.restore.plan",
            Self::RestoreReadiness(_) => "graphql_orm_ai.restore.readiness",
        }
    }
}

/// Deployment-owned exporter boundary for content-free operational telemetry.
///
/// Implementations should enqueue and return promptly. The method is
/// intentionally synchronous and infallible so exporter availability cannot
/// change authoritative provider, tool, recovery, retention, or restore state.
/// A deployment may drop observations under bounded backpressure. It must not
/// enrich them with prompt/output/tool content, credentials, endpoint URLs,
/// principal or durable resource IDs, or arbitrary error messages. Operation
/// IDs are trace/event correlation only and must never become metric labels.
pub trait AiOperationalTelemetrySink: Send + Sync {
    /// Records one owned observation for asynchronous export.
    fn record(&self, event: AiOperationalTelemetryEvent);
}

/// Cloneable content-free telemetry emitter.
#[derive(Clone)]
pub struct AiOperationalTelemetry {
    sink: Arc<dyn AiOperationalTelemetrySink>,
}

impl AiOperationalTelemetry {
    /// Creates an emitter for one deployment-owned sink.
    pub fn new(sink: Arc<dyn AiOperationalTelemetrySink>) -> Self {
        Self { sink }
    }

    /// Records one event without affecting authoritative runtime outcomes.
    pub fn record(&self, event: AiOperationalTelemetryEvent) {
        self.sink.record(event);
    }
}

impl fmt::Debug for AiOperationalTelemetry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AiOperationalTelemetry")
            .field("sink", &"[REDACTED]")
            .finish()
    }
}

/// Sink that intentionally discards every operational observation.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopAiOperationalTelemetrySink;

impl AiOperationalTelemetrySink for NoopAiOperationalTelemetrySink {
    fn record(&self, _event: AiOperationalTelemetryEvent) {}
}

#[cfg(test)]
#[path = "../../tests/telemetry/unit.rs"]
mod tests;
