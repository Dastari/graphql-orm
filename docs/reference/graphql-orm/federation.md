---
title: "Federation entities, keys, and operation roots"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-09-17
review_by: 2027-03-01
supersedes: []
---

# Federation entities, keys, and operation roots

This is the canonical reference for taking a `graphql-orm` schema into an
Apollo Federation v2 supergraph. It covers declaring resolvable entity keys,
referencing entities owned by another subgraph, what the generated entity
resolver does and does not authorize, and the operation-root conventions the
exporter relies on.

## Declaring a resolvable entity key

`#[graphql_entity(federation_key)]` opts an entity into being a resolvable
Federation entity. The bare form keys on the entity's primary key, in
declaration order:

```rust
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[graphql_entity(table = "zones", plural = "Zones", federation_key)]
pub struct Zone {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}
```

```graphql
type Zone @key(fields: "id") { id: String! name: String! }
```

`fields = [...]` selects an explicit key by **exported GraphQL field name**,
and the attribute repeats for several keys:

```rust
#[graphql_entity(
    table = "assets",
    plural = "Assets",
    federation_key,                       // @key(fields: "id")
    federation_key(fields = ["serial"]),  // @key(fields: "serial")
)]
```

A composite key lists several fields and exports them space-separated:

```rust
#[graphql_entity(
    table = "asset_placements",
    plural = "AssetPlacements",
    federation_key,   // @key(fields: "zoneId slot") for a composite primary key
)]
```

### Accepted key fields

Every field named by a key is validated when the macro runs:

| Requirement | Why |
| --- | --- |
| The GraphQL field exists on the entity | the `@key` string is a contract other subgraphs join on |
| The field is exported and readable | a key naming a private, `input_only`, or non-read field would name an absent field |
| The field carries no field-level read policy | a key value is disclosed to every subgraph that joins the entity, so it cannot also be policy-gated |
| The field is non-null | every row must have exactly one representation |
| The field is a persisted scalar column | relations and `skip_db` fields have no column to match |
| The column set is the primary key or a declared unique constraint | a `@key` must identify at most one row |

The last rule accepts a `#[unique]` field, a `unique_index(...)`, or a
`unique_composite = "..."` declaration. For an externally managed schema whose
unique index the ORM does not declare, state the assumption explicitly:

```rust
#[graphql_entity(
    backend = "mssql",
    table = "dbo.Zones",
    schema_policy = "external_read_only",
    plural = "Zones",
    federation_key(fields = ["externalRef"], assume_unique = true),
)]
```

`assume_unique = true` is rejected when the column set *is* already declared
unique, so it never silently hides a constraint the ORM does know about.

`federation_key` is rejected on `#[repository_entity(...)]` and on
`GraphQLSchemaEntity`, because neither produces a GraphQL object to resolve.
An entity that declares a key without `GraphQLOperations` in its derive list is
a compile error rather than a schema that silently exports no key.

## How a resolver becomes a `@key`

async-graphql 7 produces `@key(... resolvable: true)` from exactly one
construct: an `#[graphql(entity)]` method inside an `#[Object] impl`. The key
string is that method's **argument** names joined by spaces. `GraphQLOperations`
therefore generates one such resolver per declared key on `{Entity}Queries`,
and renames each argument to the entity's exported field name.

That rename is load-bearing. The ORM configures argument case
(`argument-case-*`) independently of field case (`field-case-*`). Under
`field-case-pascal` with `argument-case-snake` the generated single read takes
`zone_id` while the entity exports `ZoneId`; a resolver that accepted the
default argument name would export `@key(fields: "zone_id slot")`, naming
fields that do not exist.

Entity resolvers are not schema fields. `MergedObject` forwards `find_entity`
to each member and `schema_roots!` composes `QueryRoot` from the per-entity
`{Entity}Queries` objects, so the resolver participates in `_entities` without
appearing anywhere a client can select it. Consequently the **operation
catalogue and the router-protocol descriptor are byte-identical with and
without a key** — declaring one changes no root field, argument, or
authorization requirement.

