---
title: "Generated Resolver Operation Metadata"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-08-01
review_by: 2027-02-01
supersedes: []
---

# Generated Resolver Operation Metadata

`GraphQLOperations` emits project-agnostic metadata for every GraphQL resolver
it actually generates. The metadata lets a host bind reviewed configuration to
an exact generated root field without parsing expanded Rust source or
reconstructing resolver names.

Metadata is discovery and drift detection only. Finding a descriptor does not
register or enable a tool, authorize GraphQL execution, bypass entity/row/field
policy or RLS, classify data, or approve any result for disclosure.

## Generated and composed metadata

There are two intentionally separate layers:

- `Entity::generated_graphql_operations()` returns immutable declarations from
  that entity's `GraphQLOperations` expansion. These values say which resolver
  structs and fields were generated for the selected backend/entity profile.
- `graphql_orm_operation_catalog()` is emitted by `schema_roots!`. It resolves
  whether each generated mutation or subscription was actually merged after
  root/backend read-only policy and `generated_mutations` allow/deny policy.
  Queries remain exposed whenever their generated operation type is part of
  that schema.

This separation is required because an entity derive cannot know how a later
schema root will compose it.

```rust
use graphql_orm::prelude::*;

let generated = User::generated_graphql_operations();
let list = generated
    .iter()
    .find(|operation| {
        operation.category() == GeneratedGraphqlOperationCategory::List
    })
    .expect("User list resolver is generated");

assert_eq!(list.kind(), GraphqlOperationKind::Query);
assert_eq!(list.root_type(), "Query");
assert_eq!(list.field_name(), "users");

let catalog = graphql_orm_operation_catalog();
let exposed = catalog
    .resolve(GraphqlOperationKind::Query, "users")
    .expect("Query.users is uniquely exposed");

assert!(exposed.is_exposed());
assert_eq!(exposed.generated().fingerprint().len(), 64);
assert_eq!(catalog.fingerprint().len(), 64);
```

The root field coordinate (`Query.users`) is not the operation name inside a
server-authored document such as `query ReviewedUsers`. Document parsing,
variable validation, selected result fields, and document hashing remain
host/downstream responsibilities.

## Descriptor contents

Each generated descriptor exposes:

- fully qualified Rust entity identity from `module_path!`;
- entity, physical table, and backend identity;
- root kind, conventional root type, exact case-adjusted field name, and stable
  semantic category;
- arguments in declaration order with exact GraphQL names, Rust type
  spellings, and GraphQL type signatures;
- generated Rust and GraphQL result type signatures;
- a diagnostic canonical schema signature covering derive-owned entity,
  backend, naming, field visibility, input, filter, order, JSON, search,
  subscription, and policy declarations; and
- a generated descriptor fingerprint.

The schema-root-resolved descriptor adds `is_exposed()` and its own fingerprint.
The catalog retains generated-but-omitted mutations with `is_exposed() ==
false`, so diagnostics can distinguish unavailable generation from root policy
omission. `resolve` returns only a uniquely exposed coordinate and otherwise
fails closed with `None`.

Stable categories are list, single read, search, keyset list, create, upsert,
update, update-many, delete, delete-many, and subscription. `List` describes a
connection resolver shape; it does not prove a fixed runtime record limit.
`PaginationConfig` remains host-configurable, including an explicitly
unbounded trusted configuration.

Private typed read projections are repository-only and never create GraphQL
resolvers. Adding or changing one therefore does not add a descriptor or alter
the generated GraphQL operation fingerprint.

## Fingerprints

`GRAPHQL_OPERATION_FINGERPRINT_ALGORITHM` identifies
`graphql-orm-sha256-len-v1`. Version 1 hashes a domain-separated sequence of
UTF-8 label/value fields. Labels and values are each prefixed with an unsigned
eight-byte big-endian length. Digests are lowercase SHA-256 hex:

- a generated descriptor fingerprint binds its entity/type identity, backend,
  operation coordinate/category, argument/result declarations, and canonical
  derive-owned schema signature;
- a resolved descriptor fingerprint additionally binds the schema-root
  exposure decision; and
- a catalog fingerprint binds every resolved descriptor after deterministic
  ordering by operation kind, field name, entity Rust type, and category.

For a pinned graphql-orm revision, identical derive input, feature selection,
backend, module identity, and root exposure produce identical fingerprints.
Entity order in `schema_roots!` and field declaration order do not affect the
catalog/schema signature ordering. Moving an entity to another Rust module is
an identity change and therefore changes its fingerprints. Consumers should
compare the algorithm identifier as well as the digest; a future release that
changes canonical encoding must use a new identifier.

These are drift-detection fingerprints, not cryptographic signatures or
authority proofs. They do not bind:

- custom query/mutation/subscription root types;
- the complete finished host SDL or a remote schema registry;
- a server-authored GraphQL document or its operation name;
- selected result fields or a downstream result projection;
- model-facing argument schemas or data-disclosure classification;
- runtime pagination, depth, complexity, rate, or output limits; or
- current authentication, authorization, policy-provider, or RLS decisions.

A host requiring a complete target-schema fingerprint must fingerprint its
finished schema/registry and may include this catalog fingerprint as one
generated-surface component.

## Backend and policy behavior

- Writable SQLite/PostgreSQL single-key entities report the generated query,
  mutation, and subscription categories that exist for their declaration.
- Search and keyset categories appear only when those resolver methods are
  generated.
- Composite-key entities report list and exact complete-key single reads; the
  current composite mutation opt-in remains repository-only.
- Append-only entities report reads, create, and subscription, but no
  update/delete/upsert resolver categories.
- External-read-only entities and MSSQL report only generated reads.
- Schema-root generated mutation `none`/allowlist/denylist policy changes
  mutation exposure and catalog fingerprints without disabling repository
  writes or generated subscriptions. Root-level read-only policy marks both
  generated mutations and subscriptions unexposed.
