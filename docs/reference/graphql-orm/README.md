---
title: GraphQL ORM runtime reference index
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-08-10
review_by: 2027-02-01
supersedes: []
---

# `graphql-orm` runtime reference

Use the [workspace documentation index](../../README.md) to choose between
architecture, operations, development, and these mechanics. This directory
contains current backend, entity, schema, query, mutation, and runtime API
guidance for the core ORM package.

## Foundations

- [Backends and multi-backend workspaces](backends.md)
- [Macro and attribute reference](macros-and-attributes.md) — canonical derive syntax, feature flags, defaults, and constraints
- [Entities and relations](entities-and-relations.md)
- [Schema management](schema-management.md)
- [PostgreSQL](postgres.md)
- [Microsoft SQL Server](mssql.md)
- [Federation](federation.md)
- [`agql-auth` bridge](agql-auth-bridge.md)
- [Strict authorization](strict-authorization.md)
- [Stable error codes](error-codes.md)

## Generated and repository operations

- [Runtime writes, hooks, subscriptions, and policies](runtime-and-writes.md)
- [Composite mutations](composite-mutations.md)
- [Repository-only entities](repository-only-entities.md)
- [Read projections](read-projections.md)
- [Binary keys and indexes](binary-keys-and-indexes.md)
- [Resolver operation metadata](resolver-operation-metadata.md)
- [Pagination migration](pagination-migration.md)
- [Typed grouped aggregates](typed-aggregates.md)

## Choose a path

- Start a managed application with [SQLite](backends.md#features) or
  [PostgreSQL](postgres.md), then define entities and roots with the
  [macro reference](macros-and-attributes.md).
- Integrate an externally owned SQL Server schema through the
  [read-only MSSQL guide](mssql.md); do not use generated schema changes or
  writes for that backend.
- Use [repository-only entities](repository-only-entities.md) when typed Rust
  data access is required without a GraphQL object or generated resolver.
- Add relations, generated CRUD, hooks, and subscriptions using
  [runtime writes and policies](runtime-and-writes.md), then review
  [strict authorization](strict-authorization.md) before exposing the schema.

## Runtime schema APIs

- [Runtime schema IR](runtime-schema-ir.md)
- [Runtime records](runtime-records.md)
- [Runtime queries](runtime-queries.md)
- [Runtime relations](runtime-relations.md)
- [Cross-backend tenant module](cross-backend-tenant.md)
- [Backup runtime boundary](backup-runtime.md)
