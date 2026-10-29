---
title: Schema-bound transactional runtime mutations
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-29
review_by: 2027-02-01
supersedes: []
---

# Schema-bound transactional runtime mutations

ORM/macros 0.41.0 (unreleased) adds single-record runtime create, patch and
complete-key delete. Select `sqlite` or `postgres`; no dynamic GraphQL feature
is needed. This builds on [owned runtime migrations](runtime-migrations.md).
The [compiled host example](../../../crates/graphql-orm/examples/runtime_mutation_journal.rs)
executes create, CAS update and delete with atomic repository journal inserts:

```sh
CARGO_BUILD_JOBS=2 cargo run -p graphql-orm --no-default-features --features sqlite --example runtime_mutation_journal
CARGO_BUILD_JOBS=2 cargo run -p graphql-orm --no-default-features --features postgres --example runtime_mutation_journal
```

SQLite uses process-owned memory. PostgreSQL starts a labelled disposable Docker
server and verifies its identity before cleanup. Neither example consumes an
ambient database URL. The external repository consumer compiles this example
without a direct async-graphql dependency.

## Requests and execution

`ValidatedRuntimeSchema::runtime_key` validates a complete, non-null key and
orders members according to the declared primary key. Construct requests with
`runtime_create_request`, `runtime_update_request` and `runtime_delete_request`.
Requests are immutable, owned and fingerprint-bound. Keys remain immutable after
create; append-only collections reject update/delete. Duplicate, generated,
wrong-kind and stale input fields fail closed. Non-generated required create
members must be supplied unless they have an explicit default. Generated UUIDs
are minted by the ORM; other generators are unsupported in this bounded profile.

```rust,ignore
let request = schema.runtime_update_request(
    schema.runtime_key(&collection, &[(id, RuntimeValue::Integer(7))])?,
    &[(status, RuntimeValue::String("published".into()))],
    Some(expected_draft), Some(public_projection), RuntimeMutationLimits::default(),
)?;
let committed = database.transaction_with_auth(
    TransactionMode::StateMachine, db_auth.as_ref(),
    |tx| Box::pin(async move {
        let pending = tx.mutate_runtime(&environment, &request, &authority).await?;
        tx.run_runtime_before_commit(&journal_hook, &pending).await?;
        Ok(pending)
    }),
).await?;
// Only now is committed work available for a success response or notification.
```

