---
title: ADR-0010 Independent package and workspace release identities
kind: decision
status: accepted
owner: workspace-maintainers
last_reviewed: 2026-08-11
review_by: 2027-08-11
supersedes: []
---

# ADR-0010: Independent package and workspace release identities

## Context

The repository is a virtual Cargo workspace containing independently
consumable libraries and one executable. Packages evolve at different rates,
while consumers commonly select several packages from one reviewed Git
revision. Historical unqualified `vX.Y.Z` tags became ambiguous once package
versions diverged. A full commit SHA is precise but does not by itself provide
release notes, a tested compatibility-set identity, or artifact provenance.

The workspace is intentionally Git-only. Registry publication is disabled,
and exact external Git dependencies are part of the reviewed source universe.

## Decision

Package SemVer remains independent. `graphql-orm` and
`graphql-orm-macros` stay aligned because runtime and generated code form one
compatibility boundary; no other package is forced into their version.

Each released package version receives an immutable qualified tag in the form
`<package>-v<version>`. A tested repository-wide package set receives a
calendar-ordered `workspace-YYYY.MM.DD.N` release identity. The workspace
release attaches a deterministic manifest binding:

- the exact full commit and root lockfile hash;
- every package version, qualified tag, and package source-tree identity;
- exact external Git dependencies and consumers; and
- independently versioned persistence and wire contracts.

Consumers continue to pin the full commit SHA. Neither a package tag nor a
workspace tag replaces that requirement.

Release publication is a protected explicit operation after the complete
release matrix passes. Tags and release assets never move. Registry
publication remains disabled. Compiled router delivery is opt-in and requires
artifact-specific distribution evidence independently of source release
approval.

## Consequences

- Package versions communicate package compatibility without unrelated
  lockstep bumps.
- A workspace release names the exact combination tested together without
  inventing a tenth Cargo-package version.
- Package-qualified tags remain unambiguous in one Git repository.
- Release manifests make source, lockfile, dependency, schema, and wire
  identities machine-readable and attestable.
- A changed package cannot reuse an existing package version in a workspace
  release because tag verification compares its exact source tree.
- The source-only release path remains independent of binary/container
  licensing, SBOM, notice, target, and provenance approval.

## Supersession

A future change to unified versioning, registry distribution, mutable release
channels, or tag identity requires a later superseding ADR. Published release
identities remain immutable regardless of a later process change.
