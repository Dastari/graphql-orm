//! Provider-neutral durable session-title work contracts.

use agql_auth::PrincipalReference;
use async_trait::async_trait;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{AiError, AiSessionId, AiSessionView};

/// Exact fenced lease for one durable first-message title job.
///
/// Fields are private so a host cannot manufacture worker authority. Every
/// operation re-reads and validates the durable work row, generation, owner,
/// expiry, and row version. The claim contains no message or generated title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiSessionTitleWorkClaim {
    pub(crate) work_id: Uuid,
    pub(crate) session_id: AiSessionId,
    pub(crate) input_message_id: Uuid,
    pub(crate) principal_reference: PrincipalReference,
    pub(crate) worker_id: String,
    pub(crate) lease_generation: i64,
    pub(crate) lease_expires_at: OffsetDateTime,
    pub(crate) retry_count: u32,
    pub(crate) row_version: i64,
    pub(crate) expected_title_revision: i64,
}

impl AiSessionTitleWorkClaim {
    /// Durable work identity.
    pub const fn work_id(&self) -> Uuid {
        self.work_id
    }

    /// Exact session identity.
    pub const fn session_id(&self) -> AiSessionId {
        self.session_id
    }

    /// First user message identity. This ID is not content authority.
    pub const fn input_message_id(&self) -> Uuid {
        self.input_message_id
    }

    /// Safe principal reference requiring fresh rehydration before disclosure
    /// and commit.
    pub fn principal_reference(&self) -> &PrincipalReference {
        &self.principal_reference
    }

    /// Current lease owner.
    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }

    /// Monotonic fencing generation.
    pub const fn lease_generation(&self) -> i64 {
        self.lease_generation
    }

    /// Exact lease expiry.
    pub const fn lease_expires_at(&self) -> OffsetDateTime {
        self.lease_expires_at
    }

    /// Number of previously scheduled retries.
    pub const fn retry_count(&self) -> u32 {
        self.retry_count
    }

    /// Initial/default title revision eligible for automatic replacement.
    pub const fn expected_title_revision(&self) -> i64 {
        self.expected_title_revision
    }
}

/// Authorized first-message input for a host-owned title generator.
///
/// The value deliberately has no content-revealing `Debug`, serialization, or
/// GraphQL implementation. It grants no provider, application-tool, URL,
/// file, shell, screenshot, remote-control, or arbitrary-GraphQL authority.
pub struct AiSessionTitleWorkInput {
    session_id: AiSessionId,
    text: String,
}

impl AiSessionTitleWorkInput {
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn new(session_id: AiSessionId, text: String) -> Self {
        Self { session_id, text }
    }

    /// Exact session receiving the eventual conditional title.
    pub const fn session_id(&self) -> AiSessionId {
        self.session_id
    }

    /// Current-owner-authorized first user message.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Consumes the protected input into its bounded text.
    pub fn into_text(self) -> String {
        self.text
    }
}

impl std::fmt::Debug for AiSessionTitleWorkInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiSessionTitleWorkInput")
            .field("session_id", &self.session_id)
            .field("text", &"[REDACTED]")
            .field("text_bytes", &self.text.len())
            .finish()
    }
}

/// Result of a fenced automatic-title completion.
#[derive(Clone, Debug)]
pub enum AiSessionTitleCommitOutcome {
    /// The generated title was atomically committed and durably announced.
    Applied(AiSessionView),
    /// A manual rename, custom initial title, or deletion state won the race.
    Superseded,
}

/// Provider-neutral durable title-work lifecycle.
///
/// Implementations own scheduling and fenced persistence only. The host owns
/// the model/provider profile and must call it without tools or external
/// capabilities.
#[async_trait]
pub trait AiSessionTitleWorkService: Send + Sync {
    /// Claims the oldest bounded eligible item.
    async fn claim_next(&self, worker_id: &str)
    -> Result<Option<AiSessionTitleWorkClaim>, AiError>;

    /// Rehydrates and reauthorizes the exact current owner before opening the
    /// bounded first user message.
    async fn open_first_message(
        &self,
        claim: &AiSessionTitleWorkClaim,
    ) -> Result<AiSessionTitleWorkInput, AiError>;

    /// Renews a current unexpired lease and rotates its row-version fence.
    async fn heartbeat(
        &self,
        claim: &AiSessionTitleWorkClaim,
    ) -> Result<AiSessionTitleWorkClaim, AiError>;

    /// Conditionally commits a generated title while the initial/default
    /// revision remains current.
    async fn complete(
        &self,
        claim: &AiSessionTitleWorkClaim,
        title: String,
    ) -> Result<AiSessionTitleCommitOutcome, AiError>;

    /// Relinquishes the lease and schedules a bounded retry.
    async fn schedule_retry(
        &self,
        claim: &AiSessionTitleWorkClaim,
        delay: Duration,
        error_code: String,
    ) -> Result<(), AiError>;

    /// Records a redacted terminal failure without persisting provider output.
    async fn fail(
        &self,
        claim: &AiSessionTitleWorkClaim,
        error_code: String,
    ) -> Result<(), AiError>;
}
