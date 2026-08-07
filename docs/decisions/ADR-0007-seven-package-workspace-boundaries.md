---
title: ADR-0007 Seven-package workspace boundaries
kind: decision
status: accepted
owner: workspace-maintainers
last_reviewed: 2026-08-07
review_by: 2027-08-07
supersedes: [ADR-0002]
---

# ADR-0007: Seven-package workspace boundaries

## Context

The router initiative adds a reusable federation router and a versioned
subgraph/router protocol. They must remain independently consumable without
turning router runtime, federation, authentication, or application concerns
into ORM requirements.

## Decision

The workspace has seven independently consumable packages. The existing
acyclic edges remain, and these internal edges are added:

`graphql-orm-router -> graphql-orm-router-protocol` and optional
`graphql-orm -> graphql-orm-router-protocol`.

`graphql-orm-router` may consume external federation runtime and composition
packages and may optionally consume the external exact-revision `agql-auth`
contract. Neither router package depends on `graphql-orm`, AI, backup, storage,
or an application.

`graphql-orm-router-protocol` owns serializable data and versioning only. It
does not depend on a federation runtime, server framework, database backend, or
application package. `graphql-orm-macros` has no direct protocol or router
dependency; it may emit paths only to feature-gated types re-exported by
`graphql-orm`.

All internal dependencies use workspace paths and the root lockfile. Public
router APIs hide federation-engine types. No edge may introduce a cycle.

## Consequences

- Services can adopt the protocol without a federation runtime, server
  framework, database backend, or the ORM; generic router users do not acquire
  GEMA or ORM dependencies.
- The ORM can opt in to protocol export without depending on router execution.
- Router engine and composition changes stay behind the router adapter, while
  protocol compatibility remains separately versioned.
- Adding or changing an internal edge requires updating dependants, workspace
  inventory, dependency-integrity checks, CI lanes, and system context in the
  same branch.

## Supersession

This record supersedes
[ADR-0002](ADR-0002-workspace-package-boundaries.md). ADR-0002 remains
immutable and discoverable. A future package-ownership or dependency-direction
change requires a later superseding ADR.
