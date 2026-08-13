---
title: "Changelog"
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-13
review_by: 2027-02-01
supersedes: []
---

# Changelog

This file is the authoritative user-facing release chronology. The former
[release-notes ledger](docs/archive/2026/graphql-orm-release-notes.md) is retained
for historical context.

## 0.22.1 - 2026-08-13

Companion macros crate: `graphql-orm-macros` **0.22.1**.

- Fixed generated to-many relationship semantic metadata so its nullable
  `OrderBy` object exactly matches the relationship resolver SDL. Generated
  root list queries retain their existing list-valued `OrderBy` contract.
- Relationship resolver signatures and semantic argument descriptors now use
  one macro-owned contract for public names, nullability, `Where`, `OrderBy`,
  and `Page` types, preventing independent drift across case conventions and
  backends.
- Added a complete PascalCase schema/catalogue/capability regression covering
  single-key, composite-key, one-to-one, nullable and to-many relationships.
- Optional relationship-key extraction now emits the idiomatic `?` form so
  generated consumers remain warnings-clean under current Clippy.

## 0.22.0 - 2026-08-13

Companion macros crate: `graphql-orm-macros` **0.22.0**. Backend-neutral
semantic owner: `graphql-orm-operation-catalog` **0.2.0**.

- Added a versioned, strict `GraphqlSemanticCatalog` containing public entity,
  field, relationship, argument, type, capability, classification, export, and
  generated/custom root-operation semantics. Canonical JSON fingerprints and
  the optional router descriptor extension bind the complete descriptive graph.
- `GraphQLEntity` now emits selectable/filterable/sortable/groupable/aggregate
  facts, typed relationship cardinality and bounds, inherited classifications,
  field-policy presence, and structural non-exportability. Private and
  `read = false` fields remain absent; `sensitive` fields become `Secret` and
  `NeverExport`.
- Generated root and object documentation now use the same bounded semantic
  descriptions. `#[graphql_orm_custom_operations]` publishes equivalent
  metadata beside handwritten resolvers, and `schema_roots!` can compose those
  declarations through `semantic_custom_operations`. Handwritten result
  objects derive `GraphQLSemanticObject` and compose through `semantic_types`.
- `schema_roots!` now offers single-source `described_*_types` lists that
  compose a handwritten root together with its operation and direct result
  semantics. Custom scalar/enum results carry explicit fingerprinted
  classification/export metadata; unclassified leaves default to
  `Secret`/`NeverExport`, and exportable scalar lists require a positive bound.
- Generated aggregate operation descriptors bind their owning public entity.
  AI aggregate disclosure and runtime validation use only the exact selected
  grouping fields and metric field/operator pairs, preventing unrelated or
  drifted fields from affecting or weakening provider egress.
- Subscription semantics truthfully distinguish best-effort delivery from a
  bounded replay-then-live declaration. Generated broadcast subscriptions
  remain best-effort; a replayable custom declaration still requires a
  separately registered authoritative runtime source.
- Added canonical aggregate semantic enums and the opt-in `Aggregate`
  operation category for the typed aggregate implementation to consume.
- Added the canonical `AiMutationExecutionPolicy` semantic classification.
  Generated `ai_mutations(...)` category declarations and handwritten
  `graphql_orm_custom_operations(ai_execution = "...")` declarations select
  `Automatic`, `ApprovalRequired`, or the default `Prohibited` state. The
  classification participates in semantic fingerprints but grants no runtime
  authority.
- Added database-executed typed grouped aggregates with multiple `COUNT`,
  `MIN`, `MAX`, and `SUM` metrics, deterministic nullable-key ordering, bounded
  result groups, generated-filter and field-policy checks, and portable exact
  Decimal values. `aggregate = true` opts an entity into a generated GraphQL
  aggregate root and operation-catalog entry; existing schemas gain no root by
  default.
- Refactored write and transaction capabilities away from SQLx-specific
  traits. SQL Server now supports deliberate `ExternalWritable` entity DML
  through native Tiberius connections while every compatibility constructor
  remains physically read-only. The applicable generated surface includes
  scalar/composite insert, update, delete, upsert, bounded mutation,
  insert-if-absent, versioned compare-and-swap, bulk helpers, hooks, events,
  subscriptions, commit and rollback.
