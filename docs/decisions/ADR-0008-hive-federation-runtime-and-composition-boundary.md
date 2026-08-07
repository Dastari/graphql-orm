---
title: ADR-0008 Hive federation runtime and composition boundary
kind: decision
status: accepted
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-07
review_by: 2027-08-07
supersedes: []
---

# ADR-0008: Hive federation runtime and composition boundary

## Context

The router needs Federation v2 composition, query planning, execution,
subscriptions, and graph replacement without exposing an unstable engine in
its public API. Implementing those facilities locally is outside the project
scope. The chosen libraries are pre-1.0 and may change internal APIs, so the
workspace also needs an explicit update and isolation policy.

The current proof uses these exact direct versions:

- `hive-router = 0.0.87` for the executable graph, planner, executor, plugin
  hooks, downstream transport, and subscription lifecycle;
- `graphql-composition = 0.12.2` for Federation v2 composition;
- `cynic-parser = 0.11.2` for structural inspection of source root ownership.

The root lockfile currently selects `hive-router-config 0.1.10`,
`hive-router-query-planner 2.10.11`, `hive-router-plan-executor 7.0.2`, and
`ntex 3.10.0`. All are registry dependencies. Hive components are MIT;
`graphql-composition` and `cynic-parser` are MPL-2.0; `ntex` is MIT or
Apache-2.0. Architecture acceptance and artifact distribution approval are
separate decisions; the distribution boundary below applies before release.

At these pins, the router's normal-and-build dependency tree contains 611
unique package/version nodes, including the router itself. This is a material
compile-time, update-review, and supply-chain cost rather than a negligible
implementation detail. The highest declared Rust minimum in that selected
closure is currently 1.90 (`vrl 0.33.1`), so the router cannot claim a lower
MSRV without changing or constraining the dependency graph. The router now
declares Rust 1.90 and has a dedicated CI test lane at Rust 1.90.0; the complete
protocol and router suites also pass locally on that toolchain.

The selected composer currently renders valid Federation SDL that Hive can
parse, but it omits root-object `@join__type` ownership directives required by
Hive query planning. Treating the rendered SDL as an unstructured string would
make this compatibility boundary unsafe.

## Decision

Keep Hive and composition types behind a private `graphql-orm-router` adapter.
The adapter will:

- ingest immutable subgraph inputs in stable name, endpoint, and revision
  order;
- reject malformed sources, duplicate identities, composition error
  diagnostics, and runtime-construction errors before publication while
  retaining non-fatal composer warnings;
- structurally parse the composed document and add only missing root-object
  `@join__type(graph: ...)` directives derived from parsed source root
  participation and the composed `join__Graph` enum;
- construct the complete Hive `Supergraph` before atomically publishing its
  owning `Arc`;
- retain the exact last-known-good executable graph on every candidate failure;
- pin ordinary requests and subscriptions to the selected graph owner, using
  Hive retirement signalling rather than silently migrating in-flight work;
- enforce router policy through the post-coercion `on_graphql_analysis` hook so
  a denial occurs before query planning and downstream execution; and
- expose only router-owned configuration, status, handles, and error types.

Direct Hive, composition, and compatibility-parser versions remain exact.
Transitive planner and executor changes are reviewed through the root lockfile.
An update must run the structural-adapter, query-plan, downstream-denial,
in-flight replacement, and subscription-retirement regression suite before the
lockfile is accepted.

Hive's JWT and object-storage configuration are not part of the router-owned
configuration surface. The router is a resource server: it may validate JWTs
with public keys through an engine-neutral provider, but it never loads RSA
private keys, signs or issues tokens, refreshes sessions, or performs RSA
decryption. A later optional `agql-auth` adapter may use
`AccessTokenValidator`; issuer-side `AuthService` and signing configuration
remain outside the router process.

The router does not expose Hive S3 or `object_store` facilities. If durable or
object storage is later required, a deployment-owned integration must use the
workspace's `graphql-orm-storage` contract without making it a dependency of
the project-neutral router crate. Enabling a Hive storage path requires a later
architecture and security review after the affected XML parser path is fixed.

## Current evidence and audit disposition

Maintained tests now prove the query, entity, mutation, post-coercion denial,
last-known-good, in-flight replacement, and subscription gates above. The
subscription proof uses a real test-owned SSE upstream and Hive's
`graphql-transport-ws` endpoint. Retirement sends
`SUBSCRIPTION_SCHEMA_RELOAD` and completes the operation; the retained
transport rejects new work, and a fresh connection selects the replacement
graph. Structural regressions cover custom operation-root names and multiple
owners of the same query root.

