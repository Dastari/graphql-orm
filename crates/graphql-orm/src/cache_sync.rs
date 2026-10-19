//! Durable cache refresh coordination over service-owned ORM repositories.
//!
//! Collection happens outside transactions. A durable claim fences overlapping
//! workers; promotion writes the projection and status in one state-machine
//! transaction. Hosts supply partition mapping, current authority, scheduling,
//! provider validation and generated status-entity subscriptions. This module
//! never contacts a provider or grants access to data.

use crate::db::Database;
use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
use crate::graphql::orm::{
    DbAuthContext, MutationContext, TransactionBackend, TransactionError, TransactionMode,
    WriteBackend,
};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

fn invalid() -> OrmPublicError {
    OrmPublicError::with_message(
        OrmErrorCode::InvalidInput,
        "Invalid cache refresh policy or state",
    )
}
fn conflict() -> OrmPublicError {
    OrmPublicError::with_message(
        OrmErrorCode::Conflict,
        "Cache refresh claim is no longer current",
    )
}

/// Complete host-defined dataset/security/source identity, never a display name.
///
/// The partition is opaque to the ORM. Hosts must encode all tenant, account,
/// installation and other isolation keys without ambiguous concatenation.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct CachePartitionKey {
    dataset: String,
    partition: String,
}
impl CachePartitionKey {
    /// Create a bounded nonempty dataset and opaque partition identity.
    ///
    /// # Errors
    /// Returns invalid input for empty or over-1024-byte identities.
    pub fn new(
        dataset: impl Into<String>,
        partition: impl Into<String>,
    ) -> Result<Self, OrmPublicError> {
        let key = Self {
            dataset: dataset.into(),
            partition: partition.into(),
        };
        if [&key.dataset, &key.partition]
            .iter()
            .any(|s| s.is_empty() || s.len() > 1024)
        {
            return Err(invalid());
        }
        Ok(key)
    }
    /// Stable dataset identity.
    pub fn dataset(&self) -> &str {
        &self.dataset
    }
    /// Complete opaque isolation identity. Do not expose it without authorization.
    pub fn partition(&self) -> &str {
        &self.partition
    }
}

/// Per-dataset timing policy, in seconds using a host-supplied UTC clock.
///
/// Trigger selection, retention and global worker concurrency remain host policy.
#[derive(Clone, Copy, Debug)]
pub struct RefreshPolicy {
    /// Successful collection remains fresh for this duration.
    pub fresh_for: i64,
    /// Minimum delay between admitted attempts, including forced refreshes.
    pub cooldown: i64,
    /// Expired workers cannot promote, even if no replacement has started.
    pub lease_for: i64,
    /// Initial failed-attempt backoff; doubles up to `max_backoff`.
    pub retry_backoff: i64,
    /// Maximum failed-attempt backoff.
    pub max_backoff: i64,
}
impl RefreshPolicy {
    /// Check positive durations and bounded arithmetic.
    ///
    /// # Errors
    /// Returns invalid input for invalid durations or a clock outside supported range.
    pub fn validate(&self, now: i64) -> Result<(), OrmPublicError> {
        if now < 0
            || self.fresh_for <= 0
            || self.cooldown <= 0
            || self.lease_for <= 0
            || self.retry_backoff <= 0
            || self.max_backoff < self.retry_backoff
            || [
                self.fresh_for,
                self.cooldown,
                self.lease_for,
                self.max_backoff,
            ]
            .iter()
            .any(|d| now.checked_add(*d).is_none())
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Sanitized failure vocabulary; raw provider responses never enter cache status.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RefreshFailure {
    /// Provider could not be reached within the admitted request contract.
    Unavailable,
    /// Provider throttled this partition.
    Throttled,
    /// Configuration or current authority is insufficient.
    Prerequisite,
    /// The complete candidate could not be validated.
    InvalidCandidate,
    /// Collection exceeded an admitted resource budget.
    ResourceLimit,
}

#[derive(Clone, Serialize, Deserialize)]
struct ActiveClaim {
    token: Uuid,
    base_generation: i64,
    expires_at: i64,
}

impl std::fmt::Debug for ActiveClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ActiveClaim([redacted])")
    }
}

