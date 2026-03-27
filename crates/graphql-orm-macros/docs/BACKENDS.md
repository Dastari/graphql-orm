# Backend Generalisation Plan

## Objective

Generalize the generated code so the macro can support:

- SQLite
- PostgreSQL
- MySQL
- Microsoft SQL Server

without forcing backend-specific logic into every derive implementation.

## Current SQLite-Specific Assumptions

- `sqlx::SqlitePool`
- `sqlx::sqlite::SqliteRow`
- `unixepoch()`
- `?` placeholders
- SQLite-oriented query construction assumptions

## Current Feature-Flag Coverage

The proc-macro crate now accepts one backend feature at a time:

- `sqlite`
- `postgres`
- `mysql`
- `mssql`

That selection is currently used to emit backend-neutral `graphql_orm::DbPool` and `graphql_orm::DbRow` usage in generated code while keeping backend-specific behavior inside the runtime crate.

## Target Abstractions

### `SqlDialect`

Responsible for:

- placeholder syntax
- identifier quoting
- current timestamp expressions
- limit/offset syntax differences
- returning clause support
- window-function SQL fragments where needed

### `DbPool`

Abstracts the pool type used by generated helpers and resolvers through `graphql-orm`.

### `DbRow`

Abstracts row decoding so generated entity mapping is not tied to a backend-specific row type.

### Typed value binding

Avoid serializing too much through `.to_string()`. The host layer should bind:

- strings
- integers
- floats
- booleans
- bytes
- JSON
- datetimes
- nulls

## Recommended Delivery Order

1. Keep SQLite as the baseline implementation.
2. Introduce dialect traits without changing generated API shape too aggressively.
3. Refactor generated code to call backend-neutral helpers in `graphql-orm`.
4. Add PostgreSQL support first.
5. Add MySQL support next.
6. Add SQL Server support after placeholder/pagination/returning behaviour is settled.

## Pagination And Relation Loading

For batched nested relation loading with pagination:

- SQLite: use window functions where available
- PostgreSQL: use `ROW_NUMBER()` and `COUNT(*) OVER (...)`
- MySQL: use window functions on modern versions
- SQL Server: use `ROW_NUMBER()` and dialect-specific pagination syntax

If a backend lacks a required capability for a given optimisation, the fallback should be:

- grouped fetch
- regroup rows by parent
- apply pagination in memory

## Search Strategy

Portable default:

- fetch candidates through normal filtering
- rank in Rust

Optional backend-native enhancements:

- PostgreSQL trigram or full-text
- MySQL full-text
- SQL Server full-text

## Risks

- backend-specific SQL divergence can leak back into macro code if the runtime dialect boundary is too weak
- row decoding can become difficult to unify if host traits are underspecified
- typed bind/value handling needs to be designed early, or portability will remain shallow
