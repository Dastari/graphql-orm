# Development

This repository is a Rust workspace with two crates:

- `crates/graphql-orm`: runtime, backend traits, schema models, migrations, query execution, policies, and helpers.
- `crates/graphql-orm-macros`: derive and function macros that generate entity, relation, operation, and schema-root code.

## Common Checks

```bash
cargo check -p graphql-orm
cargo check -p graphql-orm --no-default-features --features sqlite
cargo check -p graphql-orm --no-default-features --features postgres
cargo check -p graphql-orm --no-default-features --features mssql
cargo check -p graphql-orm --no-default-features --features "sqlite mssql"
```

## agql-auth Bridge Alignment

The optional bridge and any direct host dependency must both use agql-auth
0.12.0 at revision `3f3b0c5365adfbe436514a681d977b600991b797`.
Exercise the Gema-facing feature combinations with:

```bash
cargo check -p graphql-orm --no-default-features --features "sqlite auth-agql resolver-case-pascal argument-case-pascal field-case-pascal"
cargo check -p graphql-orm --no-default-features --features sqlite
cargo check -p graphql-orm --no-default-features --features mssql
cargo test -p graphql-orm --no-default-features --features "sqlite auth-agql" graphql::auth_agql::tests
cargo test -p graphql-orm --no-default-features --features "sqlite auth-agql resolver-case-pascal argument-case-pascal field-case-pascal" --test backend_coexistence_fixture
```

Inspect both the workspace and direct-host fixture graphs; there must be one
agql-auth package/revision and one graphql-orm/runtime-macros universe:

```bash
cargo tree -p graphql-orm --locked --no-default-features --features "sqlite auth-agql"
cargo tree --duplicates --workspace --locked
cargo tree --manifest-path crates/graphql-orm/tests/fixtures/backend-coexistence/Cargo.toml -p auth-service
```

The bridge must remain a principal projection. It has no rate-limit-store,
OIDC-request, provider-evidence, token-minting, or product-policy role.

## Backend Dependency Isolation

Backend-only builds must not activate another SQLx database driver. Verify the
resolved graph after any manifest or feature change:

```bash
cargo tree -p graphql-orm --no-default-features --features sqlite
cargo tree -p graphql-orm --no-default-features --features postgres
cargo tree -p graphql-orm --no-default-features --features mssql
cargo tree -p graphql-orm --no-default-features --features "sqlite postgres"
```

The expected SQLx packages are:

| Features | SQLx backend packages |
| --- | --- |
| `sqlite` | `sqlx-sqlite` only |
| `postgres` | `sqlx-postgres` only |
| `mssql` | neither (`sqlx-core` remains for backend-neutral compatibility internals) |
| `sqlite postgres` | both `sqlx-sqlite` and `sqlx-postgres` |

Use `cargo tree -i sqlx-postgres` and `cargo tree -i sqlx-sqlite` with the same
feature arguments when diagnosing an unexpected reverse dependency.

Useful focused tests:

```bash
cargo test -p graphql-orm --test graphql_naming
cargo test -p graphql-orm --test backend_coexistence_fixture
cargo test -p graphql-orm --test composite_relations
cargo test -p graphql-orm --test composite_relations_ui
cargo test -p graphql-orm --no-default-features --features sqlite --test spatial_sqlite
cargo test -p graphql-orm --no-default-features --features sqlite --test full_text_search
cargo test -p graphql-orm --no-default-features --features mssql --test mssql_write_unavailable_ui
cargo test -p graphql-orm --no-default-features --features sqlite --test repository_only_entity
cargo test -p graphql-orm --no-default-features --features sqlite --test repository_only_entity_ui
```

Run all default tests with:

```bash
cargo test
```

## PostgreSQL Tests

Postgres integration tests use `TEST_DATABASE_URL` when provided. These tests create, alter, and
drop schemas and tables. Always point them at a dedicated throwaway database, never at a shared
developer, staging, or application database.

