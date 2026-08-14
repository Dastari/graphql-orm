---
title: "Implementation Status"
kind: reference
status: active
owner: graphql-orm-ai-maintainers
last_reviewed: 2026-08-14
review_by: 2027-02-01
supersedes: []
---

# Implementation Status

`graphql-orm-ai` is at crate version `0.78.5` with AI schema module
`0.59.0`. It uses workspace `graphql-orm` `0.22.1`, backend-neutral
`graphql-orm-ai-tool-profiles` `0.4.1`, and external `agql-auth`
`0.15.0` at `e841ffd382082ad7419be259fe957f949b956ff7`.

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
  kill-on-drop. Experimental native dynamic tools require an immutable closed
  dynamic-tools-only launch profile, a direct-tool model declaration, and a
  process-factory profile attestation; every exact call
  rechecks current rules and uses the ordinary registered GraphQL tool,
  disclosure, egress, budget, and resolver authorization path. Generic
  protocol bridging, shell, files, hosted web, MCP, and browser remain closed.
  Initialization uses one library-owned notification opt-out profile, while
  response-authoritative deletion, empty reasoning lifecycle, and retained
  cumulative-usage replay are admitted only through typed content-free
  controls that cannot become model output or current-run usage. Retained
  developer instructions are compile-time static, registration-fingerprinted,
  and distinct from request input. Provider definitions are projected from the
  exact registered manifest, with canonical JSON Schema validation retained at
  every dynamic-call boundary.
- The provider-neutral durable session service protects opaque retained-thread
  cursors under exact owner/scope/run/descriptor/transcript fencing and an
  exact deletion/absence lifecycle. An absence-proven deleted generation may
  be replaced once through a crate-issued short-lived rebind authorization;
  cleanup/backoff, expiry, descriptor drift, and restore quarantine remain
  unavailable. Cursor state is separate from warm processes, private from
  GraphQL, backup-redacted, and readiness-blocking on portable restore until
  drained. Provider failures may additionally emit only a closed content-free
  operational category without changing conservative run semantics.
- Retained human-approval waits atomically release the source lease with a
  protected parked checkpoint and nonterminal attempt outcome. Exact
  confirmation is crash-repairable, and only a confirmed graph can create the
  fresh attempt used for one-shot consumption and provider reclaim.
- Current rule evidence supports both generated-ORM managed hierarchies and
  immutable deployment-only ceilings with no artificial per-resource policy
  rows. Both paths share exact-principal rehydration and canonical
  fingerprinting; neither path grants ordinary application authority.
- Application tools require an explicit catalog, static disclosure contract,
  current host policy, and ordinary resolver authorization. Read-only and the
  bounded sequential supervised path are implemented; mixed, parallel, and
  stateless consequential execution remains closed.
- Read-only provider plans may expose exact static descriptors and generated
  query capabilities together. Catalogue kind is server-derived, each kind
  retains its own exact policy, and retained/stateless continuations consume
  only the crate-owned opaque continuation proof.
- Private remote GraphQL delegation preserves that same server-derived
  static/generated identity through authority issuance. Generated reads bind
  the exact target, finished schema, semantic catalogue/root and registered
  capability; dynamic operation-name conventions grant nothing.
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
