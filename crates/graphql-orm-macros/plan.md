# Macro Refactor And Optimisation Plan

## Goals

- Eliminate avoidable N+1 queries in nested relational GraphQL loads.
- Generalise the macro architecture so it can target SQLite, Postgres, MySQL, and SQL Server.
- Reduce project/framework coupling where possible without breaking the current generated API shape.
- Improve compile-time diagnostics, internal structure, and extension points.
- Package this work as a standalone Rust crate that can live in your GitHub repo and be reused across future projects.
- Produce documentation good enough for first-time setup, integration, extension, and long-term reuse.

## Current Constraints

- Nested relation batching only happens on the no-args fast path.
- Nested relations with `Where`, `OrderBy`, or `Page` fall back to one query per parent.
- Single-object nested relations are not batched.
- Generated code is still tied to crate-local ORM/auth/pagination traits.
- The code currently exists as a single standalone `macro.rs`, not as an initialised Rust crate with a manifest, module layout, tests, examples, or publishable documentation.
- Generated database code is SQLite-specific in several places:
  - `sqlx::SqlitePool`
  - `sqlx::sqlite::SqliteRow`
  - `unixepoch()`
  - positional `?` placeholders
- Some macro parsing is narrower than it should be, for example `mutation_result!` only accepts `Ident` payload types.

## Implementation Phases

### Phase 1: Safe Generality Fixes

1. Widen macro parsing and generated type support.
   - Change `mutation_result!` to parse `syn::Type` instead of `Ident`.
   - Audit generated code for assumptions that only work with simple identifiers.

2. Reduce internal brittleness.
   - Parse field metadata once per field where practical.
   - Improve error messages for invalid paths and unsupported field/relation types.

3. Clean up dead or misleading abstractions.
   - Revisit the empty `RelationLoader` methods.
   - Clarify which parts are real batching vs placeholder hooks.

### Phase 1.5: Crate Initialisation And Packaging

1. Initialise this as a standalone Rust crate.
   - Create `Cargo.toml`.
   - Move the macro implementation into `src/lib.rs`.
   - Define feature flags and dependency boundaries.

2. Prepare the crate for GitHub-hosted reuse.
   - Add `README.md`.
   - Add license metadata.
   - Add repository metadata in `Cargo.toml`.
   - Add `.gitignore` and a minimal CI/test layout if desired.

3. Add crate-level documentation for future projects.
   - installation
   - supported derives/macros
   - supported field/struct attributes
   - required host traits/runtime integrations
   - backend support matrix
   - examples for SQLite first, then other backends
   - migration notes for adopting it in an existing project

### Phase 2: Relation Loading And N+1 Optimisation

1. Introduce an argument-aware relation batching key.
   - Include relation identity, normalized `Where`, normalized `OrderBy`, and normalized `Page`.

2. Batch collection relations even when args are present.
   - First implementation: `WHERE fk IN (...)` with grouping in Rust.
   - Apply per-parent pagination in memory initially.

3. Batch single-object relations.
   - Use one `IN (...)` query and map rows back to parent keys.

4. Add request-scoped caching for identical nested relation requests.

5. Upgrade collection batching to SQL-level per-parent pagination.
   - Use window functions such as `ROW_NUMBER() OVER (PARTITION BY fk ORDER BY ...)`.
   - Return grouped total counts/page info per parent.

### Phase 3: SQL Backend Generalisation

1. Introduce backend-neutral traits in the host ORM layer.
   - `SqlDialect`
   - `DbPool`
   - `DbRow`
   - `QueryExecutor`
   - typed bind/value abstractions

2. Stop generating backend-specific SQL directly where possible.
   - Move toward a query representation or builder input that is compiled per dialect.

3. Remove direct SQLite assumptions from generated code.
   - abstract row decoding
   - abstract placeholder syntax
   - abstract current timestamp expression
   - abstract insert/returning behaviour

4. Implement backend support in order:
   - SQLite baseline
   - Postgres
   - MySQL
   - SQL Server

### Phase 4: Optional Backend-Specific Enhancements

1. Native search integration where available.
   - Postgres trigram/full-text
   - MySQL full-text
   - SQL Server full-text
   - retain portable Rust-side fuzzy fallback

2. Better typed JSON/date/boolean handling per backend.

3. Backend feature flags to limit compile surface area.

### Phase 5: Documentation, Examples, And Reuse Readiness

1. Write user-facing documentation for the crate.
   - Quick start
   - end-to-end entity example
   - relations example
   - custom hook example
   - backend setup examples

2. Add example projects or integration fixtures.
   - minimal SQLite example
   - backend-neutral example
   - optional Postgres example once dialect support lands

3. Add test coverage for generated code.
   - compile tests
   - snapshot tests for generated APIs where useful
   - integration tests for nested relation loading and batching

4. Prepare the crate for publishing and long-term maintenance.
   - changelog/versioning strategy
   - semver expectations
   - contribution notes

## Suggested First Milestone

1. Change `mutation_result!` to accept arbitrary Rust types.
2. Initialise the standalone crate structure around the macro implementation.
3. Introduce internal relation-query key structures.
4. Batch single relations.
5. Batch collection relations with shared nested args using `IN (...)`.
6. Keep per-parent pagination in Rust for the first pass.
7. Extract SQL dialect responsibilities away from inline SQLite assumptions.

## Execution Log

- Created this plan.
- Completed: widened `mutation_result!` payload type parsing from `Ident` to `syn::Type`.
- Verified formatting after the `mutation_result!` change.
- Initialised this directory as a standalone proc-macro crate with `Cargo.toml`, `README.md`, `.gitignore`, and `src/lib.rs`.
- Initialised a git repository locally and configured the local author identity.
- Removed the duplicate top-level `macro.rs` after moving the implementation into `src/lib.rs`.
- Added architecture and backend-generalisation design notes under `docs/`.
- Added host-integration notes documenting the current generated-code contract and a proposed `RelationQueryKey` / batched-loader direction.
- Added a basic GitHub Actions CI workflow for formatting and `cargo check`.
- Local `cargo check` is currently blocked by the environment missing a C linker (`cc`/`clang`/`gcc` not installed).
- Created and pushed the initial GitHub repository at `https://github.com/Dastari/graphql-orm-macros`.
- Reduced repeated field-metadata parsing in the relations generator by introducing cached parsed-field collection.
- Improved current N+1 behavior for compatible single-object relations by using the existing DataLoader fast path instead of always falling back to direct queries.
- Next execution item: design and introduce internal relation-query key abstractions so nested relation batching can work even when `Where`/`OrderBy`/`Page` are present.
