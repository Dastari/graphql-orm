---
title: "GraphQL ORM monorepo consolidation"
kind: plan
status: accepted
owner: workspace-maintainers
last_reviewed: 2026-08-01
review_by: 2027-02-01
supersedes: []
---

# GraphQL ORM monorepo consolidation

> **Completed plan.** The 2026 consolidation is complete. The detailed
> implementation evidence is retained in the
> [consolidation ledger](../../../archive/2026/monorepo-consolidation-ledger.md).

## Outcome

The independently consumable `graphql-orm`, `graphql-orm-macros`,
`graphql-orm-storage`, `graphql-orm-backup`, and `graphql-orm-ai` packages now
live in one workspace and resolve internal dependencies through workspace paths.
Consumers select only the packages and features they need from the reviewed
monorepo revision.

## Non-goals

- Moving the external `agql-auth` repository into this workspace.
- Making AI, backup, or storage optional features of the core ORM crate.
- Rewriting published history as part of a rollback.

## Dependencies

The completed cutover depends on the workspace dependency direction, the root
`Cargo.lock`, and consumers pinning a single reviewed full Git revision for
every selected `graphql-orm-*` package.

## Acceptance gates

- Each package remains independently consumable.
- Internal packages use workspace path dependencies; none uses another
  workspace package through Git.
- The workspace dependency direction remains acyclic.
- Consumer migrations use one reviewed monorepo revision and preserve only
  their required package and feature selections.

## Final checkpoint

The consolidation baseline was accepted and the old planning evidence is
archived. Ongoing setup and test guidance lives in
[development setup](../../../development/setup.md) and
[development testing](../../../development/testing.md).
