---
title: Durable cache generations and refresh coordination
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-20
review_by: 2027-02-01
supersedes: []
---

# Durable cache generations and refresh coordination

`graphql_orm::cache_sync` coordinates provider collection over an application's
own ORM entities. It provides `CachePartitionKey`, durable `CacheState`,
`RefreshPolicy`, `RefreshClaim`, `RefreshBudget`, `CacheSyncStore` and
`CacheSync::{claim,promote,fail}`. The module is available with the selected
backend; no feature switch or companion dependency is required.

## Ownership and existing capabilities

The ORM already supplies generated entities/repositories, schema modules,
state-machine transactions, fenced leases, change journals and post-commit
entity notifications. `GraphQLOperations` plus `schema_roots!` produces an
entity's `…Changed` GraphQL subscription. Every generated release rechecks
current entity, row, selected field and source-provenance authorization.

These are not a provider scheduler or a durable entity event log. The cache
module adds reusable freshness/generation state, durable trigger coalescing,
expiring worker fences, atomic successful promotion and sanitized failure
backoff. Applications still own provider protocols, configured trigger policy,
current authority, clock, global worker concurrency, schema registration and
retention. The AI package's durable job machinery remains AI-specific.

## Model the store

Use a service-owned status entity for each complete dataset/security/source
partition. Stable owner-issued identifiers must cover all tenant, account,
installation and other isolation keys. `CachePartitionKey` holds a dataset and
an opaque partition; it neither interprets those keys nor authorizes a tenant.
Use structural encoding, not ambiguous delimiter concatenation.

Implement `CacheSyncStore<B>::load/save` through generated repositories on the
supplied `MutationContext`. Persist `CacheState` privately (for example as JSON
text) and mirror its authorized public getter values in ordinary status fields.
Never expose its private active claim token. Use `tx.find_by_id` /
`tx.update_by_id` for single keys, or generated composite repository operations
where supported. A status entity's type must not be named `CachePartition`,
because its generated `CachePartitionKey` would shadow the module's key type.
Prefer a domain-specific status entity name or an explicit import alias.

`load` must address exactly the same complete partition as `save`. A hidden row
is not a new partition: the host's trusted background repository policy must
permit its own exact status lookup, and missing/conflicting identity must fail
closed. All replicas use the same durable store. `save` uses generated writes,
which enqueue the generated status change event on this transaction. Do not
open another connection, commit independently or call the provider in the adapter.
A complete SQL-free SQLite adapter and schema fixture is in
[the integration tests](../../../crates/graphql-orm/tests/cache_sync.rs).

The coordinator always uses `TransactionMode::StateMachine`: SQLite takes its
write lock before reads; PostgreSQL uses serializable transactions. The store
must preserve the backend's required row/absence fencing and uniqueness.
Serialization/deadlock conflicts are retryable transaction outcomes, not
permission to repeat an uncertain remote command. MSSQL uses its existing
explicit writable transaction boundary; a cache must never change an externally
managed source's ownership or migration policy. Prefer a separate owned store.

## Refresh lifecycle

1. Authorize the exact trigger and configured connection. Select scheduled,
   manual, on-read-when-due and/or provider-event triggers per dataset. Reads
   return the last complete local projection immediately. They may signal a
   worker, but never become provider passthrough queries.
2. Call `claim(key, policy, now, force, auth)`. It commits an expiring durable
   token before returning `Claimed`, or returns `InProgress` / `NotDue`.
   `force` bypasses freshness only; cooldown and retry backoff still apply.
   Empty state starts at generation zero. An expired claim can be replaced
   after cooldown; the old worker's token can no longer promote or fail it.
3. Collect outside the transaction. `RefreshBudget::charge_page` bounds
   cumulative page/row/byte acceptance. Enforce body limits before buffering,
   total and request deadlines, retries, continuation-origin checks and global
   concurrency separately. Validate complete snapshots or idempotent deltas.
4. Call `promote(claim, now, rows, source_observed_at, auth, apply)`. The
   callback receives the next generation and the same state-machine transaction
   used to save successful status. Perform only bounded ORM projection writes;
   for large snapshots, validate inactive staging first and atomically switch
   the active generation pointer here. Keep stable business IDs independent of
   generation. The callback is owned/`Send + 'static` so it can carry the complete
   bounded candidate without retaining a borrowed request context.
5. Callback/storage/commit errors roll everything back and emit no success
   notification. On collection failure, `fail` records a closed sanitized error
   and capped exponential backoff, retaining successful generation, rows and
   freshness. Expired/replaced claims are rejected. A crashed worker leaves an
   expiring claim; there is no permanently retained in-memory busy flag.

Each call takes its current database auth context separately; the coordinator
does not retain a principal snapshot. Hosts rehydrate current user/workload
and source authority before admission and inside protected write callbacks.
A claim is a worker fence, never an authorization grant. The host supplies
nonnegative UTC seconds and must not move its clock backwards. Lease admission
uses the supplied promotion time; hosts enforce total collection/transaction
and current-authority deadlines through completion. No independent remote
revocation can be made atomic merely by checking status before a write.

`last_success` is collection completion. `source_observed_at` remains distinct;
a successful fetch cannot make an old source observation current. `fresh_until`
uses successful collection policy; applications may tighten freshness using the
source's own observation age and maximum-staleness contract.

## Notifications and missed-event recovery

Expose the generated status `…Changed` subscription with only safe persisted
scalars, plus the normal authorized status query. Include committed generation,
last attempt/success, source observation, freshness, refresh-in-progress,
sanitized failure and policy-approved counts. Register the ordinary current
principal, tenant, row, field, scope and assurance checks; registration and
router metadata never substitute for authorization on each release.

A successful `save` queues its generated event until the transaction commits.
Update the status row once per snapshot promotion to coalesce invalidation.
Claim/progress/failure writes may notify, but keep the prior successful
generation. The generated stream reads current authorized rows, so multiple
queued hints may coalesce to the latest visible generation; event count is not
audit evidence and clients must not interpret one event as one exact snapshot.
The database's raw event bus is server-only and cannot replace generated
subscription authorization.

Generated entity notifications are **best-effort invalidation**, with no durable
cursor or replay promise. The supported recovery protocol is:

1. Subscribe before the initial query, or repeat the status query when the
   subscription is established and reconcile its generation with initial data.
2. Refetch affected active queries on any changed generation. Bind page cursors
   to a committed generation; restart a page sequence if it changed.
3. After every reconnect, lag indication, server restart or uncertain gap,
   unconditionally refetch authorized status and projection. Do not wait for a
   future event to discover a missed generation. A bounded durable application
   event log may replace this refetch only with its own tested cursor contract.

A process-local broadcast is not durable scheduling, replay or cross-replica
notification transport. Deployments using multiple GraphQL processes must
route committed invalidations through a host-owned shared transport, or bound
client refetch intervals as an additional recovery mechanism. The durable
status remains authoritative after any lost notification.

## Verification and migration

The SQL-free integration fixture exercises post-commit generated GraphQL
delivery, rollback silence, cooldown/freshness, durable reclaim, replaced-worker
refusal, partition isolation, missed-event status recovery and revoked row
policy. Existing subscription authorization suites remain the contract for
entity/field/current-authority checks.

No existing schema is automatically migrated. Hosts add/register their own
status entity and projection using supported schema APIs, deploy the worker and
subscription together, and update client race/reconnect handling. Preserve the
last complete cache throughout failure and migration. No provider, credential,
product scope, business organisation model or scheduler is introduced upstream.
