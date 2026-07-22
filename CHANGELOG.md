# Changelog

User-facing release notes live in [docs/release-notes.md](docs/release-notes.md).

## 0.14.0

Companion macros crate: `graphql-orm-macros` **0.14.0** under the aligned
Git-only version policy; macro syntax and generated output are unchanged.

- Aligned the optional one-way `auth-agql` bridge with released `agql-auth`
  0.12.0 at exact revision
  `3f3b0c5365adfbe436514a681d977b600991b797`. A matching direct host
  dependency resolves one package and public type universe.
- Preserved identity, role, scope, tenant, organization, actor, correlation,
  token/session reference, policy-version, and host-accepted assurance
  mappings. Standard scalar `acr` and separate assurance `context` remain
  byte-for-byte distinct and absent values are not synthesized.
- Hardened assurance projection to omit malformed values and values
  inconsistent with the session MFA state or access-token `auth_time`, AMR,
  and scalar ACR. `MfaAcceptance::Unsatisfied` remains an exact negative MFA
  decision rather than becoming authority.
- Restricted custom claim projection to the documented string
  `policy_version`; arbitrary `AccessTokenMetadata.additional` content no
  longer enters `AuthSubject.claims`, `DbAuthContext`, PostgreSQL settings, or
  their debug/serialized forms.
- Kept OIDC request/outcome handling, provider evidence, rate-limit
  persistence, token minting, MFA inference, and product policy outside the
  ORM. In particular, `EssentialAcrs`/`matched_acrs` alone creates no ORM
  assurance, and graphql-orm does not implement agql-auth 0.11's atomic
  `AuthRateLimitStore`.

This observable bridge hardening is a pre-1.0 minor release rather than a
dependency-only patch. No database schema, data, generated-code, or backend
migration is required.

## 0.13.0

This combined release contains two coordinated prompts. Companion macros
crate: `graphql-orm-macros` **0.13.0** under the aligned Git-only version
policy; derive syntax and generated code are unchanged.

- Added fingerprint-bound opaque parent anchors and batched runtime to-one/
  to-many relation reads with typed composite keys, nullable-key short circuit,
  bounded per-parent forward/backward `gormrr1` keysets, optional exact counts,
  stable errors, hidden grouping/cursor fields, and SQLite/PostgreSQL parity.
- Added `RuntimeRelationLimits`, `RuntimeRelationSelection`, anchored read and
  batch request/result types, plus `Database::execute_runtime_anchored_read`
  and `Database::execute_runtime_relation_batch`. MSSQL remains explicitly
  unsupported for runtime execution; static relation behavior is unchanged.
- Added `runtime_relation_batch_request_with_relation_keys` and
  `RuntimeRelationBatch::relation_parents` so an executed child layer retains
  only opaque, redacted keys for the next explicitly requested relation. A
  multi-level request remains one bounded compatible statement per layer.
- Fixed PostgreSQL introspection to group UNIQUE constraints by ordered catalog
  identity and exclude `pg_constraint.conindid`/primary backing indexes from
  ordinary indexes while preserving explicit unique and partial indexes.
- Managed CREATE TABLE now renders declared composite UNIQUE constraints, so
  the structured target and live PostgreSQL/SQLite schema agree after first
  apply. Unchanged replans and additive complete-target upgrades no longer try
  to drop constraint-owned indexes.
- Existing runtime/static cursors, public backend traits, GraphQL/generated
  APIs, serialized runtime schemas, and stored data are compatible.

## 0.12.0

Companion macros crate: `graphql-orm-macros` **0.12.0**. The Git-only aligned
release policy advances both crates for this public runtime API release; derive
syntax and generated code are unchanged.

- Added schema-fingerprint-bound `RuntimePredicate`, `RuntimeOrder`,
  `RuntimeReadRequest`, limits, page/cursor, connection, page-info, and safe
  error APIs for runtime-schema reads.
- Added validated recursive scalar filters, structural policy-filter `AND`,
  explicit portable null ordering, primary-key tie-breakers, bounded
  bidirectional keysets, hidden cursor columns, and opt-in exact count.
- Added `Database::execute_runtime_read` with typed bindings, exact existing
  runtime row decoding, and optional `DbAuthContext` on SQLite/PostgreSQL.
  MSSQL remains explicitly unsupported for runtime decoding/execution while
  static reads are unchanged.
