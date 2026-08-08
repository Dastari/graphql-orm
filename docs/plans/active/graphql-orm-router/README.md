---
title: GraphQL ORM Router implementation plan
kind: plan
status: active
owner: workspace-maintainers
last_reviewed: 2026-08-08
review_by: 2026-09-07
supersedes: []
---

# GraphQL ORM Router

## Outcome

Provide independently consumable `graphql-orm-router-protocol` and
`graphql-orm-router` crates that expose one project-neutral federated GraphQL
HTTP and WebSocket endpoint, adopt only completely validated graph changes,
enforce declared access policy in defence in depth with authoritative subgraph
guards, and support ephemeral live notifications without requiring a durable
message broker.

The detailed proposal is maintained in the supporting draft references:

- [Project scope](project-scope.md)
- [Functional requirements](functional-requirements.md)
- [Technical design](technical-design.md)

## Non-goals

- Implementing a Federation query planner from scratch.
- Making `graphql-orm`, `agql-auth`, or GEMA mandatory for generic router use.
- Providing durable event storage, replay, queues, workflow orchestration, or
  exactly-once notification delivery.
- Solving cross-instance subgraph event fan-out in the initial implementation.
- Moving application-specific policy, identity issuance, or business logic into
  the router.

## Dependencies

- Existing `graphql-orm` generated operation metadata, conventional Federation
  roots, resolver guards, and process-local subscription streams.
- A proved and version-pinned Rust Federation execution boundary; Hive Router
  is the initial candidate.
- A proved Federation v2 composition path that accepts immutable candidate
  subgraph inputs and produces a validated runtime supergraph.
- [ADR-0007](../../../decisions/ADR-0007-seven-package-workspace-boundaries.md),
  which authorizes the protocol and router package edges and supersedes the
  earlier five-package boundary.
- Accepted [ADR-0008](../../../decisions/ADR-0008-hive-federation-runtime-and-composition-boundary.md),
  which records the pinned engine seam, exposure mitigations, update policy,
  and artifact-specific release boundary.
- [ADR-0003](../../../decisions/ADR-0003-resolver-metadata-is-discovery-not-authority.md)
  and [ADR-0004](../../../decisions/ADR-0004-authentication-authorization-and-assurance-boundaries.md),
  which keep generated metadata advisory and subgraph enforcement authoritative.
- Exact compatibility semantics for JWT validation, scope matching, templated
  scopes, and WebSocket authorization. The explicitly coordinated
  `agql-auth` 0.14 work provides standard scope issuance and bounded legacy
  validation at revision `413fda3435f060604cd653c11e2cc18a668aace1`.
- Test-owned subgraphs and clients for federation, graph reload, authorization,
  and subscription evidence. GEMA remains the first migration target, not part
  of the generic router contract.

## Delivery invariants

- Build in mergeable vertical slices. A slice is complete only when its public
  behavior, failure behavior, tests, documentation, and dependency checks land
  together.
- Prove an external engine or composition API behind a private adapter before
  exposing any router public API that depends on it.
- Router authorization is an early denial layer. A router allow decision only
  permits ordinary downstream execution; it never replaces the subgraph guard,
  row policy, field policy, assurance check, or database RLS.
- Do not serialize a custom or runtime-only subgraph policy as a broader static
  router permission. Unrepresentable policy remains subgraph-only and is marked
  explicitly in metadata.
- Keep the generic router usable without `graphql-orm`, `agql-auth`, or GEMA.
  Keep `graphql-orm` usable without the router feature.
- Static configuration is the durable source in the initial release. Dynamic
  registrations and last-known-good runtime state are process-local; services
  must re-register after router restart. Shared registry state and coordinated
  multi-router activation remain future work.
- The initial live-event profile assumes one active instance of a write-capable
  subgraph. No slice may imply cross-instance fan-out, replay, or durable event
  delivery.
- Use test-owned loopback services and temporary SQLite by default. PostgreSQL
  and MSSQL follow their existing disposable-infrastructure rules. Never test
  against a live application database.
- Never use workspace `--all-features`; database backends and optional router
  integrations are verified in explicit feature lanes.
- Update the one current checkpoint below as work advances. Detailed test
  evidence belongs in code, CI, ADRs, or archived investigations rather than a
  chronological plan transcript.

## Planned package boundary

The intended acyclic direction is:

