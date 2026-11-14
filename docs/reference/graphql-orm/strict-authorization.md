---
title: "Strict Authorization Guide"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-12
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
subject can use its generated operations.
[#287](https://github.com/Dastari/graphql-orm/issues/287) tracks requiring an explicit decision.

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
| Repository `insert`, `insert_many`, `insert_if_absent` and their transaction forms | every create field through the repository decision |
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
an omission.

**What the repository callbacks receive.** Generated repository reads and
writes always call `can_read_repository_field` and `can_write_repository_field`
with `access: None`: the ORM cannot tell a hook, a worker or a custom resolver
apart on this surface (see [repository / system access](#repository-system-access)).
`None` is not system authority, and it is not a reason to refuse either. Decide
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
| Generated GraphQL field or root | rendered as written, with its code | `INTERNAL_ERROR` or `SERVICE_UNAVAILABLE`, default message |
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
filter can't probe their columns. For a dependent delete this holds for the
root row only. A dependent child row the caller can neither read nor write is
not left out. Like any other unauthorized dependent, it makes the whole delete
refuse with `Dependent delete refused`, so the refusal can reveal that such a
child exists. A write-only caller still writes rows it can't
read. Reading a row means entity read access plus `can_read_row`, which runs
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
`ScopeEntityPolicy::new` refuses them too. A policy can't distinguish hooks, workers and custom
resolvers on this surface, and `AccessContext` reaches only repository field policies.
[#291](https://github.com/Dastari/graphql-orm/issues/291) tracks an explicit authority.

Do not treat `None` as system authority. Use:

```rust
use graphql_orm::prelude::*;

let system = SystemAccess::new("backup-worker").with_capabilities(vec!["export".into()]);
let access = AccessContext::System(&system);
// or
let access = AccessContext::Principal(&db_auth);
```

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
with `?`, which keeps its code and message. Use `Error::new` or set a string `code`
for an authored refusal. `INTERNAL_ERROR`, whether set as a string `code` or on a
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
to restore legacy input omission. Required create/upsert inputs still fail on
denial. Errors from a policy and missing policy providers are never ignored.
Repository field checks retain their existing fail-closed contract. See the
[migration guide](../../../MIGRATION.md#0911-to-0920-rejected-field-writes-fail-the-mutation).

Run the cross-backend field-denial regression suite through
`scripts/run-owned-database-lanes.sh field-write-denial`; PostgreSQL and MSSQL
use test-owned disposable containers. The MSSQL fixture explicitly declares
`schema_policy = "external_writable"` on both its entity and roots.
