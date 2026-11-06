---
title: "Strict Authorization Guide"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-01
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
- An entity `write_policy` is also the row policy key for updates and deletes, so those need both an
  entity provider and a row provider.

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
