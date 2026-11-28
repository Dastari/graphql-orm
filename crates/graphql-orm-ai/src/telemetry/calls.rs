use std::time::Duration;

use super::{
    AiProviderCallTelemetry, AiProviderTelemetryPhase, AiTelemetryOperationId, AiTelemetryOutcome,
    AiToolCallTelemetry, AiToolTelemetryPhase,
};
use crate::{AiToolOperationDomain, AiToolOperationKind, ProviderKind, ToolMaturity};

impl AiProviderCallTelemetry {
    /// Creates a content-free provider-call start observation.
    pub fn started(operation_id: AiTelemetryOperationId, provider_kind: ProviderKind) -> Self {
        Self {
            operation_id,
            provider_kind,
            phase: AiProviderTelemetryPhase::Started,
            outcome: None,
            duration: None,
            input_tokens: 0,
            output_tokens: 0,
            cached_input_tokens: 0,
        }
    }

    /// Creates a content-free provider-call completion observation.
    ///
    /// Token counts must be zero unless the call succeeded. Cached input must
    /// be a subset of total input. Model name is intentionally excluded; a
    /// deployment that exports it must review its own bounded model registry.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AiError::InvalidInput`] for inconsistent token counts.
    pub fn finished(
        operation_id: AiTelemetryOperationId,
        provider_kind: ProviderKind,
        outcome: AiTelemetryOutcome,
        duration: Duration,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
    ) -> Result<Self, crate::AiError> {
        if cached_input_tokens > input_tokens
            || (outcome != AiTelemetryOutcome::Succeeded
                && (input_tokens != 0 || output_tokens != 0 || cached_input_tokens != 0))
        {
            return Err(crate::AiError::InvalidInput(
                "invalid provider telemetry counts".to_owned(),
            ));
        }
        Ok(Self {
            operation_id,
            provider_kind,
            phase: AiProviderTelemetryPhase::Finished,
            outcome: Some(outcome),
            duration: Some(duration),
            input_tokens,
            output_tokens,
            cached_input_tokens,
        })
    }

    /// Telemetry-only operation correlation.
    pub const fn operation_id(&self) -> AiTelemetryOperationId {
        self.operation_id
    }

    /// Provider family without endpoint, profile, credential, or model identity.
    pub fn provider_kind(&self) -> &ProviderKind {
        &self.provider_kind
    }

    /// Observation phase.
    pub const fn phase(&self) -> AiProviderTelemetryPhase {
        self.phase
    }

    /// Completion outcome, absent for a start observation.
    pub const fn outcome(&self) -> Option<AiTelemetryOutcome> {
        self.outcome
    }

    /// Completed operation duration, absent for a start observation.
    pub const fn duration(&self) -> Option<Duration> {
        self.duration
    }

    /// Authoritative total input tokens, present only after success.
    pub const fn input_tokens(&self) -> u64 {
        self.input_tokens
    }

    /// Authoritative output tokens, present only after success.
    pub const fn output_tokens(&self) -> u64 {
        self.output_tokens
    }

    /// Authoritative cached input-token subset, present only after success.
    pub const fn cached_input_tokens(&self) -> u64 {
        self.cached_input_tokens
    }

    /// OpenTelemetry GenAI operation value for the current inference contract.
    pub const fn otel_operation_name(&self) -> &'static str {
        "chat"
    }

    /// Standard OpenTelemetry provider value when this crate knows one.
    ///
    /// Profiled compatible endpoints and local harnesses deliberately return
    /// `None`; the host may map a reviewed immutable registration without
    /// exposing an endpoint or arbitrary profile name.
    pub const fn otel_provider_name(&self) -> Option<&'static str> {
        match self.provider_kind {
            ProviderKind::OpenAi => Some("openai"),
            ProviderKind::Anthropic => Some("anthropic"),
            ProviderKind::Xai => Some("x_ai"),
            ProviderKind::Ollama
            | ProviderKind::OpenAiCompatible
            | ProviderKind::LocalHarness
            | ProviderKind::ChatCompletions => None,
        }
    }
}

impl AiToolCallTelemetry {
    /// Creates a tool-call start observation without tool identity or content.
    pub const fn started(
        operation_id: AiTelemetryOperationId,
        operation_kind: AiToolOperationKind,
        operation_domain: AiToolOperationDomain,
        maturity: ToolMaturity,
    ) -> Self {
        Self {
            operation_id,
            operation_kind,
            operation_domain,
            maturity,
            phase: AiToolTelemetryPhase::Started,
            outcome: None,
            duration: None,
        }
    }

    /// Creates a tool-call completion observation without arguments or result.
    pub const fn finished(
        operation_id: AiTelemetryOperationId,
        operation_kind: AiToolOperationKind,
        operation_domain: AiToolOperationDomain,
        maturity: ToolMaturity,
        outcome: AiTelemetryOutcome,
        duration: Duration,
    ) -> Self {
        Self {
            operation_id,
            operation_kind,
            operation_domain,
            maturity,
            phase: AiToolTelemetryPhase::Finished,
            outcome: Some(outcome),
            duration: Some(duration),
        }
    }

    /// Telemetry-only operation correlation.
    pub const fn operation_id(self) -> AiTelemetryOperationId {
        self.operation_id
    }

    /// Registered GraphQL/internal operation kind.
    pub const fn operation_kind(self) -> AiToolOperationKind {
        self.operation_kind
    }

    /// Registered ownership domain.
    pub const fn operation_domain(self) -> AiToolOperationDomain {
        self.operation_domain
    }

    /// Registered deployment maturity.
    pub const fn maturity(self) -> ToolMaturity {
        self.maturity
    }

    /// Observation phase.
    pub const fn phase(self) -> AiToolTelemetryPhase {
        self.phase
    }

    /// Completion outcome, absent for a start observation.
    pub const fn outcome(self) -> Option<AiTelemetryOutcome> {
        self.outcome
    }

    /// Completed operation duration, absent for a start observation.
    pub const fn duration(self) -> Option<Duration> {
        self.duration
    }
}