Note that the exported SDL reads `@key(fields: "id")`, not
`@key(fields: "id", resolvable: true)`: `resolvable` defaults to true, and
async-graphql only writes it when a stub sets it to false.

## Batching

`_entities` resolves every representation in one fetch concurrently. Each
generated resolver enqueues its representation on a per-entity `DataLoader`
that `schema_roots!` registers for every entity, so **N representations of one
entity type in one `_entities` fetch issue one statement**: a single select
with `column IN (...)` for a single-column key, or an OR-of-ANDs predicate for
a composite key. The rendering is dialect-neutral; it is executed and counted
on SQLite and on a disposable PostgreSQL, and compiled and SDL-verified on SQL
Server, whose execution lane is environment-gated like the rest of the SQL
Server suite.

Representations that match no row cost nothing extra; they are simply absent
from the one result set.

Batching happens within the `DataLoader`'s dispatch window. A host that
installs an entity policy performing slow, variable-duration I/O before the
database read can push some representations outside that window and observe
more than one statement. This is the same window the generated relation
resolvers already use.

## Authorization

The generated entity resolver runs the generated single-row read chain
unchanged, on the same `GraphqlQuery` surface, with the same `DbAuthContext`
propagation:

1. `enforce_resolver_auth` for the entity's `auth` mode;
2. the entity's declared `single_read` scope enforcement;
3. `enforce_resolver_assurance` for `GraphqlOperationKind::Query`;
4. `ensure_entity_access(..., EntityAccessKind::Read, EntityAccessSurface::GraphqlQuery)`;
5. the batched fetch, carrying the caller's `DbAuthContext`;
6. `can_read_row` on the loaded row.

Field-level policies then run in the entity's own field resolvers, exactly as
they do for a direct read.

### Null versus error

`_entities` runs `find_entity` for every representation under `try_join_all`,
so **any error fails the whole fetch**, not just the one representation.

| Outcome | Result |
| --- | --- |
| No row matches the representation | `null` for that representation |
| `can_read_row` denies the loaded row | `null` for that representation — indistinguishable from a miss |
| A key argument is missing or unparseable in the representation | `null` for that representation |
| `enforce_resolver_auth` denies (entity requires auth, caller anonymous) | error; the whole `_entities` fetch fails |
| Scope enforcement denies | error; the whole `_entities` fetch fails |
| Assurance denies | error; the whole `_entities` fetch fails |
| `ensure_entity_access` denies | error; the whole `_entities` fetch fails |
| A field-level read policy denies a selected field | error from that field resolver |
| The database read fails | error; the whole `_entities` fetch fails |

The null cases are deliberate: a row the caller may not see must be
indistinguishable from a row that does not exist, or the key itself becomes an
existence oracle.

### What a denial looks like through the router

The end-to-end fixture pins the observed behaviour of the workspace's Hive
router when the owning subgraph refuses an entity fetch:

- the **whole response** is nulled, not just the joined field. The parent
  subgraph's own fields do not survive alongside the error, even though the
  same caller can read them in a query that omits the join;
- the error carries `extensions.code = "DOWNSTREAM_SERVICE_ERROR"`,
  `extensions.service = "<owning subgraph>"`, and a path beginning
  `_entities`;
- the subgraph's own error **message is replaced** with a generic one. A client
  cannot read the owning subgraph's wording through the router, so operational
  detail must come from the subgraph's own logs.

Design a joined field on the assumption that refusing it costs the caller the
whole query.

### The router does not authorize a joined field

`graphql-orm-router` enforces its authorization contract over **root fields
only**, and it deliberately skips underscore-prefixed root fields, so
`_entities` and `_service` are outside that contract. A field reached by
following a `@key` into another subgraph is therefore authorized **only by the
subgraph that owns the entity**. Any entity whose rows are not universally
readable must declare its own entity policy, row policy, scope requirement, or
`auth = "required"`; a router-level rule on the parent query grants nothing on
the far side of the join. Router-level enforcement of nested directives is a
separate router decision and does not exist today.

## Referencing an entity owned by another subgraph

