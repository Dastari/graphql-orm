//! Owner-authorized durable agent-run cancellation contracts.

#[cfg(any(feature = "sqlite", feature = "postgres"))]
use std::time::Duration as StdDuration;

use agql_auth::AuthPrincipal;
use async_graphql::{InputObject, SimpleObject};
use async_trait::async_trait;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::{AiError, AiRunId, AiSessionId};

/// Exact owner cancellation request for one session/run pair.
#[derive(Clone, Debug, InputObject)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct CancelAiRunInput {
    /// Owning session.
    pub session_id: Uuid,
    /// Active run to stop.
    pub run_id: Uuid,
    /// Client-generated idempotency key.
    pub client_request_id: Uuid,
}

/// Authoritative result of an accepted cancellation request.
#[derive(Clone, Debug, SimpleObject)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct AiRunCancellationView {
    /// Owning session.
    pub session_id: Uuid,
    /// Cancelled run.
    pub run_id: Uuid,
    /// Idempotency key that won the cancellation fence.
    pub client_request_id: Uuid,
    /// Canonical durable terminal state.
    pub state: String,
    /// Server timestamp at which cancellation won.
    pub requested_at: i64,
}

/// Current-owner cancellation boundary used by the GraphQL mutation.
#[async_trait]
pub trait AiRunCancellationService: Send + Sync {
    /// Atomically cancels the exact visible run and appends durable request and
    /// terminal events.
    ///
    /// Implementations must rehydrate current authority, apply session/scope
    /// access, fence the run, and preserve idempotency. The operation grants no
    /// provider, application-tool, or generic run-state authority.
    async fn request_cancellation(
        &self,
        principal: &AuthPrincipal,
        input: CancelAiRunInput,
    ) -> Result<AiRunCancellationView, AiError>;
}

/// Process-local wakeup acceleration for durable cancellation polling.
///
/// The database remains authoritative. Missing or lagged notifications merely
/// fall back to bounded polling, so this value carries no cancellation
/// authority and is safe to share between the request service and workers.
#[derive(Clone, Debug)]
pub struct AiRunCancellationHub {
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    sender: broadcast::Sender<AiRunId>,
}

impl AiRunCancellationHub {
    /// Creates a bounded process-local notification channel.
    pub fn new(capacity: usize) -> Result<Self, AiError> {
        if !(1..=65_536).contains(&capacity) {
            return Err(AiError::InvalidConfiguration(
                "invalid run cancellation notification capacity".to_owned(),
            ));
        }
        #[cfg(any(feature = "sqlite", feature = "postgres"))]
        {
            let (sender, _) = broadcast::channel(capacity);
            Ok(Self { sender })
        }
        #[cfg(not(any(feature = "sqlite", feature = "postgres")))]
        {
            Ok(Self {})
        }
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn notify(&self, run_id: AiRunId) {
        let _ = self.sender.send(run_id);
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) async fn wait(&self, run_id: AiRunId, maximum_wait: StdDuration) {
        let mut receiver = self.sender.subscribe();
        let wait = async {
            loop {
                match receiver.recv().await {
                    Ok(candidate) if candidate == run_id => break,
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_))
                    | Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        let _ = tokio::time::timeout(maximum_wait, wait).await;
    }
}

impl Default for AiRunCancellationHub {
    fn default() -> Self {
        Self::new(256).expect("the fixed default cancellation capacity is valid")
    }
}

/// Durable cancellation observation used by a fenced coordinator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRunCancellation {
    session_id: AiSessionId,
    run_id: AiRunId,
    client_request_id: Uuid,
    requested_at: i64,
}

impl AiRunCancellation {
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) const fn new(
        session_id: AiSessionId,
        run_id: AiRunId,
        client_request_id: Uuid,
        requested_at: i64,
    ) -> Self {
        Self {
            session_id,
            run_id,
            client_request_id,
            requested_at,
        }
    }

    /// Owning session.
    pub const fn session_id(self) -> AiSessionId {
        self.session_id
    }

    /// Cancelled run.
    pub const fn run_id(self) -> AiRunId {
        self.run_id
    }

    /// Request id that won the durable fence.
    pub const fn client_request_id(self) -> Uuid {
        self.client_request_id
    }

    /// Server cancellation timestamp.
    pub const fn requested_at(self) -> i64 {
        self.requested_at
    }
}
