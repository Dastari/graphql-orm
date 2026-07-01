# Release Notes

This page records user-facing changes for recent `graphql-orm` releases. Version numbers refer to
the runtime crate unless a macro crate version is called out separately.

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