```text
graphql-orm-router ───────────────► graphql-orm-router-protocol
         │
         ├────────────────────────► Federation runtime/composition
         └─ optional ─────────────► agql-auth

graphql-orm ─ optional ───────────► graphql-orm-router-protocol
     │
     └────────────────────────────► graphql-orm-macros
```

`graphql-orm-macros` may emit paths to feature-gated types re-exported by
`graphql-orm`, but it must not acquire a direct runtime or protocol dependency.
The protocol package contains versioned serializable declarations only; it does
not depend on a server framework, database backend, federation engine, or
application package.

## Test topology

The router package will own a reusable integration harness containing:

- one temporary-SQLite `graphql-orm` subgraph with generated queries,
  mutations, entity resolution, guards, and subscriptions;
- one project-neutral non-ORM Federation subgraph that implements the protocol
  by hand;
- compatible and deliberately incompatible SDL generations;
- a loopback JWT issuer/JWKS fixture with deterministic test keys, expiry, key
  rotation, issuer, audience, standard `scope`, and legacy `scopes` cases;
- HTTP and `graphql-transport-ws` clients;
- bounded loopback listeners allocated by the tests, with deterministic
  startup, shutdown, and timeout behavior.

This harness is the common evidence surface for all cross-process acceptance
gates. It must not depend on GEMA or external infrastructure.

## Implementation sequence

The slices below are ordered by dependency and risk. Later slices may be split
into smaller pull requests, but their exit gate must remain intact.

### Slice 0 — Prove and record the federation seams

Deliver:

- Prototype composition of two Federation v2 subgraph SDLs into a supergraph
  and construction of an executable Hive-based runtime from that result.
- Prove single- and multi-subgraph queries, entity resolution, mutation routing,
  one upstream subscription, and replacement of the executable supergraph.
- Prove the hook point used to authenticate and reject a protected root field
  before any downstream request is opened.
- Establish in-flight semantics: ordinary requests remain pinned to the graph
  they selected; a retired graph stops receiving new requests; subscriptions
  receive a defined reload error and reconnect rather than silently migrating.
- Audit the selected crates for license compatibility, supported Federation v2
  surface, Rust/toolchain compatibility, public-versus-internal API stability,
  dependency weight, security maintenance, and version-pinning strategy.
- Add the next available superseding ADR for the expanded workspace package
  graph and a separate ADR for the selected federation/runtime composition
  boundary. Update the ADR index and root workspace guidance.

Verification:

- A maintained integration test exercises the successful proof; throwaway
  experiments do not enter the authoritative documentation namespace.
- A negative test proves an invalid supergraph cannot construct or replace the
  executable runtime.
- A dependency tree records one selected planner/executor/composition universe
  without Git dependencies between workspace packages.

Exit gate:

- Continue only if the router can own composition and atomic runtime selection
  without exposing unstable engine types or bypassing pre-execution policy.
  Otherwise revise the technical design and engine decision before scaffolding
  public packages.

### Slice 1 — Add workspace packages and protocol v1

Deliver:

- Add `graphql-orm-router-protocol` and `graphql-orm-router` as workspace
  members, but do not add them to the core default members.
- Use workspace path dependencies and the root `Cargo.lock`; update the
  dependency-integrity script, generated package inventory, system context,
  development setup, and CI package lanes.
- Define protocol-version, subgraph identity, endpoint advertisement,
  capability, schema fingerprint, operation, argument, authorization, and scope
  template types.
- Separate advertised service data from deployment-owned overrides and
  credentials. Endpoint strings remain inert protocol data until the router
  validates them against network policy.
- Define compatible-minor and incompatible-major behavior. Compatible readers
  ignore unknown additive fields; unknown required semantics or major versions
  fail registration clearly.
- Add crate READMEs, changelogs, package metadata, and package-local agent guides
  for their security, dependency, and verification invariants.

Verification:

- Golden JSON round trips cover a generated-style descriptor and a hand-written
  non-ORM descriptor.
- Unit and property tests cover deterministic ordering/fingerprints, unknown
  fields, malformed values, incompatible versions, and stable error categories.
- `graphql-orm-router-protocol` resolves without Hive, Axum, a database backend,
  `graphql-orm`, `agql-auth`, or application dependencies.

Exit gate:

- The wire contract is independently consumable and versioned, and workspace
  inventory/dependency checks recognize exactly one path source for both new
  packages. This owns FR-120 through FR-124 and the package aspects of FR-180
  through FR-184.