The 2026-08-07 normal-and-build dependency audit found six affected packages.
The root lockfile now selects fixed compatible versions for `anyhow` 1.0.103,
`event-listener` 5.4.2, `rand` 0.8.6, and `rustls-webpki` 0.103.13. Two findings
remain in the compiled Hive closure and receive these accepted dispositions:

- `quick-xml` 0.39.4 remains reachable through Hive's unconditional
  `object_store` 0.13.2 dependency. That version constraint cannot select the
  0.41.0 fix for
  [RUSTSEC-2026-0194](https://rustsec.org/advisories/RUSTSEC-2026-0194.html)
  and
  [RUSTSEC-2026-0195](https://rustsec.org/advisories/RUSTSEC-2026-0195.html);
  resolution requires a compatible upstream Hive/object-store change. The
  router does not expose or initialize that storage path, and ordinary GraphQL
  input is not XML. The finding remains tracked and blocks enabling Hive
  storage, not router development.
- `rsa` 0.9.10 remains reachable through Hive's unconditional
  `jsonwebtoken` RustCrypto feature and is affected by the unpatched
  [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html).
  The advisory concerns observable private-key operations. The router neither
  configures Hive JWT authentication nor accepts private key material; its
  future authentication boundary is public-key resource-server validation.
  The crate-level finding remains monitored, but the vulnerable operation is
  outside the accepted router execution and configuration boundary.

Hive's available feature flags cannot remove either package from the compiled
closure. The
[Hive repository](https://github.com/graphql-hive/router) was actively
maintained at review time, but its
[security page](https://github.com/graphql-hive/router/security) did not
publish a security policy; exact pins and project-owned dependency monitoring
remain necessary.

## Acceptance evidence

Maintained, test-owned evidence proves:

- one- and two-subgraph queries, entity resolution, and mutation routing;
- variable-aware root-field denial with no downstream request;
- invalid-candidate last-known-good behavior and an in-flight request completing
  on its selected graph during replacement;
- an upstream `graphql-transport-ws` subscription, graph-retirement reload
  error, closure, and reconnect behavior;
- one selected planner/executor/composition universe and the supported Rust
  toolchain lane; and
- dependency license metadata, vulnerability reachability, and upstream
  maintenance review appropriate for accepting the architecture.

The engine remains a private implementation detail. Public router APIs use
router-owned types, and each release channel remains subject to the
distribution boundary below.

## Distribution boundary

This decision accepts the router architecture, private-adapter boundary, and
reviewed locked dependency selection. It does not approve publication,
delivery, or other distribution of a router-containing source archive,
library, binary, container image, or service artifact.

Every selected node declares license metadata. Six are MPL-2.0 and none declare
GPL, LGPL, AGPL, network-copyleft, EUPL, EPL, CDDL, SSPL, or BUSL expressions.
Some dependencies use Cargo's deprecated slash-form license syntax, and native
or bundled components still require artifact-specific notice review. This is
inventory evidence, not a legal conclusion.

Before a supported release channel delivers a router-containing artifact, the
release owner must retain evidence derived from the exact lockfile and
artifact: a dependency/SBOM and third-party-notice inventory; disposition of
non-strict SPDX metadata; review of MPL-2.0 notice, source-availability, and
modification obligations; and review of native or bundled components. The
evidence must identify the artifact and distribution channel and receive the
project's designated release/compliance approval.

This ADR does not determine the legal effect of a dependency license or
substitute for that release review. Release procedure and evidence belong in a
maintained runbook rather than this immutable architecture record.

## Consequences

- The project reuses a capable Rust Federation runtime while containing its
  pre-1.0 API churn in one private adapter.
- Atomic ownership gives requests a complete old or new graph and makes failed
  candidate construction unable to corrupt the active runtime.
- The root-ownership repair is a pinned compatibility adapter, not a general
  SDL rewriting facility. Its necessity must be re-evaluated on every
  composition upgrade and removed when upstream output is directly compatible.
- Hive adds substantial compile time and dependency weight, which is why the
  router remains outside the workspace default members.
- The router owns neither durable object storage nor token issuance/private-key
  operations, reducing the reachable surface of unconditional Hive
  dependencies without pretending those packages are absent.
- Artifact-specific notices, SBOM evidence, and applicable source obligations
  remain release gates; architecture acceptance is not distribution approval.

## Supersession

This decision does not supersede an earlier runtime decision. A change that
exposes engine types, Hive storage configuration, or private-key operations
requires a later ADR that supersedes this record.