A subgraph that only *refers* to a foreign entity declares a plain
async-graphql stub through the re-exported `async_graphql`. There is no ORM
table behind it, so there is nothing for the ORM to generate or validate, and
no macro is provided:

```rust
use graphql_orm::prelude::*;

/// A reference to an entity owned by another subgraph.
#[derive(graphql_orm::async_graphql::SimpleObject, Clone, Debug)]
#[graphql(name = "Zone", unresolvable = "id")]
pub struct ZoneReference {
    pub id: String,
}
```

The Rust type name may differ from the GraphQL type name; `name = "Zone"` is
what makes composition treat it as the same entity. The stub's fields must be
exactly the key fields — it is a reference, not a partial copy.

A field then returns the stub from a local column, and the planner resolves the
rest through the owning subgraph's `_entities`.

### Stub-only subgraphs must enable federation

async-graphql enables federation as soon as **any resolvable key** exists. A
subgraph that owns no entity and only declares `unresolvable` stubs has no such
key, so it would serve no `_service` field and export no federation SDL. Opt in
on the schema root:

```rust
schema_roots! {
    backend: "sqlite",
    federation: true,
    query_custom_ops: [],
    entities: [Reading],
}
```

The flag is unnecessary — and harmless — once the subgraph owns a key.

## `PageInfo` is shareable

Every ORM subgraph exports an identical `PageInfo` object for its generated
connections. Federation v2 composition rejects a supergraph in which two
subgraphs define the same non-entity object field unless the type is
`@shareable`, so two ORM subgraphs could not previously compose together.
`PageInfo` now carries the directive:

```graphql
type PageInfo @shareable { ... }
```

This changes the exported SDL of **every** subgraph, whether or not it uses
Federation. A host that post-processes or snapshots the SDL text must expect
`type PageInfo @shareable` in place of `type PageInfo`.

## Introspection gains `_entities` and `_service`

Declaring any resolvable key enables federation on the schema, which adds
`_entities(representations: [_Any!]!): [_Entity]!` and `_service: _Service!` to
the introspected schema. The federation SDL export
(`SDLExportOptions::new().federation()`) continues to omit them, along with the
`_Entity` union and the `_Any` scalar, because the composer supplies its own.

A host with a fixed introspection snapshot, a schema fingerprint, or a
field-count assertion will see this change on the first entity it keys.

## Operation roots

`schema_roots!` keeps its public Rust API:

```rust
type ProviderSchema = async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>;
```

The corresponding nonempty GraphQL object names are `Query`, `Mutation`, and
`Subscription`. Async-graphql's federation exporter omits an explicit
`schema { ... }` definition, so these conventional names provide the
standards-defined implicit operation mapping. Generated fields are therefore
direct fields of the query operation root rather than fields on an unreachable
`QueryRoot` child type.

Export federation SDL normally:

```rust
let sdl = schema.sdl_with_options(
    async_graphql::SDLExportOptions::new().federation(),
);
```

Async-graphql excludes subscriptions from federation SDL by default. When a
federation deployment intentionally exposes subscriptions, opt in while
building the schema:

```rust
let schema = schema_builder(database)
    .enable_subscription_in_federation()
    .finish();
```

`EmptyMutation` and `EmptySubscription` remain absent from the exported SDL.
They never create a dangling operation mapping or a fieldless placeholder
object.

## Composition acceptance

The repository tests parse the actual macro-generated federation SDL and
resolve explicit or conventional operation roots independently of
async-graphql. They assert that every declared root exists, that generated
provider fields are direct members of `Query`, and that each `@key` appears
exactly once with its exact field list.

An end-to-end fixture workspace serves two real ORM subgraphs over loopback —
one owning a keyed entity, one holding only an `unresolvable` stub — composes
them through `graphql-orm-router`, and proves that composition succeeds, that
the joined field resolves through an `_entities` fetch, that three
representations cost the owning subgraph one statement, and that an unscoped
caller is refused by the owning subgraph while the router grants nothing.

For a connected external graph, validate regenerated provider SDL without
publishing it, and review the resulting query root nodes before promotion. Do
not post-process the generated SDL to rename or reattach operation types.
