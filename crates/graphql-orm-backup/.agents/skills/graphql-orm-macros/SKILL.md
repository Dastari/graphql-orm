---
name: graphql-orm-macros
description: >
  Use when working on the graphql-orm runtime plus graphql-orm-macros derive
  layer for GraphQL entities, relation resolvers, CRUD operations, schema roots,
  migrations, and runtime metadata integration.
---

# graphql-orm Skill

## Use This Skill When

- adding `mutation_result!` result types
- deriving `GraphQLEntity`
- deriving `GraphQLRelations`
- deriving `GraphQLOperations`
- composing schema roots with `schema_roots!`
- reviewing relation loading behavior or N+1 implications
- changing runtime metadata, query rendering, schema diffing, or migration support
- checking backend-specific SQLite/PostgreSQL behavior

## Crates

- Application dependency: `graphql-orm`
- Runtime repo: `https://github.com/Dastari/graphql-orm`
- Macro repo: `https://github.com/Dastari/graphql-orm-macros`

## Preferred Usage

Import through the runtime crate:

- `use graphql_orm::prelude::*;`
- `use graphql_orm::mutation_result;`
- derive macros by name on structs

For `digitise`, treat `graphql-orm` as the only normal dependency surface. Do not add a direct `graphql-orm-macros` dependency unless you are explicitly developing or debugging the proc-macro crate itself.

## Integration Rules

1. Use the runtime-plus-macro split correctly.
`digitise` should depend on `graphql-orm`. Generated code comes from the re-exported macros, but runtime behavior, metadata, query rendering, relation loading, and migrations belong to `graphql-orm`.

This is the default assumption for new work. If a change requires touching the macro crate, do that in the shared library repo, but keep `digitise` depending only on `graphql-orm`.

2. Use the macros for boilerplate, not business logic.
The application should still own domain logic, permission checks, store implementations, and resolver orchestration.

3. Keep generated types aligned with async-graphql.
If a macro-generated GraphQL object wraps an entity field, ensure the entity type itself is compatible with async-graphql output expectations.

4. Treat the stack as backend/framework-opinionated.
It is domain-generic, but it still assumes an async-graphql + ORM-style host environment. Do not assume it is a fully generic Rust macro toolkit.

5. Use the generic notify hook pattern, not project-specific hard-coding.
If a mutation needs side effects after create/update/delete, prefer the `notify` / `notify_with` hook path model exposed by the macro crate.

6. Watch relation performance.
For nested relations, understand whether the generated path is batched or falls back to direct queries. Use this crate when the problem is macro-generated relation behavior, not when the issue is application auth.

7. Keep persistence backend-agnostic at the app layer.
Backend-specific SQL rendering, migration planning, and schema introspection belong in `graphql-orm`, not in `digitise`.

8. Prefer the runtime surface over old host-crate assumptions.
Generated code should target `::graphql_orm::*`. Avoid reintroducing assumptions that the application must expose `crate::db`, `crate::graphql::orm`, or similar legacy module shapes.

## When Not To Use

- when implementing authentication, refresh tokens, or guards
- when working on frontend-only GraphQL calls
- when you just need handwritten simple types and the macro would add unnecessary coupling

## Common Pattern

```rust
use graphql_orm::mutation_result;

#[derive(async_graphql::SimpleObject, Clone, Debug)]
struct User {
    id: String,
}

mutation_result!(LoginResult, user: User);
```

## Project Guidance

- use `graphql-orm` as the application-facing dependency and macro re-export surface
- in `digitise`, do not depend on `graphql-orm-macros` directly
- keep `digitise` responsible for choosing when derive-based boilerplate is worth the coupling
- if the needed change would improve multiple projects, prefer updating `graphql-orm` or `graphql-orm-macros` rather than patching around them locally
