# `graphql-orm`

Runtime support crate for `graphql-orm-macros`.

## Purpose

This crate owns the runtime contract that generated code targets:

- database pool and row aliases
- backend and dialect abstractions
- entity metadata types
- GraphQL auth, filter, pagination, and relation-loader support
- query helpers and bind execution
- shared ORM traits such as `Entity`, `DatabaseEntity`, `DatabaseSchema`, `DatabaseFilter`, `DatabaseOrderBy`, `FromSqlRow`, and `RelationLoader`
- migration traits and migration records

## Current Scope

Working today:

- SQLite runtime support
- PostgreSQL runtime support
- read/query-only SQL Server runtime support through Tiberius
- generated entity metadata through the runtime contract
- composite primary-key metadata and generated read lookups
- generated CRUD and opt-in upsert operations, subscriptions, relation loading, composite relation keys, and batched nested traversal
- backend-aware query rendering through a small typed query IR
- schema models built from runtime metadata
- schema diffing and migration planning
- explicit schema ownership policy through `SchemaPolicy`
- schema validation, structured migration plans, and ABI upgrade orchestration through `Database::schema()`
- migration file rendering and migration application helpers
- live schema introspection for SQLite and PostgreSQL

Still remaining:

- MySQL runtime support
- SQL Server writes, migrations, and schema management
- richer query IR coverage beyond the current CRUD/filter/sort/pagination subset
- more complete migration execution for backend-specific edge cases and review workflows

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