/// Durable metadata persisted beside a service-owned cache projection.
///
/// Persist this value through `CacheSyncStore` (for example as private JSON in a
/// generated status entity). Public GraphQL fields should expose authorized
/// getters, never the private claim token. Initial generation zero means empty.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CacheState {
    generation: i64,
    last_attempt: Option<i64>,
    last_success: Option<i64>,
    source_observed_at: Option<i64>,
    fresh_until: Option<i64>,
    next_attempt: i64,
    consecutive_failures: u32,
    failure: Option<RefreshFailure>,
    rows: i64,
    active: Option<ActiveClaim>,
}
impl CacheState {
    /// Last complete committed generation; monotonically increases per partition.
    pub fn generation(&self) -> i64 {
        self.generation
    }
    /// UTC time of the last admitted attempt.
    pub fn last_attempt(&self) -> Option<i64> {
        self.last_attempt
    }
    /// UTC collection completion time, distinct from provider observation time.
    pub fn last_success(&self) -> Option<i64> {
        self.last_success
    }
    /// Oldest relevant source observation time supplied by the validated candidate.
    pub fn source_observed_at(&self) -> Option<i64> {
        self.source_observed_at
    }
    /// Freshness deadline of the last complete projection.
    pub fn fresh_until(&self) -> Option<i64> {
        self.fresh_until
    }
    /// Earliest allowed next attempt (cooldown/backoff never bypassed by force).
    pub fn next_attempt(&self) -> i64 {
        self.next_attempt
    }
    /// Safe committed row count, subject to the host's disclosure policy.
    pub fn rows(&self) -> i64 {
        self.rows
    }
    /// Sanitized failure of the latest completed attempt.
    pub fn failure(&self) -> Option<RefreshFailure> {
        self.failure
    }
    /// Whether the cache has a complete generation within its freshness deadline.
    pub fn is_fresh(&self, now: i64) -> bool {
        self.generation > 0 && self.fresh_until.is_some_and(|until| now < until)
    }
    /// Whether a nonexpired durable refresh claim exists.
    pub fn refresh_in_progress(&self, now: i64) -> bool {
        self.active.as_ref().is_some_and(|a| now < a.expires_at)
    }
    fn check_claim(&self, claim: &RefreshClaim, now: i64) -> Result<(), OrmPublicError> {
        if now < self.last_attempt.unwrap_or(0)
            || !self.active.as_ref().is_some_and(|a| {
                a.token == claim.token
                    && a.base_generation == claim.base_generation
                    && a.base_generation == self.generation
                    && now < a.expires_at
            })
        {
            return Err(conflict());
        }
        Ok(())
    }
}

/// Owned refresh fence returned only after durable claim commit.
///
/// It is not user authority. Keep it private; never persist caller credentials.
#[derive(Clone)]
pub struct RefreshClaim {
    key: CachePartitionKey,
    token: Uuid,
    base_generation: i64,
    policy: RefreshPolicy,
}
impl std::fmt::Debug for RefreshClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RefreshClaim([redacted])")
    }
}

/// Outcome of a coalesced claim attempt.
#[derive(Debug)]
pub enum RefreshAdmission {
    /// A worker may collect outside the database transaction.
    Claimed(RefreshClaim),
    /// Another worker owns an unexpired claim.
    InProgress,
    /// Cooldown, retry backoff or ordinary freshness prevented a new attempt.
    NotDue,
}

