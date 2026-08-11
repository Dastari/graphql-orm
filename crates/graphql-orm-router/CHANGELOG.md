---
title: graphql-orm-router changelog
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-11
review_by: 2027-02-07
supersedes: []
---

# Changelog

## 0.1.3 - 2026-08-11

- Updated `graphql-orm-router-protocol` to 0.2.0 and retained optional
  descriptor extensions through registration, canonical admission hashing,
  last-known-good candidate state, and atomic publication. The router binds
  extension drift into the graph input fingerprint but does not interpret or
  authorize from extension payloads.
- Existing descriptors without extensions, router configuration, GraphQL
  execution, authorization preflight, and authoritative subgraph policies are
  unchanged.

## 0.1.2 - 2026-08-08

- Made public WebSocket termination first-cause-wins. A client protocol or
  frame-size failure now closes its private bridge without allowing the bridge
  forwarding task to overwrite the original outcome with a secondary generic
  `1011 Subscription transport unavailable` close.
- Added a process-wide token bucket for public WebSocket upgrade attempts. The
  secure default admits 128 attempts per second with one second of burst
  capacity; excess attempts return HTTP 429 with `Retry-After: 1`, active
  connection saturation remains HTTP 503, and both are counted by
  `router_websocket_rejections_total`.
- Added end-to-end evidence that operation IDs survive the bridge unchanged,
  a one-shot mutation emits `next` then `complete` without retiring a
  long-lived sibling, one upstream subscription failure remains
  operation-scoped, and later operations still run on the same public socket.
- The protocol-v1 descriptor, schema, token, and stored-data contracts are
  unchanged. Existing deployments receive the new admission default and may
  tune `subscriptions.maxConnectionAttemptsPerSecond` for measured connection
  bursts.

## 0.1.1 - 2026-08-07

- Fixed argument-templated authorization for variable-backed
  `graphql-transport-ws` operations. The bounded public gateway now carries
  each operation's variables across the private engine boundary, evaluates
  the rendered requirement before opening a subgraph subscription, and
  overwrites attempts to spoof its reserved internal metadata.
- Made variable-dependent HTTP authorization complete against the current
  operation's coerced values before downstream execution. Directive variables
  and variable defaults remain part of the same fail-closed decision.
- Added end-to-end coverage proving a variable-backed denied subscription
  opens no upstream connection while the same permitted value succeeds in
  both variable and inline forms. Configuration, protocol v1, public APIs, and
  subgraph resolver responsibilities are unchanged.

## 0.1.0 - 2026-08-07

- Updated the exact optional `agql-auth` pin to 0.14.0 revision
  `413fda3435f060604cd653c11e2cc18a668aace1`, whose validator natively
  normalizes standard `scope` and bounded legacy `scopes` claims. The adapter
  now consumes that verified normalized principal directly and no longer
  performs a second JWT payload decode.
- Added the standalone strict-JSON executable, pre-bind `--check`,
  environment-only schema secrets and listener overrides, production-secure
  defaults, explicit process signal handling, bounded graceful drain, and an
  authenticated HTTP/WebSocket/SIGTERM binary smoke test.
- Stabilized engine-neutral `RouterBuilder`, `PreparedRouter`, `RouterHandle`,
  readiness, status, refresh, metrics snapshot, and caller-supplied graceful
  shutdown surfaces.
- Added JSON/text telemetry policy, opt-in separate Prometheus export,
  authenticated core metrics, graph/request/subgraph/auth/composition and
  WebSocket gauges, and preserved Hive lag/drop instrumentation behind the
  private adapter.
- Added public/downstream deadlines, per-host connection-pool bounds, resource
  budgets, configuration/schema/reconnect/operations guidance, threat model,
  troubleshooting, migration notes, and explicit process-local event limits.
- Added ORM and hand-written descriptor examples plus a bounded hardening
  campaign covering repeated reload, timeout/recovery, JWKS rotation/outage,
  WebSocket churn, lag, and graceful shutdown.
- Added an authenticated, separately bound administrative service for safe
  status, refresh, explicit removal, and identity-bound dynamic registration.
  Dynamic state is process-local and services re-register after restart.
- Added deny-by-default dynamic destination policy with exact host/port/CIDR
  allowlists, bounded all-address DNS checks, private/loopback/link-local
  opt-ins, pinned no-proxy/no-redirect metadata and SDL clients, peer checks,
  response bounds, trusted origins, and router-owned schema credentials.
  Dynamic GraphQL execution destinations require IP-literal hosts to prevent a
  second resolver in the private execution transport from reopening DNS
  rebinding exposure.