The environment comes from A's `runtime_mutation_environment` after validation
of the composed physical target and managed ownership. It includes incoming
system/unowned dependencies and the backend identity. A host must pin this
certificate with its schema generation and fence external DDL; it is not a
lease that makes arbitrary external schema changes safe. Revalidate after drift.
By default an incoming delete CASCADE/SET NULL rejects with `unsupported_cascade`,
even when no child currently exists; a delete can opt in to
[dependent changes](#cascade-and-set-null-dependents). RESTRICT remains usable;
database constraint failures roll back normally. No migration or pool query occurs
inside the mutation engine.

SQLite acquires the write lock before callback reads; PostgreSQL uses the
existing serializable transaction and locks an existing exact-key row. Intent,
preimage, structural CAS, DML, actual-result reread/authorization and host journal
work share this transaction. PostgreSQL `DbAuthContext` remains transaction-local.
Other backends explicitly lack `RuntimeMutationBackend` support or report its
capability constant false; low-level execution rejects before statements. Adapters
must check this capability before opening a transaction. No live MSSQL support
is implied.

## Authority and private projections

Implement every `RuntimeWriteAuthority<B>` method; no default allow exists:

- `authorize_intent`: validate the pinned identity and complete host public/policy
  revision, collection action, every supplied field, CAS field and requested return
  field. Policy-only revision changes are host state, separate from ORM fingerprints.
- Return `RuntimeWriteGrant::new(policy_projection, row_predicate)`. The projection
  selects the minimum internal policy fields. The engine unions keys, changed/CAS
  fields and requested output privately. The row predicate limits preimages and
  actual create/update results. A result outside it denies and rolls back.
  `with_actor(value)` adds the acting principal that creates write into
  [actor stamps](runtime-schema-ir.md#actor-stamps) the request does not supply, and
  `with_actor_assignment()` permits the request itself to supply actor stamps on create
  and update; without it a supplied actor stamp is `field_denied` before any write.
- `authorize_preimage`: check the authoritative visible row before update/delete.
  Missing/invisible rows return `not_found`; conflicts disclose no current value
  and are distinguished only after preimage authorization.
- `authorize_result`: approve the actual staged record including server defaults
  and trigger effects, or the preimage for delete. Return
  `RuntimeReturnGrant::new` with exactly the requested projection. Write approval
  is not read approval; narrowing fields or replacing protected values with NULL
  is not supported. `returning: None` returns no internal record.

`RuntimePredicate::referenced_fields` supports independent CAS-field approval.
Predicates retain existing typed operator/filterability restrictions. SQL NULL
checks work; JSON equality is not added. Complete-key DML repeats the expected
condition and must affect exactly one row. Serializable/deadlock/busy errors are
transaction failures, distinct from deterministic `conflict`.

The example's authority deliberately approves its isolated public fixture. It is
not a production policy implementation. Host authorities and hooks must not send
external notifications or open independent pool transactions for policy decisions.

## Values and bounds

Omitted create fields use their declared defaults/generators, otherwise SQL NULL
when nullable, otherwise `missing_required`. [Automatic timestamps](runtime-schema-ir.md#automatic-timestamps)
are never input (`field_denied`, also for explicit `Null`). Every insert writes them,
and every update writes `OnCreateAndUpdate` fields, from the database clock in the same
statement. That includes the dependent `SetNull` updates below. Standalone link mutations
write no row field and leave them unchanged. [Actor stamps](runtime-schema-ir.md#actor-stamps)
are input only under a grant built with `with_actor_assignment()` (otherwise
`field_denied` after `authorize_intent`, before any write, also for explicit `Null`).
A create writes the grant's actor, or SQL NULL, into every actor stamp it does not
supply; updates never write the actor. Omitted patches preserve the stored
value. Explicit `RuntimeValue::Null` writes SQL NULL and never requests a default.
`RuntimeValue::Json(JSON null)` remains distinct. Integers, bytes, UUIDs and canonical
runtime DateTime values retain existing storage semantics; no epoch-unit conversion
or legacy DateTime reinterpretation is introduced. The engine reselects actual
stored results before authorization, including defaults and trigger effects.

`RuntimeMutationLimits` bounds input count, key arity, serialized value bytes,
record bytes and JSON depth, plus existing predicate/bind/projection limits.
Predicate bind budgets use the shared backend renderer's emitted value slots,
including duplicated SQLite suffix operands and repeated PostgreSQL references to
one native slot. Combined CAS/authority budgets reject before target reads.
Defaults: 128 inputs, 16 key members, 1 MiB per value, 4 MiB per record and JSON
depth 32. Bounds also apply to internal policy records; oversized decoding fails
and poisons the transaction. Driver row decoding still allocates returned values;
these limits are not a driver-level streaming memory cap.

## Many-to-many links

`runtime_link_request(relation, key, change, returning, max_link_keys, limits)` changes the links
of one source row through a join relation. `RuntimeLinkChange::Replace(keys)` makes the set of
*visible* links exactly `keys`. `RuntimeLinkChange::Delta { add, remove }` removes and then adds;
adding an existing link or removing an absent one changes nothing. Target keys must belong to the
target collection and be distinct across the request. At most `max_link_keys` keys may be
supplied (default `RUNTIME_DEFAULT_MAX_LINK_KEYS` = 1000).

The request is an `Update` of the source row that writes no fields, so the usual intent and
locked-preimage checks apply; a hidden source row is `not_found`. No `UPDATE` of the source
row is issued, so its `OnCreateAndUpdate` [automatic timestamps](runtime-schema-ir.md#automatic-timestamps)
do not change. `RuntimeWriteIntent::links()`
exposes the relation and change. The ORM then asks
`RuntimeWriteAuthority::link_target_visibility` for a row predicate on the target collection (`None`
means every target is visible). It locks and reads the row's links to *visible* targets only, at
most `max_link_keys` of them, else `limit_exceeded`. Links to hidden targets are never read,
counted, replaced or removed by a link mutation.

Before reading links, the ORM requires every requested target (`set`, `add` and `remove`) to be an
existing row of the target collection that satisfies the visibility predicate. It checks them in
bind-bounded batches; PostgreSQL locks them `FOR KEY SHARE` until commit, and SQLite's immediate
write transaction excludes concurrent writers. Any other target fails the mutation with `denied`
before `authorize_links`, hooks or any write. A hidden target, a missing one and a hidden one that
is already linked therefore give the same answer, and the committed effect never mentions a hidden
target. This holds without a predicate too: naming a missing row is `denied`, not
`constraint_violation`, and removing a missing row is refused rather than ignored.

The ORM computes the exact delta and calls `RuntimeWriteAuthority::authorize_links` before any
write. `RuntimeLinkCheck` carries every requested target (`requested_add` for `set`/`add`,
`requested_remove`) independent of current link state, and the effective delta (`added`,
`removed`). Authorizing on the requested targets keeps the outcome identical whether or not a
hidden link exists, so the endpoint is not an existence oracle. Both hook methods deny by
default, so hosts must opt in before links can change.

The delta is written in bind-bounded batches: one `DELETE … IN` and one multi-row `INSERT … ON
CONFLICT DO NOTHING` per batch. A pair already present, for example one added concurrently from the
inverse relation, is left unchanged and is not reported as added. On PostgreSQL a pair committed
after the transaction's snapshot makes the insert fail as a serialization failure
(`retryable_transaction`). The result check sees the unchanged source row.

Deleting a row of a collection that a link table references removes all of its links, visible or
not, atomically. The ORM locks them (at most `RuntimeMutationRequest::with_max_delete_links` per
side, default `RUNTIME_DEFAULT_MAX_DELETE_LINKS` = 10 000, separate from the link-key bound so a
heavily linked row stays deletable), presents them to `authorize_links` as `removed`, and deletes
them with one statement per side before the row. They are always described from the join
relation's declaring (source) side, even when the deleted row is the target, and a
self-relation lists each link once. Link tables of the schema are therefore not
`unsupported_cascade` dependencies. `RuntimeMutationEffect::links()` lists every pending
`RuntimeLinkEffect` (relation, added, removed) for `before_commit` hooks. A hook failure rolls
back the links with the row. A link inserted concurrently with a delete cannot slip through: the
insert waits on the deleted row's lock, then fails its foreign key once the delete commits.

## Cancellation, rollback and commit

An operation guard starts on the first poll before any await. Only complete
success disarms it. Error/drop marks the transaction rollback-only; an outstanding
or forgotten guard independently prevents commit. The outer runner and
`commit_and_emit` both enforce this. A caught denial, conflict, limit error or
inner timeout cannot commit staged runtime work, even if later operations succeed
and the callback returns `Ok`. An unpolled future has staged nothing.

Use `run_runtime_before_commit` for journal hooks. Its separate guard covers hook
writes and awaits, including cancellation after mutation success. Propagate errors
from direct repository journal writes. Calling a hook directly or catching an
unrelated static repository error is not this guarded hook contract.

Whole-runner cancellation retains transaction-drop rollback. Commit cancellation
or transport failure can leave an ambiguous outcome. Existing `TransactionError`
variants remain; `commit_outcome()` reports confirmed `RolledBack` only for
`Rejected`, conservatively `Unknown` for `Failed`/`Retryable`. Do not blindly retry
unknown outcomes. `OrmPublicError::runtime_mutation_code()` retains safe runtime
codes through callback error conversion; no caller values, SQL or preimages are
formatted by runtime request/grant/effect/error Debug implementations.

### Validating stored inputs and staging related writes

Host validation of a stored input — for example that a formatted UUID references a resource
the caller may claim — belongs in the existing callbacks, which receive the pinned
`MutationContext` of the same transaction. No new hook and no pool preflight is involved:

1. `RuntimeWriteAuthority::authorize_intent(intent, tx)` inspects `intent.fields()` and reads or
   locks host rows through typed ORM calls on `tx` (row locks on PostgreSQL; SQLite state-machine
   transactions begin `IMMEDIATE`). Returning an error rolls back the transaction.
2. `run_runtime_before_commit(hook, &effect)` runs `RuntimeMutationHook::before_commit(tx,
   pending)` in the same transaction. `RuntimeMutationEffect::staged()` is the authoritative
   post-write image (keys, written fields and the write grant projection; the preimage for
   delete) and `preimage()` the authorized update/delete preimage. Hooks use them to claim new
   values and release replaced ones with typed repository writes, even when the request returns
   nothing. `record()` remains the only approved return projection; staged images are never part
   of a GraphQL payload and their `Debug` output is redacted.
3. A hook error, a rejected typed write or cancellation poisons the transaction; the runtime
   row and every staged host write roll back together. Concurrent conflicting claims serialize
   on host locks and should also be backed by a host uniqueness constraint, so at most one can
   commit.

`RuntimeMutationEffect` is pending, not an event or commit receipt. The engine emits
no events. Static repository events/actions remain deferred until commit. Durable
journal storage and replay are host responsibilities. Static-only transactions
retain their existing limitation: catching an inner static mutation timeout and
returning `Ok` does not guarantee rollback.

Bulk/nested writes, new upsert semantics, dynamic GraphQL and durable delivery are
outside B. C/D remain separate reviewed successors. The combined candidate
contains the independent #112 string-binding fix and exercises successful
PostgreSQL prefix/suffix CAS and emitted-bind boundaries without test-side skips.
SQLite empty suffixes match every non-null string. Final combined review and
owner merge/release remain required before downstream adoption. Static APIs and
their cancellation limitation are unchanged.

## Cascade and set-null dependents

ORM/macros 0.66.0 lets a delete honour incoming `Cascade` and `SetNull` foreign
keys between runtime collections. It is opt-in per request:

```rust,ignore
let request = schema
    .runtime_delete_request(key, None, None, RuntimeMutationLimits::default())?
    .with_dependent_changes(RuntimeDependentLimits::new(256, 8)?)?;
let pending = tx.mutate_runtime(&environment, &request, &authority).await?;
for dependent in pending.dependents() {
    // dependent.action(), dependent.collection(), dependent.preimage(), dependent.cause()
}
tx.run_runtime_before_commit(&journal_hook, &pending).await?;
```

The ORM never lets the database change a row the host did not see. In the same
transaction, after the requested row's intent and preimage checks, it:

1. walks the dependency graph from the requested row in level order, locking each
   referencing row (`FOR UPDATE` on PostgreSQL; SQLite state-machine transactions
   already hold the write lock). Incoming foreign keys are visited in table and column
   order, and referencers in primary-key order, so the outcome never depends on storage
   order. Every row is found at its minimum depth; its `cause()` names the first such
   path in that order. `Cascade` rows are expanded; `SetNull` rows are collected unless
   they are deleted anyway;
2. refuses with `constraint_violation` when any `Restrict` foreign key (from any
   table, runtime or not) references a row that would be deleted. This is
   conservative: a restricting row that would itself be cascade-deleted still
   refuses;
3. refuses with `limit_exceeded` when more than `max_rows` dependent rows (deleted
   plus nulled, excluding the requested row) or a row whose minimum depth exceeds
   `max_depth` relation hops would be changed. Defaults are 256 rows and depth 8;
   `RuntimeDependentLimits::new` rejects values above `RUNTIME_DEPENDENT_MAX_ROWS`
   (65 536) and `RUNTIME_DEPENDENT_MAX_DEPTH` (64);
4. refuses with `unsupported_cascade` for a data cycle, meaning rows that reference each
   other through cascading relations so no delete order exists. This includes a row that
   references itself. It also refuses a modifying foreign key whose source table is not a
   runtime collection;
5. authorizes every dependent change through the same `RuntimeWriteAuthority`:
   `authorize_intent` and `authorize_preimage` for all of them before any write.
   `RuntimeWriteIntent::cause()` is `Some(RuntimeDependentCause)` with the owning
   relation, the policy, the referenced row's key and the depth. A dependent row
   hidden by the returned grant predicate, or any denial, refuses the whole delete
   with `denied`. Because the row exists and is locked, this refusal tells the caller
   that some dependent they cannot write exists; it never discloses its values;
6. performs the changes itself: each `SetNull` update (the relation's key fields set
   to SQL NULL), then the deletes, each row after every row that references it
   (deepest first, then key order), then the requested row. Each change gets an
   `authorize_result` check.

A row nulled through several relations gets one `SetNull` update per relation. Each
`SetNull` update refreshes the row's `OnCreateAndUpdate`
[automatic timestamps](runtime-schema-ir.md#automatic-timestamps), because the row changed. It
leaves [actor stamps](runtime-schema-ir.md#actor-stamps) unchanged unless the stamp is one of
the relation's key fields it clears. All
preimages are captured before any write, so the preimage of a later update still shows
the key fields an earlier update of the same row cleared.

Bounded limits, every intent and preimage check, and the Restrict check all
complete before the first write. `RuntimeMutationEffect::dependents()` lists the
pending dependent effects in execution order, each with `collection()`, `cause()`,
`staged()` and `preimage()`, so a `before_commit` hook can journal, emit events or
release resources for every changed row. Any error or hook failure rolls back the
requested row and every dependent change together.

A referencing row inserted concurrently cannot escape authorization. On SQLite the
immediate write transaction excludes other writers. On PostgreSQL a writer that has
not committed before the rows are locked waits and then fails its foreign-key check.
A row committed after the delete's serializable snapshot is unseen, but the final
delete's own foreign-key action then fails with a serialization failure
(`retryable_transaction`), so nothing changes silently. This also covers a
re-parenting update that commits while the delete waits. Two deletes whose cascades
overlap can deadlock on PostgreSQL; the loser fails with `40P01`, which is also
`retryable_transaction`.

Without `with_dependent_changes`, behaviour is unchanged. Only delete requests may
opt in (`invalid_input` otherwise). Runtime GraphQL deletes opt in through
`RuntimeGraphqlMutationOptions::dependent_changes`. Dependent intents there go to the
host write authority without the root collection's read-visibility restriction. The
GraphQL statement budget reserves only the requested row's statements. The dependent
work is bounded by these limits instead.