### Slice 2 — Produce canonical ORM policy metadata and native events

Deliver:

- Extend the existing ORM operation catalogue with a core-owned, project-neutral
  authorization declaration covering public, authenticated, fixed any/all
  scopes, and supported argument templates.
- Preserve the current discovery fingerprint contract. Add a separately
  versioned authorization fingerprint and combined router-export fingerprint
  instead of silently changing the meaning of the existing fingerprint.
- Finalize one macro grammar for generated-operation policy, validate malformed
  templates and statically knowable argument references at compile time, and
  retain the current `auth = "required" | "optional" | "none"` migration path.
- Make the same static declaration construct the generated resolver guard and
  the feature-gated protocol export. Dynamic/custom policy hooks remain
  subgraph-only and cannot be represented as router permission.
- Emit standard Federation `@authenticated` and `@requiresScopes` directives
  for representable fixed policies. Keep argument templates in protocol
  metadata unless the composition proof accepts a project-neutral composed
  directive without ambiguity.
- Expose deterministic schema and authorization metadata from the finished
  generated root catalogue, including subscription ownership.
- Prove the existing post-commit Tokio broadcast path and subscription request
  context; do not introduce a router-specific event bus.

Verification:

- Macro `trybuild` tests cover valid declarations, duplicate policy, invalid
  any/all structure, malformed templates, unknown arguments, and feature-off
  compilation.
- Unit tests prove deterministic descriptors and independent schema,
  authorization, and combined fingerprints.
- Resolver tests prove the generated guard still denies independently when a
  router-style preflight would allow.
- SQLite integration proves committed generated writes emit subscription events
  and failed/rolled-back writes do not. Explicit backend compile lanes prove no
  regression on PostgreSQL or MSSQL profiles.
- Feature-off dependency trees prove ordinary ORM consumers do not resolve the
  protocol or router package.

Exit gate:

- One declaration drives representable generated guards and advisory export
  without making metadata authoritative. This owns FR-090 through FR-100 and
  FR-110 through FR-115, plus the subgraph side of FR-060 and FR-065 through
  FR-066.

### Slice 3 — Serve a static atomic HTTP graph

Deliver:

- Implement validated configuration for listener, GraphQL path, static
  subgraphs, downstream headers, explicit anonymous-development mode, and
  internal schema credentials.
- Implement immutable candidate inputs, composition diagnostics, runtime graph
  construction, an atomic active-graph store, and graph version/fingerprint
  identities.
- On startup, fetch every configured SDL, compose the complete graph, build the
  runtime, and become ready only after full success. Invalid startup has no
  partially active graph.
- Expose `POST /graphql`, liveness, and readiness through the library; keep
  federation-engine types private.
- Route single-subgraph queries, federated queries/entities, and mutations;
  preserve standard GraphQL response/error paths and only approved downstream
  headers.

Verification:

- End-to-end tests cover valid and invalid startup, one- and two-subgraph
  queries, entity resolution, mutation ownership, downstream failure paths,
  header allowlisting, liveness, and readiness.
- Concurrency tests prove readers see only a complete old or complete new
  `ActiveGraph` and that failed runtime construction cannot swap it.
- Tests use explicit anonymous-development configuration until the secured
  default lands; no production example presents anonymous mode as the default.

Exit gate:

- Static subgraphs form a usable federated HTTP graph with no restart-time
  partial state. This owns FR-001 and FR-003 through FR-005, FR-010, FR-015
  through FR-019, FR-050 through FR-056, and FR-130 through FR-131.

### Slice 4 — Add fail-closed HTTP authentication and authorization

Deliver:

- Define engine-neutral authenticated-principal and authentication-provider
  interfaces plus a configured JWT resource-server implementation supporting
  signature, issuer, audience, expiry, key ID, JWKS retrieval/cache/rotation,
  and explicit clock behavior.
- Keep the router resource-server-only: it does not issue tokens, own login or
  session state, refresh credentials, or synthesize authentication evidence.
- Treat OAuth `scope` as a space-delimited string. Accept legacy `scopes` arrays
  only under an explicit migration option; reject malformed claims and fail
  closed when both forms conflict.
