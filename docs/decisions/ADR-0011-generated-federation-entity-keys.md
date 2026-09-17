---
title: ADR-0011 Generated Federation entity keys
kind: decision
status: accepted
owner: graphql-orm-maintainers
last_reviewed: 2026-09-17
review_by: 2027-09-17
supersedes: []
---

# ADR-0011: Generated Federation entity keys

## Context

The workspace has a Hive-based Federation router
([ADR-0008](ADR-0008-hive-federation-runtime-and-composition-boundary.md)) and
an ORM that generates complete subgraphs, but nothing connecting the two: an
ORM subgraph exported no `@key`, so the query planner could not join across two
ORM subgraphs. Every cross-subgraph relationship had to be resolved by the
client or by a hand-written aggregation layer.

Three facts about async-graphql 7.2.1 constrain any solution:

- `@key(... resolvable: true)` is produced by exactly one construct: an
  `#[graphql(entity)]` method inside an `#[Object] impl`. The key string is the
  method's *argument* names joined by spaces. There is no attribute that
  declares a key on a type directly.
- Entity resolvers are not schema fields. `MergedObject` forwards `find_entity`
  to each member, so a resolver generated on the per-entity queries object that
  `schema_roots!` already composes participates in `_entities` without becoming
  selectable.
- `_entities` resolves every representation concurrently under `try_join_all`.
  `Ok(None)` yields null for one representation; any `Err` fails the whole
  fetch.

A fourth fact blocked composition outright: every ORM subgraph exports an
identical `PageInfo` object, and Federation v2 composition rejects a supergraph
in which two subgraphs define the same non-entity object field without
`@shareable`.

The router's authorization contract covers root fields and deliberately skips
underscore-prefixed ones, so `_entities` is outside it. Whatever joins the
graph must therefore be authorized entirely by the subgraph that owns the
entity.

## Decision

Generate Federation entity keys from a declarative attribute, and authorize
them exactly like the read they replace.

**Declaration.** `#[graphql_entity(federation_key)]` keys on the primary key;
`federation_key(fields = [...])` names exported GraphQL fields; the attribute
repeats for several keys. The ORM validates at macro time that each named field
exists, is exported and readable, carries no field-level read policy, is
non-null, is a persisted scalar column, and that the column set is the primary
key or a declared unique constraint. `assume_unique = true` is the explicit,
functionally named escape hatch for an externally managed schema whose unique
index the ORM does not declare, and is itself rejected when the constraint *is*
declared.

**Codegen.** `GraphQLOperations` emits one `#[graphql(entity)]` resolver per
key on the generated queries object, with each argument renamed to the entity's
exported GraphQL field name. The rename is not cosmetic: the ORM configures
argument case independently of field case, so accepting the default argument
name would export a `@key` naming fields that do not exist.

**Authorization.** The resolver reproduces the generated single-row read chain
unchanged, on the same `GraphqlQuery` surface and with the same `DbAuthContext`
propagation. A missing row and a row-policy denial both resolve to null, so a
row the caller may not see is indistinguishable from a row that does not exist
and the key is not an existence oracle. Entity-policy, scope, assurance, and
authentication denials are errors, and `try_join_all` makes them fail the whole
fetch.

**Batching.** A dedicated `FederationKeyLoader` batches representations per
entity type per fetch into one statement. It does not reuse the relation
loader: `BatchLoadEntity` is written by hand by consumers and names a single
batch column, while a generated `@key` may span several columns.

**Composition.** `PageInfo` is declared `@shareable`. It carries no owned data,
so the directive is an accurate description rather than a composition
workaround.

**Reference stubs stay hand-written.** A subgraph that only refers to a foreign
entity declares a plain async-graphql `SimpleObject` with
`unresolvable = "..."`. There is no table behind it, so the ORM has nothing to
generate or validate, and a macro would only obscure that. `schema_roots!`
gains `federation: true` for the case where such stubs are a subgraph's only
Federation participation and no resolvable key exists to enable federation.

**A declared key without `GraphQLOperations` is a compile error.** The entity
derive cannot see the rest of the derive list, so it emits a bound against a
trait only the operations derive implements.

## Consequences

- Two ORM subgraphs can compose and the planner can join between them without a
  hand-written aggregation layer.
- Declaring a key changes the operation catalogue and the router-protocol
  descriptor by nothing at all, so discovery, drift detection, and the router's
  root contract are unaffected. This is asserted by test.
- Declaring a key enables federation on the schema, which adds `_entities` and
  `_service` to introspection. The federation SDL export still omits them. A
  host with a fixed introspection snapshot or schema fingerprint sees this on
  its first keyed entity.
- `PageInfo` gains `@shareable` in every subgraph's exported SDL, whether or
  not it uses Federation. A host that post-processes or snapshots
  `type PageInfo {` must expect `type PageInfo @shareable {`.
- **A joined field is authorized only by the subgraph that owns the entity.**
  The router's gate covers root fields, so an entity whose rows are not
  universally readable must carry its own entity policy, row policy, scope
  requirement, or `auth = "required"`. Router-level enforcement of nested
  authorization directives would be a separate router decision; it does not
  exist and this ADR does not create it.
- Key uniqueness is enforced from ORM declarations, not from the database. An
  external schema can still violate it through `assume_unique`, in which case
  the resolver returns one arbitrary matching row. The escape hatch is named
  for what it asserts so that this is visible at the declaration site.
- Batching depends on the `DataLoader` dispatch window. A host whose entity
  policy performs slow, variable-duration I/O before the read can split a batch.
  This is the same window the generated relation resolvers already use.

## Alternatives considered

- **A `#[federation_entity]` derive producing its own object.** Rejected: the
  key must be registered against the entity's existing GraphQL type, and a
  second object would fork the field-policy and relation surface.
- **Reusing the relation `DataLoader`.** Rejected: its `BatchLoadEntity` bound
  is implemented by hand by consumers, so every federated entity would have
  needed a hand-written impl naming a single batch column.
- **Generating the reference stub.** Rejected: the stub is three lines of plain
  async-graphql with no ORM state, and a macro would imply the ORM validates a
  type it knows nothing about.
- **Enforcing the joined field in the router.** Rejected here as out of scope:
  the router's contract is root-field shaped, and changing it is a router
  architecture decision requiring its own record.

## Supersession

This decision does not supersede an earlier record. Router-level enforcement of
nested authorization directives, or any change that makes `_entities` part of
the router's authorization contract, requires a later ADR owned by the router
maintainers.