/// ORM adapter for the host's durable cache status entity.
///
/// Both methods execute on the supplied state-machine transaction. `load` must
/// lock/fence the complete existing partition on backends where a serializable
/// transaction alone does not lock rows. `save` must use generated ORM writes,
/// preserving ordinary repository authorization and queued status-entity change
/// notifications. All replicas must use this same store and key mapping.
///
/// No SQL, separate pool, provider call or independently committed write belongs
/// in this adapter. The host registers/migrates its entity and enforces current
/// authority before admission and again inside protected write callbacks.
pub trait CacheSyncStore<B: WriteBackend>: Send + Sync + 'static {
    /// Load the complete partition's state, or initial empty state.
    ///
    /// # Errors
    /// Returns policy/storage errors, including conflicting partition identity.
    fn load<'a>(
        &'a self,
        tx: &'a mut MutationContext<'_, B>,
        key: &'a CachePartitionKey,
    ) -> BoxFuture<'a, Result<CacheState, OrmPublicError>>;
    /// Persist status and enqueue the generated authorized change notification.
    ///
    /// # Errors
    /// Returns policy/storage errors; the containing transaction then rolls back.
    fn save<'a>(
        &'a self,
        tx: &'a mut MutationContext<'_, B>,
        key: &'a CachePartitionKey,
        state: &'a CacheState,
    ) -> BoxFuture<'a, Result<(), OrmPublicError>>;
}

/// Generic refresh coordinator; all durable state remains service-owned.
pub struct CacheSync<B: WriteBackend, S: CacheSyncStore<B>> {
    db: Database<B>,
    store: Arc<S>,
}
impl<B: TransactionBackend, S: CacheSyncStore<B>> CacheSync<B, S> {
    /// Bind the owning database and generated-repository adapter.
    pub fn new(db: Database<B>, store: Arc<S>) -> Self {
        Self { db, store }
    }

    /// Atomically coalesce a refresh trigger and persist an expiring worker fence.
    ///
    /// `force` bypasses freshness only. Authenticate/authorize the trigger first.
    /// Missing/expired workers are reclaimable; the old token can never promote.
    ///
    /// # Errors
    /// Returns invalid policy, storage or transaction conflicts. Retry only the
    /// transaction admission, never repeat an uncertain external command.
    pub async fn claim(
        &self,
        key: CachePartitionKey,
        policy: RefreshPolicy,
        now: i64,
        force: bool,
        auth: Option<&DbAuthContext>,
    ) -> Result<RefreshAdmission, TransactionError> {
        let store = self.store.clone();
        self.db
            .transaction_with_auth(TransactionMode::StateMachine, auth, move |tx| {
                Box::pin(async move {
                    policy.validate(now)?;
                    CachePartitionKey::new(key.dataset.clone(), key.partition.clone())?;
                    let mut state = store.load(tx, &key).await?;
                    if state.refresh_in_progress(now) {
                        return Ok(RefreshAdmission::InProgress);
                    }
                    if now < state.next_attempt || (!force && state.is_fresh(now)) {
                        return Ok(RefreshAdmission::NotDue);
                    }
                    let token = Uuid::new_v4();
                    let base_generation = state.generation;
                    state.active = Some(ActiveClaim {
                        token,
                        base_generation,
                        expires_at: now + policy.lease_for,
                    });
                    state.last_attempt = Some(now);
                    state.next_attempt = now + policy.cooldown;
                    store.save(tx, &key, &state).await?;
                    Ok(RefreshAdmission::Claimed(RefreshClaim {
                        key,
                        token,
                        base_generation,
                        policy,
                    }))
                })
            })
            .await
    }

