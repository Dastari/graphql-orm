# `graphql-orm-macros`

Procedural macros for [`graphql-orm`](../../README.md).

Applications normally use these macros through `graphql-orm` re-exports:

```rust
use graphql_orm::prelude::*;
```

## Macros

- `#[derive(GraphQLEntity)]`: entity metadata, filters, order inputs, row decoding, query helpers, and optional write inputs.
- `#[derive(GraphQLSchemaEntity)]`: schema metadata without GraphQL operation generation.
- `#[derive(GraphQLRelations)]`: single-key and composite-key relation resolvers with batching support.
- `#[derive(GraphQLOperations)]`: generated query, mutation, and subscription operation types.
- `schema_roots!`: generated root query/mutation/subscription aliases for a set of entities.
- `mutation_result!`: GraphQL mutation result object generation.

## Backend Selection

For normal single-backend builds, derives keep the existing behavior and infer the backend from enabled features.

For multi-backend workspaces, select the backend explicitly:

```rust
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.Jobs",
    plural = "Jobs",
    schema_policy = "external_read_only"
)]
pub struct Job {
    #[primary_key]
    pub job_id: i32,
}
```

Naming feature groups remain independent:

- `resolver-case-*`
- `argument-case-*`
- `field-case-*`

Enable at most one feature from each group.

## Documentation

See the root [README](../../README.md), project [docs](../../docs/README.md),
and generated rustdocs for the full public contract.
