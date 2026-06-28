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

Useful focused tests:

```bash
cargo test -p graphql-orm --test graphql_naming
cargo test -p graphql-orm --test backend_coexistence_fixture
cargo test -p graphql-orm --test composite_relations
cargo test -p graphql-orm --test composite_relations_ui
cargo test -p graphql-orm --no-default-features --features mssql --test mssql_write_unavailable_ui
```

Run all default tests with:

```bash
cargo test
```

## PostgreSQL Tests

Postgres integration tests use `TEST_DATABASE_URL` when provided.

```bash
docker run -d --name graphql-orm-postgis-test \
  -e POSTGRES_USER=postgres \
  -e POSTGRES_PASSWORD=postgres \
  -e POSTGRES_DB=postgres \
  -p 55432:5432 \
  postgis/postgis:16-3.4

TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:55432/postgres \
  cargo test -p graphql-orm --no-default-features --features postgres -- --test-threads=1
```

## SQL Server Tests

MSSQL live tests are opt-in. See [SQL Server read-only backend](mssql.md) for Docker and environment-variable details.

Compile-time MSSQL coverage can be run without a live server:

```bash
cargo check -p graphql-orm --no-default-features --features mssql
cargo test -p graphql-orm --no-default-features --features mssql --test mssql_write_unavailable_ui
```

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
