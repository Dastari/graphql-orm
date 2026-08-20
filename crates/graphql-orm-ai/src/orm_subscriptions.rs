//! ORM-backed catch-up-to-watermark durable subscriptions.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

use std::sync::Arc;
use std::time::Duration;

use agql_auth::{AuthError, CurrentPrincipalResolver};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, MissedTickBehavior};
use uuid::Uuid;

use crate::{
    AiError, AiSessionEventEnvelope, AiSessionEventStream, AiSessionId, AiSessionService,
    AiSessionStreamClose, AiSessionWakeup, AiSubscriptionService, OrmAiSessionService,
};

/// Classification of one reauthorization failure.
///
/// The default is deny: only a dependency that is explicitly unavailable is
/// worth waiting for. Every other class, including one this crate does not
/// recognize, ends the stream immediately.
fn is_transient_authorization_failure(error: &AuthError) -> bool {
    matches!(
        error,
        AuthError::AuthServiceUnavailable
            | AuthError::Store(_)
            | AuthError::AuthThrottled { .. }
            | AuthError::AuthLocked { .. }
    )
}

/// Returns a bounded per-session jittered backoff.
///
/// The jitter is derived from the session ID rather than a random source so
/// that concurrent subscribers to *different* sessions desynchronize (which is
/// the point: a single authorization restart must not produce one synchronized
/// bootstrap storm) while one session's behavior stays reproducible in tests.
fn jittered_backoff(base: Duration, attempt: u32, session_id: Uuid) -> Duration {
    let scaled = base.saturating_mul(1_u32 << attempt.min(4));
    let mut hasher = Sha256::new();
    hasher.update(b"graphql-orm-ai-reauthorization-backoff-v1");
    hasher.update(session_id.as_bytes());
    hasher.update(attempt.to_be_bytes());
    let digest = hasher.finalize();
    // Map the digest into [0.5, 1.5) of the scaled delay.
    let fraction = u32::from(digest[0]) * 2 + 256;
    scaled
        .saturating_mul(fraction)
        .checked_div(512)
        .unwrap_or(scaled)
}

/// Durable subscription service. Broadcast events are commit-only wakeup hints;
/// every client item is re-read from protected durable storage.
pub struct OrmAiSubscriptionService {
    sessions: Arc<OrmAiSessionService>,
    principal_resolver: Arc<dyn CurrentPrincipalResolver>,
    reauthorization_interval: Duration,
    reauthorization_grace: Duration,
    replay_check_interval: Duration,
    replay_page_size: i64,
}

impl OrmAiSubscriptionService {
    /// Creates a service with a 30-second reauthorization interval, a
    /// 2-minute reauthorization grace window, a 10-second durable replay
    /// fallback check, and bounded 100-event replay pages.
    pub fn new(
        sessions: Arc<OrmAiSessionService>,
        principal_resolver: Arc<dyn CurrentPrincipalResolver>,
    ) -> Self {
        Self {
            sessions,
            principal_resolver,
            reauthorization_interval: Duration::from_secs(30),
            reauthorization_grace: Duration::from_secs(120),
            replay_check_interval: Duration::from_secs(10),
            replay_page_size: 100,
        }
    }

    /// Overrides the reauthorization interval. Zero is rejected when opening
    /// a stream.
    #[must_use]
    pub fn with_reauthorization_interval(mut self, interval: Duration) -> Self {
        self.reauthorization_interval = interval;
        self
    }

    /// Overrides how long an unavailable authorization dependency may be
    /// tolerated before the stream closes.
    ///
    /// An authoritative denial is never subject to this window. Zero disables
    /// the grace period and restores fail-on-first-failure behavior.
    #[must_use]
    pub fn with_reauthorization_grace(mut self, grace: Duration) -> Self {
        self.reauthorization_grace = grace;
        self
    }

