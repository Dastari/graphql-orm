---
title: "Strict Authorization Guide"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-11
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

Generated Federation entity-key resolvers check `RowPolicy::can_read_row` on
each fetched entity, using its declared entity `read_policy` key. Subgraphs
with that key must install a row provider in addition to the entity provider.
The row decision must authorize the typed row using current request authority.
The [disposable zones subgraph](../../../crates/graphql-orm/tests/fixtures/federation-end-to-end/zones-subgraph/src/lib.rs)
uses a synthetic forwarded-scope header as test input for both providers and
denies row writes. Production providers must derive decisions from authenticated
current authority; an arbitrary forwarded header does not establish that authority.
Collection/relation paths that obtain `Unrestricted` visibility without a row
provider retain their entity-policy-only contract. That visibility behavior
does not bypass the Federation resolver's independent per-row check.

Unmarked operations still require explicit host policy
for protection; this default does not establish tenant isolation. For sensitive
applications select `ExplicitPolicyForAllExposedOperations`. Immediate revocation
requires the [current-authority bridge](agql-auth-bridge.md) or equivalent host
enforcement, not merely projected roles/scopes.

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
uncommittable. A refusal before any row changes leaves the transaction usable. Updates and upserts still check the existing row first. A client-writable owner or tenant column therefore can't
create a row outside the caller's write scope or move one out of it.

**The returned row.** Generated GraphQL create, update and upsert payloads
contain the committed row only when the caller has entity read access and
`can_read_row` allows the row. Write access does not imply read access. The
outcome depends on which check refuses:

- `can_read_row` refuses the row: the payload's entity field is `null` and it
  still reports `success: true`.
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
`std::error::Error::source()`. Use `Error::new` or set a string `code` for an authored
refusal. Custom `From` conversions leaving that field empty preserve their text
and extensions. Direct SQLx sources retain their safe bulk category; other
sources become `INTERNAL_ERROR`. Internal details use the existing opt-in
`GRAPHQL_ORM_LOG_INTERNAL_ERRORS` server path. No data migration is needed.

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