- Added bounded public body/header/parser/depth/alias/directive/field limits,
  safe registered/candidate/active/unhealthy/rejected/disabled status, and
  hostile loopback evidence for identity, redirect, metadata-address,
  oversized-response, duplicate, removal, and restart behavior.
- Added conditional SDL/protocol polling, canonical router-input fingerprints,
  bounded retries, serialized candidate admission, and atomic executable
  graph-plus-authorization publication. Unavailable and rejected inputs retain
  the exact last-known-good runtime; unchanged accepted or rejected ETags skip
  composition.
- Added the engine-neutral `RouterHandle` refresh, explicit process-local
  removal, and safe lifecycle status APIs with active, unhealthy, rejected, and
  disabled source states. Cancellation and shutdown cannot publish a partially
  evaluated candidate.
- Added loopback evidence for live HTTP graph replacement, in-flight request
  pinning, failure/recovery, stale refresh ordering, and authenticated
  subscription retirement/reconnect after an atomic graph-and-policy reload.

- Added an authenticated public `graphql-transport-ws` gateway on the configured
  GraphQL path. It validates `connection_init` before acknowledgement,
  authorizes each operation, propagates only the approved bearer credential,
  requires usable token expiry, forbids in-place refresh, and closes at expiry.
- Added bounded WebSocket connections, operations per connection, client
  messages, upstream event buffers, and downstream fan-out. The private bridge
  preserves Hive's upstream WebSocket, lag/drop, graph-pinning, and
  schema-retirement behavior; subscription deduplication remains disabled.
- Added loopback end-to-end coverage for timeout and invalid initialization,
  scope denial without downstream work, filtered and multi-client events,
  connection/operation limits, upstream failure isolation, no replay on
  reconnect, credential propagation, and token-expiry closure.

- Added engine-neutral authentication/principal and scope-matcher contracts,
  plus a bounded remote-JWKS RS256 resource-server provider with issuer,
  audience, key-ID, explicit-clock, expiry/not-before, rotation, cache-staleness,
  and secure loopback-development policy.
- Added standards-compatible space-delimited `scope` parsing. The legacy
  `scopes` array requires an explicit migration mode, malformed claims fail
  closed, and conflicting standard/legacy sets are rejected rather than
  unioned.
- Added optional one-way `auth-agql` adapters for the exact-pinned
  `AccessTokenValidator` and scope matcher without exposing issuer, private-key,
  token issuance, refresh-session, storage, or decryption services.
- Added graph-bound authorization metadata admission and pre-execution root
  policy for authenticated, fixed any/all-scope, and scalar argument-template
  requirements across aliases, fragments, directives, variable defaults, and
  multiple selections. Missing, stale, ambiguous, or incompatible metadata
  prevents readiness; denied operations open no downstream work.
- Propagate only successfully validated original bearer credentials to GraphQL
  destinations while keeping SDL/protocol service credentials separate and
  redacted.
- Added the public static-router library surface: validated fail-closed
  configuration, explicit anonymous-development mode, schema-only credentials,
  bounded startup SDL retrieval, downstream-header allowlisting, complete
  candidate preparation, process-local graph identity, and owned-runtime or
  async serving APIs.
- Added custom-path HTTP GraphQL serving plus liveness and active-graph
  readiness. The public loopback harness covers one- and two-subgraph queries,
  entity resolution, mutation routing, downstream error paths, header policy,
  invalid startup, and coherent concurrent graph selection.
- Added the private Slice 0 Federation composition, executable query-planning,
  atomic replacement, last-known-good, and retirement proof.
- Added a maintained, test-owned loopback HTTP proof for entity and mutation
  routing, pre-downstream denial, graph replacement, and in-flight completion.
- Added a maintained `graphql-transport-ws` loopback proof for upstream
  subscription events, retirement signalling and completion, stale connection
  rejection, and replacement-graph reconnect.
- Declared and verified Rust 1.90 support for the pinned runtime closure.
- Updated compatible vulnerable transitive dependencies in the workspace
  lockfile; unresolved upstream-constrained findings remain documented in
  ADR-0008 with configuration-level exposure mitigations.
- Accepted the private Hive boundary while reserving durable storage for a
  higher-level `graphql-orm-storage` integration and private-key/token issuance
  for an external identity service.
