---
title: "graphql-orm"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-08-01
review_by: 2027-02-01
supersedes: []
---

# `graphql-orm`

Runtime crate for [`graphql-orm`](../../README.md).

This crate provides the public runtime contract targeted by the derive macros:

- backend traits and database handles
- filters, ordering, pagination, and row decoding
- relation loaders and nested relation batching
- repository helpers and write hooks
- opt-in repository-only entities with no async-graphql type surface
- row, field, and entity policies
- `AuthSubject`, `AuthorizationMode`, safe public errors, structural tenant helpers,
  generated resolver auth modes, exact-scope `ScopeEntityPolicy`, and optional `auth-agql` bridge
- provider-neutral operation assurance registries, mutation completeness
  audits, directive metadata, deterministic client manifests, and guards
- generated-operation fixed and argument-templated scope declarations,
  independent authorization fingerprints, standard namespaced Federation
  authorization metadata, and optional project-neutral router protocol export
- schema models, validation, migration planning, and explicit migration application
- SQLite, Postgres, and read-only SQL Server runtime support

Most users should start with the repository [README](../../README.md) and the
root [docs](../../docs/README.md). This crate README is intentionally short so
the package page points at the maintained project documentation.

## Example

```rust
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(table = "users", plural = "Users")]
pub struct User {
    #[primary_key]
    pub id: i64,

    #[filterable]
    #[sortable]
    pub name: String,
}
```

## Generated operation authorization

`#[graphql_orm(operation_authorization(...))]` accepts disjoint generated
operation categories. Use `all_scopes` or `any_scopes` for fixed requirements;
use `all_scope_templates` or `any_scope_templates` when a scope references a
coerced scalar root argument such as `records.{id}.read`. The derive rejects
unknown categories and arguments, policies for operations the entity does not
generate, malformed placeholders, nullable values, and complex input objects.

The authoritative generated resolver guard and optional router protocol export
come from the same declaration. Fixed requirements also emit standard
Federation authorization metadata. Argument templates remain protocol metadata
because emitting them as literal `@requiresScopes` values would change their
meaning.

## Documentation

- [Workspace setup](../../docs/development/setup.md)
- [Backend features](../../docs/reference/graphql-orm/backends.md)
- [Entities and relations](../../docs/reference/graphql-orm/entities-and-relations.md)
- [Schema management](../../docs/reference/graphql-orm/schema-management.md)
- [Operation assurance](../../docs/architecture/operation-assurance.md)
- [Runtime writes and policies](../../docs/reference/graphql-orm/runtime-and-writes.md)
- [SQL Server read-only backend](../../docs/reference/graphql-orm/mssql.md)
