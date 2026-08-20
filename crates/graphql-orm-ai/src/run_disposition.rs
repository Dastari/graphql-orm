//! Owner-authorized disposition of a failed or recovery-required run.
//!
//! A terminal run never resumes. Its durable state, immutable attempt
//! outcomes, and session/inbox events are permanent. This module adds the two
//! things an owner may still do about a failure:
//!
//! - **Retry** authors a *new* run over the same already-persisted user
//!   message, under current policy, and only when the server can prove
//!   re-execution is safe.
//! - **Acknowledge** durably dismisses the failure so a client can stop
//!   surfacing it, without removing any audit history.
//!
//! Neither operation mutates the source run, deletes a row, or grants
//! provider, application-tool, approval, or run-state authority.

use agql_auth::AuthPrincipal;
use async_graphql::{Enum, InputObject, SimpleObject};
use async_trait::async_trait;
use uuid::Uuid;

use crate::{AiError, AiRunRetryAdmission};

/// Closed owner-authored disposition of one failed run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Enum)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_items = "PascalCase"))]
pub enum AiRunDisposition {
    /// A new run was authored over the same durable user message.
    Retried,
    /// The failure was dismissed without authoring a new run.
    Acknowledged,
}

impl AiRunDisposition {
    /// Stable durable storage value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retried => "retried",
            Self::Acknowledged => "acknowledged",
        }
    }

    /// Parses one stable durable storage value.
    pub const fn from_persisted(value: &str) -> Option<Self> {
        match value.as_bytes() {
            b"retried" => Some(Self::Retried),
            b"acknowledged" => Some(Self::Acknowledged),
            _ => None,
        }
    }
}

/// Exact owner request to author a new run for a failed run's user message.
#[derive(Clone, Debug, InputObject)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct RetryAiRunInput {
    /// Owning session.
    pub session_id: Uuid,
    /// Failed or recovery-required run to supersede.
    pub run_id: Uuid,
    /// Client-generated idempotency key.
    pub client_request_id: Uuid,
}

/// Exact owner request to dismiss a failed run without retrying it.
#[derive(Clone, Debug, InputObject)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct AcknowledgeAiRunFailureInput {
    /// Owning session.
    pub session_id: Uuid,
    /// Failed or recovery-required run to dismiss.
    pub run_id: Uuid,
    /// Client-generated idempotency key.
    pub client_request_id: Uuid,
}

/// Authoritative result of an accepted disposition request.
#[derive(Clone, Debug, SimpleObject)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct AiRunDispositionView {
    /// Owning session.
    pub session_id: Uuid,
    /// Disposed source run.
    pub run_id: Uuid,
    /// Idempotency key that won the disposition fence.
    pub client_request_id: Uuid,
    /// Closed disposition that was recorded.
    pub disposition: AiRunDisposition,
    /// Newly authored run, present only for a retry.
    pub retry_run_id: Option<Uuid>,
    /// Durable user message the source run consumed.
    pub input_message_id: Uuid,
    /// Server timestamp at which the disposition won.
    pub decided_at: i64,
}

/// Why a retry request was refused, without disclosing provider detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Enum)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_items = "PascalCase"))]
pub enum AiRunRetryRefusal {
    /// The source run is not in a terminal failed state.
    NotFailed,
    /// Re-execution could not be proven safe.
    Uncertain,
    /// The user message already has a durable assistant answer.
    AlreadyAnswered,
    /// A different disposition already won for this run.
    AlreadyDisposed,
}

impl AiRunRetryRefusal {
    /// Stable public value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFailed => "not_failed",
            Self::Uncertain => "uncertain",
            Self::AlreadyAnswered => "already_answered",
            Self::AlreadyDisposed => "already_disposed",
        }
    }

    /// Maps a refusing admission to its public reason.
    pub const fn from_admission(admission: AiRunRetryAdmission) -> Option<Self> {
        match admission {
            AiRunRetryAdmission::Allowed => None,
            AiRunRetryAdmission::RefusedUncertain => Some(Self::Uncertain),
            AiRunRetryAdmission::RefusedAlreadyAnswered => Some(Self::AlreadyAnswered),
        }
    }
}

/// Current-owner failure-disposition boundary used by the GraphQL mutations.
#[async_trait]
pub trait AiRunDispositionService: Send + Sync {
    /// Authors a new run for the same durable user message as one failed run.
    ///
    /// Implementations must rehydrate current authority, apply session/scope
    /// access, and re-decide retry admission from committed rows inside the
    /// same transaction that records the disposition. Replaying the same
    /// `client_request_id` returns the original result. The new run carries a
    /// fresh principal reference so it executes under current policy; it never
    /// inherits the source run's lease, attempt, checkpoint, approval, or
    /// provider session.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::Forbidden`] for an unauthorized principal,
    /// [`AiError::NotFound`] for an invisible session or run, and
    /// [`AiError::Conflict`] when retry is refused or another disposition
    /// already won.
    async fn retry_run(
        &self,
        principal: &AuthPrincipal,
        input: RetryAiRunInput,
    ) -> Result<AiRunDispositionView, AiError>;

    /// Durably dismisses one failed run's failure.
    ///
    /// Acknowledgement is always available for a terminal failed or
    /// recovery-required run, including one whose retry is refused: dismissing
    /// a failure asserts nothing about whether re-execution would be safe. It
    /// removes no row and no event.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::Forbidden`] for an unauthorized principal,
    /// [`AiError::NotFound`] for an invisible session or run, and
    /// [`AiError::Conflict`] when the run is not terminally failed or another
    /// disposition already won.
    async fn acknowledge_run_failure(
        &self,
        principal: &AuthPrincipal,
        input: AcknowledgeAiRunFailureInput,
    ) -> Result<AiRunDispositionView, AiError>;
}
