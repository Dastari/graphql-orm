# `graphql-orm-macros`

Procedural macros for generating GraphQL entity types, relation resolvers, CRUD operations, and schema roots for an `async-graphql` + ORM-style backend.

## Status

This crate now targets the `graphql-orm` runtime crate directly instead of expecting application-local host modules.

Use it with:

- `graphql-orm`
- `graphql-orm-macros`

## Included Macros

- `mutation_result!`
- `#[derive(GraphQLEntity)]`
- `#[derive(GraphQLRelations)]`
- `#[derive(GraphQLOperations)]`
- `schema_roots!`

## Runtime Status

The paired `graphql-orm` runtime now provides:

- runtime metadata types generated from derives
- backend-aware query rendering for SQLite, PostgreSQL, and read-only SQL Server
- schema models, diffing, migration planning, migration-file rendering, and live schema introspection
- live integration coverage for generated CRUD, nested relations, subscriptions, and N+1-preload behavior

The macro crate remains responsible for code generation. Runtime execution, schema inspection, and migration behavior live in `graphql-orm`.

## Development

```bash
cargo check
```

## Feature Flags

- `sqlite`
- `postgres`
- `mysql`
- `mssql`
- `resolver-case-pascal`
- `resolver-case-snake`
- `resolver-case-screaming-snake`
- `resolver-case-lower`
- `resolver-case-upper`
- `argument-case-pascal`
- `argument-case-snake`
- `argument-case-screaming-snake`
- `argument-case-lower`
- `argument-case-upper`
- `field-case-pascal`
- `field-case-snake`
- `field-case-screaming-snake`
- `field-case-lower`
- `field-case-upper`

When exactly one backend flag is enabled, the selected backend controls the generated runtime pool
and row aliases:

- `sqlite` -> `graphql_orm::DbPool`, `graphql_orm::DbRow`
- `postgres` -> `graphql_orm::DbPool`, `graphql_orm::DbRow`
- `mysql` -> planned
- `mssql` -> `graphql_orm::DbPool`, `graphql_orm::DbRow` in read-only builds

More than one backend may be enabled in a workspace through Cargo feature unification. In that mode,
generated code must select its backend explicitly with `#[graphql_entity(backend = "...")]` and
`schema_roots! { backend: "...", ... }`; `DbPool` and `DbRow` are not exported.

SQLite and PostgreSQL are covered by live integration tests through `graphql-orm`. SQL Server has
read-only compile and opt-in integration coverage. MySQL remains planned.

The naming feature groups are independent:

- `resolver-case-*` controls generated root query/mutation/subscription field names.
- `argument-case-*` controls generated GraphQL argument names.
- `field-case-*` controls generated GraphQL object/input/filter/order/relation fields plus runtime helper fields exposed by `graphql-orm`.

Enable at most one feature from each group. Default GraphQL naming remains camelCase when no naming feature is enabled.

## License

License has not been selected yet.
