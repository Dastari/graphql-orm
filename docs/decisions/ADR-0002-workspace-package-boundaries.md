---
title: ADR-0002 Workspace package boundaries
kind: decision
status: accepted
owner: workspace-maintainers
last_reviewed: 2026-08-01
review_by: 2027-08-01
supersedes: []
---

# ADR-0002: Workspace package boundaries

## Context

Consolidation put the ORM, macros, storage, backup, and AI packages in one
repository. Workspace proximity can tempt circular dependencies, feature-based
bundling, or internal Git pins that destroy independent consumption and create
multiple public type universes.

## Decision

The five packages remain independently consumable. The permitted dependency
direction is:

`graphql-orm-ai -> graphql-orm-backup -> graphql-orm-storage`,
`graphql-orm-ai -> graphql-orm`, optional
`graphql-orm-backup -> graphql-orm`, and
`graphql-orm -> graphql-orm-macros`.

Internal dependencies use workspace path dependencies and the root lockfile.
AI, backup, and storage do not become core ORM features or optional core
dependencies. Reusable contracts are implemented in their owning package and
all affected dependants change in the same branch. `agql-auth` remains an
external exact-revision dependency and only documented one-way integrations may
consume it.

## Consequences

- The graph stays acyclic and packages retain clear responsibilities.
- Cross-package changes are easier to review atomically in one repository.
- Consumers select only the packages and backend/provider features they need.
- A convenience that violates ownership must be expressed as a host adapter,
  not a reverse dependency or copied contract.

## Supersession

A dependency-direction or package-ownership change requires a new ADR that
supersedes this one.