```bash
docker run -d --name graphql-orm-postgis-test \
  -e POSTGRES_USER=graphql_orm \
  -e POSTGRES_PASSWORD=graphql_orm \
  -e POSTGRES_DB=graphql_orm_test \
  -p 55433:5432 \
  postgis/postgis:17-3.5

TEST_DATABASE_URL=postgres://graphql_orm:graphql_orm@127.0.0.1:55433/graphql_orm_test \
  cargo test -p graphql-orm --no-default-features --features postgres -- --test-threads=1
```

Focused PostGIS spatial coverage can be run with:

```bash
TEST_DATABASE_URL=postgres://graphql_orm:graphql_orm@127.0.0.1:55433/graphql_orm_test \
  cargo test -p graphql-orm --no-default-features --features postgres --test spatial_fields
```

Focused Postgres full-text search DDL coverage can be run without a live server:

```bash
cargo test -p graphql-orm --no-default-features --features postgres --test full_text_search
```

The retention-maintenance parity test is deliberately different from legacy
PostgreSQL tests: it ignores all database URL environment variables and creates
and removes its own loopback-only disposable Docker container with generated
credentials and no persistent volume:

```bash
cargo test -p graphql-orm --no-default-features --features postgres \
  --test retention_purge_postgres -- --ignored --nocapture
```

The repository-only parity test follows the same owned-resource rule: it
ignores `DATABASE_URL`/`TEST_DATABASE_URL`, starts a loopback-only PostgreSQL
container with generated credentials/database identity, and removes it on
success or failure:

```bash
cargo test -p graphql-orm --no-default-features --features postgres \
  --test repository_only_postgres -- --ignored --nocapture
```

The 0.15 bounded-mutation sentinel parity test likewise owns a uniquely
labelled loopback-only PostgreSQL 17 container and exercises the same
single-key, composite-key, and retention ceiling/count matrix as SQLite:

```bash
cargo test -p graphql-orm --features sqlite \
  --test bounded_mutation_sentinels
cargo test -p graphql-orm --no-default-features --features postgres \
  --test bounded_mutation_sentinels -- --ignored --nocapture
```

The 0.13 PostgreSQL constraint-index and runtime-relation regressions also own
their complete PostgreSQL 17 lifecycle. They ignore database URL variables,
use generated credentials, publish only on loopback, attach a unique ownership
label, create no volume, verify that label before cleanup, and never inspect or
remove a pre-existing container:

```bash
cargo test -p graphql-orm --no-default-features --features postgres \
  --test postgres_constraint_index_idempotency -- --ignored --nocapture
cargo test -p graphql-orm --no-default-features --features postgres \
  --test runtime_relations_postgres -- --ignored --nocapture
```

## SQL Server Tests

MSSQL live tests are opt-in. See [SQL Server read-only backend](mssql.md) for Docker and environment-variable details.

Compile-time MSSQL coverage can be run without a live server:

```bash
cargo check -p graphql-orm --no-default-features --features mssql
cargo test -p graphql-orm --no-default-features --features mssql --test mssql_write_unavailable_ui
cargo test -p graphql-orm --no-default-features --features mssql --doc
```

The focused UI suite retains macro-owned diagnostics for unsupported composite
mutations and retention. Basic absence of ordinary MSSQL write helpers uses
paired compiling/`compile_fail` doctests instead of rustc-prose snapshots, so
the assertion remains stable across supported compiler releases.

## UI Tests

The macro compile tests use `trybuild`. When intentionally changing compiler diagnostics, update expected output with:

```bash
TRYBUILD=overwrite cargo test -p graphql-orm --test composite_relations_ui
```

Review generated `.stderr` changes before committing them.

## Documentation

Build crate docs with:

```bash
cargo doc -p graphql-orm --no-deps
cargo doc -p graphql-orm-macros --no-deps
```

The root `README.md` should stay short. Long-form material belongs in `docs/` and should be linked from the README or `docs/README.md`.

## Versioning

When public macro output, runtime APIs, or documentation examples change, update the package versions consistently:

- `crates/graphql-orm/Cargo.toml`
- `crates/graphql-orm-macros/Cargo.toml`
- examples in README/docs that show a concrete version
