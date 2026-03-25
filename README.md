# `graphql-orm`

Runtime support crate for `graphql-orm-macros`.

## Purpose

This crate owns the runtime contract that generated code targets:

- database pool and row aliases
- GraphQL auth, filter, pagination, and relation-loader support
- query helpers and bind execution
- shared ORM traits such as `DatabaseEntity`, `DatabaseFilter`, `DatabaseOrderBy`, `FromSqlRow`, and `RelationLoader`

## Current Scope

The current implementation is focused on making the proc-macro output target a real runtime crate instead of application-local shim modules.

Working today:

- SQLite runtime support
- PostgreSQL runtime support
- generated CRUD operations
- generated subscriptions
- generated relation loading and batched nested traversal

## Usage

Add `graphql-orm` and derive through its re-exports:

```rust
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLRelations, GraphQLOperations)]
pub struct Entity {
    #[primary_key]
    pub id: String,
}
```

The generated code now targets `::graphql_orm::*` directly.

## Notes

Current macro output still expects the application to derive or import GraphQL-facing item macros such as `SimpleObject` on its entity types, and to provide any application-specific auth user data in the GraphQL context.
