---
title: ADR-0005 Restore readiness and uncertain effects
kind: decision
status: accepted
owner: workspace-maintainers
last_reviewed: 2026-08-01
review_by: 2027-08-01
supersedes: []
---

# ADR-0005: Restore readiness and uncertain effects

## Context

Restoring rows and blobs is insufficient when durable work includes leases,
external provider calls, budgets, approvals, checkpoints, or append-only audit
facts. A crash can leave an effect uncertain: the system cannot safely infer
that no external action occurred simply because no local terminal record was
written.

## Decision

Restore is a staged, fail-closed transition: validate compatibility, apply
bounded state, reconcile incomplete and externally uncertain effects, validate
package invariants, advance a recovery epoch where applicable, and establish
an explicit readiness result. Runtime workers, subscriptions, callbacks, and
consequential operations remain closed until readiness succeeds.

Manifests are checked against backend/schema identity before target writes.
Fences, idempotency bindings, scope/principal identity, policy versions,
budgets, append-only facts, and deletion/tombstone state retain their documented
meaning. An uncertain effect is reconciled or retained with a truthful blocked
reason; it is not replayed as a fresh action.

The package that owns durable state owns its restore validation and readiness
conditions. Backup orchestrates; storage transports bytes; neither invents
package-specific recovery semantics.

## Consequences

- Backup completion and restore readiness are distinct facts.
- Some records deliberately remain blocked or retained until an operator or
  provider can resolve ambiguity.
- Every durable feature must define restore and failure-window behavior before
  it can be considered production-ready.

## Supersession

A weaker readiness model or a new uncertain-effect replay rule requires a new
ADR with explicit duplicate-effect analysis.
