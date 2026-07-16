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

## Documentation

- [Getting started](../../docs/getting-started.md)
- [Backend features](../../docs/backends.md)
- [Entities and relations](../../docs/entities-and-relations.md)
- [Schema management](../../docs/schema-management.md)
- [Runtime writes and policies](../../docs/runtime-and-writes.md)
- [SQL Server read-only backend](../../docs/mssql.md)
