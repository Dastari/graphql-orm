---
title: "Workspace setup"
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-07
review_by: 2027-02-01
supersedes: []
---

# Workspace setup

This repository is a Rust workspace containing independently consumable ORM,
macro, storage, backup, AI, router-protocol, and router packages. The core ORM
does not acquire backup, storage, AI, or router runtime capabilities through
optional features.

## Prerequisites

Install the Rust toolchain specified by the repository's Cargo metadata. Docker
is needed only for the explicitly documented disposable integration-test lanes;
it is not required for ordinary SQLite and compile-only checks.

Clone the repository, then fetch and validate the default workspace members:

```sh
cargo fetch --locked
cargo check
```

The default members are `graphql-orm` and `graphql-orm-macros`. Select
companion packages explicitly when working on them:

```sh
cargo check -p graphql-orm-storage
cargo check -p graphql-orm-backup
cargo check -p graphql-orm-ai
cargo check -p graphql-orm-router-protocol
cargo check -p graphql-orm-router
cargo check -p graphql-orm-router --features auth-agql
```

The router is intentionally excluded from default members because its pinned
Federation execution stack is substantially heavier than the protocol and core
ORM packages. Select it only for router work.

## Backend selection

Database backends are alternative configurations. Do not run workspace
`--all-features`; use an explicit package and backend feature set instead.

```sh
cargo check -p graphql-orm --no-default-features --features sqlite
cargo check -p graphql-orm --no-default-features --features postgres
cargo check -p graphql-orm --no-default-features --features mssql
cargo check -p graphql-orm --no-default-features --features "sqlite mssql"
```

Use a checked-in workspace path dependency for another package in this
repository. The root `Cargo.lock` is shared, and `agql-auth` remains an
external exact-revision dependency.

For consumer dependency configuration and feature descriptions, see the
[ORM reference](../reference/graphql-orm/backends.md).
