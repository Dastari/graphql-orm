# Release Notes

This page records user-facing changes for recent `graphql-orm` releases. Version numbers refer to
the runtime crate unless a macro crate version is called out separately.

## 0.3.0

Security hardening release for multi-tenant, authorization-sensitive services.

- Bumped `graphql-orm` to `0.3.0`.
- Bumped `graphql-orm-macros` to `0.3.23` for aligned SQLite/Postgres epoch
  default generation and runtime expression helpers (backward-compatible
  correctness fix; do not continue shipping macros as `0.3.22`).
- Added `AuthorizationMode` (`LegacyPermissive`, `DeclaredPoliciesRequired`,
  `ExplicitPolicyForAllExposedOperations`) on `Database`.
- Expanded `AuthSubject` with optional `user_id`, `claims`, `token_id`,
  `session_id`, and `actor_id`; redacted `Debug`.
- Expanded `DbAuthContext` with token/session/actor/policy-version fields and
  claim-fingerprint cache keys.
- Added `AccessContext` / `SystemAccess`.
- Added `OrmPublicError` / `OrmErrorCode` safe public error contract.
- Added structural tenant/owner helpers (`structural_auth`).
- Added `FilterExpression::TrustedFragment`.
- Enabled optional `auth-agql` bridge against upstream `agql-auth` 0.7.0
  (`rev = 5e7f230b96350f55496477c11f8a0505e6438779`, tag `v0.7.0`) without path
  or `[patch]` overrides.
- Changed pagination defaults from 1000/1000 to 50/100; restore with
  `PaginationConfig::legacy()`.
- Event sender locks no longer panic on poisoning.
- Fixed SQLite managed-schema replan idempotency for generated timestamp
  defaults: `unixepoch()` and `(unixepoch())` (and similar redundant outer
  parentheses) compare equal after file reopen, so `additive_only` startups no
  longer fail with a false `AlterColumn` on `created_at`/`updated_at`.

Compatibility notes:

- Behavioral/security: GraphQL auth/policy denials expose stable codes and safe
  messages. Callers that parsed raw SQL/error strings must migrate.
- Behavioral/security: pagination defaults are smaller. Use `legacy()` if a
  service still needs 1000-row pages.
- Behavioral: authorization mode default remains permissive for migration;
  production should set `DeclaredPoliciesRequired`.
- Structural: `DbAuthContext` struct literals need new optional fields (or use
  constructors).

## 0.2.21

Auth bridge release for generated resolvers and policy hooks.

- Bumped `graphql-orm` to `0.2.21`.
- Bumped `graphql-orm-macros` to `0.3.22`.
- Added `AuthSubject { id, roles, scopes, tenant_id }` as the project-agnostic auth principal shape
  understood by `graphql-orm`.
- Added `AuthExt::auth_user_id()`, `AuthExt::auth_subject()`, and
  `AuthExt::auth_subject_opt()`. `AuthExt::auth_user()` remains available as a deprecated alias for
  source compatibility.
- `AuthExt` now prefers an `AuthSubject` in the async-graphql context, then falls back to the legacy
  `String` user id and upgrades it to an empty-role/empty-scope subject.
- Added `ScopeEntityPolicy`, an exact-scope `EntityPolicy` helper with separate read and write scope
  lists and `require_auth` handling.
- Added `DbAuthContext::from_subject`, `DbAuthContext::from_parts`, and
  `DbAuthContext::from_context_parts` helpers for request-local PostgreSQL RLS settings.
- Added generated resolver auth modes:
  - entity-level `#[graphql_entity(auth = "required" | "optional" | "none")]`
  - schema-root-level `schema_roots! { auth: "required" | "optional" | "none", ... }`
- Generated query, mutation, subscription, and relation resolvers now route auth through
  `AuthSubject`-aware enforcement instead of discarding `ctx.auth_user()?`.
- Added a reserved optional `auth-agql` feature that compiles without pulling in `agql-auth`. The
  concrete `agql-auth` converters remain deferred until the upstream `agql-auth` 0.7 API is tagged.

Compatibility notes:

