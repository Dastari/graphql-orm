---
title: "graphql-orm-operation-catalog"
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-11
supersedes: []
---

# graphql-orm-operation-catalog

`graphql-orm-operation-catalog` owns the database-backend-neutral metadata and
fingerprinting contracts emitted for generated GraphQL root operations. The
metadata is discovery and drift evidence only; it grants no resolver authority.

Most applications consume these types through the compatible `graphql-orm`
re-exports. Backend-neutral tooling may depend on this package directly without
selecting SQLite, PostgreSQL, or MSSQL.
