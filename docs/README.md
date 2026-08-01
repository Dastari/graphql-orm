---
title: GraphQL ORM documentation index
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-01
review_by: 2026-11-01
supersedes: []
---

# GraphQL ORM documentation

This is the authoritative index for the monorepo. Start with the system
context, then follow the document type that matches the question:

- [Architecture](architecture/system-context.md) describes durable system
  boundaries and current design.
- [Decisions](decisions/README.md) explain why durable choices were made.
- [Operations](operations/README.md) contains executable runbooks and release
  guidance.
- [Development](development/README.md) covers setup, tests, and GraphQL work.
- [Reference](reference/README.md) describes current APIs and mechanics.
- [Plans](plans/README.md) records current outcomes and the bounded backlog.
- [Investigations](investigations/README.md) retains evidence and conclusions.
- [Archive](archive/README.md) retains superseded chronology, prompts, and
  ledgers; it is not current guidance.

## Current architecture

- [System context and package boundaries](architecture/system-context.md)
- [Authentication and authorization](architecture/authentication-and-authorization.md)
- [Operation assurance](architecture/operation-assurance.md)
- [Portable persistence](architecture/portable-persistence.md)
- [Schema modules and fenced leases](architecture/schema-modules-and-leases.md)
- [Storage and backup boundaries](architecture/storage-and-backup-boundaries.md)

## Component documentation

Component-local documents remain beside the package whose contract they
describe:

- [`graphql-orm`](../crates/graphql-orm/README.md)
- [`graphql-orm-macros`](../crates/graphql-orm-macros/README.md)
- [`graphql-orm-storage`](../crates/graphql-orm-storage/docs/README.md)
- [`graphql-orm-backup`](../crates/graphql-orm-backup/docs/README.md)
- [`graphql-orm-ai`](../crates/graphql-orm-ai/docs/README.md)

The [generated workspace package inventory](reference/workspace-packages.md)
is the only manually linked version/dependency overview. Regenerate it with
`python3 scripts/generate-workspace-inventory.py` from the workspace root.

## Authority and lifecycle

One active canonical document owns each topic. Code, configuration, schemas,
and generated inventory describe current mechanics. Architecture documents
describe durable boundaries. Accepted ADRs explain decisions and are immutable;
a later ADR supersedes an earlier one. Runbooks describe repeatable operations.

Active plans exist only at `docs/plans/active/<initiative>/README.md`. They
contain the outcome, non-goals, dependencies, acceptance gates, and current
checkpoint—not session transcripts. Completed plans move to `completed/`.
Investigations and incident evidence are archived rather than deleted.
Temporary agent/session material belongs in the ignored `.handoff/` directory.

The complete policy and exceptions are defined by
[ADR-0001](decisions/ADR-0001-documentation-authority-and-lifecycle.md). The
[disposition inventory](document-inventory.md) records the 2026 cleanup.

## Required metadata

Every governed Markdown document starts with:

```yaml
---
title: A unique descriptive title
kind: architecture | decision | runbook | plan | investigation | reference
status: draft | active | accepted | superseded | archived
owner: accountable-maintainer-group
last_reviewed: YYYY-MM-DD
review_by: YYYY-MM-DD | none
supersedes: []
---
```

Use the [templates](templates/README.md) for new ADRs, plans, runbooks, and
investigations. CI checks metadata, local links, ADR numbering and immutability,
active-plan placement, stale paths, generated inventory drift, and pull-request
documentation impact.
