---
title: "Strict Authorization Guide"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-14
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
let database = Database::new(pool)
    .with_authorization_mode(AuthorizationMode::DeclaredPoliciesRequired);
database.set_entity_policy(MyEntityPolicy);
database.set_field_policy(MyFieldPolicy);
```

## Declared Policy Requirements

When `DeclaredPoliciesRequired` is active:

- An entity declaring `read_policy` / `write_policy` requires an entity policy provider.
- A field declaring `read_policy` / `write_policy` requires a field policy provider.
- A row policy key requires a row policy provider.
- Missing providers return `AUTHORIZATION_MISCONFIGURED` with a safe public message.

They never silently allow. Unmarked operations still require explicit host policy
for protection; this default does not establish tenant isolation. For sensitive
applications select `ExplicitPolicyForAllExposedOperations`. Immediate revocation
requires the [current-authority bridge](agql-auth-bridge.md) or equivalent host
enforcement, not merely projected roles/scopes.

## Repository / System Access

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