- Existing static queries, generated CRUD/GraphQL, backend traits, `SqlValue`,
  legacy cursor formats, schemas, and migrations are source-compatible. No
  schema or data migration is required.

## 0.11.0

Companion macros crate: `graphql-orm-macros` **0.11.0**. Both Git-only crates
advance together because this release adds a public derive, generated code,
repository authorization callbacks, and runtime query types.

- Added opt-in `RepositoryEntity` / `#[repository_entity(...)]` generation for
  one canonical managed entity with typed repository CRUD, filters, ordering,
  projections, transactions, CAS/composite operations, hooks, events, search,
  backup, and authorization, but no async-graphql types or roots.
- Added bounded Database-bound `RepositoryQuery` reads and separate fail-closed
  repository field-policy callbacks. Search-enabled entities use a bounded,
  policy-aware `RepositorySearchQuery`. Private/sensitive fields remain
  available to trusted Rust write inputs without widening GraphQL inputs.
- Sensitive generated input/projection debug output, mutation-hook state, and
  change events are redacted; repository entity/row/field policies continue to
  apply without treating an absent GraphQL context as authority.
- Equivalent repository-only and GraphQL-enabled declarations retain identical
  managed schema models and stable hashes. No DDL or data migration is needed.
- SQLite/PostgreSQL provide the applicable full contract; MSSQL repository-only
  entities are read-only and reject write configurations at compile time.

## 0.10.0

Companion macros crate: `graphql-orm-macros` **0.10.0**. Repository release
policy keeps the Git-only companion versions aligned when public runtime APIs
change; derive syntax and generated code are unchanged.

- Added owned `RuntimeValue`, `RuntimeRecord`, finite-float, and canonical
  datetime types covering every existing `RuntimeValueKind`.
- Added fingerprint-bound collection, field, relation, and projection handles
  resolved only by `ValidatedRuntimeSchema`; unknown, cross-collection,
  duplicate, empty, and stale inputs fail before query execution.
- Added the source-compatible `RuntimeRowDecoder` capability and exact,
  projection-only SQLite/PostgreSQL decoding with stable safe errors and
  retained backend sources. MSSQL/no-default configurations remain explicit
  unsupported capabilities while existing static reads continue unchanged.
- Added real SQLite and owned disposable-PostgreSQL parity, hostile-row,
  nullability, type-mismatch, serialization, and feature-boundary coverage.
- Runtime query rendering/execution, dynamic GraphQL, filters, ordering,
  pagination, relation batching, and writes remain deliberately deferred.

## 0.9.0

Companion macros crate: `graphql-orm-macros` **0.9.0**. Both crates require a
pre-1.0 minor version because public schema descriptors and generated code
change.

- Added opt-in `retention_purge = "policy.key"` metadata for append-only managed
  SQLite/PostgreSQL entities.
- Added host-only `Database::retention_transaction[_with_auth]`, narrow
  `RetentionContext`, generated bounded typed purge, exact outcomes, and
  redacted post-commit notifications.
- Added transaction-local SQLite/PostgreSQL append-only enforcement exceptions,
  structural introspection, stable schema/module/backup fingerprints, explicit
  migration work, policy/RLS integration, and fail-closed tamper detection.
- Existing append-only entities remain non-purgeable and retain their previous
  stable fingerprints. Ordinary repository, transaction, and GraphQL mutation
  surfaces are unchanged.
- Manual public metadata/model struct literals and exhaustive enum matches
  require the 0.9.0 source updates listed in MIGRATION.md. Low-level backend
  traits retain fail-closed default methods, and older serialized
  descriptors/catalogs default retention to disabled.

## 0.8.0

Companion macros crate: `graphql-orm-macros` **0.8.0**.

- Added an owned, backend-neutral runtime schema IR (`runtime_schema` module):
  stable ID newtypes, owned collection/field/relation/index metadata with
  ordered relation key pairs and composite primary keys, fail-closed structured
  validation diagnostics, deterministic canonical serialization, and separate
  full and ID-free structural fingerprints.
- Added `RuntimeSchema::from_static_entities` so derive-generated
  `EntityMetadata` graphs convert into the owned IR; equivalent static and
  runtime definitions agree on the ID-free structural fingerprint.
