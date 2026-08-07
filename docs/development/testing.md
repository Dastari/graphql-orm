---
title: "Testing and verification"
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-07
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

SQLite tests may use temporary local databases. PostgreSQL and MSSQL tests may
run only against documented, disposable test infrastructure—never an
application, staging, or shared developer database. Follow the
[PostgreSQL testing runbook](../operations/runbooks/postgres-testing.md) before
running PostgreSQL integration tests. MSSQL live tests are opt-in; consult the
[MSSQL reference](../reference/graphql-orm/mssql.md).

Some PostgreSQL tests create their own labelled loopback-only Docker resources
and are intentionally ignored. Run the named target with `--ignored` only
when that target's test source documents the owned-resource contract.

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
