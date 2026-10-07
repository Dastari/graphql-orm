---
title: Schema-bound transactional runtime mutations
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-07
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
Incoming delete CASCADE/SET NULL rejects with `unsupported_cascade`, even when
no child currently exists. RESTRICT remains usable; database constraint failures
roll back normally. No migration or pool query occurs inside the mutation engine.

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
when nullable, otherwise `missing_required`. Omitted patches preserve the stored
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

`RuntimeMutationEffect` is pending, not an event or commit receipt. The engine emits
no events. Static repository events/actions remain deferred until commit. Durable
journal storage and replay are host responsibilities. Static-only transactions
retain their existing limitation: catching an inner static mutation timeout and
returning `Ok` does not guarantee rollback.

Bulk/nested writes, new upsert semantics, dynamic GraphQL and durable delivery are
outside B. C/D remain separate reviewed successors. Owner merge/release of #112
and final reconciliation of A/B are required before combined downstream adoption;
this branch does not claim to contain the independent string-binding fix.
