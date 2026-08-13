---
title: "Testing and verification"
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-13
review_by: 2027-02-01
supersedes: []
---

# Testing and verification

Run the narrowest package and backend lane that covers a change. Database
backends are alternative configurations, so never use workspace
`--all-features`.

## Baseline ORM lane

```sh
cargo fmt --all -- --check
cargo test -p graphql-orm --no-default-features --features sqlite
cargo clippy -p graphql-orm --no-default-features --features sqlite -- -D warnings
cargo doc -p graphql-orm --no-deps
```

Compile every affected backend lane explicitly:

```sh
cargo check -p graphql-orm --no-default-features --features sqlite
cargo check -p graphql-orm --no-default-features --features postgres
cargo check -p graphql-orm --no-default-features --features mssql
cargo check -p graphql-orm --no-default-features --features "sqlite mssql"
```

When manifests or backend features change, inspect the resolved graph:

```sh
cargo tree -p graphql-orm --locked --no-default-features --features sqlite
cargo tree -p graphql-orm --locked --no-default-features --features postgres
cargo tree -p graphql-orm --locked --no-default-features --features mssql
cargo tree --duplicates --workspace --locked
```

## Package-specific lanes

Select companion crates explicitly and use their documented provider features.
For example:

```sh
cargo test -p graphql-orm-storage
cargo test -p graphql-orm-backup --features orm-sqlite
cargo check -p graphql-orm-ai --no-default-features --features postgres
cargo test -p graphql-orm-router-protocol
cargo test -p graphql-orm-router
cargo test -p graphql-orm-router --features auth-agql
```

Run package-local tests and their required feature matrices whenever a change
crosses the package boundary.

Provider implementations are separate feature lanes. Run them locally one at
a time so an accidental dependency on another provider feature cannot make a
lane pass:

```sh
scripts/check-ai-provider-lanes.sh test
scripts/check-ai-provider-lanes.sh clippy
scripts/check-ai-provider-lanes.sh doc
```

The runner covers `provider-openai`, `provider-anthropic`, `provider-xai`,
`provider-ollama`, `provider-openai-compatible`, `local-harness`, and
`provider-codex-app-server`, each with only SQLite and that provider enabled.
Pass one provider feature as the second argument for a focused run. These local
commands are release evidence; a hosted workflow result is not a substitute.

Router changes also require warnings-denied package lanes and the dependency
boundary check:

```sh
cargo clippy -p graphql-orm-router-protocol --all-targets -- -D warnings
cargo clippy -p graphql-orm-router --all-targets -- -D warnings
cargo clippy -p graphql-orm-router --all-targets --features auth-agql -- -D warnings
RUSTDOCFLAGS="-D warnings -D missing_docs" cargo doc -p graphql-orm-router-protocol --no-deps
RUSTDOCFLAGS="-D warnings -D missing_docs" cargo doc -p graphql-orm-router --no-deps
RUSTDOCFLAGS="-D warnings -D missing_docs" cargo doc -p graphql-orm-router --no-deps --features auth-agql
scripts/check-workspace-dependencies.sh
python3 scripts/generate-workspace-inventory.py --check
python3 scripts/check-documentation.py
```

Router integration tests own their loopback subgraphs, listeners, and
timeouts. They include raw HTTP and `graphql-transport-ws` clients plus an
upstream WebSocket subgraph and exercise authentication, expiry, bounded
connections/operations, failure isolation, conditional schema/protocol polling,
in-flight request pinning, invalid-candidate last-known-good behavior, and
authenticated subscription retirement/reconnect after atomic graph-and-policy
replacement. The standalone-binary smoke test performs pre-bind checking,
authenticated HTTP and WebSocket execution, downstream timeout/recovery,
signal drain, and listener-release checks. The bounded hardening campaign also
covers repeated reload, JWKS outage/rotation, WebSocket churn, and subscription
lag. They must not contact a deployed subgraph or application database.

## External-service tests

SQLite tests may use temporary local databases. PostgreSQL and MSSQL acceptance
tests may run only against infrastructure the test creates, labels,
loopback-publishes, and removes itself—never an application, staging, shared
developer, or manually supplied database. The canonical local runner rejects
ambient database URL variables:

```sh
scripts/run-owned-database-lanes.sh sqlite
scripts/run-owned-database-lanes.sh postgres
scripts/run-owned-database-lanes.sh mssql
scripts/run-owned-database-lanes.sh ai-postgres
```

Use `all` only when the machine has enough memory for the lanes to run
sequentially. Docker absence or failure is a failed required lane, not a skip.
Follow the [PostgreSQL testing runbook](../operations/runbooks/postgres-testing.md)
and [MSSQL reference](../reference/graphql-orm/mssql.md) for the owned-resource
contract and coverage.

Owned PostgreSQL and SQL Server tests are intentionally ignored in ordinary
unit-test runs because they start containers. Invoke them through the canonical
runner rather than exporting `TEST_DATABASE_URL`, `MSSQL_TEST_DATABASE_URL`, or
`DATABASE_URL`. Older opt-in tests that still accept a URL are not release
acceptance evidence until migrated to the owned harness.

## Authentication bridge lane

Changes involving `auth-agql` must retain the external exact revision and test
the bridge feature explicitly:

```sh
cargo check -p graphql-orm --no-default-features --features "sqlite auth-agql"
cargo test -p graphql-orm --no-default-features --features "sqlite auth-agql" --test agql_assurance
cargo tree -p graphql-orm --locked --no-default-features --features "sqlite auth-agql"
cargo test -p graphql-orm-router --locked --features auth-agql
cargo tree -p graphql-orm-router --locked --features auth-agql
```

The graph must resolve a single `agql-auth` revision and one
`graphql-orm`/macro universe. The router lane exercises only the one-way
resource-server validator and matcher adapters; it must not instantiate
issuer-side or storage services.
