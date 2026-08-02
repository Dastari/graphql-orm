---
title: "Implementation Status"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-01
review_by: 2027-02-01
supersedes: []
---

# Implementation Status

`graphql-orm-ai` is at crate version `0.61.0` with AI schema module
`0.51.0`. It uses workspace `graphql-orm` `0.17.0` and external `agql-auth`
`0.13.0` at `d6b9cef663d52125c52f3fb90d4155ee25d34775`.

The active work order, dependencies, and exit gates are maintained in the
[AI production-readiness plan](../../../docs/plans/active/ai-production-readiness/README.md).
This page is the concise crate-local capability boundary; detailed design and
verification evidence belongs in the focused guides.

## Current capability boundary

- SQLite is the default implementation profile. PostgreSQL has an owned,
  disposable-container parity harness. MSSQL remains a compile/schema profile,
  not a production-support claim.
- The crate supplies bounded provider adapters, protected persistence,
  budgets, egress, current-principal rehydration, fencing, session lifecycle,
  and the implemented OpenAI background-reconciliation path.
- Application tools require an explicit catalog, static disclosure contract,
  current host policy, and ordinary resolver authorization. Read-only and the
  bounded sequential supervised path are implemented; broader mixed,
  parallel, and stateless consequential execution remains closed.
- Provider-persistent upload, indexing, and search remain closed. Inline
  attachment input and exact deletion of known provider artifacts are separate
  implemented seams.
- The first applied-restore prerequisite is implemented: bounded generated-ORM
  collection derives conservative run classifications plus approval and
  egress-consent revalidation candidates. With host-attested deployment
  ceilings supplied, it also completes budget-policy and immutable
  pricing-catalog integrity,
  including exact creation-audit linkage. With host-attested attachment
  bounds, it also completes the stable attachment/artifact lifecycle,
  ownership, parent, and safe unique object-reference metadata graph. Verified
  manifest plus restored-target object-byte integrity remains a separate fatal
  incomplete audit. The repair applier,
  complete validator, recovery epoch, and runtime-start proof remain closed.
- Applied backup/restore, durable tool-policy management, generic privileged
  uncertain-effect recovery, and production MSSQL writes remain closed.

## Read next

- [Backend and capability acceptance matrix](backend-capability-matrix.md)
  defines support claims and required host evidence.
- [Recovery, retention, backup, and restore](recovery-and-restore.md) defines
  the readiness-closed restore boundary.
- [Development and verification](development.md) and
  [release process](release-process.md) define the required checks.
- The detailed historical record is preserved in the
  [2026 implementation ledger](../../../docs/archive/2026/graphql-orm-ai-implementation-ledger.md).
