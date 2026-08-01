---
title: GraphQL ORM runtime reference index
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-08-01
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
- [Binary keys and conditional indexes](binary-keys-and-indexes.md)
- [Resolver operation metadata](resolver-operation-metadata.md)
- [Pagination migration](pagination-migration.md)

## Runtime schema APIs

- [Runtime schema IR](runtime-schema-ir.md)
- [Runtime records](runtime-records.md)
- [Runtime queries](runtime-queries.md)
- [Runtime relations](runtime-relations.md)
- [Cross-backend tenant module](cross-backend-tenant.md)
- [Backup runtime boundary](backup-runtime.md)