- Structural/source-facing: code that names `ctx.auth_user()` will compile with a deprecation warning;
  migrate to `ctx.auth_user_id()` when only the id is needed or `ctx.auth_subject()` when roles,
  scopes, or tenant id are needed.
- Structural/source-facing: any downstream custom implementation of `AuthExt` must implement
  `auth_user_id`, `auth_subject`, and `auth_subject_opt`.
- Structural/source-facing: `schema_roots!` and `#[graphql_entity]` now validate the optional `auth`
  mode string when present.
- Behavioral: the generated resolver default remains fail-closed for compatibility with prior
  generated `ctx.auth_user()?` behavior. Use `auth = "none"` for public generated resolvers, or
  `auth = "optional"` when entity/row policies should decide without a hard auth precheck.
- Behavioral: `ScopeEntityPolicy` uses exact string matching only. Scope hierarchies, wildcards, and
  product-specific bypasses are intentionally outside the base ORM policy helper.

## 0.2.20

SQLX-free application boundary and service bulk helpers.

- Bumped `graphql-orm` to `0.2.20`.
- Bumped `graphql-orm-macros` to `0.3.21`.
- Added public `graphql_orm::Error` and `graphql_orm::Result<T>` aliases. Generated repository
  helpers and runtime query/search/schema APIs now use these aliases in public signatures so
  downstream app crates do not need SQLX in normal public API types.
- Added `Database::<SqliteBackend>::connect_sqlite(...)`,
  `Database::<PostgresBackend>::connect_postgres(...)`, and
  `Database::<MssqlBackend>::connect_ado(...)` constructors.
- Added `ConnectionOptions` for ORM-owned connection helpers, currently covering `max_connections`.
  SQLite in-memory URLs default to one connection so schema/data remain visible across ORM calls.
- Added generated no-pool read helpers: `find_all`, `find_many`, `count_all`, and `count`.
- Added generated transactional bulk write helpers for write-capable entities: `insert_many`,
  explicit `delete_all`, and `replace_all`.
- Added generated `upsert_many` for entities configured with `#[graphql_entity(upsert = "...")]`.
- Kept `delete_where` safe by continuing to reject empty filters; table-wide deletion now requires
  explicit `delete_all`.
- Added `search_db(&database, SearchInput)` for searchable entities so search helpers can be used
  without naming the raw pool type.
- Added `PlanOptions` and `plan_*_with_options(...)` schema APIs. `PlanOptions::managed_tables_only()`
  ignores live tables outside the target schema for shared databases where graphql-orm owns only a
  subset of tables.
- Added `ApplyOptions::additive_only`; when enabled, migration application rejects any non-additive
  step even if it is not classified as destructive.
- Documented raw SQLX pools as compatibility/advanced escape hatches rather than the default app
  integration path.

Breaking/source-facing notes:

- Public generated helper return types changed from `graphql_orm::sqlx::Error` to
  `graphql_orm::Error` via the new `graphql_orm::Result<T>` alias. The alias currently points to
  SQLX's error type, so most `?` usage remains compatible, but downstream public signatures should
  be updated to name `graphql_orm::Result<T>`.
- `ApplyOptions` gained the `additive_only` field. Struct literals must add the field or use
  `..Default::default()`.

## 0.2.19

Full-text search JSON path support.

- Bumped `graphql-orm` to `0.2.19`.
- Bumped `graphql-orm-macros` to `0.3.20`.
- Added `#[graphql_orm(search_json(path = "...", weight = "..."))]` for persisted
  `#[graphql_orm(json)]` fields.
- JSON search paths are extracted in Rust into the existing denormalized `SearchDocument`, so
  Postgres and SQLite continue to use the current managed search storage without app-specific SQL.
- Supported portable path forms are `$.field`, `$.nested.field`, `$.array[*].field`, and
  `$[*].field`.
- Missing paths, nulls, non-string scalars, empty wildcard matches, and invalid runtime values
  contribute empty text rather than failing writes or rebuilds.
- Search schema metadata, hashes, migrations, rebuild helpers, and generated GraphQL/Rust search
  resolvers now include configured JSON path chunks.

