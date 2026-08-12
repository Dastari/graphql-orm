---
title: Learning GraphQL ORM
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-12
review_by: 2027-02-01
supersedes: []
---

# Learn GraphQL ORM

Start with the [SQLite GraphQL quickstart](sqlite-quickstart.md). It is the
canonical runnable example in this repository: it creates a managed SQLite
schema, inserts data through generated repository helpers, exposes generated
GraphQL roots over HTTP, and verifies the query in a smoke test.

## Checked-in examples

The quickstart is the only complete, HTTP-serving onboarding application. The
other examples are focused, runnable programs that demonstrate one contract;
they are not substitutes for a production-ready host.

| Example | Scope | Run it |
| --- | --- | --- |
| [`sqlite_quickstart`](../../crates/graphql-orm/examples/sqlite_quickstart.rs) | **Complete onboarding:** managed SQLite schema, seed data, generated GraphQL roots, and local Axum HTTP server | `cargo run -p graphql-orm --example sqlite_quickstart` |
| [`mssql_readonly`](../../crates/graphql-orm/examples/mssql_readonly.rs) | **Focused:** compile the read-only SQL Server entity and schema shape; it does not connect to a database | `cargo run -p graphql-orm --no-default-features --features mssql --example mssql_readonly` |
| [`operation_assurance`](../../crates/graphql-orm/examples/operation_assurance.rs) | **Focused:** generate and audit operation-assurance metadata; it does not run a request-serving schema | `cargo run -p graphql-orm --example operation_assurance` |
| [`router_descriptor`](../../crates/graphql-orm/examples/router_descriptor.rs) | **Focused:** export generated operation metadata as a router-protocol descriptor | `cargo run -p graphql-orm --features router-protocol --example router_descriptor` |
| [`programmatic_router`](../../crates/graphql-orm-router/examples/programmatic_router.rs) | **Focused:** construct and print a development router configuration; it does not start the router | `cargo run -p graphql-orm-router --example programmatic_router` |
| [`handwritten_descriptor`](../../crates/graphql-orm-router-protocol/examples/handwritten_descriptor.rs) | **Focused:** emit a project-neutral router-protocol descriptor from handwritten declarations | `cargo run -p graphql-orm-router-protocol --example handwritten_descriptor` |

For an example-backed full-stack service, begin with the quickstart and then
follow its [security note](sqlite-quickstart.md#security-note) before adapting
the transport or authorization model.

After it runs, continue according to the application you are building:

- Use [entities and relations](../reference/graphql-orm/entities-and-relations.md)
  to expand the data model.
- Use [schema management](../reference/graphql-orm/schema-management.md) before
  evolving a managed database or adopting an existing one.
- Use [backend features](../reference/graphql-orm/backends.md) when selecting
  SQLite, PostgreSQL, or the read-only SQL Server integration.
- Use the [how-to index](../how-to/README.md) for a task-oriented path into
  security, storage, backup, AI, and Federation router documentation.

The quickstart is a local demonstration, not a production security template.
