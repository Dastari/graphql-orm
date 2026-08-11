---
title: "Router Subgraph Lifecycle Resilience Backlog"
kind: plan
status: draft
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-07
review_by: 2026-11-07
supersedes: []
---

# Router Subgraph Lifecycle Resilience Backlog

This backlog defines the intended lifecycle semantics for temporary subgraph
outages, clean maintenance, unexpected crashes, and permanent decommissioning.
It extends the router's existing atomic graph and last-known-good guarantees;
it does not change the implemented behavior described by the component
documentation.

## Current State

- A failed SDL or protocol refresh marks the source unhealthy while retaining
  the exact last-known-good executable graph. Temporary disappearance never
  means implicit schema removal.
- Requests that do not select the unavailable subgraph can continue. Requests
  that require it retain the active schema but encounter a downstream failure
  or timeout; healthy branches may still return data where GraphQL nullability
  and the query plan permit.
- A clean subgraph stop and an unexpected crash are indistinguishable unless
  an operator explicitly removes the subgraph. There is no maintenance or
  drain handshake.
- Subscription failures are ephemeral. An affected operation errors or its
  transport closes, after which the client must refetch authoritative state,
  reconnect, and resubscribe without replay.
- Explicit removal composes a complete candidate without the subgraph and
  publishes it atomically only when composition, execution, subscriptions, and
  authorization metadata remain valid. Static-source removal is process-local
  and the unchanged configuration restores the source after restart.
- Public readiness means that the router owns a complete active graph. It does
  not mean that every execution destination is currently reachable, so one
  unhealthy subgraph does not remove the router and every healthy subgraph from
  service.
- A router process cannot currently cold-start while a required static
  subgraph is offline because startup must fetch every configured SDL and
  protocol descriptor. Last-known-good inputs are process-local.
- Lifecycle status distinguishes active and unhealthy sources, but the current
  subgraph-health metric is based on graph participation and does not represent
  schema-source or execution reachability precisely.

## Planned Lifecycle Model

The router should represent lifecycle intent and observed health separately:

```text
Active --clean maintenance--> Draining --> Offline
   |                              |
   +--unexpected failure------> Unhealthy
                                  |
                                  +--recovery--> Active

Active --validated retirement--> Removed
```

An unhealthy or maintenance state must not silently delete schema. Removal is
a deliberate topology change that publishes a newly validated graph.

## Planned Features

### Clean Drain and Maintenance

- Add an authenticated, identity-bound way for an operator or trusted subgraph
  to announce maintenance, readiness recovery, and bounded drain completion.
- Stop assigning new work to a draining subgraph while allowing already pinned
  HTTP work to finish within the router's configured deadline.
- Retain the public schema during temporary maintenance and fail affected new
  operations promptly instead of waiting for a network timeout.
- Preserve explicit service identity, authorization, input bounds, and
  process/restart semantics for every lifecycle operation.

### Crash Detection and Circuit Breaking

- Track schema-source reachability and GraphQL execution reachability as
  separate health dimensions.
- Open a bounded per-subgraph circuit after a configured failure threshold,
  fail only dependent operations promptly, and use bounded half-open probes for
  recovery.
- Continue routing operations whose query plans do not require the unavailable
  destination.
- Do not automatically retry mutations. Any query retry policy must be
  explicitly bounded, observable, and limited to operations that are safe to
  repeat.

### Stable External Failure Contract

- Define a router-owned, sanitized GraphQL execution error for temporary
  destination failure, with a stable code such as `SUBGRAPH_UNAVAILABLE` and a
  retryability signal.
- Return healthy partial data where the GraphQL type system permits and apply
  ordinary non-null propagation otherwise.
- Do not expose internal origins, socket errors, credentials, schema-fetch
  headers, or untrusted downstream bodies.
- Use an execution-level GraphQL response for a selected unavailable service;
  reserve HTTP service-unavailable responses for router-wide failures that
  prevent GraphQL execution from starting.
- Specify HTTP, federated dependency, mutation, and subscription failure cases
  in end-to-end tests rather than inheriting unstable engine wording.

### Permanent Decommissioning

- Require clients and cross-subgraph Federation dependencies to migrate before
  retirement.
- Compose and validate the graph without the retiring subgraph before changing
  active routing.
- Atomically publish the reduced graph, retire affected subscriptions with the
  documented schema-reload signal, and allow requests pinned to the previous
  graph to drain before the service stops.
- Make the deployment's desired configuration the durable authority. An
  administrative process-local removal alone is not permanent for a static
  source.
- After retirement, operations using removed fields fail GraphQL validation;
  they must not be presented as a temporary availability failure.

### Last-Known-Good Cold Start

- Evaluate persistence of bounded, integrity-checked last-known-good SDL and
  protocol inputs so a router can recompose and start in an explicitly degraded
  state while a configured subgraph is unavailable.
- Never persist schema credentials, client credentials, private signing
  material, downstream response data, or an opaque executable engine runtime.
- Bind a snapshot to its format version, router compatibility boundary,
  topology, canonical fingerprints, and complete authorization metadata;
  reject incomplete, corrupt, oversized, or incompatible state.
- Keep recovery observable and continue bounded probes until the authoritative
  source returns.
- If durable snapshots are approved, integrate through a narrow storage
  contract backed by `graphql-orm-storage` rather than creating another
  storage implementation. This changes the router's current no-storage
  boundary and therefore requires an explicit architecture decision before
  implementation.

### Health and Operations

- Expose graph activity, schema/descriptor health, execution health, circuit
  state, maintenance/drain state, last success, and consecutive failures as
  distinct sanitized status and metrics values.
- Keep public liveness and graph-readiness semantics independent from one
  subgraph's degradation so an outage does not unnecessarily remove healthy
  graph capabilities.
- Provide alerts and runbook guidance for degraded operation, failed recovery,
  permanent retirement, and cold-start fallback use.

## Non-Goals

- Implicitly removing schema after an outage or elapsed timeout.
- Making the router the authoritative source of business data or replayable
  domain events.
- Retrying mutations or concealing partial execution behind fabricated data.
- Persisting credentials, tokens, request variables, downstream bodies, or
  engine-private runtime state.
- Coupling the router to a new storage provider instead of reusing the
  workspace storage contract if persistence is approved.

## Acceptance Gates

- Clean maintenance, crash, recovery, and permanent removal are distinguishable
  in lifecycle state and have deterministic transitions.
- An unavailable subgraph never changes the public graph without an explicit,
  successfully composed removal.
- Requests and subscriptions that do not depend on the unavailable subgraph
  continue, while dependent work receives a stable sanitized failure.
- In-flight graph pinning, non-null propagation, partial data, bounded drain,
  circuit recovery, and subscription reconnect behavior have executable tests.
- Static permanent removal survives restart through deployment configuration.
- If cold-start persistence is selected, corrupt, stale, unauthorized,
  incomplete, and incompatible snapshots fail closed, while an accepted
  snapshot permits tested degraded startup and eventual authoritative recovery.
- Status, metrics, and runbooks distinguish graph participation from schema and
  execution health.

## Current Checkpoint

The implemented router provides atomic graph admission, process-local
last-known-good retention, explicit validated removal, bounded downstream
timeouts, and subscription recovery signals. The lifecycle-intent protocol,
execution circuit breaker, stable transport-outage error contract, persistent
cold-start recovery, and corrected multi-dimensional health reporting remain
backlog work. An initial consumer migration may require every static
subgraph to be reachable at router startup until this backlog is promoted and
implemented.
