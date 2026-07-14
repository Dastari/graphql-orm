# Portable Persistence Primitives

These APIs are opt-in and support managed SQLite and PostgreSQL schemas without requiring host
code to import SQLx or construct backend SQL.

## ORM-managed transactions

`Database::transaction` supplies only a transaction-bound `MutationContext`. `Default` uses the
backend default isolation. `StateMachine` starts SQLite with `BEGIN IMMEDIATE` before the callback's
first read and sets PostgreSQL `SERIALIZABLE` before application statements.

```rust
let result = database.transaction(TransactionMode::StateMachine, |tx| {
    Box::pin(async move {
        let order = tx.insert::<Order>(new_order).await?;
        tx.update_by_id::<Account>(&account_id, debit).await?;
        let visible = tx.find_by_id::<Order>(&order.id).await?;
        Ok::<_, OrmPublicError>(visible.expect("insert is transaction-visible"))
    })
}).await?;
```

The callback future is boxed because it borrows `MutationContext`. Callback errors roll back.
Cancellation and panic are not caught: dropping the future drops the driver transaction guard,
which rolls back and returns the connection to the pool. Queued events and deferred actions are
consumed only after commit succeeds. Commit failure discards them.

Nested calls on the same Tokio task fail with a safe `CONFLICT`; graphql-orm never silently opens an
independent nested transaction. `transaction_with_auth` additionally installs `DbAuthContext` as
transaction-local PostgreSQL settings before the callback. `TransactionError::is_retryable()` is
true for SQLite busy/snapshot conflicts and PostgreSQL serialization, deadlock, and lock failures;
retry the complete callback, never only its last statement.

## Versioned compare-and-swap

Mark one `i64` field as the database-managed version. It is omitted from create/update inputs and
should have an initial default:

```rust
#[graphql_orm(version, default = "0")]
#[filterable(type = "number")]
version: i64,
```

The generated repository method and `MutationContext::compare_and_swap` accept an expected version
plus the normal typed `WhereInput`, allowing expected status and other same-entity predicates:

```rust
let outcome = Job::compare_and_swap(
    &database,
    &job_id,
    7,
    JobWhereInput {
        status: Some(StringFilter { eq: Some("pending".into()), ..Default::default() }),
        ..Default::default()
    },
    UpdateJobInput { status: Some("running".into()), ..Default::default() },
).await?;

match outcome {
    ConditionalUpdateOutcome::Updated(job) => assert_eq!(job.version, 8),
    ConditionalUpdateOutcome::Conflict => { /* stale version/state */ }
    ConditionalUpdateOutcome::NotFound => { /* absent or invisible */ }
}
```

The predicate and checked `version = version + 1` execute in one `UPDATE ... RETURNING` statement.
The generated path validates that no more than one row returned and preserves transforms, row/entity
policies, hooks, search documents, journals, and change events.

## Append-only entities

```rust
#[graphql_entity(table = "audit_events", plural = "AuditEvents", append_only = true)]
struct AuditEvent { /* persisted fields */ }
```

Append-only entities generate query, insert, backup, and subscription paths but do not generate
update, upsert, replace, or delete inputs/methods/GraphQL fields. Incompatible upsert and UPDATE or
DELETE RLS declarations fail macro expansion.

Managed SQLite installs separate `BEFORE UPDATE` and `BEFORE DELETE` triggers. PostgreSQL installs a
`SECURITY DEFINER` trigger function with a fixed `pg_catalog` search path, revokes public function
privileges, and installs a `BEFORE UPDATE OR DELETE` trigger. The migration role owns the function;
ordinary application roles need only table INSERT/SELECT privileges and cannot bypass the trigger.
Roles with schema-owner DDL authority can remove enforcement, so applications should not run with
that authority. Live introspection records the triggers and managed validation fails closed when
either is missing or weakened.

Introspection does not trust generated names alone. SQLite requires the complete unconditional
`BEFORE UPDATE` and `BEFORE DELETE` abort-trigger grammar. PostgreSQL checks the exact row-level
event/timing catalog structure and the generated function's exception body, ownership,
`SECURITY DEFINER`, fixed search path, language, enablement, and execute-privilege posture. A
same-name inert body, conditional `WHEN`, extra statement, or disabled/wrong-operation trigger is
reported as drift. An approved fresh-version repair drops only the expected managed object names
before recreating them, so stale bodies and PostgreSQL function grants cannot survive repair.

Append-only remains absolute by default. Regulated hosts that need physical
expiry may separately opt in to the policy-gated bounded maintenance contract;
see [Bounded append-only retention maintenance](retention-maintenance.md).

## Portable constraints

```rust
#[graphql_orm(non_negative, max = 100)]
amount: i64,

#[graphql_orm(min_length = 2, max_length = 40)]
name: String,

#[graphql_orm(one_of = ["pending", "running", "done"])]
status: String,

created_at: i64,
#[graphql_orm(gte_field = "created_at")]
updated_at: i64,
```

`min`, `max`, and `non_negative` require numeric scalars. String length counts database characters
(`length` on both backends); `Vec<u8>` length counts bytes (`length` for SQLite blobs and
`octet_length` for PostgreSQL `bytea`). `one_of` currently supports string states.
`gte_field` requires the same scalar type on both persisted fields. Named checks participate in
stable hashes, introspection, drift diagnostics, and migration rebuilds. Violations map to
`CONSTRAINT_VIOLATION` without exposing constraint or SQL text.

## Stable keyset pagination

Opt in with a deterministic order whose final field is the unique primary key:

```rust
#[graphql_entity(
    table = "jobs",
    plural = "Jobs",
    keyset = "priority desc nulls last, created_at asc, id asc"
)]
struct Job { /* fields */ }
```

```rust
let page = Job::keyset_page(
    &database,
    JobWhereInput::default(),
    KeysetPageInput { after, limit: Some(50), include_total_count: false },
).await?;
```

The same opt-in adds a generated `jobsKeyset` GraphQL connection. Page limits are resolved and
capped before execution, the query requests only `limit + 1`, and total count runs only when
`includeTotalCount` is true. Cursors start with `gomk1.`, contain a version/order fingerprint and
checksum, and strictly reject malformed, tampered, order-mismatched, and legacy offset cursors.
Nullable ordering is explicit in generated SQL. Legacy offset connections remain available; migrate
clients by switching fields and discarding stored numeric cursors rather than decoding them as
keysets.