## 0.2.18

Generated mutation exposure controls.

- Bumped `graphql-orm` to `0.2.18`.
- Bumped `graphql-orm-macros` to `0.3.19`.
- Added `generated_mutations: "all" | "none" | "allowlist" | "denylist"` to
  `schema_roots!`.
- Added `generated_mutation_allowlist: [Entity]` and
  `generated_mutation_denylist: [Entity]` for mixed public mutation exposure.
- Kept generated repository writes, write inputs, mutation hooks, mutation contexts, and
  subscriptions generated regardless of the public mutation exposure mode.
- Kept `extra_mutation_types` available when generated mutations are hidden, so applications can
  expose only intentional custom mutations.
- Added compile-time validation for invalid modes, missing allow/deny lists, mismatched list modes,
  and allow/deny entries that are not present in `entities`.
- Allowed `query_custom_ops` to be omitted from `schema_roots!`; it now defaults to an empty list.

## 0.2.17

Pagination compatibility follow-up.

- Bumped `graphql-orm` to `0.2.17`.
- Bumped `graphql-orm-macros` to `0.3.18`.
- Deprecated `PageInput::limit()` because it clamps with the default pagination cap and cannot see
  per-`Database` `PaginationConfig` overrides.
- Documented `PageInput::limit_with_config(...)` and `PaginationConfig::resolve_page(...)` as the
  correct host-code APIs for direct `PageInput` handling.
- Updated the full-stack fixture to route page limits through the provider pagination config.
- Added regression coverage showing raised max limits, such as `5_000`, are honored by configured
  pagination paths.

## 0.2.16

Pagination configuration follow-up for the audit release.

- Bumped `graphql-orm` to `0.2.16`.
- Bumped `graphql-orm-macros` to `0.3.17`.
- Added runtime `PaginationConfig` with `default_limit` and `max_limit`.
- Added `Database::builder(...).pagination_config(...)`, `.default_page_limit(...)`,
  `.max_page_limit(...)`, and `.unbounded_pagination()`.
- Generated GraphQL list, search, and relation connections now apply a default limit of `1000` when
  `page.limit` is omitted.
- Applications can raise, lower, disable, or fully unbound pagination defaults/caps per `Database`
  handle.
- Repository-style `fetch_all` paths remain intentionally unbounded unless the caller supplies
  pagination.

## 0.2.15

Audit follow-up release focused on correctness and native execution paths.

- Bumped `graphql-orm` to `0.2.15`.
- Bumped `graphql-orm-macros` to `0.3.16`.
- Fixed field-level write policy handling so denied optional create fields are omitted and denied
  update fields are skipped instead of hard-failing the whole mutation. Required create fields still
  fail when denied because the ORM cannot safely synthesize a value.
- Fixed relative date filter SQL to use the selected backend dialect instead of Cargo feature
  detection in multi-backend workspaces.
- Escaped `%`, `_`, and `\` in generated `LIKE`/`ILIKE` patterns and added SQL `ESCAPE '\\'`
  clauses so wildcard characters in user input are treated literally.
- Clamped negative and very large page limits at SQL rendering and `PageInput` handling.
- Reduced fallback connection pagination from two full scans to one filtered scan plus in-memory
  slicing.
- Added residual-filter prefiltering so fallback paths, such as SQLite spatial predicates, still
  push safe SQL predicates before exact in-memory checks.
- Added native full-text search execution under `DbAuthContext`, so PostgreSQL RLS/authenticated
  requests can still use native search indexes.
- Added native full-text search pagination and native count queries for generated search
  connections.
- Added windowed relation pagination for paged relation batches using `ROW_NUMBER() OVER
  (PARTITION BY ...)` plus grouped counts.
- Batched bulk-update after-hook refetches into one `WHERE pk IN (...)` query.
- Collapsed PostgreSQL auth context setup into one transaction-local `set_config` statement.
- Improved placeholder normalization so placeholder-looking text inside SQL string literals is not
  rewritten.
- Normalized SQLite float relation-key projections with `printf` to avoid silent relation misses.

## 0.2.14

Documentation and release metadata pass for the spatial and full-text search work.

- Bumped `graphql-orm` to `0.2.14`.
- Bumped `graphql-orm-macros` to `0.3.15`.
- Updated install snippets to the current runtime version.
- Expanded Rustdoc coverage for the portable spatial and search APIs.
- Added release notes for the recent spatial, search, and PostgreSQL RLS changes.

## 0.2.13

Portable spatial fields, portable per-entity full-text search, and PostgreSQL RLS support.

### Spatial Fields

- Added `#[graphql_orm(spatial(...))]` field metadata for GeoJSON-backed spatial fields.
- Added `#[filterable(type = "spatial")]` and generated `SpatialFilter` support for `equals`,
  `disjoint`, `intersects`, `touches`, `crosses`, `within`, `contains`, and `overlaps`.
