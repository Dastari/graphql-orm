# Architecture Notes

## Intent

`graphql-orm-macros` is being extracted into a reusable proc-macro crate for generating:

- GraphQL entity filter/order input types
- relation resolvers
- CRUD query/mutation/subscription wrappers
- schema root composition helpers

The immediate goal is to preserve the current generated API shape while reducing hard-coded assumptions and opening a path to backend portability.

## Current Runtime Expectations

Today the generated code targets `graphql-orm` directly rather than application-local host modules.

That means the intended app integration shape is:

- application depends on `graphql-orm`
- `graphql-orm` re-exports the derive macros
- generated code targets `::graphql_orm::*`

This is a major improvement over the older host-crate model because applications no longer need to mirror a crate-local `db` / `graphql` module structure just to consume the macros.

## Planned Extraction Boundary

The long-term design should split responsibilities like this:

### Proc-macro crate

- parse struct and field metadata
- generate GraphQL-facing types and resolver wrappers
- emit backend-agnostic query intent or trait-based calls
- validate macro inputs and produce useful diagnostics

### Host integration crate or module

- ORM trait implementations
- SQL compilation
- query execution
- backend-specific row decoding and bind handling
- auth/context/database integration

That host integration layer is now effectively the `graphql-orm` runtime crate.

## Relation Loading Direction

The current relation strategy only batches one-to-many loads when no nested args are supplied. The target design is:

1. normalize nested relation args into a reusable key
2. batch sibling relation loads across parents
3. execute one grouped query per `(relation, args)` combination
4. regroup rows by parent key
5. compute page metadata per parent

The first version can paginate in memory after grouped fetches. A later version should support SQL-level per-parent pagination with window functions.

## Packaging Direction

This repository should become:

- a standalone Rust crate
- documented well enough for future project reuse
- versioned with semver
- testable through compile tests and integration fixtures

## Open Design Questions

- Should the host-facing ORM abstraction stay crate-local or become a companion crate?
- Should backend portability use feature flags only, or also runtime-selected dialects?
- How much SQL should remain string-built versus moving to an intermediate query representation?
- Should the current `GraphQLEntity` derive stay monolithic or be split internally into smaller generators?
