---
title: "Implementation Status"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-01
supersedes: []
---

# Implementation Status

`graphql-orm-ai` is at crate version `0.70.0` with AI schema module
`0.55.0`. It uses workspace `graphql-orm` `0.21.0`, backend-neutral
`graphql-orm-ai-tool-profiles` `0.2.0`, and external `agql-auth`
`0.14.0` at `413fda3435f060604cd653c11e2cc18a668aace1`.

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
- Native OpenAI supports host-selected bounded visible reasoning summaries and
  explicit public/allowed/blocked hosted web search. Provider-retained calls
  may offer search beside exact registered application tools; stateless mixed
  built-in/application-tool replay remains closed. Completed web searches have
  distinct cumulative rule ceilings, pricing/usage settlement, protected
  lifecycle activity, and validated provider citation provenance.
- The optional Codex app-server adapter retains one strictly allowlisted
  process per exact claimed run and may resume one exact protected provider
  thread. It is globally and per-owner bounded, cancellation-aware, and
  kill-on-drop. Experimental native dynamic tools require an immutable
  default-off registration flag and remain coordinator-owned; every exact call
  rechecks current rules and uses the ordinary registered GraphQL tool,
  disclosure, egress, budget, and resolver authorization path. Generic
  protocol bridging, shell, files, hosted web, MCP, and browser remain closed.
- The provider-neutral durable session service protects opaque retained-thread
  cursors under exact owner/scope/run/descriptor/transcript fencing and an
  exact deletion/absence lifecycle. Cursor state is separate from warm
  processes, private from GraphQL, backup-redacted, and readiness-blocking on
  portable restore until drained.
- Current rule evidence supports both generated-ORM managed hierarchies and
  immutable deployment-only ceilings with no artificial per-resource policy
  rows. Both paths share exact-principal rehydration and canonical
  fingerprinting; neither path grants ordinary application authority.
- Application tools require an explicit catalog, static disclosure contract,
  current host policy, and ordinary resolver authorization. Read-only and the
  bounded sequential supervised path are implemented; broader mixed,
  parallel, and stateless consequential execution remains closed.
- Owning subgraphs can compile the same canonical generated/custom GraphQL
  tool manifests through `graphql-orm-ai-tool-profiles` without selecting an
  AI persistence backend. The runtime consumes those exact wire values and
  fingerprints without transformation.
- Application-tool lifecycle streams include protected, metadata-only start
  and completion events. Browser result previews are independently
  fingerprinted, opt-in, owner-authorized, current-policy checked, bounded,
  protected at rest, and subject to a mandatory host row/field projection;
  they never expose the raw stored result by default.
- The read-only coordinator also accepts an exact initial tool-free chat plan.
  It retains current rule/provider/egress/budget/output checks but has no
  application or built-in tool exposure, tool-result route, tool checkpoint,
  or continuation authority.
- Session titles support owner-authorized idempotent GraphQL rename and a
  private durable first-message work queue. The host owns provider selection;
  the library owns current-principal disclosure checks, lease fencing,
  protected events, and the conditional commit that preserves manual and
  pre-upgrade titles.
- Provider-persistent file upload, indexing, and file search remain closed.
  This does not include provider-hosted public web search. Inline
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
