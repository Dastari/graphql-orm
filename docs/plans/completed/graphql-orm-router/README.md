---
title: GraphQL ORM Router implementation completion
kind: plan
status: accepted
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-11
supersedes: []
---

# GraphQL ORM Router implementation

## Outcome

The workspace provides independently consumable
`graphql-orm-router-protocol` and `graphql-orm-router` packages for one
project-neutral federated GraphQL HTTP and WebSocket boundary. The router
publishes only completely validated graph candidates, preserves a
last-known-good graph on failure, and treats router authorization as
defence-in-depth before authoritative subgraph enforcement.

## Delivered boundaries

- Versioned engine-neutral subgraph declarations and optional descriptor
  extensions with deterministic canonical fingerprints.
- Exact-pinned private Federation composition/execution adapters that do not
  leak engine types through public APIs.
- Atomic static and dynamic graph lifecycle, bounded conditional polling,
  stale-attempt rejection, and immutable in-flight graph selection.
- Authenticated HTTP and `graphql-transport-ws`, fixed and templated scope
  preflight, resource-server-only JWKS validation, and authoritative subgraph
  guards.
- Identity-bound administrative operations, deny-by-default destination and
  SSRF controls, strict configuration, bounded requests/subscriptions,
  telemetry, metrics, and graceful shutdown.
- Explicit ephemeral subscription semantics: no durable replay or
  cross-instance fan-out is implied.
- Router protocol 0.2.0 and router 0.1.3 preserve optional descriptor
  extensions through registration, graph input hashing, and atomic
  publication without interpreting application payloads.

## Acceptance evidence

- Test-owned loopback suites cover composition, HTTP, WebSocket,
  authorization denial before downstream work, graph replacement,
  subscription retirement, connection containment, SSRF resistance, JWKS
  rotation/outage, resource bounds, and shutdown.
- CI covers default and optional authentication profiles, MSRV, warnings-denied
  Clippy/Rustdoc, dependency direction, documentation, and package release
  policy.
- ADR-0008 retains the exact engine boundary and requires separate
  artifact-specific SBOM, notices, advisory, native-component, hash, target,
  and distribution approval for every compiled delivery.

## Follow-up

Durable multi-instance registration, cross-instance subscription fan-out, and
subgraph lifecycle coordination remain separately bounded backlog topics.
Current mechanics and operating instructions live in the component README,
schema/reconnect guidance, threat model, troubleshooting, and operations
runbook rather than this completed plan.
