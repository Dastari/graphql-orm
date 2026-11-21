---
title: "Generated Resolver Operation Metadata"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-20
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
`graphql-orm-sha256-len-v2`. Version 2 does not hash the Rust module path of
the entity, which version 1 did; every value differs from its version 1 value.
It hashes a domain-separated sequence of
UTF-8 label/value fields. Labels and values are each prefixed with an unsigned
eight-byte big-endian length. Digests are lowercase SHA-256 hex:

- a generated descriptor fingerprint binds its entity name and table, backend,
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

## Relation and entity resolvers

The operation catalogue lists root fields. Relationship fields and Federation
entity resolvers are not root fields: a relation resolves below an object and
an entity resolver is reached through `_entities`. `schema_roots!` therefore
also emits `graphql_orm_resolver_catalog()`, which returns a
`GraphqlResolverCatalog` (catalogue 0.11.0): a separate, versioned and
fingerprinted document bound to `graphql_orm_semantic_catalog()`. Composing it
changes no SDL, descriptor or fingerprint described above.

```rust,ignore
let resolvers = graphql_orm_resolver_catalog()?;

// Every relationship field the schema serves, generated or handwritten.
let orders = resolvers.relation("Customer", "orders").unwrap();
assert_eq!(orders.target, "Order");

// Every entity the schema resolves by a `@key`, including the `NodeId` key.
let customer = resolvers.entity("Customer").unwrap();
assert_eq!(customer.keys[0].fields, ["id"]);
```

The function returns a `Result`. A document that does not validate against
the semantic catalogue is an error, and so is a semantic catalogue that does
not compose; neither is a panic. `graphql_orm_semantic_catalog()` keeps its
signature and still panics in the second case.

The served schema decides what is listed and under which names. The function
reads the query root's type registry, so:

- a relation declared with `#[relation(...)]` on an entity that mounts no
  resolver for it (no `GraphQLRelations`, or a composed object that omits it)
  is not listed, although the semantic catalogue and the client metadata still
  describe it;
- a served field counts as the relationship only when it returns the related
  type (or a connection over it) and takes the declared arguments. A
  handwritten `parent: String` beside a declared `parent: Parent` relation is
  not listed;
- a relation is `generated` when the entity's object mounts the generated
  resolver, including through `#[graphql_complex_object(...)]` with any
  options; any other served relationship is `custom`;
- a declared key the schema does not resolve is not listed, and neither is
  any key of a type marked `unresolvable`;
- an entity whose type the schema does not register has no entry, even when
  another served type has its name;
- a type is listed under its registered name. An entity served as
  `#[graphql(name = "PublicCustomer")]` appears as `PublicCustomer`, with
  `semantic_entity_name: "Customer"` so the entry still binds to the semantic
  catalogue; relations into it carry `semantic_target` the same way;
- field and key names follow the configured naming rules and any
  `#[graphql(name = "...")]` on the field.

Each descriptor carries an authorization reference instead of a requirement:

| Resolver | Static requirement the ORM enforces | Reference |
| --- | --- | --- |
| to-many relation | the target's auth mode and its `list` scopes | the target's list root, when exposed |
| to-one relation | the target's auth mode and its `single_read` scopes | the target's single-read root, when exposed |
| entity resolver, `NodeId` resolver | the entity's auth mode and its `single_read` scopes | the entity's single-read root, when exposed |

The reference is `subgraph_only` when the comparison would not be exact:

- the mapped category declares scope templates. A traversal has no root
  arguments to bind and is refused; an entity resolver binds the template from
  key fields, not root arguments;
- the mapped category is undeclared while another read category is declared.
  The traversal then requires every declared read category's scopes;
- `query_shapes` does not expose the root; or
- the relation is handwritten, or its target is not in `entities:`.

The owning entity's auth mode, field policies, row policies and RLS apply to
every traversal in addition. The catalogue is descriptive and is never read
back by the server.

Handwritten `#[graphql(entity)]` resolvers are not listed.
See the [catalogue README](../../../crates/graphql-orm-operation-catalog/README.md#relation-and-entity-resolvers)
for the document format, its RFC 8785 fingerprint, and
[what each export discloses](../../../crates/graphql-orm-operation-catalog/README.md#what-an-export-discloses).

## Backend and policy behavior

- Writable SQLite/PostgreSQL single-key entities report the generated query,
  mutation, and subscription categories that exist for their declaration.
- Search and keyset categories appear only when those resolver methods are
  generated.
- Composite-key entities report list and exact complete-key single reads; the
  current composite mutation opt-in remains repository-only.
- Append-only entities report reads, create, and subscription, but no
  update/delete/upsert resolver categories.
- External-read-only entities report only generated reads. External-writable
  MSSQL entities report the generated DML categories supported by their
  declaration while schema management remains absent.
- Schema-root generated mutation `none`/allowlist/denylist policy changes
  mutation exposure and catalog fingerprints without disabling repository
  writes or generated subscriptions. Root-level read-only policy marks both
  generated mutations and subscriptions unexposed.
