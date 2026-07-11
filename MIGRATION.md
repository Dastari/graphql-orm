# Migration Guide

`graphql-orm` is distributed from GitHub only. Use a reviewed full 40-character commit in `rev`;
neither the runtime nor macros crate is published to crates.io.

## 0.4.3 Structural Introspection Hardening

This compatible patch needs no application API change. Pin both crates to the reviewed `v0.4.3`
commit and run managed validation with a new migration version.

Conditional indexes created by graphql-orm remain restart-idempotent. A same-name live index is now
accepted only when the entire stored predicate parses as the supported `field IN (closed set)` form
or PostgreSQL's equivalent `field = ANY (ARRAY[...])` representation. Extra boolean expressions,
comments that SQLite persists, functions, and unsupported casts are drift. PostgreSQL discards SQL
comments when storing index expressions, so comment-only spelling has no persistent structural
meaning on that backend.

Append-only validation now checks complete SQLite trigger definitions and PostgreSQL trigger and
function catalog contracts, including unconditional enforcement and privilege/search-path posture.
If an older deployment contains a recognizable managed name with hand-edited SQL, planning will
produce repair work. Reusing an already recorded migration version then fails closed; review and
apply that work under a fresh version with the schema-owner migration role.

## 0.4.2 Legacy Migration-History Adoption

This compatible patch needs no application API change. Move both crates to the reviewed `v0.4.2`
commit before adopting a database created by an older migration helper.

At managed-schema preparation, a history table containing exactly `version` as a non-null textual
sole primary key and a non-null textual/timestamp `applied_at` is upgraded in one transaction.
Every existing version and timestamp is preserved. Missing descriptions are set to
`Legacy migration <version>`, and missing current metadata columns are added as nullable text. No
historical migration is re-executed. Repeated preparation is idempotent.

SQLite rebuilds the recognized legacy table to install the complete current schema while preserving
rows verbatim. PostgreSQL requires `applied_at TIMESTAMPTZ NOT NULL`; arbitrary legacy text is not
converted because doing so could change timestamp meaning. PostgreSQL restores the
`CURRENT_TIMESTAMP` default for future rows without changing existing values. Unknown columns,
incorrect types or nullability, an empty version, or any other primary-key identity are rejected.
Recorded-version reuse and remaining-plan drift checks still run after adoption and still fail
closed.

Back up the database before first preparation. If a legacy table is rejected, inspect and migrate it
explicitly rather than renaming columns until it happens to pass validation.

## 0.4.1 Binary Keys and Conditional Indexes

This is a compatible Git-pin update. Move both crates to the reviewed `v0.4.1` commit.

- Binary `Vec<u8>` keys require no host encoding. Mark host-assigned keys
  `#[graphql_orm(auto_generated = false)]`; use `private`, `skip_input`, or `#[graphql(skip)]` when
  they must not appear in public GraphQL inputs. Add `min_length` and `max_length` before migration
  when fixed digest width is an invariant.
- Existing repository upsert entities with hidden conflict targets now compile. Their repository and
  transaction helpers remain available, while the unsafe GraphQL upsert field is absent.
- Before adding a unique conditional index, validate that rows inside the selected predicate set
  contain no duplicate indexed keys. Apply the generated create-index plan under a new migration
  version.
- Adding `gt_field`, `gte_field`, `lte_field`, or `lt_field` creates managed checks. SQL check
  predicates evaluate to UNKNOWN when either nullable operand is NULL, so NULL rows pass unless a
  separate non-null constraint applies.

## 0.4.0 Portable Persistence

This is an additive migration for existing entities. Upgrade both crates together to `0.4.0`.

1. Replace host-owned pool transactions with `Database::transaction`; use `StateMachine` for
   security-sensitive read/decide/write flows and retry the whole callback when classified
   retryable.
2. Add `#[graphql_orm(version, default = "0")]` to an `i64` field, apply the planned column
   migration, then move guarded updates to `compare_and_swap`.
