---
title: GraphQL ORM AI production-readiness plan
kind: plan
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-01
review_by: 2026-09-01
supersedes:
  - crates/graphql-orm-ai/docs/completion-plan.md
---

# GraphQL ORM AI production readiness

## Outcome

Provide a defensible production profile for `graphql-orm-ai` on SQLite and
PostgreSQL: durable AI state can be backed up, applied to an empty compatible
target, reconciled after interruption, validated, and opened only when restore
readiness succeeds. Capabilities that lack complete authority, effect, cost,
retention, or restore proofs remain closed.

## Non-goals

- MSSQL production/write parity before the ORM owns those reusable contracts.
- Provider-persistent upload/search before creation ambiguity, pricing, quota,
  cleanup, and restore are all proven.
- Parallel or autonomous consequential execution.
- Deployment-owned principals, policy, credentials, routes, isolation, or
  product-specific mutation behavior.

## Dependencies

- `graphql-orm` 0.17 schema-module, transaction, fencing, operation-metadata,
  and restore contracts.
- `graphql-orm-backup` 0.7 snapshot, repository, verification, and restore
  orchestration.
- `graphql-orm-storage` 0.6 streaming provider boundary.
- Exact external `agql-auth` 0.13 revision declared by the workspace.
- Test-owned SQLite and disposable PostgreSQL infrastructure.

## Acceptance gates

- An empty compatible target can receive and validate the AI schema module and
  its backed-up state without application-specific SQL or copied upstream
  contracts.
- Restore preserves principal/scope bindings, fences, policy versions,
  budgets, checkpoints, provider-object state, and append-only audit/usage
  facts according to their documented semantics.
- Interrupted or externally uncertain work is reconciled without duplicating
  provider or application side effects.
- Runtime startup, workers, subscriptions, callbacks, and consequential tools
  remain closed until reconciliation and restore readiness both pass.
- SQLite and disposable PostgreSQL tests cover backup, applied restore,
  validation, failure windows, and readiness. Relevant Clippy, Rustdoc, SDL,
  naming, dependency, release-policy, and SemVer lanes are green.
- The capability matrix distinguishes implemented, host-supplied,
  experimental, and deliberately unsupported behavior.

## Current checkpoint

Monorepo consolidation and the 0.17/0.13 dependency alignment are complete.
The protected runtime, provider adapters, exact completed-batch adoption,
retention foundations, restore planning, and readiness observation contracts
exist. The next implementation slice is the AI-specific empty-target restore
collector, bounded repair applier, validator, recovery epoch, and readiness
gate using the reviewed workspace contracts. Durable tool-policy management
and provider-persistent upload/search stay closed until this gate passes.

Historical slice-by-slice evidence is retained in the
[archived completion ledger](../../../archive/2026/graphql-orm-ai-completion-ledger.md)
and [implementation ledger](../../../archive/2026/graphql-orm-ai-implementation-ledger.md).