    /// Overrides the durable replay fallback interval. Zero is rejected when a
    /// stream opens.
    ///
    /// This bounded head-sequence read is the delivery path that does not
    /// depend on the process-local wakeup channel.
    #[must_use]
    pub fn with_replay_check_interval(mut self, interval: Duration) -> Self {
        self.replay_check_interval = interval;
        self
    }

    /// Overrides the durable replay page size, bounded to 1..=500 when a stream
    /// opens.
    #[must_use]
    pub fn with_replay_page_size(mut self, page_size: i64) -> Self {
        self.replay_page_size = page_size;
        self
    }
}

#[async_trait]
impl AiSubscriptionService for OrmAiSubscriptionService {
    async fn session_events(
        &self,
        principal: agql_auth::AuthPrincipal,
        session_id: AiSessionId,
        after_sequence: i64,
    ) -> Result<AiSessionEventStream, AiError> {
        if after_sequence < 0
            || self.reauthorization_interval.is_zero()
            || self.replay_check_interval.is_zero()
            || !(1..=500).contains(&self.replay_page_size)
        {
            return Err(AiError::InvalidConfiguration(
                "invalid AI subscription bounds".to_owned(),
            ));
        }
        let principal_reference = principal.reference();
        let mut wakeups = self
            .sessions
            .database()
            .ensure_event_sender::<AiSessionWakeup>()
            .subscribe();
        let sessions = self.sessions.clone();
        let resolver = self.principal_resolver.clone();
        let reauthorization_interval = self.reauthorization_interval;
        let reauthorization_grace = self.reauthorization_grace;
        let replay_check_interval = self.replay_check_interval;
        let replay_page_size = self.replay_page_size;

        Ok(Box::pin(async_stream::try_stream! {
            let mut current_principal = principal;
            let mut delivered_sequence = after_sequence;
            let mut replay_required = true;
            let mut reauthorize = tokio::time::interval_at(
                Instant::now() + reauthorization_interval,
                reauthorization_interval,
            );
            reauthorize.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut replay_check = tokio::time::interval_at(
                Instant::now() + replay_check_interval,
                replay_check_interval,
            );
            replay_check.set_missed_tick_behavior(MissedTickBehavior::Skip);
            // Grace state for an unavailable authorization dependency.
            let mut grace_deadline: Option<Instant> = None;
            let mut grace_attempt: u32 = 0;

            loop {
                if replay_required {
                    let mut page = sessions
                        .session_event_page(
                            &current_principal,
                            session_id,
                            delivered_sequence,
                            replay_page_size,
                        )
                        .await?;
                    let target_watermark = page.watermark;
                    if target_watermark < delivered_sequence || page.reset_required {
                        yield AiSessionEventEnvelope::ended(
                            AiSessionStreamClose::ResetRequired,
                            target_watermark,
                        );
                        return;
                    }

                    loop {
                        let mut crossed_watermark = false;
                        for event in page.events {
                            if event.sequence > target_watermark {
                                crossed_watermark = true;
                                break;
                            }
                            if event.sequence <= delivered_sequence {
                                continue;
                            }
                            delivered_sequence = event.sequence;
                            yield AiSessionEventEnvelope::delivered(event, target_watermark);
                        }
                        if delivered_sequence >= target_watermark || crossed_watermark {
                            break;
                        }
                        if !page.has_more {
                            yield AiSessionEventEnvelope::ended(
                                AiSessionStreamClose::ResetRequired,
                                target_watermark,
                            );
                            return;
                        }
                        page = sessions
                            .session_event_page(
                                &current_principal,
                                session_id,
                                delivered_sequence,
                                replay_page_size,
                            )
                            .await?;
                        if page.reset_required {
                            yield AiSessionEventEnvelope::ended(
                                AiSessionStreamClose::ResetRequired,
                                target_watermark,
                            );
                            return;
                        }
                    }
                    replay_required = false;
                }

                enum Tick {
                    Reauthorize,
                    ReplayCheck,
                    Wakeup,
                    WakeupChannelClosed,
                }

                let tick = tokio::select! {
                    _ = reauthorize.tick() => Tick::Reauthorize,
                    _ = replay_check.tick() => Tick::ReplayCheck,
                    wakeup = wakeups.recv() => {
                        match wakeup {
                            Ok(wakeup)
                                if wakeup.session_id == session_id.0
                                    && wakeup.sequence > delivered_sequence =>
                            {
                                replay_required = true;
                                Tick::Wakeup
                            }
                            Ok(_) => Tick::Wakeup,
                            Err(RecvError::Lagged(_)) => {
                                replay_required = true;
                                Tick::Wakeup
                            }
                            Err(RecvError::Closed) => Tick::WakeupChannelClosed,
                        }
                    }
                };

                match tick {
                    Tick::Wakeup => {}
                    Tick::WakeupChannelClosed => {
                        // Durable history is intact; only the process-local
                        // hint path is gone. Tell the client so it can
                        // resubscribe instead of reading silence.
                        yield AiSessionEventEnvelope::ended(
                            AiSessionStreamClose::WakeupChannelClosed,
                            delivered_sequence,
                        );
                        return;
                    }
                    Tick::ReplayCheck => {
                        // Fallback delivery path: a missed or dropped in-process
                        // wakeup is otherwise unrecoverable. One bounded
                        // authorized head read decides whether to replay.
                        match sessions.session_stream_head(&current_principal, session_id).await? {
                            Some(head) if head > delivered_sequence => replay_required = true,
                            Some(_) => {}
                            None => {
                                yield AiSessionEventEnvelope::ended(
                                    AiSessionStreamClose::AuthorizationRevoked,
                                    delivered_sequence,
                                );
                                Err(AiError::Forbidden)?;
                            }
                        }
                    }
                    Tick::Reauthorize => {
                        match resolver.resolve(&principal_reference).await {
                            Ok(resolved) => {
                                grace_deadline = None;
                                grace_attempt = 0;
                                current_principal = resolved.into_principal();
                                if sessions
                                    .session(&current_principal, session_id)
                                    .await?
                                    .is_none()
                                {
                                    yield AiSessionEventEnvelope::ended(
                                        AiSessionStreamClose::AuthorizationRevoked,
                                        delivered_sequence,
                                    );
                                    Err(AiError::Forbidden)?;
                                }
                            }
                            Err(error) if is_transient_authorization_failure(&error) => {
                                // A brief authorization restart must not drop
                                // every open stream at once. Keep the existing
                                // principal only until the bounded grace window
                                // expires, and retry on a per-session jittered
                                // schedule so recovery is not synchronized.
                                let now = Instant::now();
                                let deadline = *grace_deadline
                                    .get_or_insert_with(|| now + reauthorization_grace);
                                if reauthorization_grace.is_zero() || now >= deadline {
                                    yield AiSessionEventEnvelope::ended(
                                        AiSessionStreamClose::ReauthorizationUnavailable,
                                        delivered_sequence,
                                    );
                                    Err(AiError::ReauthorizationFailed)?;
                                }
                                let backoff = jittered_backoff(
                                    replay_check_interval,
                                    grace_attempt,
                                    session_id.0,
                                );
                                grace_attempt = grace_attempt.saturating_add(1);
                                let next = (now + backoff).min(deadline);
                                reauthorize = tokio::time::interval_at(
                                    next,
                                    reauthorization_interval,
                                );
                                reauthorize
                                    .set_missed_tick_behavior(MissedTickBehavior::Skip);
                            }
                            Err(_) => {
                                // Authoritative denial, or a class this crate
                                // does not recognize. Deny fast.
                                yield AiSessionEventEnvelope::ended(
                                    AiSessionStreamClose::AuthorizationRevoked,
                                    delivered_sequence,
                                );
                                Err(AiError::ReauthorizationFailed)?;
                            }
                        }
                    }
                }
            }
        }))
    }
}
