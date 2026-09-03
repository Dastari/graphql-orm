//! Public error contract.

use async_graphql::ErrorExtensions;
use thiserror::Error;

/// Stable library error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AiError {
    /// Configuration is invalid.
    #[error("invalid AI configuration: {0}")]
    InvalidConfiguration(String),
    /// Requested item already exists.
    #[error("AI resource already exists: {0}")]
    AlreadyExists(String),
    /// Requested item was not found or is not visible.
    #[error("AI resource not found")]
    NotFound,
    /// Current state changed or an idempotency/CAS precondition failed.
    #[error("AI operation conflicts with current state")]
    Conflict,
    /// Current principal is not authorized.
    #[error("AI operation forbidden")]
    Forbidden,
    /// A configuration operation requires current host-accepted MFA.
    #[error("additional authentication is required")]
    RecentMfaRequired,
    /// External disclosure was denied.
    #[error("AI data egress denied")]
    EgressDenied,
    /// No applicable atomic budget had enough capacity for the operation.
    #[error("AI budget denied")]
    BudgetDenied,
    /// Provider execution was denied by an atomic budget before dispatch.
    ///
    /// This is an execution-boundary proof, not a generic budget error. It may
    /// be returned only when provider transport was never attempted and no
    /// budget reservation remains held. Budget limits reached during a
    /// provider/tool loop must use [`Self::BudgetDenied`] instead.
    #[error("AI budget denied")]
    PreTransportBudgetDenied,
    /// Provider execution failed before the request crossed its dispatch boundary.
    ///
    /// This is a proof-bearing execution classification, not a generic provider
    /// error. It may be returned only after the adapter reports
    /// `RejectedBeforeDispatch` and the executor durably releases the unstarted
    /// budget reservation. A failure after possible dispatch must use
    /// [`Self::ProviderFailed`] and remain recovery-required.
    #[error("AI provider operation failed before dispatch")]
    PreTransportProviderFailed,
    /// Input failed a public schema contract.
    #[error("invalid AI input: {0}")]
    InvalidInput(String),
    /// A tool result exceeded its reviewed byte or record budget.
    #[error("AI tool result exceeded its reviewed budget")]
    ResultBudgetExceeded,
    /// Authentication dependency failed closed.
    #[error("AI principal reauthorization failed")]
    ReauthorizationFailed,
    /// Host GraphQL execution failed safely.
    #[error("AI tool execution failed")]
    ToolExecutionFailed,
    /// Provider operation failed safely.
    #[error("AI provider operation failed")]
    ProviderFailed,
    /// A stateless provider turn completed and was metered, but the adapter
    /// refused one provider-native item outside the admitted model surface.
    ///
    /// This is a proof-bearing terminal classification, not a generic parser
    /// error. It may be returned only after authoritative usage was committed,
    /// no assistant answer was admitted, no application or hosted tool effect
    /// crossed the host boundary, and the adapter's deployment contract proves
    /// the refused native item was contained. Retained-session turns and
    /// incomplete streams must use [`Self::ProviderFailed`] instead.
    #[error("AI provider-native item was rejected")]
    StatelessNativeItemRejected,
    /// Runtime has not passed startup/restore readiness checks.
    #[error("AI runtime is not ready")]
    RuntimeNotReady,
    /// Durable persistence is temporarily unavailable or failed safely.
    #[error("AI persistence operation failed")]
    PersistenceFailed,
    /// A retained provider session is cleaning up or waiting to rebind.
    #[error("AI provider session is temporarily deferred")]
    ProviderSessionDeferred,
}

impl AiError {
    /// Stable public error code.
    pub const fn public_code(&self) -> &'static str {
        match self {
            Self::InvalidConfiguration(_) => "AI_INVALID_CONFIGURATION",
            Self::AlreadyExists(_) => "AI_ALREADY_EXISTS",
            Self::NotFound => "AI_NOT_FOUND",
            Self::Conflict => "AI_CONFLICT",
            Self::Forbidden => "AI_FORBIDDEN",
            Self::RecentMfaRequired => "AI_RECENT_MFA_REQUIRED",
            Self::EgressDenied => "AI_EGRESS_DENIED",
            Self::BudgetDenied => "AI_BUDGET_DENIED",
            Self::PreTransportBudgetDenied => "AI_BUDGET_DENIED",
            Self::PreTransportProviderFailed => "AI_PROVIDER_FAILED",
            Self::InvalidInput(_) => "AI_INVALID_INPUT",
            Self::ResultBudgetExceeded => "AI_RESULT_BUDGET_EXCEEDED",
            Self::ReauthorizationFailed => "AI_REAUTHORIZATION_FAILED",
            Self::ToolExecutionFailed => "AI_TOOL_EXECUTION_FAILED",
            Self::ProviderFailed => "AI_PROVIDER_FAILED",
            Self::StatelessNativeItemRejected => "AI_PROVIDER_FAILED",
            Self::RuntimeNotReady => "AI_RUNTIME_NOT_READY",
            Self::PersistenceFailed => "AI_PERSISTENCE_FAILED",
            Self::ProviderSessionDeferred => "AI_PROVIDER_SESSION_DEFERRED",
        }
    }
}

impl ErrorExtensions for AiError {
    fn extend(&self) -> async_graphql::Error {
        async_graphql::Error::new(self.to_string()).extend_with(|_, extensions| {
            extensions.set("code", self.public_code());
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stateless_native_item_rejection_keeps_the_provider_failure_public_code() {
        assert_eq!(
            AiError::StatelessNativeItemRejected.public_code(),
            "AI_PROVIDER_FAILED"
        );
    }

    #[test]
    fn pre_transport_provider_failure_keeps_the_provider_failure_public_code() {
        assert_eq!(
            AiError::PreTransportProviderFailed.public_code(),
            "AI_PROVIDER_FAILED"
        );
    }

    #[test]
    fn result_budget_exceeded_has_a_distinct_public_code() {
        assert_eq!(
            AiError::ResultBudgetExceeded.public_code(),
            "AI_RESULT_BUDGET_EXCEEDED"
        );
    }
}