- SQL Server upserts use a locked update/insert transaction rather than
  `MERGE`; transactions discard a Tiberius client after cancellation or an
  indeterminate error. Fixed-decimal and JSON values have native validated
  MSSQL bind/decode paths.
- Closed a generated-write authorization race across SQLite, PostgreSQL, and
  SQL Server. Authoritative row lookup, row policy, input transformation,
  before hooks, and exact-key DML now share one pinned mutation transaction;
  predicate writes cannot capture newly matching rows after authorization.
  Top-level upserts use state-machine isolation, while caller-owned default
  transactions fail closed before an unfenced upsert.
- Logical backups represent Decimal columns as exact validated Decimal values,
  including precision and scale. SQLite scaled integers and PostgreSQL native
  numerics now share one canonical backup form, and restore rejects changed
  definitions or lossy values.
- Added local, individually selectable provider and owned-database acceptance
  runners, complete workspace package/SemVer release-policy coverage, and a
  deterministic release-manifest row for semantic-catalogue wire version 1.
  The release BOM also records the canonical automatic query, mutation, and
  subscription capability contract versions in stable name order.
  The coordinated compile fixture consumes semantic, grouped-aggregate, and
  deliberate MSSQL writable contracts without a downstream application.

The semantic catalogue is discovery and disclosure-shape metadata only. It
does not authorize resolvers, fields, rows, provider egress, or database work.
There is no database, stored-data, or migration-history change. Decimal-aware
logical backup descriptors and schema hashes are new in this release. SDL
descriptions and semantic/catalogue fingerprints change where new generated
documentation participates. MSSQL write adoption is an explicit application
configuration change against an externally managed schema; the ORM still does
not create or migrate SQL Server tables.

## 0.21.1 - 2026-08-13

Companion macros crate: `graphql-orm-macros` **0.21.1** under the aligned
Git-only version policy. The generated macro contract is unchanged.

- Updated the one-way external `agql-auth` integration to 0.15.0 at exact
  revision `e841ffd382082ad7419be259fe957f949b956ff7`.
- The existing ORM principal, scope, assurance, actor, tenant, and database
  authorization projections remain unchanged. Hosts can now share the same
  auth type universe with applications using the reusable session-bound,
  access-token-only delegation API.
- Backend-coexistence coverage proves the SQLite AI runtime, MSSQL application
  schema, optional ORM auth bridge, and direct auth consumer resolve exactly
  one reviewed `agql-auth` source.

No database, GraphQL SDL, generated-code, migration-history, backup, or stored
data migration is required.

## 0.21.0 - 2026-08-11

Companion macros crate: `graphql-orm-macros` **0.21.0**. New independently
consumable packages: `graphql-orm-operation-catalog` **0.1.0** and
`graphql-orm-ai-tool-profiles` **0.1.0**.

- Generated resolver-operation metadata now has a backend-neutral canonical
  owner and remains re-exported unchanged from `graphql-orm`.
- Reviewed AI GraphQL profile compilation and manifest wire contracts now have
  a database-neutral package. MSSQL application subgraphs can publish exact
  manifests in the same Cargo invocation as a SQLite AI runtime without
  selecting an AI persistence backend or duplicating serialized types.
- `#[backend_selected_graphql_entity(...)]` lets a reusable companion crate
  bind derives to its own mutually exclusive host backend feature when Cargo
  has unified other `graphql-orm` backend features elsewhere in the workspace.

Existing ORM imports and operation fingerprints remain source/wire compatible.
There is no database, GraphQL SDL, migration-history, backup, or stored-data
migration.

## 0.20.0 - 2026-08-11

Companion macros crate: `graphql-orm-macros` **0.20.0** under the aligned
Git-only version policy. This additive pre-1.0 minor exposes semantic entity
metadata and aligns the optional router protocol dependency to 0.2.0.

