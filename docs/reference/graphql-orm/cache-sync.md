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
expiring worker fences using the existing `FencedLeaseState` / `LeaseProof`,
atomic successful promotion and sanitized failure
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
Never expose its private active claim token. Disable public generated cache/status
mutations with `schema_roots! { generated_mutations: "none", ... }` (or an
explicit reviewed allowlist); background repository writes remain available.
Use `tx.find_by_id` /
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
2. Call `claim(key, policy, clock, force, auth)`. It commits an expiring durable
   token before returning `Claimed`, or returns `InProgress` / `NotDue`.
   `force` bypasses freshness only; cooldown and retry backoff still apply.
   Empty state starts at generation zero. An expired claim can be replaced
   after cooldown; the old worker's token can no longer promote or fail it.
3. Collect outside the transaction. `RefreshBudget::charge_page` bounds
   cumulative page/row/byte acceptance. Enforce body limits before buffering,
   total and request deadlines, retries, continuation-origin checks and global
   concurrency separately. Validate complete snapshots or idempotent deltas.
4. Call `promote(claim, clock, rows, source_observed_at, auth, apply)`. The
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
A claim is a worker fence, never an authorization grant. Pass a live clock function
such as `cache_sync::unix_time_seconds`, not an earlier captured timestamp.
The coordinator re-reads nonnegative UTC seconds after store reads, projection
writes and status persistence, and rolls back a claim/promotion/failure write
when its lease expires across those awaits. Backwards clock observations fail
closed. Hosts still enforce total collection/transaction and current-authority
deadlines. The final clock/authority reads cannot atomically fence time passing
or independent remote revocation during the later database commit await.

`last_success` is collection completion. `source_observed_at` remains distinct;
a successful fetch cannot make an old source observation current. `fresh_until`
uses successful collection policy; applications may tighten freshness using the
source's own observation age and maximum-staleness contract.

## Notifications and missed-event recovery

`CacheSync::changes` is the reusable cache invalidation stream for a native
async-graphql subscription root. It returns the Federation-shareable
`CacheSyncChange` object (`Generation: String`, `RefetchRequired: Boolean!`).
Generation strings are lossless opaque decimal identities. The stream:

- registers its bounded process-local receiver immediately, before any initial
  authorization await;
- sends an initial `RefetchRequired: true` signal after authorization, allowing
  the client to refetch an earlier query or perform its initial query while
  concurrent commits are already queued;
- sends one generation hint per successful promotion, queued until commit;
- sends an explicit authorized refetch signal after receiver lag; and
- reauthorizes initial, committed and gap signals and ends without metadata on
  denial/error. Reconnect starts a new receiver and initial signal.

The mandatory callback is a trusted host policy boundary: rehydrate current
principal/source authority and enforce complete partition, owner, row, field,
scope and assurance restrictions. Do not pass a cached allow decision or retain
raw bearer credentials. With `auth-agql`, prefer `changes_with_agql` with an
`AgqlOwnedOperation<CachePartitionKey>` captured from a coherent request context
and mandatory owned binding/business authorizers. It reuses the existing
post-await original-status/current-grant checks rather than fabricating context.
No registration or callback type alone grants authority. The
[SQL-free integration fixture](../../../crates/graphql-orm/tests/cache_sync.rs)
shows native GraphQL initial/committed delivery, denied signals, racing commits,
rollback silence and authorized lag recovery. Declare the host's custom root
and equivalent authorization in its operation/router metadata.

Generated status-entity `…Changed` subscriptions remain available for status
and progress. Update that row once per promotion to coalesce invalidation;
claim/failure updates retain the prior successful generation. Public status can
include generation, attempt/success, source observation, freshness, sanitized
failure and policy-approved counts. Expose `refresh_expires_at` under policy and
derive progress against the current clock so crashed workers cannot leave a
permanently true progress flag. Keep claim tokens private. Generated events read
current authorized rows, so hints may coalesce to the latest generation and are
not an audit log. The raw database event bus is server-only and cannot replace
subscription authorization.

A WebSocket `connection_ack` acknowledges the connection, **not operation
registration**. For an existing generated best-effort stream without an initial
ready/refetch signal, subscribe before the query and perform finite periodic
authorized generation reconciliation; a single refetch at connection time does
not by itself prove the initial race closed. Refetch affected active queries
when generation differs, bind page sequences to a generation, and restart them
after promotion. Refetch unconditionally on reconnect or uncertain gaps. The
new cache stream's authorized initial signal supplies the registration barrier;
its lag signal supplies explicit recovery while the connection remains open.

Neither stream promises durable cursor/replay or cross-process delivery.
Replicated deployments must supply a host-owned shared invalidation transport
or finite periodic authorized generation reconciliation. The coordinator and
its stream share one `Database` event runtime for immediate in-process delivery.
The durable status remains authoritative after any lost notification. A durable
application event log can replace refetch only with its own bounded, tested
cursor contract.

## Verification and migration

The SQL-free integration fixture exercises post-commit generated GraphQL
delivery, rollback silence, cooldown/freshness, durable reclaim, replaced-worker
refusal, partition isolation, missed-event status recovery revoked row policy, native initial/committed GraphQL signals,
operation-registration races and authorized lag recovery. Existing subscription authorization suites remain the contract for
entity/field/current-authority checks.

No existing schema is automatically migrated. Hosts add/register their own
status entity and projection using supported schema APIs, deploy the worker and
subscription together, and update client race/reconnect handling. Preserve the
last complete cache throughout failure and migration. No provider, credential,
product scope, business organisation model or scheduler is introduced upstream.
