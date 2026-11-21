---
title: ADR-0015 Router enforces composed authorization directives
kind: decision
status: accepted
owner: graphql-orm-router-maintainers
last_reviewed: 2026-11-16
review_by: 2027-11-16
supersedes: []
---

# ADR-0015: Router enforces composed authorization directives

## Context

`graphql-orm-router` authorized an operation from its protocol descriptors,
and only for root fields. Composition kept Federation's `@authenticated` and
`@requiresScopes` directives in the supergraph, and the router restored their
`SECURITY` links, but nothing below the root evaluated them. A nested field
that carried one, including a field reached through an entity join, was
authorized only by its subgraph. An old changelog entry read as if the router
enforced them. [ADR-0011](ADR-0011-generated-federation-entity-keys.md) left
router enforcement of nested directives to a later router decision.

Federation routers either remove an unauthorized field and return the rest
with an error, or refuse the operation. The first trims the operation that is
planned and executed. The router's root policy already refuses the whole
operation.

## Decision

An authenticated router evaluates the composed `@authenticated` and
`@requiresScopes` directives on every selected field of the normalized
operation, at the root and below it, before any downstream request opens.

- A field `P.f` returning named type `R` requires the rules on `P`, `P.f` and
  `R`. A field selected on an interface also requires the rules of every
  implementation of that field and of its type, and a field of an
  implementation requires the rules declared on the same interface field and
  on the interface, including through `@interfaceObject`. An inline
  fragment, or a normalized fragment spread, requires the rule on its type
  condition. `@requiresScopes` is an OR of AND sets, uses the router's scope
  matcher, and implies authentication.
- Composition merges the `@requiresScopes` sets of several subgraphs into one
  list of alternatives. The router instead keeps each subgraph's declaration,
  read from its SDL with composition's directive-name resolution, and
  requires all of them. A malformed declaration, a directive application
  composition could read as either directive that does not resolve to one,
  either directive on a location the router cannot enforce (arguments, input
  fields, enum values, unions, input objects, the schema), or a composed
  directive the declarations do not account for, refuses the graph, so the
  last-known-good graph stays active.
- A refused selection refuses the whole operation, with `UNAUTHENTICATED` or
  `FORBIDDEN` errors that name only the response path. The router does not
  trim the operation or return partial data for it.
- `__typename` is never refused; `__schema` and `__type` follow the
  introspection policy.
- The check may deny early but never grants. Subgraph resolver guards and
  policies stay authoritative, and `_entities` remains outside the descriptor
  contract.
- Anonymous development mode, which has no authorization catalogue, evaluates
  nothing.

## Consequences

- Hosts whose clients selected protected nested fields without the authority
  now get a refused operation instead of the subgraph's answer. This is a
  breaking change shipped in the breaking batch with an **Action needed**
  migration.
- Refusing rather than trimming keeps the planned operation the one the
  client sent and keeps the response shape the root policy already has. A
  client that wants the rest of the data must split the protected selection
  into its own operation.
- Requiring every implementation's rules on an interface field fails closed
  even when composition does not propagate implementation rules onto the
  interface. It can refuse an interface field that one implementation
  protects, for a caller that would only ever receive another implementation.
- Reading each subgraph's declarations duplicates part of composition's
  directive-name resolution. The cross-check against the composed sets turns
  any disagreement into a refused graph rather than a weaker rule.
- ORM policies without an SDL directive (row and entity policies, templated
  scopes, custom guards) are still enforced only by their subgraph.

## Supersession

This decision does not supersede an earlier record. It is the later router
decision that [ADR-0011](ADR-0011-generated-federation-entity-keys.md) names
for nested directive enforcement; ADR-0011's entity-key decision and its rule
that the owning subgraph authorizes a joined field both stand.