- Added native PostGIS storage for PostgreSQL as `geometry(<type>, <srid>)`.
- Added managed PostGIS extension planning through `CREATE EXTENSION IF NOT EXISTS postgis`.
- Added generated PostGIS read/write SQL using `ST_AsGeoJSON`, `ST_GeomFromGeoJSON`, and
  `ST_SetSRID`.
- Added GiST spatial index metadata and migration rendering for PostgreSQL when `index = true`.
- Added SQLite spatial compatibility by storing canonical GeoJSON in `TEXT` columns.
- Added SQLite in-memory spatial predicate evaluation so applications can use the same field and
  filter API without branching on the database backend.
- Documented SQLite spatial indexing options for future work: SpatiaLite, SQLite R*Tree, and
  GeoPackage.

### Full-Text Search

- Added entity-level `#[graphql_orm(search(...))]` metadata.
- Added field-level `#[graphql_orm(searchable(...))]` metadata for `String` and `Option<String>`.
- Added relation-level `#[graphql_orm(search_relation(...))]` metadata for denormalized related
  search fields.
- Added generated per-entity GraphQL search resolvers such as `articlesSearch`.
- Added generated Rust search helpers returning scored `SearchHit<T>` values.
- Added `SearchInput` and `SearchMode` with `Plain`, `Phrase`, `Web`, and `Prefix` modes.
- Added PostgreSQL native full-text search structures using managed search shadow tables,
  `tsvector`, `tsquery`, `ts_rank_cd`, and GIN indexes.
- Added SQLite FTS5 search table planning with the `unicode61` tokenizer by default.
- Added deterministic Rust fallback scoring for search paths where native execution is unavailable
  and fallback is enabled.
- Added generated rebuild APIs including `Entity::rebuild_search_index` and
  `Entity::rebuild_search_document`.
- Added search schema metadata, schema hash participation, migration steps, and introspection
  support for managed search structures.
- Added policy validation rules that reject private searchable fields and require explicit search
  policies for read-policy-protected fields.
- Added future strategy metadata for MySQL and MSSQL full-text search without enabling execution for
  those backends in this pass.

### PostgreSQL RLS

- Added PostgreSQL row-level security metadata through `#[graphql_rls(...)]`.
- Added schema planning and validation for PostgreSQL RLS helper functions, enabled/forced table
  RLS state, and generated policies.
- Added request-local `DbAuthContext` support for transaction-local PostgreSQL settings consumed by
  RLS policies.
- Added relation preloading safeguards so requests with different database auth contexts do not
  share loader batches.

### Schema Management And Backend Behavior

- Clarified that `Database::new`, `Database::builder`, and GraphQL schema construction do not apply
  schema changes automatically.
- Added search structures and spatial metadata to structured schema models and migration planning.
- Updated managed migration behavior for Postgres and SQLite spatial/search structures.
- Preserved MSSQL as read/query-only while adding diagnostics for unsupported spatial and full-text
  execution paths.

### Tests And Verification

- Added unit and integration coverage for spatial predicate rendering, SQLite spatial fallback,
  PostGIS migrations, full-text query rendering, SQLite FTS5 structures, search rebuild behavior,
  and RLS planning.
- Verified focused checks across SQLite, PostgreSQL, and MSSQL feature builds during release work.