- Add exact scope matching by default and an explicit matcher adapter contract.
  Reuse the exact-pinned `agql-auth` `AccessTokenValidator` through an optional
  one-way feature. Any upstream scope-claim change and revision update is a
  separately authorized external-repository task.
- Parse and select the GraphQL operation, coerce variables, enumerate every
  selected protected root field across aliases, fragments, directives, and
  multiple root selections, then perform advisory preflight denial before the
  federation runtime opens downstream work.
- Support authenticated, any-of, all-of, and argument-template requirements.
  Canonicalize only documented scalar input kinds; missing, null, complex, or
  uncoercible substitutions deny.
- Bind authorization metadata to the exact active graph/fingerprint. Reject a
  candidate graph whose required metadata is missing, stale, ambiguous, or
  incompatible rather than guessing at request time.
- Propagate the original approved bearer credential only to configured GraphQL
  destinations. Use separate service credentials for registration/SDL access
  and never log either credential.

Verification:

- JWT tests cover valid, missing, malformed, expired, wrong issuer/audience,
  unknown key, JWKS rotation/cache failure, standard/legacy scope migration,
  and conflicting claims.
- Authorization tests cover public/authenticated defaults, fixed any/all rules,
  exact and configured hierarchical matching, variables, literals, defaults,
  aliases, fragments, skipped fields, multiple root fields, unresolved
  templates, and unsupported values.
- Equivalence vectors run the same generated policy against the router preflight
  and authoritative subgraph guard. Tests prove a router allow cannot bypass a
  subgraph denial and a router denial opens no downstream request.
- Redaction tests inspect structured logs and GraphQL errors for raw bearer,
  JWKS, service credential, and sensitive variable leakage.

Exit gate:

- Secured HTTP requests fail closed and router/subgraph decisions agree for all
  representable generated policies. This owns FR-080 through FR-082, FR-085
  through FR-100, including the resource-server boundary in FR-087, plus
  FR-162 through FR-163, FR-170, and FR-176.

### Slice 5 — Add authenticated federated subscriptions

Deliver:

- Serve `graphql-transport-ws` on the public GraphQL path and connect to the
  owning subgraph using the proved upstream transport.
- Authenticate `connection_init`, authorize each operation against its selected
  active graph, and propagate only the approved credential.
- Define the initial long-lived-token policy: no in-place token refresh;
  connections close at token expiry and clients reconnect/re-authenticate.
  Optional revocation or assurance-aging hooks may close earlier but never
  extend token validity.
- Pin a subscription to its selected graph. On graph retirement send the
  documented schema-reload error, close the affected subscription, and require
  client resubscription against the new graph.
- Bound WebSocket connections, operations per connection, upstream buffers, and
  downstream fan-out. Expose lag/drop metrics; do not persist or replay.
- Use engine-supported compatible subscription deduplication only after tests
  prove authenticated principals and variables are part of the deduplication
  identity.

Verification:

- End-to-end tests cover connection authentication failure, scope denial before
  upstream open, generated write/event receipt, filtered subscription, multiple
  clients, disconnect/no replay, bounded lag, upstream failure isolation, token
  expiry, and schema-reload reconnect.
- One test uses the hand-written non-ORM subgraph to prove the router contract is
  not tied to generated subscriptions.
- Metrics tests prove active connection/subscription gauges return to baseline
  after success, denial, timeout, and disconnect.

Exit gate:

- HTTP and WebSocket share the public graph securely, and connected clients
  receive bounded ephemeral events without NATS, JetStream, or EDFS. This owns
  FR-002, FR-060 through FR-072, FR-083 through FR-086, and FR-175.

### Slice 6 — Add polling, candidate composition, and graph lifecycle

Deliver:

- Poll authenticated SDL endpoints with conditional ETag/fingerprint requests,
  configurable intervals, request timeouts, body limits, and bounded retry
  behavior. Unchanged inputs skip composition.
- Canonicalize router-relevant schema and authorization inputs before SHA-256
  fingerprinting; exclude deployment/runtime noise.
- Serialize refresh/admission attempts so a slower old candidate cannot replace
  a newer accepted candidate. Cancellation and shutdown leave the active graph
  untouched.
- Compose a changed candidate with every other active subgraph's last-known-good
  input, validate the complete runtime, and atomically activate only on success.
- Retain active membership and schema during health, fetch, metadata,
  composition, or runtime-construction failure.
- Implement authenticated manual refresh and explicit removal as candidate graph
  operations; disappearance or unhealthiness never means removal.
