---
title: "graphql-orm-macros"
kind: reference
status: active
owner: graphql-orm-macros-maintainers
last_reviewed: 2026-08-31
review_by: 2027-02-01
supersedes: []
---

# `graphql-orm-macros`

This is the procedural-macro implementation for `graphql-orm`. Applications
normally depend on the runtime crate and import its prelude; that keeps the
macro/runtime versions aligned:

```toml
[dependencies]
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.29.0", default-features = false, features = ["sqlite"] }
```

Direct use is supported for tooling that needs the macro package:

```toml
graphql-orm-macros = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.29.0", default-features = false, features = ["sqlite"] }
```

The direct dependency still requires a compatible `graphql-orm` runtime in the
consumer crate because generated code refers to `::graphql_orm`. The macros do
not connect to a database, run migrations, host GraphQL, or authorize requests.

## Public macros

| Item | Purpose |
| --- | --- |
| `GraphQLEntity` | GraphQL object/filter/order/input types, row decoding, metadata, and query helpers |
| `GraphQLSchemaEntity` | schema metadata only |
| `RepositoryEntity` | typed repository CRUD and private projections with no GraphQL surface |
| `GraphQLRelations` | batched single/composite-key relation resolvers |
| `graphql_complex_object` | handwritten complex fields composed with generated relations |
| `GraphQLOperations` | generated GraphQL root operation types and operation metadata |
| `schema_roots!` | query/mutation/subscription roots, schema builders, metadata, and resolved catalog |
| `graphql_orm_custom_operations` | semantic metadata emitted beside a handwritten root impl |
| `GraphQLSemanticObject` | classified public field metadata for a handwritten result object |
| `mutation_result!` | a simple GraphQL mutation result object |
| `backend_selected_graphql_entity` | emits cfg-selected entity definitions for a multi-backend consumer |

For a handwritten root annotated by `graphql_orm_custom_operations`, prefer
the matching `described_query_types`, `described_mutation_types`, or
`described_subscription_types` list in `schema_roots!`. One entry composes the
root and automatically includes its operation descriptors and direct
`GraphQLSemanticObject` result metadata. The older `extra_*` plus
`semantic_custom_operations`/`semantic_types` lists remain compatible for
incremental migration, but a described root cannot also appear in them.

Custom scalar and enum results are fail-safe by default. To make one eligible
for provider disclosure, declare `result_classification` and `result_export`
together on the resolver method. An exportable scalar/enum list also requires
positive `result_maximum_items`; `secret` may only use `never_export`.

## Feature configuration

`sqlite` is the default. Select `postgres` or `mssql` by disabling default
features. The macro compiler rejects a build with no backend and a consumer
must select an explicit backend when Cargo feature unification enables more
than one. `mysql` exists as a macro feature but the runtime package does not
offer a MySQL backend in this release; do not use it as application support.

The independent `resolver-case-*`, `argument-case-*`, and `field-case-*`
groups each permit at most one feature: `pascal`, `snake`,
`screaming-snake`, `lower`, or `upper`.

## Minimum use

```rust
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(table = "notes", plural = "Notes")]
struct Note {
    #[primary_key]
    id: i64,
    #[filterable(type = "string")]
    #[sortable]
    body: String,
}
```

The generated surface depends on the selected backend and schema policy.
MSSQL and `external_read_only` schemas retain reads but omit generated
mutations and subscriptions. Repository-only entities intentionally report no
generated GraphQL operations.

## Authoritative reference and security

The [macro and attribute reference](../../docs/reference/graphql-orm/macros-and-attributes.md)
is the canonical accepted-syntax, defaults, constraints, naming, relation,
index, projection, search/spatial, schema-policy, and generated-authorization
reference. Attribute metadata is not authorization: resolver auth, scope
metadata, semantic descriptions, and operation fingerprints never replace
application row/field policy or database controls.

Generated write resolvers and repository helpers lower authorization-sensitive
reads and DML onto one driver-neutral mutation transaction. Existing-row
decisions use backend write locks, predicate mutations materialize typed
primary keys before exact-key DML, and absent-key upserts require the runtime's
state-machine isolation mode. Consumers do not provide SQL or lock clauses.

Generated to-many relation resolvers accept one nullable `OrderByInput`
object, while generated root list resolvers retain their nullable list of
non-null ordering objects. The macro derives relation resolver signatures and
semantic relationship descriptors from the same internal contract so public
names, nullability and `Where`/`OrderBy`/`Page` shapes remain byte-equivalent.

Conditional relations accept `source_condition(field = "...", equals = ...)`
or `target_condition(column = "...", equals = ...)` with string, integer,
float, or boolean literals. Source fields are compile-time type checked; target
columns are quoted by the selected backend and all values are bound. These
logical discriminator joins require `emit_fk = false` and are enforced by
single, pageable, DataLoader, and nested bulk-preload paths.

Server-defined computed ordering uses repeatable entity-level
`graphql_orm(order_expression(name = "...", expression = "..."))`
declarations. An expression containing `:named` bind parameters also declares
`parameters = "server_function_path"`; that function returns
`OrderExpressionParameters` from the GraphQL server context. The generated
input exposes only `OrderDirection`; the fixed, validated expression and bind
names remain compile-time server configuration. Raw backend placeholders are
rejected, and generated pagination adds missing primary-key tie-breakers.
Entities that need relationship counts can add
`order_aggregate(name = "...", aggregate = "count")` to an unconditional,
readable relation; the generated correlated count uses only its declared key
mapping and target entity table. Entities that also have handwritten complex fields use
`graphql_orm(compose_complex_object)` and apply `graphql_complex_object` to the
handwritten impl so generated relations are flattened into the same
`ComplexObject` surface.

See [core runtime documentation](../graphql-orm/README.md),
and the [macro and attribute reference](../../docs/reference/graphql-orm/macros-and-attributes.md).
