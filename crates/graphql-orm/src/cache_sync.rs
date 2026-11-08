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
    DbAuthContext, FencedLeaseState, LeaseProof, MutationContext, TransactionBackend,
    TransactionError, TransactionMode, WriteBackend,
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

/// Current UTC seconds for refresh lease checks.
///
/// Pass this function (not its result) to coordinator operations, so deadlines
/// are re-read after repository and promotion awaits. A clock outside the
/// supported range causes policy validation to fail closed.
pub fn unix_time_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(i64::MAX)
}

fn check_deadline(now: i64, admitted: i64, expires_at: i64) -> Result<(), OrmPublicError> {
    if now < admitted || now >= expires_at {
        Err(conflict())
    } else {
        Ok(())
    }
}

fn milliseconds(seconds: i64) -> Result<i64, OrmPublicError> {
    seconds
        .checked_mul(1000)
        .filter(|value| *value >= 0)
        .ok_or_else(invalid)
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
        milliseconds(now + self.lease_for)?;
        milliseconds(self.lease_for)?;
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
#[serde(transparent)]
struct CacheLease(FencedLeaseState);
impl std::fmt::Debug for CacheLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CacheLease([redacted])")
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
    lease: Option<CacheLease>,
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
    /// Durable lease deadline, without the private worker token.
    ///
    /// Hosts may expose this under status policy so progress does not remain
    /// stuck after a worker crash; compare it with the current clock on reads.
    pub fn refresh_expires_at(&self) -> Option<i64> {
        self.lease
            .as_ref()
            .and_then(|lease| lease.0.expires_at_ms)
            .map(|ms| ms / 1000)
    }
    /// Whether a nonexpired durable refresh claim exists.
    pub fn refresh_in_progress(&self, now: i64) -> bool {
        milliseconds(now).ok().is_some_and(|now_ms| {
            self.lease
                .as_ref()
                .is_some_and(|lease| lease.0.is_active(now_ms))
        })
    }
    fn check_claim(&self, claim: &RefreshClaim, now: i64) -> Result<(), OrmPublicError> {
        if now < self.last_attempt.unwrap_or(0) || self.generation != claim.base_generation {
            return Err(conflict());
        }
        self.lease
            .as_ref()
            .ok_or_else(conflict)?
            .0
            .validate(&claim.proof, milliseconds(now)?, claim.row_version)
            .map_err(|_| conflict())
    }
    fn release_claim(&mut self, claim: &RefreshClaim, now: i64) -> Result<(), OrmPublicError> {
        self.lease
            .as_mut()
            .ok_or_else(conflict)?
            .0
            .release(&claim.proof, milliseconds(now)?, claim.row_version)
            .map_err(|_| conflict())
    }
}