- `#[graphql_entity(description = "...")]` and
  `#[graphql_orm(description = "...")]` now emit bounded public semantic
  descriptions through `Entity::graphql_semantic_metadata`. The projection
  contains public GraphQL field identities and explicit relationship shape,
  but no physical table/column names or authorization policy. It remains
  discovery/documentation metadata and grants no field, resolver, row, or AI
  authority.
- Handwritten `Entity` implementations remain source compatible through the
  default `None` semantic-metadata method.
- The optional `router-protocol` adapter now resolves
  `graphql-orm-router-protocol` 0.2.0. Existing protocol v1 operation exports
  are unchanged.

No database, GraphQL SDL, migration-history, backup, or stored-data migration
is required. Description changes do not alter physical schema hashes.

## 0.19.0 - 2026-08-10

Companion macros crate: `graphql-orm-macros` **0.19.0** under the aligned
Git-only version policy. This release changes public physical-schema model
types and generated migration metadata, so it is a pre-1.0 minor release.

- Added ordered compound physical foreign keys. Relation lowering now retains
  every `from`/`to` pair, translates source Rust fields through `db_column`,
  validates the exact referenced primary or unique key, and renders one
  compound constraint with its configured delete policy.
- SQLite introspection now groups `PRAGMA foreign_key_list` members by
  constraint ID and sequence. PostgreSQL introspection pairs `conkey` and
  `confkey` members by ordinal through `pg_constraint`. Both backends therefore
  round-trip compound foreign keys without flattening them.
- Added stable, optional ordinary-index names and typed per-column `ASC`/`DESC`
  metadata. SQLite `index_xinfo` and PostgreSQL `indoption` introspection retain
  direction, and planner drift recreates an index whose order differs.
- Added exact numeric `min_exclusive` and `max_exclusive` field checks and
  `default = false` for suppressing the conventional implicit timestamp
  default when adopting an existing column without one.
- Added conservative semantic comparison for supported simple check
  expressions. Physical constraint names, whitespace, identifier quoting, and
  redundant outer parentheses do not force DDL, while changed or unrecognized
  expressions remain drift.
- Existing semantically equivalent SQLite layouts can now be recorded by the
  ordinary empty-plan migration-history path. There is no generic adoption
  override: partial, reordered, differently targeted, weakened, or ambiguous
  foreign keys/checks remain migration work or fail closed.
- Physical-schema validation now rejects malformed index-direction arity and
  foreign keys whose members are absent, duplicated, type-incompatible, or do
  not reference an exact managed unique key.