- Expose registered, candidate, active, unhealthy, rejected, and disabled state
  with safe rejection diagnostics.

Verification:

- Deterministic tests cover unchanged polling, valid addition/change/removal,
  incompatible update, unavailable SDL, unhealthy subgraph, stale concurrent
  refresh, manual refresh, explicit removal rejection, and recovery.
- In-flight HTTP requests finish on the graph they selected; new requests see
  the replacement. Subscription reload behavior remains as established in
  Slice 5.
- Last-known-good tests assert both the schema and executable runtime identity,
  not merely a stored SDL string.

Exit gate:

- Valid graph changes arrive without process restart and every invalid candidate
  leaves the exact executable graph unchanged. This owns FR-030 through FR-040
  and completes FR-015 through FR-020 and FR-130 through FR-134.

### Slice 7 — Add dynamic registration and administrative security

Deliver:

- Implement framework-neutral versioned descriptor construction and an example
  `/.well-known/graphql-router` host route, plus the router's authenticated
  candidate registration endpoint. The protocol package must not acquire a
  server-framework dependency.
- Bind service identity, registered subgraph name, metadata/SDL destination,
  and allowed GraphQL destination. Reject duplicate or conflicting identities.
- Enforce scheme/host/port/network allowlists, post-resolution IP checks,
  link-local/metadata-address denial, bounded DNS behavior, redirect policy,
  response limits, and credential non-forwarding to prevent SSRF and confused
  deputy behavior.
- Implement authenticated status and refresh endpoints showing active graph
  version/fingerprint, known subgraphs, health, current fingerprints, last
  successful composition, rejected candidates, and safe errors.
- Document and test initial restart semantics: static subgraphs rebuild at
  startup; dynamic services re-register; no process-local graph is represented
  as durable or shared across router instances.
- Complete structured file/environment configuration, externally supplied
  secrets, public/admin listener policy, request body/parser/depth/complexity
  limits, and WebSocket limits.

Verification:

- Tests cover trusted registration, unauthenticated/unauthorized registration,
  incompatible protocol, duplicate identity, malicious advertised override,
  loopback/private/link-local policy, DNS rebinding defense, redirect escape,
  oversized metadata/SDL, credential isolation, explicit disable/removal, and
  re-registration after restart.
- Administrative response/log snapshots contain no tokens, keys, credentials,
  private variables, or unsafe downstream errors.
- Limit tests cover request body, parse work, depth, complexity/field count,
  WebSocket connections, and subscriptions per connection.

Exit gate:

- Dynamic candidates can be admitted without weakening network or
  administrative trust boundaries. This owns FR-011 through FR-014, FR-020,
  FR-040, FR-140 through FR-152, and FR-171 through FR-176.

### Slice 8 — Harden operations, public APIs, and release evidence

Deliver:

- Stabilize engine-neutral `RouterConfig`, `RouterBuilder`, `RouterHandle`,
  startup/readiness, refresh, status, and graceful-shutdown APIs.
- Ship the `graphql-orm-router` executable with structured configuration,
  signal handling, safe startup failures, and production-secure defaults.
- Complete tracing and metrics for requests, downstream latency/errors,
  WebSockets/subscriptions/lag, graph versions, refresh/composition outcomes,
  health, rejected candidates, and authorization denials.
- Add an ORM example and a hand-written non-ORM example, operator configuration,
  schema-evolution guidance, WebSocket reconnect guidance, threat model,
  troubleshooting, and explicit single-instance/event limitations.
- Add package changelogs and migration notes, public Rustdoc, feature/dependency
  documentation, release-policy/semver lanes, and CI coverage for both new
  packages.
- Run a bounded soak/failure campaign covering repeated reload, subgraph
  timeout/recovery, JWKS outage/rotation, subscription churn, lag, and graceful
  shutdown. Define resource budgets before calling the binary production-ready.

Verification:

- `cargo fmt --all -- --check`.
- Package tests, warnings-denied Clippy, and warnings-denied Rustdoc for the
  protocol and router packages under their explicit default and optional-auth
  feature profiles.
- Relevant `graphql-orm` SQLite tests plus explicit SQLite, PostgreSQL, MSSQL,
  combined-backend, feature-off, and `auth-agql` compile/test/tree lanes.
- `scripts/check-workspace-dependencies.sh`, generated workspace inventory
  check, duplicate dependency review, and documentation validation.