/// Owned refresh fence returned only after durable claim commit.
///
/// It is not user authority. Keep it private; never persist caller credentials.
#[derive(Clone)]
pub struct RefreshClaim {
    key: CachePartitionKey,
    proof: LeaseProof,
    row_version: i64,
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

/// Authorized cache invalidation delivered by a native GraphQL subscription.
///
/// The initial/gap signal has no generation and requires an authorized refetch.
/// Committed generations use lossless decimal strings; treat them as opaque
/// identities rather than JavaScript numbers. This is not a durable event cursor.
#[derive(Clone, Debug, async_graphql::SimpleObject)]
#[graphql(rename_fields = "PascalCase", shareable)]
pub struct CacheSyncChange {
    /// Last observed committed generation, absent for initial/gap reconciliation.
    pub generation: Option<String>,
    /// Refetch even if no generation change has been observed by the client.
    pub refetch_required: bool,
}

#[derive(Clone)]
#[cfg_attr(test, derive(Debug))]
struct CommittedCacheChange {
    key: CachePartitionKey,
    generation: i64,
}
struct CacheChangeStream<A> {
    receiver: tokio::sync::broadcast::Receiver<CommittedCacheChange>,
    key: CachePartitionKey,
    authorize: A,
    initial: bool,
}

fn cache_change_stream<A, F>(
    state: CacheChangeStream<A>,
) -> impl futures::Stream<Item = async_graphql::Result<CacheSyncChange>>
where
    A: FnMut(CachePartitionKey) -> F,
    F: std::future::Future<Output = async_graphql::Result<()>>,
{
    futures::stream::unfold(state, |mut state| async move {
        let mut refetch = state.initial;
        loop {
            let mut notice = if state.initial {
                state.initial = false;
                CacheSyncChange {
                    generation: None,
                    refetch_required: true,
                }
            } else {
                match state.receiver.recv().await {
                    Ok(event) if event.key == state.key => CacheSyncChange {
                        generation: Some(event.generation.to_string()),
                        refetch_required: false,
                    },
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        refetch = true;
                        CacheSyncChange {
                            generation: None,
                            refetch_required: true,
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            };
            if (state.authorize)(state.key.clone()).await.is_ok() {
                if refetch {
                    notice.generation = None;
                    notice.refetch_required = true;
                }
                return Some((Ok(notice), state));
            }
            // Keep reconciliation pending across denied initial/gap signals.
            refetch |= notice.refetch_required;
        }
    })
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

    /// Register an authorized initial/live/gap cache invalidation stream.
    ///
    /// Registration happens immediately, before the initial signal's authority
    /// await. Query after receiving that signal (or refetch an earlier result):
    /// any concurrent committed generation is already queued. Broadcast lag
    /// produces an explicit refetch signal; reconnect creates a new initial one.
    /// The channel is process-local, not durable replay or replica transport.
    ///
    /// The mandatory host callback runs before **every** signal, including
    /// initial and gap signals, after queue waits. It must rehydrate current
    /// authority and enforce the complete partition, resource/row/field, scope,
    /// assurance and source-owner policy. It grants no provider write authority.
    /// Prefer `changes_with_agql` when using the owned Auth bridge.
    ///
    /// The same callback is also called on the Database handle's independent
    /// authority timer (30 seconds by default, clamped to 1–300 seconds, plus
    /// 0–10% jitter). It must describe current access to this entire partition,
    /// independently of notices. Use [`Self::changes_with_authority`] when
    /// event filtering and subscriber authority need separate checks.
    ///
    /// # Errors
    /// Event denials/failures silently withhold signals. Only the independent
    /// timer terminates authority: definite denial yields sanitized `FORBIDDEN`,
    /// host/infrastructure failure yields `SUBSCRIPTION_CONTINUITY_LOST`.
    pub fn changes<A, F>(
        &self,
        key: CachePartitionKey,
        authorize: A,
    ) -> impl futures::Stream<Item = async_graphql::Result<CacheSyncChange>> + Send + 'static
    where
        A: FnMut(CachePartitionKey) -> F + Send + 'static,
        F: std::future::Future<Output = async_graphql::Result<()>> + Send + 'static,
    {
        // Lock only callback construction, never an authority await. A pending
        // event check must not prevent the independent current-authority check.
        let authorize = Arc::new(std::sync::Mutex::new(authorize));
        let current = authorize.clone();
        self.changes_with_authority(
            key,
            move |key| (current.lock().unwrap())(key),
            move |key| (authorize.lock().unwrap())(key),
        )
    }

    /// Register with separate current-partition authority and event checks.
    ///
    /// `current_authority` must rehydrate subscriber authority for the complete
    /// partition without consulting notices. It runs only on an independent
    /// jittered timer configured by `Database::with_subscription_authority_recheck_interval`.
    /// `authorize_event` enforces complete partition/row/field/source policy
    /// before every initial, committed or gap signal; every failure is silent.
    /// It must also rehydrate current subscriber, scope and assurance authority;
    /// a successful timer check never grants delivery authority.
    /// A denied initial/gap signal remains pending as a refetch requirement until
    /// a later authorized signal, so no generation can bypass reconciliation.
    /// Neither notices nor event checks reset the timer. A due timer takes
    /// priority even over pending event checks. Checks may overlap; hosts must
    /// bound callback work and latency. Polling/backpressure can delay delivery.
    ///
    /// # Errors
    /// Only timer failures terminate authority. Source-free authored refusals
    /// mean definite denial (`FORBIDDEN`); typed host failures and infrastructure
    /// categories mean `SUBSCRIPTION_CONTINUITY_LOST`. Private details are removed.
    pub fn changes_with_authority<A, F, C, G>(
        &self,
        key: CachePartitionKey,
        mut current_authority: C,
        authorize_event: A,
    ) -> impl futures::Stream<Item = async_graphql::Result<CacheSyncChange>> + Send + 'static
    where
        A: FnMut(CachePartitionKey) -> F + Send + 'static,
        F: std::future::Future<Output = async_graphql::Result<()>> + Send + 'static,
        C: FnMut(CachePartitionKey) -> G + Send + 'static,
        G: std::future::Future<Output = async_graphql::Result<()>> + Send + 'static,
    {
        let current_key = key.clone();
        let state = CacheChangeStream {
            receiver: self
                .db
                .ensure_event_sender::<CommittedCacheChange>()
                .subscribe(),
            key,
            authorize: authorize_event,
            initial: true,
        };
        let stream = cache_change_stream(state);
        crate::graphql::subscriptions::watch_subscription_authority(
            stream,
            self.db.subscription_authority_recheck_interval(),
            move || current_authority(current_key.clone()),
        )
    }

    /// Register cache changes using the existing owned current-authority bridge.
    ///
    /// Capture `AgqlOwnedOperation` from a coherent real request context with
    /// mandatory host binding/business checks for the complete partition. Its
    /// `check_release` revalidates original status/current grants after policy
    /// awaits; this adapter uses it for initial, committed and gap signals.
    ///
    /// # Errors
    /// Event denials are silent; the independent timer emits a sanitized
    /// forbidden or continuity error for denied/unavailable current authority.
    #[cfg(feature = "auth-agql")]
    pub fn changes_with_agql(
        &self,
        key: CachePartitionKey,
        authority: Arc<crate::graphql::auth_agql::AgqlOwnedOperation<CachePartitionKey>>,
    ) -> impl futures::Stream<Item = async_graphql::Result<CacheSyncChange>> + Send + 'static {
        self.changes(key, move |key| {
            let authority = authority.clone();
            async move { authority.check_release(&key).await }
        })
    }

    /// Atomically coalesce a refresh trigger and persist an expiring worker fence.
    ///
    /// `force` bypasses freshness only. Authenticate/authorize the trigger first.
    /// Supply a live clock such as [`unix_time_seconds`], re-read after awaits.
    /// Missing/expired workers are reclaimable; the old token can never promote.
    ///
    /// # Errors
    /// Returns invalid policy, storage or transaction conflicts. Retry only the
    /// transaction admission, never repeat an uncertain external command.
    pub async fn claim<C: Fn() -> i64 + Send + 'static>(
        &self,
        key: CachePartitionKey,
        policy: RefreshPolicy,
        clock: C,
        force: bool,
        auth: Option<&DbAuthContext>,
    ) -> Result<RefreshAdmission, TransactionError> {
        let store = self.store.clone();
        self.db
            .transaction_with_auth(TransactionMode::StateMachine, auth, move |tx| {
                Box::pin(async move {
                    policy.validate(clock())?;
                    CachePartitionKey::new(key.dataset.clone(), key.partition.clone())?;
                    let mut state = store.load(tx, &key).await?;
                    let now = clock();
                    policy.validate(now)?;
                    if state.refresh_in_progress(now) {
                        return Ok(RefreshAdmission::InProgress);
                    }
                    if now < state.next_attempt || (!force && state.is_fresh(now)) {
                        return Ok(RefreshAdmission::NotDue);
                    }
                    if state.generation < 0 || state.generation.checked_add(1).is_none() {
                        return Err(invalid());
                    }
                    let resource_id = serde_json::to_string(&(key.dataset(), key.partition()))
                        .map_err(|_| invalid())?;
                    let lease = state.lease.get_or_insert_with(|| {
                        CacheLease(FencedLeaseState::new(resource_id.clone(), 0))
                    });
                    if lease.0.resource_id != resource_id {
                        return Err(conflict());
                    }
                    let attempt = Uuid::new_v4();
                    let proof = lease
                        .0
                        .claim(
                            attempt.to_string(),
                            attempt,
                            milliseconds(now)?,
                            milliseconds(policy.lease_for)?,
                            lease.0.row_version,
                        )
                        .map_err(|_| conflict())?;
                    let row_version = lease.0.row_version;
                    let base_generation = state.generation;
                    state.last_attempt = Some(now);
                    state.next_attempt = now + policy.cooldown;
                    store.save(tx, &key, &state).await?;
                    check_deadline(clock(), now, now + policy.lease_for)?;
                    Ok(RefreshAdmission::Claimed(RefreshClaim {
                        key,
                        proof,
                        row_version,
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
    /// The live clock is rechecked after `apply` and status persistence; a lease
    /// that expires while those futures run causes rollback, not promotion.
    ///
    /// # Errors
    /// Rejects expired/replaced claims, invalid counts/times, generation overflow,
    /// authorization, callback or commit failures. Never announce success on error.
    #[allow(clippy::too_many_arguments)]
    pub async fn promote<T: Send + 'static, F, C: Fn() -> i64 + Send + 'static>(
        &self,
        claim: RefreshClaim,
        clock: C,
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
                    claim.policy.validate(clock())?;
                    let mut state = store.load(tx, &claim.key).await?;
                    let now = clock();
                    claim.policy.validate(now)?;
                    if rows < 0 || source_observed_at.is_some_and(|t| t < 0 || t > now) {
                        return Err(invalid());
                    }
                    state.check_claim(&claim, now)?;
                    let generation = state.generation.checked_add(1).ok_or_else(invalid)?;
                    let expires_at = state.refresh_expires_at().ok_or_else(conflict)?;
                    let value = apply(tx, generation).await?;
                    let completed = clock();
                    claim.policy.validate(completed)?;
                    check_deadline(completed, now, expires_at)?;
                    state.check_claim(&claim, completed)?;
                    state.generation = generation;
                    state.rows = rows;
                    state.last_success = Some(completed);
                    state.source_observed_at = source_observed_at;
                    state.fresh_until = Some(completed + claim.policy.fresh_for);
                    state.release_claim(&claim, completed)?;
                    state.failure = None;
                    state.consecutive_failures = 0;
                    store.save(tx, &claim.key, &state).await?;
                    check_deadline(clock(), completed, expires_at)?;
                    tx.queue_event(CommittedCacheChange {
                        key: claim.key.clone(),
                        generation,
                    });
                    Ok(value)
                })
            })
            .await
    }

    /// Record a sanitized failed attempt without altering committed data/freshness.
    ///
    /// # Errors
    /// Rejects expired/replaced claims and storage/transaction errors.
    pub async fn fail<C: Fn() -> i64 + Send + 'static>(
        &self,
        claim: RefreshClaim,
        clock: C,
        failure: RefreshFailure,
        auth: Option<&DbAuthContext>,
    ) -> Result<(), TransactionError> {
        let store = self.store.clone();
        self.db
            .transaction_with_auth(TransactionMode::StateMachine, auth, move |tx| {
                Box::pin(async move {
                    claim.policy.validate(clock())?;
                    let mut state = store.load(tx, &claim.key).await?;
                    let now = clock();
                    claim.policy.validate(now)?;
                    state.check_claim(&claim, now)?;
                    let expires_at = state.refresh_expires_at().ok_or_else(conflict)?;
                    state.release_claim(&claim, now)?;
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
                    store.save(tx, &claim.key, &state).await?;
                    check_deadline(clock(), now, expires_at)
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

#[cfg(test)]
mod authority_tests {
    use super::*;
    use futures::{StreamExt, future::poll_fn};
    use std::{task::Poll, time::Duration};

    async fn pending<S: futures::Stream + Unpin>(stream: &mut S) {
        poll_fn(|cx| match stream.poll_next_unpin(cx) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("hidden notices must not emit a signal"),
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn quiet_and_hidden_notices_terminate_at_the_same_independent_deadline() {
        for infrastructure in [false, true] {
            let key = CachePartitionKey::new("dataset", "private-partition").unwrap();
            let (sender, _) = tokio::sync::broadcast::channel(4);
            let make_stream = || {
                let state = CacheChangeStream {
                    receiver: sender.subscribe(),
                    key: key.clone(),
                    initial: true,
                    authorize: |_| async { Err(async_graphql::Error::new("private-event-detail")) },
                };
                crate::graphql::subscriptions::watch_authority_with_delay(
                    cache_change_stream(state),
                    move || async move {
                        if infrastructure {
                            Err(OrmPublicError::new(OrmErrorCode::ServiceUnavailable)
                                .into_graphql_error())
                        } else {
                            Err(async_graphql::Error::new("private-authority-detail"))
                        }
                    },
                    || Duration::from_millis(1037),
                )
            };
            let mut hidden = Box::pin(make_stream());
            pending(&mut hidden).await;
            // Only hidden receives matching and mismatching notices, including lag.
            let (idle_sender, idle_receiver) = tokio::sync::broadcast::channel(4);
            // Replace quiet with a separate channel that receives no notices.
            let mut quiet = Box::pin(crate::graphql::subscriptions::watch_authority_with_delay(
                cache_change_stream(CacheChangeStream {
                    receiver: idle_receiver,
                    key: key.clone(),
                    initial: true,
                    authorize: |_| async { Err(async_graphql::Error::new("private-event-detail")) },
                }),
                move || async move {
                    Err(if infrastructure {
                        OrmPublicError::new(OrmErrorCode::ServiceUnavailable).into_graphql_error()
                    } else {
                        async_graphql::Error::new("private-authority-detail")
                    })
                },
                || Duration::from_millis(1037),
            ));
            pending(&mut quiet).await;
            for step in 0..103 {
                for generation in 0..8 {
                    sender
                        .send(CommittedCacheChange {
                            key: key.clone(),
                            generation,
                        })
                        .unwrap();
                }
                sender
                    .send(CommittedCacheChange {
                        key: CachePartitionKey::new("dataset", "other-partition").unwrap(),
                        generation: step,
                    })
                    .unwrap();
                pending(&mut hidden).await;
                pending(&mut quiet).await;
                tokio::time::advance(Duration::from_millis(10)).await;
            }
            tokio::time::advance(Duration::from_millis(6)).await;
            pending(&mut hidden).await;
            pending(&mut quiet).await;
            tokio::time::advance(Duration::from_millis(1)).await;
            let errors = [
                hidden.next().await.unwrap().unwrap_err(),
                quiet.next().await.unwrap().unwrap_err(),
            ];
            for error in errors {
                assert!(!format!("{error:?}").contains("private"));
                assert_eq!(
                    error.extensions.unwrap().get("code"),
                    Some(&async_graphql::Value::from(if infrastructure {
                        "SUBSCRIPTION_CONTINUITY_LOST"
                    } else {
                        "FORBIDDEN"
                    }))
                );
            }
            assert!(hidden.next().await.is_none());
            assert!(quiet.next().await.is_none());
            drop(idle_sender);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn denied_initial_and_gap_require_refetch_before_releasing_generations() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let key = CachePartitionKey::new("dataset", "partition").unwrap();
        let (sender, receiver) = tokio::sync::broadcast::channel(2);
        let allowed = Arc::new(AtomicBool::new(false));
        let check = allowed.clone();
        let mut stream = Box::pin(cache_change_stream(CacheChangeStream {
            receiver,
            key: key.clone(),
            initial: true,
            authorize: move |_| {
                let allowed = check.load(Ordering::SeqCst);
                async move {
                    if allowed {
                        Ok(())
                    } else {
                        Err(async_graphql::Error::new("hidden"))
                    }
                }
            },
        }));
        pending(&mut stream).await;
        allowed.store(true, Ordering::SeqCst);
        sender
            .send(CommittedCacheChange {
                key: key.clone(),
                generation: 1,
            })
            .unwrap();
        let initial = stream.next().await.unwrap().unwrap();
        assert!(initial.refetch_required);
        assert!(initial.generation.is_none());
        allowed.store(false, Ordering::SeqCst);
        for generation in 2..8 {
            sender
                .send(CommittedCacheChange {
                    key: key.clone(),
                    generation,
                })
                .unwrap();
        }
        pending(&mut stream).await;
        allowed.store(true, Ordering::SeqCst);
        sender
            .send(CommittedCacheChange { key, generation: 8 })
            .unwrap();
        let gap = stream.next().await.unwrap().unwrap();
        assert!(gap.refetch_required);
        assert!(gap.generation.is_none());
    }
}

#[cfg(test)]
mod pending_authority_test {
    use super::*;
    use futures::StreamExt;
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn pending_event_policy_cannot_block_or_trigger_authority_termination() {
        let (sender, receiver) = tokio::sync::broadcast::channel(2);
        let stream = cache_change_stream(CacheChangeStream {
            receiver,
            key: CachePartitionKey::new("dataset", "partition").unwrap(),
            initial: true,
            authorize: |_| futures::future::pending::<async_graphql::Result<()>>(),
        });
        let mut stream = Box::pin(crate::graphql::subscriptions::watch_authority_with_delay(
            stream,
            || async {
                Err(async_graphql::Error::from(std::io::Error::other(
                    "private host error",
                )))
            },
            || Duration::from_millis(1037),
        ));
        let started = tokio::time::Instant::now();
        let error = stream.next().await.unwrap().unwrap_err();
        assert_eq!(started.elapsed(), Duration::from_millis(1037));
        assert_eq!(
            error.extensions.unwrap().get("code"),
            Some(&async_graphql::Value::from("SUBSCRIPTION_CONTINUITY_LOST"))
        );
        assert!(stream.next().await.is_none());
        drop(sender);
    }
}