No stored row rewrite is required. Fresh SQLite and PostgreSQL schemas use the
new metadata directly; compatible existing SQLite schemas are adopted only
after live introspection proves semantic equality. Schema hashes change where
check, foreign-key, or index-order metadata participates. See the
[0.19.0 migration guide](MIGRATION.md#0190-compound-foreign-keys-and-directional-indexes)
for the source migration from public struct literals.

## graphql-orm-router 0.1.1 - 2026-08-07

This patch fixes variable-backed argument scope templates on authenticated
HTTP and `graphql-transport-ws` operations. Subscription authorization now
uses the variables from the individual operation and rejects a missing
rendered scope before opening a subgraph connection. Client attempts to supply
router-reserved internal metadata are overwritten. No configuration, protocol,
schema, public API, or stored-data migration is required; subgraph resolver
guards remain authoritative.

## 0.18.0 - 2026-08-07

Companion macros crate: `graphql-orm-macros` **0.18.0** under the aligned
Git-only version policy. The new `graphql-orm-router-protocol` and
`graphql-orm-router` packages begin at **0.1.0**. This release adds public ORM
metadata, macro declaration/generated-code behavior, and optional router
integration, so it is a pre-1.0 minor release. No database or stored-data
migration is required.

- Updated the one-way external `agql-auth` integration to 0.14.0 at exact
  revision `413fda3435f060604cd653c11e2cc18a668aace1`. Its validator now
  normalizes standard OAuth `scope` and bounded legacy `scopes` claims into the
  existing principal scope vector; the ORM bridge API and stored data are
  unchanged. Direct JWT decoders must follow the upstream rolling migration.
- Hardened `graphql-orm-router` for standalone operation with strict JSON and
  environment-only secrets, pre-bind checks, public/downstream deadlines,
  connection and graceful-drain budgets, structured telemetry, authenticated
  core metrics, optional Prometheus execution/subscription metrics, signal
  handling, operator/schema/reconnect/threat-model guidance, migration notes,
  release-policy lanes, and an executable HTTP/WebSocket/shutdown smoke test.
- Added identity-bound dynamic registration and a separately bound,
  scope-protected administrative surface with deny-by-default SSRF policy,
  safe status/metrics, explicit refresh/removal, request limits, and documented
  process-local restart semantics.
- Added conditional schema and authorization-metadata polling to
  `graphql-orm-router`, with canonical no-op fingerprints, bounded retry,
  serialized complete-candidate admission, exact executable last-known-good
  retention, process-local refresh/removal/status APIs, and atomic
  graph-plus-policy replacement across HTTP and WebSocket work.

- Added authenticated `graphql-transport-ws` serving to
  `graphql-orm-router` on the public GraphQL path. Connection-init credentials
  are verified before acknowledgement, operations reuse graph-bound scope
  policy, the approved token is propagated to an upstream WebSocket subgraph,
  and connections close at expiry without in-place refresh. Connections,
  operations, client messages, upstream buffers, and ephemeral fan-out are
  bounded; no events are persisted or replayed.

- Added fail-closed HTTP authentication and advisory authorization to
  `graphql-orm-router`: bounded rotating RS256 JWKS validation, explicit
  standard/legacy scope migration, graph-bound operation metadata, fixed and
  scalar-templated scope preflight, approved bearer propagation, and an
  optional one-way exact-pinned `agql-auth` validator/matcher adapter. The
  router remains resource-server-only and subgraph guards remain authoritative.
- Added a public `graphql-orm-router` static HTTP graph with validated
  fail-closed configuration, bounded credentialed SDL retrieval, atomic graph
  preparation and identity, configurable GraphQL path, downstream-header
  allowlisting, and liveness/readiness endpoints. Anonymous access requires an
  explicit development-only opt-in.
- Added repeatable authorization declarations for every generated operation
  category, including fixed `all_scopes` and any-of/all-of requirements plus
  argument-dependent `all_scope_templates` and `any_scope_templates`.
  Declarations drive server-side resolver enforcement, protocol metadata, and
  native Federation scope metadata where representable. Invalid categories,
  missing generated operations, malformed placeholders, unknown arguments,
  and unsupported complex substitutions fail at compile time.
- Versioned generated-operation authorization fingerprints as v2 so templated
  policies bind the GraphQL type and requiredness of referenced arguments
  without changing the established discovery fingerprint.
- Generated operations with entity-level `auth = "required"` now emit the
  standard namespaced Federation `@federation__authenticated` directive. The
  router structurally restores the composed `authenticated` and
  `requiresScopes` SECURITY links required by Hive enforcement.
- Added deterministic authorization and router-export fingerprints without
  changing the existing generated-operation discovery fingerprint.
- Added optional `router-protocol` export support. Ordinary `graphql-orm`
  consumers do not resolve the protocol package unless the feature is enabled.
- Applied the existing authentication, assurance, and entity read-policy
  guards consistently to append-only generated subscriptions.
- Added integration evidence that generated subscription events are released
  only after commit and are discarded when commit rolls back.

## 0.17.0

Companion macros crate: `graphql-orm-macros` **0.17.0** under the aligned
Git-only version policy. This additive public runtime and generated-code
surface is a pre-1.0 minor release.

- Added provider-neutral operation assurance registries for generated and
  custom root fields, configurable interactive-mutation defaults, explicit
  interactive/machine/service/safety-teardown actor classes, requirements,
  and documented exemptions.
- Added strict completeness audits that fail when an exposed mutation has
  neither a requirement nor exemption. Compatibility mode remains the default;
  queries and subscriptions receive no assurance default.
- Added provider-neutral schema directive definitions/metadata and a
  deterministic advisory client manifest containing exact field identity,
  policy ID, actor class, custom/generated origin, and exemption reason.
- Generated resolvers now call a generic assurance enforcement hook before
  database work. The hook is a compatibility no-op until a schema installs
  `AssuranceEnforcement`; `DeclaredAssuranceGuard` gives custom fields the same
  integration.
- Extended the optional one-way `auth-agql` bridge with
  `AgqlAssuranceEvaluator`, pinned to upstream revision
  `d6b9cef663d52125c52f3fb90d4155ee25d34775`. It evaluates the upstream
  `AssuranceRequirement` with the current user and injected clock, then emits
  lowercase GraphQL extension key `code` with `STEP_UP_REQUIRED`,
  `UNAUTHENTICATED`, or `FORBIDDEN`.
- Added generated/custom mutation, machine principal, safety exemption,
  strict audit, manifest determinism, and stable error-code coverage.

Existing resolver names, SDL, database schema/data, authorization/RLS, and
runtime behavior remain unchanged until assurance enforcement is explicitly
installed. The manifest is advisory; server enforcement is authoritative.

## 0.16.0

Companion macros crate: `graphql-orm-macros` **0.16.0** under the aligned
Git-only version policy. This additive public runtime and generated-code
surface is a pre-1.0 minor release.

- Added `GraphqlOperationMetadata` and immutable generated descriptors for
  every query, mutation, and subscription resolver actually emitted by
  `GraphQLOperations`, including exact case-adjusted root field/argument names,
  stable categories, Rust/GraphQL input/result signatures, backend/entity
  identity, and derive-owned schema declarations.
- Added `graphql_orm_operation_catalog()` generation to `schema_roots!`.
  Catalog descriptors distinguish generated mutations omitted by
  `generated_mutations` none/allowlist/denylist policy from operations actually
  merged into the public root. Root-level read-only policy also resolves
  generated subscriptions as unexposed.
- Added domain-separated, length-framed SHA-256 generated/resolved/catalog
  fingerprints with deterministic ordering and documented compatibility
  limits. Fingerprints detect generated surface and exposure drift; they do
  not authorize execution or bind custom roots, complete host SDL, documents,
  result projections, disclosure policy, runtime pagination, or current
  resolver/RLS decisions.
- Added SQLite/PostgreSQL/MSSQL and default/PascalCase coverage for renamed
  plurals, composite keys, list/single/search/keyset operations, every mutation
  category, subscriptions, read-only and append-only profiles, mutation
  allowlists, hidden JSON fields, private read projections, and fingerprint
  drift.

Existing resolver names, GraphQL SDL, database schema, stored data, repository
APIs, authorization, and RLS behavior are unchanged. No database or data
migration is required.

## 0.15.0

Companion macros crate: `graphql-orm-macros` **0.15.0** under the aligned
Git-only version policy. This is a pre-1.0 minor because generated bounded
mutation behavior changes.

- Fixed generated single-key and repository-only composite-key bounded update
  and delete so their exact `MutationLimit + 1` look-ahead is no longer
  clamped by the public 100-row pagination maximum.
- Applied the same exact internal sentinel selection to host-only retention
  purge. The path orders by the complete primary key and is reachable only
  through generated bounded mutation/retention execution; ordinary GraphQL,
  connection, repository, and runtime reads retain their existing caps.
- Bounded mutations now reject residual/in-memory predicates before selection,
  hooks, events, notifications, or writes instead of risking an unbounded
  candidate scan. Database-renderable filters retain exact all-or-nothing
  outcomes at ceilings above 100.
- Added checked sentinel/count arithmetic and fail-closed selected-versus-
  affected cardinality checks. Any intervening cardinality change rolls back
  the complete transaction and releases no queued events.
- Preserved the optional one-way agql-auth 0.12.0 bridge at exact revision
  `3f3b0c5365adfbe436514a681d977b600991b797` and its single-type-universe
  requirement.

No database schema or stored-data migration is required.

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
[portable persistence primitives](docs/architecture/portable-persistence.md).

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

See the 0.2.21 section in the [historical release-notes ledger](docs/archive/2026/graphql-orm-release-notes.md#0221) and the migration
guide in [MIGRATION.md](MIGRATION.md).