    /// Atomically promote a completely validated candidate and successful status.
    ///
    /// `apply` performs only bounded generated ORM writes. Snapshot replacement or
    /// delta writes and the status save share one transaction; prior data survives
    /// callback/commit failure. Generated events emit after commit. Readers must
    /// bind pagination to the returned generation and refetch after reconnect.
    ///
    /// # Errors
    /// Rejects expired/replaced claims, invalid counts/times, generation overflow,
    /// authorization, callback or commit failures. Never announce success on error.
    #[allow(clippy::too_many_arguments)]
    pub async fn promote<T: Send + 'static, F>(
        &self,
        claim: RefreshClaim,
        now: i64,
        rows: i64,
        source_observed_at: Option<i64>,
        auth: Option<&DbAuthContext>,
        apply: F,
    ) -> Result<T, TransactionError>
    where
        F: for<'tx> FnOnce(
                &'tx mut MutationContext<'_, B>,
                i64,
            ) -> BoxFuture<'tx, Result<T, OrmPublicError>>
            + Send
            + 'static,
    {
        let store = self.store.clone();
        self.db
            .transaction_with_auth(TransactionMode::StateMachine, auth, move |tx| {
                Box::pin(async move {
                    claim.policy.validate(now)?;
                    if rows < 0 || source_observed_at.is_some_and(|t| t < 0 || t > now) {
                        return Err(invalid());
                    }
                    let mut state = store.load(tx, &claim.key).await?;
                    state.check_claim(&claim, now)?;
                    let generation = state.generation.checked_add(1).ok_or_else(invalid)?;
                    let value = apply(tx, generation).await?;
                    state.generation = generation;
                    state.rows = rows;
                    state.last_success = Some(now);
                    state.source_observed_at = source_observed_at;
                    state.fresh_until = Some(now + claim.policy.fresh_for);
                    state.active = None;
                    state.failure = None;
                    state.consecutive_failures = 0;
                    store.save(tx, &claim.key, &state).await?;
                    Ok(value)
                })
            })
            .await
    }

    /// Record a sanitized failed attempt without altering committed data/freshness.
    ///
    /// # Errors
    /// Rejects expired/replaced claims and storage/transaction errors.
    pub async fn fail(
        &self,
        claim: RefreshClaim,
        now: i64,
        failure: RefreshFailure,
        auth: Option<&DbAuthContext>,
    ) -> Result<(), TransactionError> {
        let store = self.store.clone();
        self.db
            .transaction_with_auth(TransactionMode::StateMachine, auth, move |tx| {
                Box::pin(async move {
                    claim.policy.validate(now)?;
                    let mut state = store.load(tx, &claim.key).await?;
                    state.check_claim(&claim, now)?;
                    state.active = None;
                    state.failure = Some(failure);
                    state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                    let backoff = claim
                        .policy
                        .retry_backoff
                        .saturating_mul(
                            1_i64
                                .checked_shl((state.consecutive_failures - 1).min(62))
                                .unwrap_or(i64::MAX),
                        )
                        .min(claim.policy.max_backoff);
                    state.next_attempt = state.next_attempt.max(now + backoff);
                    store.save(tx, &claim.key, &state).await
                })
            })
            .await
    }
}

/// Checked cumulative collection budget; use before accepting every provider page.
///
/// Hosts separately enforce HTTP body limits, per-request/total deadlines,
/// continuation-origin validation, retry count and global worker concurrency.
#[derive(Clone, Debug)]
pub struct RefreshBudget {
    remaining_pages: u64,
    remaining_rows: u64,
    remaining_bytes: u64,
}
impl RefreshBudget {
    /// Create explicit finite page/row/byte ceilings (zero admits no work).
    pub fn new(pages: u64, rows: u64, bytes: u64) -> Self {
        Self {
            remaining_pages: pages,
            remaining_rows: rows,
            remaining_bytes: bytes,
        }
    }
    /// Charge a page atomically; refusal does not consume partial allowance.
    ///
    /// # Errors
    /// Returns resource limit if any cumulative ceiling would be exceeded.
    pub fn charge_page(&mut self, rows: u64, bytes: u64) -> Result<(), OrmPublicError> {
        let next = self
            .remaining_pages
            .checked_sub(1)
            .zip(self.remaining_rows.checked_sub(rows))
            .zip(self.remaining_bytes.checked_sub(bytes))
            .ok_or_else(|| OrmPublicError::new(OrmErrorCode::PageLimitExceeded))?;
        self.remaining_pages = next.0.0;
        self.remaining_rows = next.0.1;
        self.remaining_bytes = next.1;
        Ok(())
    }
}
