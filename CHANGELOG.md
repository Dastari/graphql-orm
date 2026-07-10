# Changelog

User-facing release notes live in [docs/release-notes.md](docs/release-notes.md).

## 0.4.0

Companion macros crate: `graphql-orm-macros` **0.4.0**.

### Added

- SQLx-free `Database::transaction` / `transaction_with_auth`, transaction-bound reads and writes,
  state-machine isolation, safe retry classification, nested-call rejection, and cancellation-safe
  rollback.
- Opt-in `#[graphql_orm(version)]` atomic compare-and-swap with typed expected filters,
  database-side monotonic increments, and explicit not-found/conflict/updated outcomes.
- Opt-in `append_only = true` generated surfaces and managed SQLite/PostgreSQL trigger enforcement
  with stable metadata, introspection, and drift planning.
- Portable numeric, length, closed-set, and cross-field constraints generated as named managed
  checks and mapped to safe constraint errors.
- Opt-in composite keyset pagination for repository, transaction, and GraphQL paths with bounded
  look-ahead queries and strict versioned opaque cursors.

### Compatibility

- Both crates are GitHub-only and set `publish = false`. Consumers must pin the reviewed full
  `graphql-orm` commit SHA; the optional bridge retains its exact full-SHA `agql-auth` dependency.
- Existing offset connections and mutable entity APIs remain unchanged unless the new attributes
  are selected. Append-only entities intentionally omit mutation APIs.
- `WriteBackend` was not extended; the public transaction runner uses the additive
  `TransactionBackend` capability.
- Stored numeric offset cursors are not accepted by keyset fields. Clients must begin keyset
  traversal without a cursor after switching fields.

See [MIGRATION.md](MIGRATION.md) and
[portable persistence primitives](docs/portable-persistence.md).

## 0.3.0

Companion macros crate: `graphql-orm-macros` **0.3.23** (epoch-default
generation and runtime expression alignment; patch release for compatibility).

### Security

- Added `AuthorizationMode` with fail-closed `DeclaredPoliciesRequired` and
  `ExplicitPolicyForAllExposedOperations` modes. Default remains
  `LegacyPermissive` for one migration release; production should opt into
  `DeclaredPoliciesRequired`.
- Public GraphQL errors now use stable codes via `OrmPublicError` /
  `OrmErrorCode`. SQL and configuration strings are not exposed by default
  (**breaking** for callers that parsed raw infrastructure messages).
- `AuthSubject` and `DbAuthContext` redact sensitive claim bodies in `Debug`.
- DataLoader / auth cache keys fingerprint claims instead of embedding raw JSON.
- Event sender locks recover from poisoning instead of panicking.
- Pagination defaults reduced from 1000/1000 to 50/100 (**breaking**). Use
  `PaginationConfig::legacy()` during migration.
- Added structural tenant/owner authorization helpers for backend-independent
  predicates.
- Added optional `auth-agql` bridge mapping `agql_auth::AuthPrincipal` →
  `AuthSubject` / `DbAuthContext`, pinned to upstream
  `agql-auth` 0.7.0 (`rev = 5e7f230b96350f55496477c11f8a0505e6438779`) with no
  path/`[patch]` overrides.

### Fixed

- **SQLite migration idempotency:** column defaults such as `unixepoch()` and
  `(unixepoch())` are now treated as equivalent during planning, hashing, and
  live-schema introspection. Reopening a file-backed SQLite database and
  replanning the same managed schema no longer emits a false `AlterColumn`
  step that breaks `ApplyOptions::additive_only` restarts. Canonicalization is
  general for balanced outer parentheses and SQL keyword/boolean defaults; it
  does not weaken additive-only validation for real changes.
- **Empty migration re-apply:** `SchemaManager::apply_migration` (and
  `apply_schema_target`) treat an already-recorded version as a no-op **only
  when the plan has no remaining steps or statements**. Restart paths that
  replan an empty list for the same version no longer insert a second history
  row. If the version is already recorded but the plan still has work, apply
  fails closed (schema drift / unsafe version reuse) instead of silently
  reporting success.
- **Schema-target remaining work:** `apply_schema_target` evaluates remaining
  work from the full plan (nested migration steps/statements, RLS statements,
  and combined executable statements). An empty nested `plan.migration` with
  remaining RLS/combined statements is no longer treated as already applied.
- **SQLite UNIQUE introspection:** inline `UNIQUE` column constraints (and
  multi-column `UNIQUE (...)` constraints) are recovered from
  `sqlite_autoindex_*` entries with origin `u`. Generated `#[unique]` fields
  no longer cause false `AlterColumn` plans after reopening a file-backed
  database.

### Added

- `AccessContext` / `SystemAccess` for deliberate repository system authority.
- `FilterExpression::TrustedFragment` and `trusted_fragment` constructor.
- `canonicalize_column_default_expression` for shared default comparison.
- Documentation: strict authorization, error codes, agql-auth bridge,
  cross-backend tenant isolation, pagination migration.

### Migration

See [MIGRATION.md](MIGRATION.md).

## 0.2.21

- Added `AuthSubject`, upgraded `AuthExt`, exact-scope `ScopeEntityPolicy`, and `DbAuthContext`
  constructors.
- Added generated resolver auth modes on entities and schema roots.
- Kept `auth_user()` as a deprecated alias for source compatibility.
- Added a reserved optional `auth-agql` feature; concrete agql-auth converters are deferred until
  the upstream agql-auth 0.7 API is tagged.

See the 0.2.21 section in [docs/release-notes.md](docs/release-notes.md#0221) and the migration
guide in [MIGRATION.md](MIGRATION.md).