3. Add `append_only = true` only after removing update/delete/upsert callers. Review and apply the
   trigger plan with the schema-owner migration role; remove UPDATE/DELETE grants from ordinary
   PostgreSQL roles as defense in depth.
4. Add portable constraint attributes, validate existing data, and apply the planned SQLite table
   rebuild or PostgreSQL named checks. New checks may reject historical invalid rows during rebuild.
5. Add a deterministic `keyset = "..., id asc"` order and migrate clients to the generated keyset
   field. Discard legacy numeric cursors; they intentionally fail strict keyset decoding.

Managed startup should validate after every step. Missing append-only triggers or named checks are
schema drift and must not be ignored or repaired with a reused migration version.

## 0.3.0 Security Hardening

### SemVer Recommendation

Release as **0.3.0** (minor with documented breaking security defaults for pagination
and public error messages). A future major can flip `AuthorizationMode` default to
`DeclaredPoliciesRequired`.

### Authorization Mode

```rust
// Before (implicit fail-open when no policy provider)
let database = Database::new(pool);

// After (recommended production setting)
let database = Database::new(pool)
    .with_authorization_mode(AuthorizationMode::DeclaredPoliciesRequired);
database.set_entity_policy(MyEntityPolicy);
```

| Mode | Current default | Secure recommended | Future default |
| --- | --- | --- | --- |
| `LegacyPermissive` | yes | no | removed as default |
| `DeclaredPoliciesRequired` | no | yes | planned default |
| `ExplicitPolicyForAllExposedOperations` | no | for high-assurance APIs | optional |

### AuthSubject Expansion

```rust
// Before
AuthSubject::from_parts(id, roles, scopes, tenant_id)

// After (compatible) — same helper still works
// Prefer builder for new fields:
AuthSubject::builder(id)
    .user_id(user_id)
    .roles(roles)
    .scopes(scopes)
    .tenant_id(tenant)
    .token_id(jti)
    .session_id(session)
    .actor_id(actor)
    .build()
```

`Debug` no longer prints claim JSON bodies.

### Public Errors

```rust
// Before
Err(async_graphql::Error::new(error.to_string()))

// After
Err(OrmPublicError::from_sqlx(&error).into_graphql_error())
```

Missing auth messages changed from `"missing auth"` to `"unauthenticated"` with
`extensions.code = "UNAUTHENTICATED"`.

### Pagination Defaults

```rust
// Restore 0.2.x limits
Database::new(pool).with_pagination_config(PaginationConfig::legacy())
```

Default limit: `1000` → `50`. Max limit: `1000` → `100`.

### agql-auth Bridge

```toml
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.4.3", features = ["sqlite", "auth-agql"] }
```

The optional feature depends on upstream `agql-auth` 0.7.0 via git revision
`5e7f230b96350f55496477c11f8a0505e6438779` (tag `v0.7.0`). It does not use a
local path, sibling checkout, or Cargo `[patch]`.

```rust
use graphql_orm::graphql::auth_agql::auth_bundle_from_principal;
let (subject, db_auth) = auth_bundle_from_principal(&principal);
```

### Structural Tenant Helpers

```rust
let resolution = resolve_structural_auth(
    StructuralAuthMetadata::new(Some("tenant_id"), None, StructuralAuthorization::Required),
    &StructuralAuthValues::from_subject(&subject),
);
```

Macro-generated wiring of structural predicates on every operation path remains a
follow-up; helpers are available for host and incremental macro integration.

### Trusted SQL Fragments

Prefer `FilterExpression::trusted_fragment(clause, values)` for host-authored
predicates. `FilterExpression::Raw` remains for generated compatibility and is
documented as a trusted surface.

### SQLite Default Expression Idempotency

No application migration is required. After upgrading, reopening a file-backed
SQLite database and replanning the same managed schema should produce an empty
plan even when live `PRAGMA table_info` returns `unixepoch()` while generated
metadata previously declared `(unixepoch())`.

