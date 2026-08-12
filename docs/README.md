---
title: GraphQL ORM documentation index
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-12
review_by: 2027-02-01
supersedes: []
---

# GraphQL ORM documentation

Choose the route that matches what you are trying to do. The learning and
how-to paths are for application developers; architecture, decisions,
operations, and plans preserve the deeper contracts and maintainer record.

## Learn

Build and query a small local service first.

- [SQLite GraphQL quickstart](learn/sqlite-quickstart.md) — managed schema,
  seed data, generated roots, Axum transport, and a smoke test.
- [Learn index](learn/README.md) — the recommended sequence and example
  inventory.

## How-to guides

Follow a focused task rather than reading reference material end-to-end.

- [How-to index](how-to/README.md)
- [Choose backend features](reference/graphql-orm/backends.md)
- [Manage a schema](reference/graphql-orm/schema-management.md)
- [Model entities and relations](reference/graphql-orm/entities-and-relations.md)
- [Use SQL Server safely](reference/graphql-orm/mssql.md)
- [Configure PostgreSQL and RLS](reference/graphql-orm/postgres.md)

## Reference

Start with the [`graphql-orm` reference](reference/graphql-orm/README.md) for
core runtime mechanics and configuration. Use the [macro and attribute
reference](reference/graphql-orm/macros-and-attributes.md) for the accepted
derive and `schema_roots!` syntax. The [reference index](reference/README.md)
also links generated inventories and component-local contract documentation.

## Concepts

Architecture explains durable boundaries and trade-offs:

- [System context and package boundaries](architecture/system-context.md)
- [Portable persistence](architecture/portable-persistence.md)
- [Authentication and authorization](architecture/authentication-and-authorization.md)
- [Operation assurance](architecture/operation-assurance.md)
- [Schema modules and fenced leases](architecture/schema-modules-and-leases.md)
- [Storage and backup boundaries](architecture/storage-and-backup-boundaries.md)

## Component documentation

Each package owns its local contract documentation:

- [`graphql-orm`](../crates/graphql-orm/README.md)
- [`graphql-orm-macros`](../crates/graphql-orm-macros/README.md)
- [`graphql-orm-operation-catalog`](../crates/graphql-orm-operation-catalog/README.md)
- [`graphql-orm-storage`](../crates/graphql-orm-storage/docs/README.md)
- [`graphql-orm-backup`](../crates/graphql-orm-backup/docs/README.md)
- [`graphql-orm-ai`](../crates/graphql-orm-ai/docs/README.md)
- [`graphql-orm-ai-tool-profiles`](../crates/graphql-orm-ai-tool-profiles/README.md)
- [`graphql-orm-router-protocol`](../crates/graphql-orm-router-protocol/README.md)
- [`graphql-orm-router`](../crates/graphql-orm-router/README.md)

The [generated workspace package inventory](reference/workspace-packages.md)
is the canonical version and dependency overview.

## Maintainers and governance

- [Development](development/README.md) — local setup, testing, and GraphQL
  workflow.
- [Operations](operations/README.md) — repeatable operational and release
  guidance.
- [Decisions](decisions/README.md) — accepted durable rationale.
- [Plans](plans/README.md) — active outcomes and bounded backlog.
- [Investigations](investigations/README.md) — retained evidence and
  conclusions.
- [Archive](archive/README.md) — superseded history, not current guidance.

One active canonical document owns each topic. The lifecycle, metadata, and
exceptions are defined by
[ADR-0001](decisions/ADR-0001-documentation-authority-and-lifecycle.md).