- `ColumnDef` and `FieldMetadata` gained `api_name`, `is_sortable`, and
  `is_date_time` fields (with const builders), emitted by the derives so
  public GraphQL names, sortability, and date-time semantics are recorded in
  metadata. Existing backup hashing, schema planning, and generated GraphQL
  behavior are unchanged.
- Hand-written `ColumnDef`/`FieldMetadata` struct literals must add the new
  fields or use the const builders; see MIGRATION.md.
- Fixed `Option<Vec<u8>>` logical type inference: nullable byte columns now
  carry `BackupValueKind::Bytes` instead of falling through to `Json`. Storage
  DDL was already BYTEA/BLOB; logical backup descriptors and stable schema
  hashes change for affected entities (see MIGRATION.md).
- The IR fails closed: Serde deserialization enforces stable-ID validity and
  rejects unknown properties; validation proves foreign-key target uniqueness,
  default/value-kind compatibility, global stable-ID uniqueness, and duplicate
  key members; canonical rendering escapes literal defaults. The ID-free
  fingerprint is named `structural_fingerprint` and conversion reports policy,
  backup, redaction, ownership, and propagation semantics as unsupported
  rather than dropping them.
## 0.7.1

Companion macros crate: `graphql-orm-macros` remains **0.7.0**.

- Fixed backend dependency isolation so a SQLite-only build activates
  `sqlx-sqlite` but not `sqlx-postgres`, a PostgreSQL-only build activates
  `sqlx-postgres` but not `sqlx-sqlite`, and an MSSQL-only build activates
  neither SQLx database driver.
- SQLite now uses SQLx's Tokio runtime without an unused SQLx TLS stack;
  PostgreSQL retains Tokio plus Rustls. Combined SQLite/PostgreSQL builds still
  activate both drivers.
- No public API, generated code, schema, migration, authorization, repository,
  transaction, backup, GraphQL, or naming behavior changed. No data migration
  is required.

## 0.7.0

Companion macros crate: `graphql-orm-macros` **0.7.0**.

- Added dependency-owned `OrmSchemaModule` composition with stable module ID,
  semantic version, reserved table namespace, schema fingerprint, migration
  target, backup descriptors, and declared restore phases.
- Added module-aware schema/backup snapshots and fail-closed validation for
  duplicate ownership, overlapping namespaces, invalid or duplicate restore
  hooks, and source-controlled fingerprint drift.
- Added backend-neutral fenced lease state, proof bindings, monotonic fencing,
  CAS row versions, heartbeats, fenced child writes, release, and reclaim
  contracts. Failed transitions leave the in-memory state unchanged.
- Added validated `first`/`after` and `last`/`before` keyset windows, portable
  before-cursor SQL predicates, and generated SQLite/PostgreSQL repository and
  transaction helpers that restore backward reads to canonical order.
- Aligned the optional `auth-agql` bridge with `agql-auth` 0.10.0 at exact
  revision `c92dcb441237bbe308499b26525945f60ffa394a` while preserving the existing
  principal/session-assurance mapping.
- Existing GraphQL fields, CRUD behavior, offset pagination, authorization,
  and database schemas are unchanged. The new APIs are opt-in and create no
  automatic data migration.

## 0.6.3

Companion macros crate: `graphql-orm-macros` **0.6.1**.

- `schema_roots!` retains the public Rust root names while exporting their GraphQL object names as
  the conventional `Query`, `Mutation`, and `Subscription`, making async-graphql federation SDL
  unambiguously composable without downstream rewriting.
- Schemas with no subscription contributors now use `EmptySubscription`; they do not emit a fake
  empty object or a dangling operation root.
- Added parsed federation-SDL coverage for complete, zero-subscription, read-only MSSQL, and
  multi-chunk query schemas, including PascalCase resolver naming.
- No repository, authorization, transaction, backup, migration, or database behavior changed.

## 0.6.2

Companion macros crate: `graphql-orm-macros` remains **0.6.0**.

- Aligned the optional Git-only `auth-agql` bridge with `agql-auth` 0.8.1 at exact revision
  `f1fb5fe8c42d29806821d5f1a9032b007dee63e4`, so hosts using the bridge and a direct dependency
  resolve one `agql-auth` type universe.
