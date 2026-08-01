---
title: ADR-0001 Documentation authority and lifecycle
kind: decision
status: accepted
owner: workspace-maintainers
last_reviewed: 2026-08-01
review_by: 2027-08-01
supersedes: []
---

# ADR-0001: Documentation authority and lifecycle

## Context

The repository accumulated flat references, implementation ledgers, agent
handoffs, duplicated release notes, and active plans beside individual crates.
Several documents described older standalone repositories and dependency
versions after monorepo consolidation. Readers could not reliably distinguish
current mechanics, durable rationale, active work, and historical evidence.

## Decision

The central `docs/` hierarchy is authoritative by document purpose:
architecture, decisions, operations, development, reference, plans,
investigations, and archive. Component-local READMEs and topical references
remain beside their code and are linked from the central index.

Every governed Markdown document declares `title`, `kind`, `status`, `owner`,
`last_reviewed`, `review_by`, and `supersedes`. Embedded skill-package Markdown
under `crates/*/.agents/` is vendor-maintained and exempt. The GitHub PR form is
also exempt because GitHub owns its body syntax.

One active canonical document owns each topic. Code/configuration/schema and
generated inventories describe mechanics; architecture describes durable
boundaries; ADRs explain why; runbooks describe operations. Accepted ADRs are
immutable and can be replaced only by a later superseding ADR.

Active plans exist only at `docs/plans/active/<initiative>/README.md` and hold
outcome, non-goals, dependencies, acceptance gates, and one current checkpoint.
Completed plans move to `completed/`. Investigations and incident evidence are
archived, not deleted. Temporary prompts/session handoffs use ignored
`.handoff/`, never the authoritative namespace.

Release chronology lives in package `CHANGELOG.md`; migration obligations live
in package `MIGRATION.md`. Dependency/version inventories are generated from
manifests. Pull requests declare documentation impact. CI checks the enforceable
parts of this decision.

## Consequences

- Historical evidence remains available without presenting itself as current.
- Moving or adding docs requires link and index maintenance.
- Review dates deliberately turn stale guidance into a visible CI failure.
- Semantic duplication still requires reviewer judgment; CI cannot determine
  whether differently titled documents discuss the same topic.

## Supersession

Future changes to this authority model require a new ADR that supersedes this
record. Do not edit this accepted decision.
