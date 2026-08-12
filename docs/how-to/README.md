---
title: GraphQL ORM how-to index
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-12
review_by: 2027-02-01
supersedes: []
---

# How-to guides

Use these focused routes after completing the
[SQLite quickstart](../learn/sqlite-quickstart.md).

## Core ORM

- [Select backend features and work safely in a multi-backend workspace](../reference/graphql-orm/backends.md)
- [Model entities, keys, columns, naming, and relations](../reference/graphql-orm/entities-and-relations.md)
- [Validate, plan, and apply a schema change](../reference/graphql-orm/schema-management.md)
- [Use generated writes, repositories, policies, hooks, and subscriptions](../reference/graphql-orm/runtime-and-writes.md)
- [Connect an existing read-only SQL Server schema](../reference/graphql-orm/mssql.md)
- [Configure PostgreSQL auth-aware execution and RLS](../reference/graphql-orm/postgres.md)

## Companion packages

- [Store objects with `graphql-orm-storage`](../../crates/graphql-orm-storage/docs/README.md)
- [Back up and restore with `graphql-orm-backup`](../../crates/graphql-orm-backup/docs/README.md)
- [Build AI runtime integrations with `graphql-orm-ai`](../../crates/graphql-orm-ai/docs/README.md)
- [Declare least-privilege AI tool profiles](../../crates/graphql-orm-ai-tool-profiles/README.md)
- [Compose subgraphs with the router protocol and router](../../crates/graphql-orm-router/README.md)

For the complete set of configuration and API references, use the
[reference index](../reference/README.md). For durable design boundaries, use
[the system context](../architecture/system-context.md).