- No bridge API, authorization behavior, persistence behavior, or generated code changed.

## 0.6.1

Companion macros crate: `graphql-orm-macros` remains **0.6.0**.

- Fixed PostgreSQL logical-backup restores so null values bind with the column's declared type,
  including JSONB, UUID, byte, numeric, and boolean columns, instead of falling back to text.
- Added dependency-aware ordering for rows with self-referential foreign keys so parent rows are
  inserted before their children during empty-database restores.
- Self-reference cycles and references to rows missing from the backup now fail with explicit
  protocol errors before the table transaction commits.
- Added PostgreSQL nullable-JSON round-trip coverage and focused child-before-parent restore-order
  coverage.

## 0.6.0

Companion macros crate: `graphql-orm-macros` **0.6.0**.

- Updated the optional Git-only `auth-agql` bridge to `agql-auth` 0.8.0 at exact revision
  `be4e0a213ce9c9b9fbe9fe985602743a584e019b` and preserved authoritative session assurance,
  organization, correlation, actor, active-scope, and policy metadata.
- Added opt-in repository-only composite-key mutations with generated ordered key/create/update
  types, complete-key CRUD, insert-if-absent, private upsert, and transaction-bound equivalents.
- Added atomic complete-key plus typed-predicate updates with distinct not-found, predicate-conflict,
  and updated outcomes.
- Added explicit `MutationLimit` and no-partial-write bounded update/delete outcomes for single and
  composite key entities.
- New composite mutation SQL dialect-quotes identifiers, binds values, validates exact affected-row
  counts, and preserves policies, transforms, hooks, search, events, rollback, and PostgreSQL RLS.
- Opted-in composite writes require an explicit `EntityPolicy` provider even in legacy mode; the
  new mutation surface is never default-allow.

## 0.5.0

Companion macros crate: `graphql-orm-macros` **0.5.0**.

- Added private entity-level identifier-based `projection(...)` declarations that generate exact typed DTOs and
  select only their declared columns on SQLite and PostgreSQL.
- Added bounded typed repository queries, primary/unique lookup helpers, auth-aware reads, and
  transaction-bound `MutationContext::project` queries with own-write visibility.
- Projection reads preserve entity authorization and PostgreSQL RLS. Application row policies and
  residual in-memory filters fail closed because evaluating them would require a full entity.
- Added `sensitive` field metadata and redacting projection `Debug` implementations. Projections are
  never exposed through GraphQL.

## 0.4.3

Companion macros crate: `graphql-orm-macros` **0.4.3**.

- Conditional-index introspection now accepts only the complete portable closed-set grammar;
  leading/trailing boolean expressions, comments, functions, casts outside PostgreSQL's generated
  text literals, and other tokens are drift.
- SQLite append-only introspection validates both complete generated trigger definitions rather
  than trusting managed names.
- PostgreSQL append-only introspection validates the exact trigger event/timing/enablement,
  unconditional function body, ownership, language, security-definer, search-path, and privilege
  posture.

## 0.4.2

Companion macros crate: `graphql-orm-macros` **0.4.2**.

- Migration-history preparation now transactionally adopts the recognized legacy
  `(version, applied_at)` table on SQLite and PostgreSQL.
- Legacy rows retain their version and timestamp and receive the deterministic description
  `Legacy migration <version>`; current optional metadata remains unknown (`NULL`).
- Existing tables with ambiguous columns, types, nullability, or primary-key identity fail closed.

## 0.4.1

Companion macros crate: `graphql-orm-macros` **0.4.1**.

- Added raw `Vec<u8>` primary-key support across repository/transaction CRUD, CAS, exact filters,
  hooks, row policies, and keyset cursors on SQLite `BLOB` and PostgreSQL `BYTEA`.
- Repository and `MutationContext` upserts may now target host-supplied private keys. When the
  conflict target is absent from the public create input, the GraphQL upsert field is omitted.
- Added structural `conditional_index(...)` metadata for portable closed-set partial indexes,
  including stable hashes, quoted DDL, SQLite/PostgreSQL introspection, and drift recreation.
- Added `gt_field`, `lte_field`, and `lt_field` portable comparisons alongside `gte_field`.
- PostgreSQL managed-schema comparison now canonicalizes harmless SQL type-name case differences.

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
