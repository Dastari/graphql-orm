---
title: ADR-0009 Nine-package backend-neutral discovery boundaries
kind: decision
status: accepted
owner: workspace-maintainers
last_reviewed: 2026-08-11
review_by: 2027-08-11
supersedes: [ADR-0007]
---

# ADR-0009: Nine-package backend-neutral discovery boundaries

## Context

Generated GraphQL operation metadata and reviewed AI GraphQL tool profiles are
needed by owning subgraphs before an AI persistence backend or coordinator is
selected. Keeping those contracts in `graphql-orm` and `graphql-orm-ai`
couples otherwise database-neutral manifest producers to mutually exclusive
backend features. Cargo feature unification then prevents a workspace from
building an MSSQL application subgraph and a SQLite AI runtime together.

Discovery metadata never grants execution authority, so it also benefits from
a smaller dependency and trust boundary than either runtime package.

## Decision

The workspace has nine independently consumable packages. Two backend-neutral
packages own the canonical discovery contracts:

- `graphql-orm-operation-catalog` owns generated resolver-operation metadata
  and optional conversion into router-protocol declarations.
- `graphql-orm-ai-tool-profiles` owns reviewed tool-profile inputs, validation,
  compiled GraphQL documents, disclosure contracts, manifests, serialized wire
  values, and stable fingerprints.

`graphql-orm` and `graphql-orm-ai` re-export their former public surfaces for
source compatibility. There is one serialized manifest type and one
fingerprint implementation; runtime consumers do not translate producer
values.

The added dependency edges are:

`graphql-orm -> graphql-orm-operation-catalog` and
`graphql-orm-ai -> graphql-orm-ai-tool-profiles -> graphql-orm-operation-catalog`.

The operation-catalog package has no database, macro, execution, or
application-policy dependency. The tool-profile package has no persistence,
backup, storage, provider, coordinator, or database-backend dependency.
Generated candidates still require an exact host admission policy and current
operation catalogue. Custom profiles validate against a complete finished
schema. Neither package discovers operations at runtime or grants authority.

AI persistence backends remain mutually exclusive. Host applications that use
backup declare `graphql-orm-backup` directly with their chosen features; the AI
runtime does not pull backup into a profile producer's feature graph.

## Consequences

- Mixed-backend workspaces can compile manifests in owning subgraphs and
  consume them in a separately backed AI runtime in one Cargo invocation.
- Producer and consumer fingerprints remain byte-identical because the wire
  model and compiler have one owner.
- Resolver discovery, profile validation, runtime authorization, and actual
  GraphQL execution remain distinct security boundaries.
- Changes to either neutral wire contract require coordinated SemVer,
  compatibility, dependency-integrity, and backend-coexistence testing.

## Supersession

This record supersedes
[ADR-0007](ADR-0007-seven-package-workspace-boundaries.md). ADR-0007 remains
immutable and discoverable. A future package-ownership or dependency-direction
change requires a later superseding ADR.