`ApplyOptions::additive_only` behavior is unchanged for real non-additive steps
such as altering a default from `unixepoch()` to `date('now')`.

### Empty Migration History Idempotency

`SchemaManager::apply_migration` is idempotent **only** when:

1. the version is already present in `__graphql_orm_migrations`; and
2. the plan has no remaining steps or statements.

Restart code that re-plans and re-applies the same version when the schema is
already current receives
`AppliedMigrationReport { already_applied: true, statements_applied: 0, .. }`.

If the version is recorded but the plan still has work, apply fails with an
explicit protocol error. That is intentional: it surfaces schema drift or
unsafe reuse of a migration version rather than silently treating the plan as
done.

For `apply_schema_target`, “remaining work” includes nested migration
steps/statements, RLS statements, and the combined executable `plan.statements`.
An empty nested table migration with remaining RLS work is **not** a no-op.

Callers that pattern-match `AppliedMigrationReport` must accept the new
`already_applied` field.

## 0.2.21 Auth Bridge

### Structural Changes

`AuthExt::auth_user()` is deprecated but still available. Migrate call sites based on what they need:

```rust
// Before
let user_id = ctx.auth_user()?;

// After: id only
let user_id = ctx.auth_user_id()?;

// After: id, roles, scopes, tenant id
let subject = ctx.auth_subject()?;
```

Applications can keep injecting the legacy `String` user id while migrating. `graphql-orm` upgrades it
to `AuthSubject { id, roles: [], scopes: [], tenant_id: None, ... }`. New code should inject
`AuthSubject` directly.

If a downstream crate implemented `AuthExt` itself, add implementations for `auth_user_id`,
`auth_subject`, and `auth_subject_opt`. Most applications only use the built-in implementation for
`async_graphql::Context<'_>` and do not need to change anything beyond call-site names.

```rust
let request = request.data(AuthSubject {
    id: user.id.to_string(),
    user_id: None,
    roles: user.roles.clone(),
    scopes: user.scopes.clone(),
    tenant_id: user.tenant_id.clone(),
    claims: None,
    token_id: None,
    session_id: None,
    actor_id: None,
});
```

`DbAuthContext::from_subject(&subject)` can mirror the same principal into PostgreSQL RLS settings:

```rust
let request = request
    .data(subject.clone())
    .data(DbAuthContext::from_subject(&subject));
```

### Generated Resolver Auth Modes

Generated resolvers keep the previous fail-closed default: if no `auth` setting is present, they
require an auth subject before database access. This preserves the old generated `ctx.auth_user()?`
gate.

Use `auth = "none"` for public generated resolvers:

```rust
#[graphql_entity(table = "pages", plural = "Pages", auth = "none")]
pub struct Page {
    // fields...
}
```

Use `auth = "optional"` when a schema should read a subject if present but leave allow/deny decisions
to `EntityPolicy`, `RowPolicy`, or `FieldPolicy`:

```rust
schema_roots! {
    auth: "optional",
    query_custom_ops: [],
    entities: [Record],
}
```

Use `auth = "required"` explicitly for new private schemas or entities:

```rust
schema_roots! {
    auth: "required",
    query_custom_ops: [],
    entities: [Ticket, Session],
}
```

Entity-level `auth` overrides the schema-root mode.

### ScopeEntityPolicy

`ScopeEntityPolicy` is exact-match only:

```rust
let mut database = Database::new(pool);
database.set_entity_policy(ScopeEntityPolicy::new(
    &["tickets.read"],
    &["tickets.write"],
));
```

`require_auth: true` returns an unauthenticated GraphQL error when no subject exists. A subject that
lacks the required exact scope returns `Ok(false)` from the policy and is denied by the generated
access path.

### Behavioral Notes

- No JWT, OIDC, cookie, wildcard, or product-specific scope logic was added to `graphql-orm`.
- PostgreSQL RLS helper functions still use exact scope matching.
- The `auth-agql` feature is available against `agql-auth` 0.7.
