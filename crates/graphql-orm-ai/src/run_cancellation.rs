//! Owner-authorized durable agent-run cancellation contracts.

use std::time::Duration as StdDuration;

use agql_auth::AuthPrincipal;
use async_graphql::{InputObject, SimpleObject};
use async_trait::async_trait;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::{AiError, AiRunId, AiSessionId};

/// Exact owner cancellation request for one session/run pair.
#[derive(Clone, Debug, InputObject)]
#[cfg_attr(graphql_case_pascal, graphql(rename_fields = "PascalCase"))]
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
#[cfg_attr(graphql_case_pascal, graphql(rename_fields = "PascalCase"))]
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
        {
            let (sender, _) = broadcast::channel(capacity);
            Ok(Self { sender })
        }
        #[cfg(not(any(feature = "sqlite", feature = "postgres", feature = "mssql")))]
        {
            Ok(Self {})
        }
    }

    pub(crate) fn notify(&self, run_id: AiRunId) {
        let _ = self.sender.send(run_id);
    }

    /// Starts listening for wake-ups.
    ///
    /// Subscribe before the durable cancellation check: a request that is
    /// recorded and announced after that check but before the wait would
    /// otherwise be missed until the wait times out.
    pub(crate) fn subscribe(&self) -> AiRunCancellationSubscription {
        AiRunCancellationSubscription {
            receiver: self.sender.subscribe(),
        }
    }
}

/// A wake-up subscription taken before the durable cancellation check.
pub(crate) struct AiRunCancellationSubscription {
    receiver: broadcast::Receiver<AiRunId>,
}

impl AiRunCancellationSubscription {
    /// Waits for a wake-up for `run_id` announced since the subscription was
    /// taken, or for `maximum_wait`.
    pub(crate) async fn wait(mut self, run_id: AiRunId, maximum_wait: StdDuration) {
        let receiver = &mut self.receiver;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A cancellation recorded after the durable check but before the wait
    /// used to be missed until the wait timed out, because the wait
    /// subscribed only once it started.
    #[tokio::test]
    async fn a_wakeup_announced_between_the_check_and_the_wait_is_delivered() {
        let hub = AiRunCancellationHub::default();
        let run_id = AiRunId(Uuid::new_v4());
        let subscription = hub.subscribe();
        // The durable check would run here and see no cancellation.
        hub.notify(AiRunId(Uuid::new_v4()));
        hub.notify(run_id);
        tokio::time::timeout(
            StdDuration::from_secs(5),
            subscription.wait(run_id, StdDuration::from_secs(600)),
        )
        .await
        .expect("the wake-up sent before the wait must end it");
    }

    #[tokio::test]
    async fn a_wait_without_a_wakeup_still_ends_at_its_deadline() {
        let hub = AiRunCancellationHub::default();
        let subscription = hub.subscribe();
        hub.notify(AiRunId(Uuid::new_v4()));
        subscription
            .wait(AiRunId(Uuid::new_v4()), StdDuration::from_millis(20))
            .await;
    }
}