- Example smoke tests start the binary, reach readiness, execute HTTP and
  WebSocket operations, and shut down without leaked tasks or listeners.

Exit gate:

- The generic packages are independently consumable and meet FR-001 through
  FR-184 in the automated matrix. No GEMA-specific type, scope, route, service,
  or deployment default exists in their public contract.

### Slice 9 — Validate and migrate GEMA as a separate consumer track

Deliver:

- Create a GEMA-owned migration plan and obtain explicit authority for changes
  outside this workspace, including any required `agql-auth` revision work.
- Run Cosmo/NATS and the new router path in parallel where practical. Compare
  composed schema, query/mutation results, error paths, fixed and parameterized
  authorization, WebSocket behavior, live notifications, and last-known-good
  failure behavior.
- Cut over clients behind a reversible deployment switch. Keep rollback until
  production observation meets the agreed window and error/resource budgets.
- Remove EDFS, NATS/JetStream GraphQL notification paths, WGC, Cosmo execution
  configuration/router, and the Go authorization module only after proving no
  remaining workload owns them.

Verification:

- The GEMA acceptance list in the
  [functional requirements](functional-requirements.md#18-gema-acceptance-requirements)
  and the production-readiness tests in
  [section 19](functional-requirements.md#19-acceptance-criteria) pass against
  deployment-owned disposable or shadow infrastructure.
- Direct subgraph access remains protected, Apollo HTTP/WebSocket behavior is
  preserved, missed events recover through ordinary queries, and rollback is
  exercised before destructive cleanup.

Exit gate:

- GEMA no longer requires Cosmo or NATS/JetStream for GraphQL federation and
  notifications, with rollback evidence and no removal of infrastructure still
  used by another workload.

## Requirement ownership

| Requirement area | Primary slice |
| --- | --- |
| Public GraphQL endpoint, FR-001–FR-005 | 3 and 5 |
| Registration/admission, FR-010–FR-020 | 3, 6, and 7 |
| Discovery/activation, FR-030–FR-040 | 6 |
| Federation execution, FR-050–FR-056 | 0 and 3 |
| Subscriptions, FR-060–FR-072 | 2 and 5 |
| Authentication, FR-080–FR-087 | 4 and 5 |
| Scope authorization, FR-090–FR-100 | 2 and 4 |
| ORM metadata, FR-110–FR-115 | 2 |
| Router protocol, FR-120–FR-124 | 1 |
| Health/state, FR-130–FR-134 | 3 and 6 |
| Administration, FR-140–FR-141 | 7 |
| Configuration, FR-150–FR-152 | 3 and 7 |
| Observability, FR-160–FR-163 | incremental, completed in 8 |
| Security, FR-170–FR-176 | 4, 5, and 7 |
| Compatibility, FR-180–FR-184 | 1 and 8 |
| GEMA acceptance | 9 |

## Acceptance gates

- A bounded prototype proves the selected Federation engine, composition API,
  executable graph swap, downstream transport, and subscription reload
  behavior before the production crate architecture depends on them.
- Each implementation slice has automated tests at its owning contract and an
  end-to-end test when it crosses crate, process, HTTP, or WebSocket boundaries.
- Static subgraphs can serve federated queries and mutations before dynamic
  discovery is introduced.
- Candidate schema admission is immutable, deterministic, and atomic; invalid
  or unavailable candidates cannot replace the last-known-good executable
  graph.
- Fixed and argument-templated router authorization decisions match
  independently enforced subgraph decisions across HTTP and WebSocket flows.
- Connected clients receive bounded ephemeral subscription events, missed
  events remain explicitly unreplayed, and schema-reload reconnection behavior
  is tested.
- Dynamic registration, schema retrieval, and administrative operations have
  authenticated identities, bounded inputs, SSRF-resistant endpoint policy,
  secret-safe telemetry, and explicit restart/state semantics.
- Relevant formatting, tests, warnings-denied Clippy and Rustdoc, dependency
  direction, backend compile lanes, and documentation checks pass at handoff.
- All FR-001 through FR-184 requirements have an automated owner in the table
  above; any deliberate deferral requires a scope change in the canonical
  requirements before production readiness can be claimed.
- The generic router release candidate is independently acceptable at Slice 8.
  Overall initiative completion additionally requires the separately authorized
  GEMA migration and cleanup evidence in Slice 9.

## Current checkpoint

Slices 0 through 8 are implemented. The workspace contains independently
consumable protocol and router packages with the private exact-pinned Hive
composition/execution seam accepted in ADR-0008. Generated and hand-written
subgraphs share protocol v1; representable ORM declarations drive both
authoritative guards and advisory router metadata without changing the existing
discovery fingerprint or pulling the router into ordinary ORM builds.

The generic release candidate now provides atomic static and dynamic graph
lifecycle, last-known-good polling/admission, authenticated HTTP and
`graphql-transport-ws`, graph-bound fixed and templated scope preflight,
resource-server-only JWKS and optional `agql-auth` validation, identity-bound
administration, deny-by-default SSRF controls, strict file/environment
configuration, bounded requests/subscriptions/deadlines, structured telemetry,
authenticated metrics, an opt-in Prometheus exporter, stable library handles,
and bounded signal-driven shutdown. Operator, schema evolution, reconnect,
threat-model, troubleshooting, and migration guidance is component-local.

Maintained evidence covers every generic FR-001 through FR-184 owner, including
the real executable's pre-bind check, authenticated HTTP and WebSocket work,
downstream timeout/recovery, and listener release. The hardening campaign also
covers repeated atomic reloads, JWKS outage/rotation, WebSocket churn, bounded
lag, rejection/recovery, and graceful drain. Package/backend, feature-off,
optional-auth, MSRV, Clippy, Rustdoc, dependency, inventory, duplicate, and
documentation gates pass, so the generic Slice 8 boundary is closed.

The external `agql-auth` 0.14 interoperability change is implemented and the
workspace exact pin now resolves revision
`413fda3435f060604cd653c11e2cc18a668aace1`. New access tokens use the standard
OAuth `scope` string; bounded legacy `scopes` validation remains available for
a staged expiry window. The router stays validation-only and never receives
private signing material.

The separately owned GEMA consumer has completed source migration, reversible
live cutover, and deployment validation at consumer revision
`7a3150cbe6c4332c1b786cb2a1a1d680bfbeb8bb`, using the reviewed router 0.1.1
revision `d178af46648881d1959701b1fb56f2885bb326cb`. Its exact router artifact
has SHA-256
`021a68d07c763ad47a78d1da6a54ef06eeb245db30598123642f472da74253a9`;
the unchanged configuration composed all eight generated protocol-v1
descriptors with graph fingerprint
`sha256:4425e7c84a2fc4cb0f277fbcac4602b4b3758cd4bf1e1fbb074939ae8fdaf71b`.
Artifact-specific SBOM, notices, native/linked-component inventory, approval,
checksums, and deployment results are retained by the deployment owner.

The variable-backed subscription matrix is acceptance-green: matching and
inline values each reached FAME with one subgraph request, while a mismatched
value returned `FORBIDDEN` with zero subgraph requests. Lowercase HTTP and
WebSocket authorization propagation passed. FAME now rejects ambiguous header
variants while reading names case-insensitively; FAME and its retained media
process run from the selected release root. Strict runtime status is 14/14,
public monitoring passed, and a graceful stop/readiness-close/restart cycle
preserved post-restart subscriptions without changing RBI or relay tiers.

A later live agent log-upload operation exposed a connection-containment gap:
an oversized public WebSocket message ended the client transport, a secondary
bridge task masked that cause with generic 1011, and an unbounded consumer
retry loop exhausted the FAME connection cap. Router 0.1.2 now makes terminal
ownership first-cause-wins, closes the private bridge on public termination,
rate-limits public upgrade attempts, and proves stable operation IDs,
one-shot mutation completion, sibling-operation isolation, and continued use
of the same downstream socket after an upstream subscription failure. The
64 KiB serialized-message boundary is unchanged; the consumer must move or
chunk bulk log payloads and add mutation-safe jittered reconnect behavior.

The live cutover remains in place, but Slice 9 acceptance is reopened until the
0.1.2 artifact and consumer containment changes pass live validation. Cosmo,
NATS, WGC, and JetStream assets remain inactive with no process, container,
unit, or listener. Permanent deletion must not occur until the deployment
owner confirms both renewed acceptance and that no other workload owns the
retained assets. Every later binary, container, hosted service, lockfile, or
delivery channel still requires its own artifact-specific review under
ADR-0008.
