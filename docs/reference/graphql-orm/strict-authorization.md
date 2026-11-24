---
title: "Strict Authorization Guide"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-23
review_by: 2027-02-01
supersedes: []
---

# Strict Authorization Guide

`graphql-orm` supports three authorization policy modes on `Database`:

| Mode | Behavior when a policy provider is missing |
| --- | --- |
| `LegacyPermissive` (explicit compatibility opt-in) | Allow |
| `DeclaredPoliciesRequired` (default) | Deny / misconfiguration error when a policy key is declared |
| `ExplicitPolicyForAllExposedOperations` | Deny all entity operations without a provider |

These modes are policy-provider requirements, not complete application policy.
Authentication presence, supplied scopes and typed query builders do not establish
resource/tenant authorization or current session status. `LegacyPermissive` is an
explicit compatibility escape hatch: missing providers allow. Sensitive applications
must configure business/row/field policy and immediate authority checks; see the
[complete accepted-request integration](agql-auth-bridge.md#accepted-request-and-current-authority).

## Defaults And Migration

| Flag | Current default | Secure recommended | Planned future default | Removal timeline |
| --- | --- | --- | --- | --- |
| `AuthorizationMode` | `DeclaredPoliciesRequired` | `ExplicitPolicyForAllExposedOperations` for sensitive applications | unchanged | `LegacyPermissive` remains explicit compatibility opt-in |
| Generated resolver `auth` | `required` | `required` for private schemas | unchanged | `none` remains explicit for public schemas |

```rust
let mut database = Database::new(pool)
    .with_authorization_mode(AuthorizationMode::DeclaredPoliciesRequired);
database.set_entity_policy(MyEntityPolicy);
database.set_field_policy(MyFieldPolicy);
database.set_row_policy(MyRowPolicy);
```

## Declared Policy Requirements

When `DeclaredPoliciesRequired` is active:

- An entity declaring `read_policy` / `write_policy` requires an entity policy provider.
- A field declaring `read_policy` / `write_policy` requires a field policy provider.
- A row policy key requires a row policy provider.
- Missing providers return `AUTHORIZATION_MISCONFIGURED` with a safe public message.
- An entity `write_policy` is also the row write policy key for creates, updates, upserts and deletes,
  so those need both an entity provider and a row provider. See
  [row policy on generated writes](#row-policy-on-generated-writes).

The row provider requirement for an entity `write_policy` applies in every mode,
including `LegacyPermissive` and `ExplicitPolicyForAllExposedOperations`.
Without a row provider, every generated write of such an entity fails with
`AUTHORIZATION_MISCONFIGURED` before it looks up any row. This covers creates,
updates, upserts, deletes and bulk writes on GraphQL, repository and transaction
paths, and retention purges. A dependent delete checks every entity registered
in its `StaticDependentDeletes` before it selects or locks any dependent. An
entity with no row isolation still needs a provider; one that allows
its rows is enough.

An entity that declares no policy has no such requirement: with `auth: "required"`, any authenticated
subject can use its generated operations. To make that a recorded decision rather than an omission,
set `require_policies: true` in `schema_roots!`: every listed entity must then declare a
`read_policy`, a `write_policy`, an entity-level `auth = "none"` or the `policy = "open"` marker, or the build fails with an error
naming the entity. The marker states that the entity is deliberately open; it changes nothing at
run time and cannot be combined with a policy key. `Entity::POLICY_DECISION_DECLARED` exposes the
same fact to host tooling. Without the setting, nothing is required.

Generated single-row reads (get-by-key roots, `NodeId` lookups and Federation
`_entities`) apply the same row authority as generated list reads. The SQL
predicate from `RowPolicy::read_visibility` restricts the row, and
`RowPolicy::can_read_row` runs when that decision is `CallbackOnly` or
`Prefilter`. A `Complete` or `Unrestricted` decision is sufficient on its own,
so a row outside a `Complete` predicate is `null` even when `can_read_row`
would allow it. The default `read_visibility` is `CallbackOnly`, so a provider
that implements only `can_read_row` keeps checking each fetched entity, using
its declared entity `read_policy` key. Subgraphs
with that key must install a row provider in addition to the entity provider.
The row decision must authorize the typed row using current request authority.
The [disposable zones subgraph](../../../crates/graphql-orm/tests/fixtures/federation-end-to-end/zones-subgraph/src/lib.rs)
uses a synthetic forwarded-scope header as test input for both providers and
denies row writes. Production providers must derive decisions from authenticated
current authority; an arbitrary forwarded header does not establish that authority.
Without a row provider, an entity that declares a `read_policy` key no longer
reads as `Unrestricted` outside `LegacyPermissive`. Under
`DeclaredPoliciesRequired` every read of it fails with
`AUTHORIZATION_MISCONFIGURED` before its row decision is applied, so on
get-by-key, `_entities` and `NodeId` an existing and a missing key answer
alike. Under `ExplicitPolicyForAllExposedOperations` its visibility is
`CallbackOnly`, so every read defers to `can_read_row`, which refuses:
single-row reads return `null`, a list or search root with an authorized scan
configuration returns no rows, and paths that need SQL-only visibility (offset
lists and search roots without a scan configuration, relation filters and
counts, aggregates and group pages, projections, repository counts and Keyset
roots) fail with `AUTHORIZATION_MISCONFIGURED`. Generated `Changed`
subscriptions for such an entity silently deliver no events in either mode;
the runtime emits one `tracing` warning per process on the
`graphql_orm::subscriptions` target. Entities without
a key keep `Unrestricted` visibility and their entity-policy-only contract.

Generated mutation payloads release the committed row under the same
authority: after the commit it is re-read by key under the
`GraphqlMutation`-surface predicate, the re-read row is the one checked and
released, and it is withheld (`null`, with `success: true`) outside it.
Repository reads (`find_by_key`, `RepositoryQuery`, and
`MutationContext::query()` reads in a transaction) add the `Repository`-surface
predicate to their SQL and still call `can_read_row` for every row;
generated writes look up their target rows with an internal locking fetch that
checks `can_read_row` only, and `find_by_key_in_mutation_context` wraps the same
fetch for hosts as a trusted host surface. Errors from entity
policies, `read_visibility` and `can_read_row` on these paths, Federation
`_entities` and `NodeId` included, are sanitized like other callback errors.

Unmarked operations still require explicit host policy
for protection; this default does not establish tenant isolation. For sensitive
applications select `ExplicitPolicyForAllExposedOperations`. Immediate revocation
requires the [current-authority bridge](agql-auth-bridge.md) or equivalent host
enforcement, not merely projected roles/scopes.

## Field Read Decisions

One rule applies to every field, whether or not it declares a `read_policy`:

> A generated path that reads or discloses a field for a GraphQL caller asks
> `Database::can_read_field` for that field, with its declared policy key or
> `None`. Repository paths ask `Database::ensure_repository_readable_field`
> the same way.

So an installed `FieldPolicy` decides every field, and can refuse a field that
declares no key by its entity and field name. What the decision is without a
provider depends on the mode:

| Mode | Declared key, no provider | No key, no provider |
| --- | --- | --- |
| `LegacyPermissive` | allowed | allowed |
| `DeclaredPoliciesRequired` | `AUTHORIZATION_MISCONFIGURED` | allowed |
| `ExplicitPolicyForAllExposedOperations` | `AUTHORIZATION_MISCONFIGURED` | allowed |

A key-less field without a provider is readable in every mode: no policy was
declared for it, and the entity and row decisions have already applied. A
declared key without a provider is a configuration error in both strict
modes. The default `can_read_repository_field` allows a key-less field and
denies a declared key.

> **Exception: plain `SimpleObject` entities.** An entity that derives
> `SimpleObject` (with `#[graphql(complex)]` or `schema_only = true`) without
> `#[graphql_orm::graphql_policy_object]` has its scalar getters generated by
> async-graphql. The ORM cannot ask the field decision when such a field is
> selected on a root read, and nothing reports that at run time: a provider
> that denies one of these fields by name is silently not consulted there.
> A field `read_policy` on such an entity is a compile error for the same
> reason. Apply `graphql_policy_object` (before the derives) to every
> `SimpleObject` entity whose fields a name-based policy must cover, or let
> the ORM generate the object. Relation loading, filters, ordering,
> aggregates, search and writes do ask for these entities.

A denial is `FORBIDDEN`. Where the path can't evaluate the decision it refuses
the whole request instead of dropping the field: a filter, an order term, a
search source, an aggregate or a projection is refused before any SQL. A
generated `Changed` subscription checks the selected fields of each event
before releasing it and drops an event with a denied field without a frame,
as it does for every per-event denial.

| Path | How the field decision is asked |
| --- | --- |
| Scalar getters of a generated object (get-by-key roots, list and connection items, mutation payloads, `_entities` and `NodeId` results, search hits) | per row, with the row as `record` |
| `Changed` events | per selected field and key field of the event's row, before the event is released; the getter asks again |
| `RecordVersion` | as the managed version field |
| `NodeId` | for the field name `NodeId` and for each primary-key field it encodes, each under a field context of its own name (`ctx.field()` names the field asked about) |
| Relation fields | on the source row, before any target is looked up |
| Selected fields of relation targets | on each loaded target row, before the relation resolves (also for a plain `SimpleObject` target); the target's getter asks again |
| `where`, `orderBy`, relation `where`, bulk-write filters, Keyset cursors | per referenced field, without a row, before any SQL |
| Aggregates and group pages | per group, metric and filter field, without a row |
| Search (`…Search` roots and `search_db`) | per search source, including a `search_relation` field, without a row |
| Typed projections and repository reads | per projected field through the repository decision |
| `#[graphql_policy_object]` `SimpleObject` fields | per row |

Key arguments of get-by-key roots, `_entities` and `NodeId` lookups are not
field reads: they name a row. Row visibility decides whether the row exists
for the caller, and each field of the result is then asked.

The provider is asked once per selected field per row on a root read, and
twice where a path checks before it releases: for the selected fields of
relation targets (the loader's check, then the getter) and for `Changed`
events (the release check, then the getter).

## Field Write Decisions

Writes follow the same rule, with one difference between the two surfaces:

> Repository writes ask `Database::ensure_repository_writable_field` about
> every field the host code writes, with the field's declared `write_policy`
> key or `None`. GraphQL mutations ask `Database::can_write_field` about every
> field that declares a key, and about a key-less field when the client could
> and did supply it. An upsert that resolves to an existing row also asks about
> every field its update writes, supplied or not.

| Path | How the write decision is asked |
| --- | --- |
| GraphQL create | a keyed field always (optional ones also when absent, as before); a key-less field when it is part of the GraphQL input type and the client supplied it (a required input always is) |
| GraphQL update | a keyed field the input supplies; a key-less field the client supplied; with the current row |
| GraphQL upsert, new row | as create |
| GraphQL upsert, existing row | every writable input field outside the primary key and upsert key, `skip_input` and host-managed timestamps included, keyed or key-less, supplied or omitted, with the current row and the value about to be written; ORM-managed timestamps and the managed version are not asked about, and a keyed field outside that set is not asked |
| GraphQL `updateMany` | as update, per matched row |
| Repository `insert`, `insert_many`, `insert_if_absent` and their transaction forms | every create field through the repository decision; `insert` with a bare pool instead of a `Database` takes the same path on a default handle over that pool, so a declared key is `AUTHORIZATION_MISCONFIGURED` there |
| Repository `update_by_id`, `update_by_key`, `update_where`, `update_if`, `compare_and_swap` and their transaction forms | every supplied field through the repository decision, with the current row |
| Repository `upsert`, `upsert_many` | every create field for a new row; every field its update sets for an existing one (changed or not; primary-key and upsert-key fields are asked only on insert); through the repository decision |
| Repository-only entities | every create and supplied update field (unchanged) |

The existing-row upsert asks more because its statement writes more: the
conflict update sets an omitted optional field to `NULL` and a `skip_input`
field to its default, so the decision is asked for those too (`None` is the
value passed for an omitted optional field). It is asked whether or not the
written value equals the stored one, as it always was for fields with a key.

Apart from that case, a GraphQL write never asks about a key-less field the
client cannot send (`skip_input`, host-managed timestamps, managed versions)
or about a value only a `WriteInputTransform` or hook set. No GraphQL write
asks the repository decision. Which
fields the client supplied is recorded before input transforms run. A field
with a declared key keeps its earlier contract, including being asked about
the transformed value. Repository writes are not asked about ORM-managed
columns such as a managed version.

Without a provider the table above for reads applies: a key-less field is
writable in every mode, and a declared key is `AUTHORIZATION_MISCONFIGURED` in
the strict modes. The default `can_write_repository_field` allows a key-less
field and denies a declared key, so a host that writes fields with a
`write_policy` through the repository implements it. A denial is `FORBIDDEN`
and names the public field on both surfaces: the message is
`write denied for field <name>`, and a GraphQL error carries
`extensions.deniedWriteField` (see
[denied generated field writes](#denied-generated-field-writes)).
`ignore_denied_field_write` still turns a denied optional GraphQL input into
an omission. On an upsert that resolves to an existing row the omission is
real: the denied column is left out of the conflict update, so its stored
value is kept.

**What the repository callbacks receive.** Generated repository reads and
writes call `can_read_repository_field` and `can_write_repository_field` with
`access: None` unless the caller presents a named system access: a handle
labelled with `Database::with_system_access`, or a hook write inside
`MutationContext::with_system_access`, passes `Some(AccessContext::System(..))`
(see [repository / system access](#repository-system-access)). Without a label
the ORM cannot tell a hook, a worker or a custom resolver apart on this
surface. `None` is not system authority, and it is not a reason to refuse either. Decide
on the entity name, field name and policy key. A provider that refuses every
call without an access context refuses every generated repository write,
including the writes of companion crates on their own entities;
`graphql-orm-ai` therefore answers repository field decisions for its own
records itself and delegates everything else to the host policy.

Deletes write no field. The ORM's own maintenance writes (managed versions and
timestamps, dependent `set_null` updates, retention purges, search documents)
are not caller-supplied field writes and ask no field decision. The static
macros generate no relation-link mutation and no nested-object input: a link
row is an entity and is written through the paths above.

## Policy Callback Errors

One classification applies to an error returned by an entity, row or field
policy callback, on every generated path:

- An error the callback **authored** itself has no source: `Error::new(...)`,
  with or without a `code` extension, or a typed `OrmPublicError` returned with
  `?`. It is a refusal.
- An error the callback **propagated** from something else has a source, as
  `?` on a database or I/O error leaves. It is a failure, and so is any error
  that carries `INTERNAL_ERROR` or `SERVICE_UNAVAILABLE`.

| Where the error surfaces | Authored refusal | Propagated or declared failure |
| --- | --- | --- |
| Generated GraphQL field or root | rendered as written, with its code, when it carries a `code` extension; an uncoded refusal is `INTERNAL_ERROR` with the default message | `INTERNAL_ERROR` or `SERVICE_UNAVAILABLE`, default message |
| Relation boundary | `FORBIDDEN`, or `UNAUTHENTICATED` / `AUTHORIZATION_MISCONFIGURED` when it carries that code; no text | the failure class; no text |
| `Changed` subscription event | the event is dropped; a timer refusal ends the stream with `FORBIDDEN` | the stream ends with a continuity-loss error |
| Repository field policy callbacks | its code and message when the code is an ORM public code; otherwise `INTERNAL_ERROR`, default message | `INTERNAL_ERROR` or `SERVICE_UNAVAILABLE`, default message |

## Row Policy On Generated Writes

`RowPolicy` decides three things for every generated write: which existing
rows it may target, whether the row it writes is allowed, and whether the
caller receives that row back. Row-level isolation that rests on `RowPolicy`
instead of PostgreSQL RLS relies on all three.

**The written row.** Generated creates, updates, upserts and bulk updates check
`can_write_row` on the row as written (the post-image), inside the transaction
and before the `After` hook. This covers GraphQL and repository writes,
single-column and composite keys, append-only creates, `insert_many`,
`insert_if_absent`, `upsert_many`, `update_if`, `compare_and_swap`, bounded
updates, transaction helpers and dependent `set_null` updates. A refusal is
`FORBIDDEN` and rolls the write back. In a caller-owned transaction, such as
`Database::transaction` or a `MutationContext` in a hook, the refusal also
blocks the commit. `commit_and_emit` then rolls back and returns `FORBIDDEN`,
even if the caller ignored the error, and `Database::transaction` returns
`TransactionError::Rejected` with `FORBIDDEN`. `compare_and_swap` rolls back its
savepoint on any failure after it opens, including a refused existing row. Any
generated write helper that fails after changing rows, for example in an
`After` hook or on a cardinality check, also leaves a caller-owned transaction
uncommittable. A refusal before any row changes leaves the transaction usable,
including an `insert_if_absent` that finds an existing row the caller may not
write. Updates and upserts still check the existing row first. A client-writable owner or tenant column therefore can't
create a row outside the caller's write scope or move one out of it.

**The returned row.** Generated GraphQL create, update and upsert payloads
contain the committed row only when the caller has entity read access and the
row passes the read authority a generated list read would apply: the
`GraphqlMutation`-surface read visibility predicate, re-read by key after the
commit, then `can_read_row` when that decision is `CallbackOnly` or
`Prefilter`. Write access does not imply read access. The outcome depends on
which check refuses:

- The row is outside the predicate, or `can_read_row` refuses it: the payload's
  entity field is `null` and it still reports `success: true`.
- The caller lacks entity read access, or a policy returns an error: the
  mutation field fails with an error carrying `extensions.outcome = "committed"`,
  and the whole field is `null`. A principal with entity-level write access but
  no read access that selects the entity gets this error; it should select only
  `success`.

Either way the write has committed, and no row is released. Field read
policies still apply to a released row. Delete payloads never contain the row, and bulk
payloads contain counts. Repository helpers return the written row to host
code unchanged. Host code that passes it on to a client must check it with
`Database::can_read_row` first.

**The targeted rows.** Generated GraphQL updates, deletes, `updateMany`,
`deleteMany` and the dependent-delete steps built on them classify each
existing row they select:

| Caller can | Single-row update or delete | `updateMany` / `deleteMany` |
| --- | --- | --- |
| write the row | proceeds | row is written |
| read the row, not write it | `FORBIDDEN` | the whole write is refused (`errorCode = "FORBIDDEN"`) |
| neither read nor write it | `success: false`, `error: "Entity not found"`, as for a missing row | row is left out: not written, counted or held against the `notify_with` 1,000-row bound |

So the response never depends on rows the caller can't read or write, and a
filter can't probe their columns. Without more help from the host, those rows
are still selected and locked, and passed to the row callbacks, before they are
treated as missing. `RowPolicy::write_target_prefilter` removes that
difference: return a `ReadPredicate` that selects the rows the caller can read
**or** write, and generated GraphQL `update`, `delete`, `updateMany` and
`deleteMany` add it to their locking read, so a row outside it is never
selected or passed to a callback, and is not locked except as described under
**Locks on hidden rows** below. The predicate only narrows. Every
selected row is still classified by `can_write_row` and `can_read_row`, so it
cannot grant a write; a row it leaves out is treated as missing even where the
callbacks would have allowed it, so it must not exclude rows the caller can
write. The default returns `None`. Repository writes do not ask.

**Narrowing bulk writes to read visibility (default).** GraphQL
`updateMany` and `deleteMany` also narrow their locking read by read
visibility when the policy declares no `write_target_prefilter`. This is the
default since the breaking release;
`DatabaseBuilder::narrow_bulk_writes_to_read_visibility(false)` (or
`Database::with_narrow_bulk_writes_to_read_visibility(false)`) turns it off. If
`RowPolicy::read_visibility` returns `Prefilter` or `Complete` for the
`GraphqlMutation` surface, its predicate is added to the bulk write's locking
read. A row outside the caller's read visibility is then never passed to a row
callback or counted. The predicate only narrows. Every selected row is still
classified as in the table above, so the payload and the rows written for
visible rows are unchanged.

With the setting off, bulk writes select every row their filter matches, as
before the breaking release. With it on:

- A row outside read visibility that `can_write_row` would allow is left out,
  as a missing row is. If callers may bulk-write rows they cannot read, return
  a `write_target_prefilter` that selects them. It takes precedence in both
  modes, and the bulk write uses it instead of read visibility.
- An error from `read_visibility` on the `GraphqlMutation` surface refuses the
  bulk write before any row is selected. A policy that returns `Err` for some
  callers, such as write-only ones, must return `CallbackOnly` for them instead
  to keep their bulk writes working.
- Single-row updates and deletes still use only the `write_target_prefilter`.

**Locks on hidden rows.** On PostgreSQL a row a SQL predicate leaves out (a
`write_target_prefilter`, or read visibility with the setting on) is never
locked: the row lock is taken only on rows that pass the `WHERE` clause. SQLite
locks the whole database for a write, so there are no row locks to observe.

On SQL Server the single locking read uses `WITH (UPDLOCK, HOLDLOCK)`, which
locks each row it reads before testing the predicate, so a scan locks, and
waits on, rows outside the predicate. The setting alone keeps that read, so a
narrowed SQL Server bulk write stays serializable but may still lock hidden
rows that a scan reads.

**Snapshot target selection on SQL Server (separate opt-in).**
`DatabaseBuilder::mssql_snapshot_bulk_write_targets(true)` (or
`Database::with_mssql_snapshot_bulk_write_targets(true)`) lets a narrowed
bulk write avoid that read. It is off by default, and the narrowing default
does not turn it on. It needs `narrow_bulk_writes_to_read_visibility` too. It
applies to a bulk write with a SQL predicate (read visibility or a
`write_target_prefilter`) **when all of these hold**:

- the database has `READ_COMMITTED_SNAPSHOT` on;
- the target is a base table, not a view;
- the table has an enabled, unfiltered unique index whose only key column is
  the key (a primary key or unique constraint on the key is one);
- every index of the table allows row locks (`ALLOW_ROW_LOCKS = ON`, the
  default);
- the key column is `uniqueidentifier`, `nvarchar`, `nchar`, `bigint`, `int`,
  `smallint` or `tinyint`. The ORM binds strings as Unicode, so a `varchar` or
  `char` key is converted to compare, and seeks on it, including the per-row
  writes, may become scans
  ([#522](https://github.com/Dastari/graphql-orm/issues/522)).

Otherwise the write keeps the single serializable locking read. Where it
applies, the write takes three steps:

1. **Candidates.** One statement reads every candidate: the rows that match
   the filter and the predicate, at most `max_scanned_rows` + 1 of them, read
   committed (`WITH (READCOMMITTED)`). With `READ_COMMITTED_SNAPSHOT` on, that
   is one consistent snapshot, and the read takes no lock on the target
   table. Related or policy tables that the filter reads are still read at the
   transaction's `SERIALIZABLE` level. More than the budget refuses the write.
2. **Lock and check.** Up to 500 keys at a time, it locks the candidates with
   `XLOCK` and a `FORCESEEK` on that unique index, with a `WHERE` on the key
   alone, so each seek finds at most one row. The lock runs read committed, so
   it takes no key-range lock that would reach a neighbouring row. It then
   checks the filter and the predicate again on each locked row, one seek on
   the same index at a time, at the transaction's own isolation. A row that no
   longer matches is left out before any row callback sees it. The locked rows
   go through the row callbacks as before.
3. **Validate.** One more statement reads the candidates again, in a new
   snapshot, under the same budget. If they are not exactly the locked rows
   that still match, another transaction changed which rows match since step 1.
   The write is then refused with `errorCode = "SERVICE_UNAVAILABLE"`, marked
   retryable, and nothing is written. Retry the mutation. More than the budget
   refuses the write with the scan budget's refusal.

What this guarantees, exactly:

- **The target set equals the rows that match at the validation snapshot,**
  and the exclusive locks keep those rows unchanged until the write commits.
  A transaction that inserts several matching rows at once never has some of
  them written and others not.
- **The write never locks a row outside its candidates,** and never waits on a
  lock another transaction holds on one, whatever plan the optimizer picks,
  except through lock escalation (see below).
  Rows outside read visibility are not candidates.

What it does **not** guarantee:

- **The bulk write's transaction is no longer serializable.** Generated bulk
  writes run `SERIALIZABLE`, but no range is locked after validation. Another
  transaction can commit a row that starts to match, together with changes to
  other rows or tables, without waiting for the write. That row is not written,
  yet later reads in the same bulk write (before-update hooks, field
  authorizers, triggers) can see the transaction's other changes. With the
  serializable locking read, that transaction would wait.

What else remains:

- The candidates are locked exclusively until the write ends, including those
  that turn out not to be targets: a row that left the filter or the predicate
  before step 2, or that a row callback leaves out or refuses. A reader that
  uses row versions (read committed with `READ_COMMITTED_SNAPSHOT` on,
  snapshot isolation) does not wait for them. A `READCOMMITTEDLOCK` read can
  also read an exclusively locked row that has not been changed yet. Readers
  that hold their locks (`REPEATABLE READ`, `SERIALIZABLE`, `UPDLOCK`) wait.
- SQL Server can escalate a transaction's row locks on a table to one table
  lock, by default when one statement takes about 5,000 locks on it or when
  lock memory runs short, and a table lock covers hidden rows. Each lock
  statement of this path locks at most 500 keys.
  `ALTER TABLE ... SET (LOCK_ESCALATION = DISABLE)` prevents escalation.
- The write reads its candidates twice, and a concurrent change to them makes
  it fail with a retryable error rather than wait.

With `READ_COMMITTED_SNAPSHOT` off, a read committed read takes and releases
shared locks as it goes, so it is not a consistent snapshot. No read of a
range is consistent without locking the range, and the range includes hidden
rows, so the write keeps the serializable locking read even with the opt-in.

Without the setting, a `write_target_prefilter` narrows SQL Server's single
locking read as before, and whether a scan locks rows outside it depends on
the access path.

Without any SQL predicate (`CallbackOnly` or `Unrestricted` visibility, no
prefilter, or the setting off) the bulk write still selects and locks every
matching row and passes each one to the row callbacks before it leaves hidden
rows out. The payload still does not depend on hidden rows, but how many hidden
rows match a readable filter can still be observed. It shows in latency and,
on PostgreSQL and SQL Server, in waits on locks that other transactions hold on
hidden rows. The caller also briefly locks rows it cannot see. A SQL predicate
removes the callback and counting difference on every backend, and the lock
difference on PostgreSQL and, with `mssql_snapshot_bulk_write_targets` and the
conditions above, on SQL Server.

A generated GraphQL bulk write may write at most
`PaginationConfig::max_bulk_write_rows` rows (default `10_000`) per mutation
field and is refused with `errorCode = "PAGE_LIMIT_EXCEEDED"` when more rows
the caller may write match; see
[bulk write bound](runtime-and-writes.md#bulk-write-bound). The bound counts
rows after the classification above, so hidden rows never decide whether it is
reached. With a row policy installed the candidates are classified in pages
under the read scan budget (`max_scanned_rows`); a scan that would need more
candidates is refused as an authorized read is, which is the one way hidden
rows can still affect the outcome. A prefilter, or with the opt-in a
read-visibility predicate, removes them from the scan.

For a dependent delete, leaving hidden rows out holds for
the root row only. A dependent child row the caller can neither read nor write is
not left out. Like any other unauthorized dependent, it makes the whole delete
refuse with `Dependent delete refused`, so the refusal can reveal that such a
child exists. A write-only caller still writes rows it can't
read, except that a bulk write narrowed by read visibility, with that
setting on, leaves them out (see above). Reading a row means entity read access plus `can_read_row`, which runs
only for rows `can_write_row` refuses. An upsert whose conflicting row is
unwritable refuses with `FORBIDDEN` whether or not the caller can read it. Its
key is unique, so the row can't be treated as absent; a plain create discloses
the same row through its unique constraint. Repository helpers evaluate
policies without the request (see below) and still refuse any unwritable row
with `FORBIDDEN`.

Custom resolvers can apply the same rule with `Database::authorize_write_target`,
which returns `Ok(false)` for a GraphQL mutation target the caller can neither
read nor write. Runtime GraphQL mutations already restrict their targets to the
caller's read visibility, report a refused preimage as `not_found`, check the
written row with `RuntimeWriteAuthority::authorize_result` and release payloads
through `release_mutation_result`.
PostgreSQL `#[graphql_rls(insert(...), update(...))]` policies remain an
independent database-level check.

## Repository / System Access

Repository helpers evaluate policies with `ctx = None` and `EntityAccessSurface::Repository`, including
when a custom resolver calls them, so a policy can't see that resolver's caller. `ScopeEntityPolicy::new`
refuses these calls as unauthenticated. A custom resolver applies its own checks with
`enforce_resolver_auth`, `Database::ensure_entity_access` and `Database::ensure_writable_row`, passing
`Some(ctx)`.

Writes from a mutation hook reach policies the same way, without the originating request, so
`ScopeEntityPolicy::new` refuses them too. By default a policy can't distinguish hooks, workers
and custom resolvers on this surface. A named system access (below) or the opt-in
[originating request](#hook-writes-on-the-originating-requests-authority) tells them apart.

Do not treat `None` as system authority. A trusted caller presents a named one instead, and the
policy decides on it:

```rust
use graphql_orm::prelude::*;

// A worker: every repository call and transaction on this handle presents it.
let worker = db.clone().with_system_access(
    SystemAccess::new("backup-worker").with_capabilities(vec!["export".into()]),
);

// A mutation hook: the writes it makes inside the scope present it.
hook_ctx
    .with_system_access(SystemAccess::new("order-cleanup"), |tx| {
        Box::pin(async move { tx.delete_where::<OrderLine>(lines).await })
    })
    .await?;
```

| Caller | `ctx` | Surface | `db.system_access()` | Repository field callbacks' `access` |
| --- | --- | --- | --- | --- |
| Generated GraphQL operation | `Some` | a `Graphql*` surface | `None` | not asked |
| Repository helper, transaction or hook write with no label | `None` | `Repository` | `None` | `None` |
| The same through `Database::with_system_access` | `None` | `Repository` | the handle's `SystemAccess` | `Some(AccessContext::System(..))` |
| A hook write inside `MutationContext::with_system_access` | `None` | `Repository` | the scope's `SystemAccess` | `Some(AccessContext::System(..))` |

Entity and row policies receive the `Database` the call runs on and read the label with
`Database::system_access()`; `FieldPolicy::can_read_repository_field` and
`can_write_repository_field` receive it as `AccessContext::System`. The label grants nothing by
itself: a policy that does not look at it decides as it did, and a call without one still presents
no authority. A hook is given the originating request, so it decides from that request whether to
open a scope; the policy then decides whether the named access may do the write. Every policy
check of a write made inside the scope uses the scope's access: entity access, field decisions,
the existing row and the row as written. A handle with broad access that enters a narrower scope
is therefore held to the narrower one, and a row it writes that the scope may not write is refused
and blocks the commit, like any other refused written row. A scope ends when
its future completes and restores the previous access; if the future is dropped first, the
transaction can no longer commit. Keep a labelled handle out of GraphQL schema data, and let a
policy that honours a label also require `ctx` to be `None`.

### The authority a policy receives

Every entity, row and repository field callback the ORM makes goes through a `*_with_authority`
method (`can_access_entity_with_authority`, `read_visibility_with_authority`,
`can_read_row_with_authority`, `can_write_row_with_authority`,
`can_read_repository_field_with_authority`, `can_write_repository_field_with_authority`). It
receives the existing arguments and a `PolicyAuthority`:

| `PolicyAuthority` | Meaning |
| --- | --- |
| `system()` | The named `SystemAccess` of the handle or scope, the same value as `db.system_access()` |
| `hook_request()` | The originating request of the mutation hook making the call, when the host opted in (below). Entity and row decisions only |
| `is_none()` | Neither is presented |

The default implementations ignore the authority and call the method without it, with the same
arguments, so a policy that overrides none of them decides exactly as before. `ctx = None` with
`is_none()` is a call that presents no authority at all, and a policy can refuse it while
admitting a named worker or a hook:

```rust
fn can_access_entity_with_authority<'a>(
    &'a self,
    authority: PolicyAuthority<'a>,
    ctx: Option<&'a async_graphql::Context<'_>>,
    db: &'a Database,
    entity_name: &'static str,
    policy_key: Option<&'static str>,
    kind: EntityAccessKind,
    surface: EntityAccessSurface,
) -> BoxFuture<'a, async_graphql::Result<bool>> {
    Box::pin(async move {
        if let Some(ctx) = ctx {
            return self.request_may(ctx, entity_name, kind);
        }
        if let Some(access) = authority.system() {
            return Ok(access.name == "billing-worker" && entity_name == "Invoice");
        }
        if let Some(request) = authority.hook_request() {
            // The same rule the request's own operations get.
            return Ok(request.subject().is_some_and(|subject| subject.has_scope("orders.write")));
        }
        Ok(false) // no authority
    })
}
```

A policy that wraps another forwards the authority to the inner policy's `*_with_authority`
method. `RowPolicy::can_read_deleted_preimage` and `write_target_prefilter` have no such form:
they are asked only for a GraphQL request or subscription, which presents its `ctx`.

### Hook writes on the originating request's authority

By default a mutation hook's nested writes present nothing of the request that caused them.
`DatabaseBuilder::propagate_request_authority_to_hooks(true)` (or
`Database::with_request_authority_propagated_to_hooks(true)`) opts in on the handle the schema
serves. A mutation hook that a generated GraphQL mutation invokes then presents a
`HookRequestAuthority` on the **entity and row** policy checks of the operations it makes through
its `MutationContext`, including the hooks those nested writes invoke in turn. It holds the
request's `AuthSubject` (and, with `auth-agql`, its accepted identity), not the GraphQL `Context`.
`ctx` stays `None` and the surface stays `Repository`.

The opt-in grants nothing by itself; a policy decides on the authority:

- `ScopeEntityPolicy` holds the originating subject to the same scope requirement as the
  request's own operations. A cleanup hook then succeeds for a caller holding the write scope and
  is still refused for one that does not, or without the opt-in.
- A host `EntityPolicy` or `RowPolicy` reads `authority.hook_request()` as above. One that
  overrides no `*_with_authority` method refuses the hook write as before.
- `AgqlCurrentAuthorityPolicy` is an entity policy. For every nested **entity** decision it
  revalidates the originating identity's status and current authority around the binding check
  and the business entity policy, as it does for a request, and asks
  `AgqlBindingAuthorizer::authorize_hook_operation` in place of `authorize`. Its default denies,
  so the host also implements that method. Row decisions are not revalidated by the bridge,
  for a hook write as for a request; they follow the entity decision of the same operation. See
  the [agql-auth bridge](agql-auth-bridge.md#accepted-request-and-current-authority).

The authority belongs to the hook's transaction, and to nothing else:

- **No `Database` handle carries it.** The handle `MutationContext::database()` returns, a clone
  of it, and the `db` a policy callback receives all present no originating request. A
  repository call, a `Database::transaction`, or a task spawned from the hook therefore has no
  request authority, whichever handle it uses, also while the hook is still running.
- **Only a generated GraphQL mutation attaches it,** and only when its request is admitted to
  write the mutation's entity: the entity decision is asked once more, on the `GraphqlMutation`
  surface, before each hook invocation that attaches it. `MutationContext::run_mutation_hook`,
  which host code can call, runs the hook without it.
- **It ends with the hook invocation,** including on an error or a dropped future, and every read
  of it checks that: `PolicyAuthority::hook_request()` and `HookRequestAuthority::subject()`
  return `None` afterwards, also for a value obtained earlier. Post-commit actions, later
  transactions and later requests start with none.
- A hook invoked by a repository write or a worker's transaction has no originating request.
- Inside `MutationContext::with_system_access`, the scope's named access replaces it.

What it does not cover:

- **Repository field decisions.** `can_read_repository_field_with_authority` and
  `can_write_repository_field_with_authority` receive the named system access only;
  `hook_request()` is always `None` there. A hook write of a field with a declared policy key is
  decided as before, by key, entity and field name or a named hook scope.
- **`RuntimeMutationHook`.** Only `MutationHook` invocations of statically generated mutations
  present the request. The runtime GraphQL surface does not.
- **Policies wrapped by a companion crate** receive what that crate forwards. `graphql-orm-ai`
  forwards host-owned repository field decisions on its internal handle to the host policy's
  `*_with_authority` methods (AI 0.158.0); its own key-less fields are answered by the crate.

A policy that honours the originating request trusts every installed mutation hook to act for
the caller on the entities and rows that request could itself write. Decide per entity where a
hook must not have that reach.

## Subscriptions

Under `ExplicitPolicyForAllExposedOperations`, subscription surfaces deny when no entity policy is registered. Prefer explicit enablement plus policy even in legacy mode.

## Denied generated field writes

A `false` field write decision fails generated create, update, upsert and bulk
update before any row changes, using the forbidden error and naming the GraphQL
field (`extensions.code = "FORBIDDEN"`, `extensions.deniedWriteField` names the
field). Bulk field denials use `success: false`, `error` naming the field and
`errorCode = "FORBIDDEN"` in the payload, like other application refusals.

Generated bulk update/delete payloads preserve the message and string code from
application hooks, row policies and input transforms. Select nullable `errorCode`
to receive the callback code (null if absent). Only unexpected failures are masked;
database/driver text cannot forge a refusal through a string prefix. The SQLx
compatibility path uses a private typed carrier. Refresh pinned SDL and client
code generation for the additive bulk result field.
Single-row generated mutations, generated entity/field policy checks, residual
single-row read checks and default propagating `notify_with` errors use
that same classifier. An error whose own `async_graphql::Error.source` field is
`Some` without a string `code` is masked; the wrapped error need not have a nested
`std::error::Error::source()`. The exception is a typed `OrmPublicError` returned
with `?`, which keeps its code and message. Set a string `code` for an authored
refusal: an error with no `code` extension, such as a plain
`async_graphql::Error::new(text)`, is not a public refusal and renders as
`INTERNAL_ERROR` with the default message, its text logged only. A host whose
uncoded callback text is safe for clients can restore the earlier rendering
for one schema: `DatabaseBuilder::expose_uncoded_callback_messages(true)` or
`Database::with_uncoded_callback_messages_exposed(true)` on the `Database` the
schema serves. Generated `schema_builder`s then install the
`graphql_orm::graphql::errors::UncodedCallbackMessages` extension, which
applies to the queries, mutations and subscriptions of that schema only; a
handwritten schema installs the extension itself. Work outside a schema, such
as a task the host spawns, keeps the default. `INTERNAL_ERROR`, whether set as a string `code` or on a
typed `OrmPublicError`, always renders the default message. Custom `From` conversions leaving that field empty preserve their text
and extensions. Direct SQLx sources retain their safe bulk category; other
sources become `INTERNAL_ERROR`. Internal details use the existing opt-in
`GRAPHQL_ORM_LOG_INTERNAL_ERRORS` server path, reported through `tracing` (see
[error codes](error-codes.md#server-side-diagnostics)). No data migration is needed.
Repository field policy callbacks (`can_read_repository_field`,
`can_write_repository_field`) use the same classifier, so their authored
refusals keep `FORBIDDEN`, `UNAUTHENTICATED` or `AUTHORIZATION_MISCONFIGURED`
for repository callers.

Optional create/upsert fields are checked even when absent, as before;
updates check only supplied fields, including explicit nulls.

`FieldPolicy::ignore_denied_field_write(entity_name, field_name)` defaults to
`false`. Override it to return `true` for selected GraphQL fields (or all fields)
to restore legacy input omission. An omitted field is never written: a create
or an inserting upsert stores the column's default, and an update or an upsert
onto an existing row keeps the stored value (the column is left out of the
statement, and out of the changes mutation hooks see). Required create/upsert
inputs still fail on denial. Errors from a policy and missing policy providers are never ignored.
Repository field checks retain their existing fail-closed contract. See the
[migration guide](../../../MIGRATION.md#0911-to-0920-rejected-field-writes-fail-the-mutation).

Run the cross-backend field-denial regression suite through
`scripts/run-owned-database-lanes.sh field-write-denial`; PostgreSQL and MSSQL
use test-owned disposable containers. The MSSQL fixture explicitly declares
`schema_policy = "external_writable"` on both its entity and roots.
