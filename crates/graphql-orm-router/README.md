---
title: graphql-orm-router
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-08
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router

`graphql-orm-router` is the project-neutral Federation router planned for this
workspace. Its public library API prepares and serves a statically configured
atomic HTTP graph while keeping composition and Hive execution types private.

Static startup is deliberately fail closed. The router validates the listener,
public GraphQL path, URL and header policy, loads its verification keys, fetches
every bounded SDL and protocol response, composes the complete candidate, binds
authorization metadata to that runtime, and constructs its executable graph
before opening the listener. A deployment without an authentication provider
must explicitly opt into anonymous development mode; sensitive bearer and
cookie headers cannot be enabled through the ordinary downstream-header
allowlist.

```rust,no_run
use std::sync::Arc;
use graphql_orm_router::{
    JwksAuthenticationConfig, JwksAuthenticationProvider, RouterConfig,
    StaticSubgraph, SubscriptionConfig,
};

let authentication = JwksAuthenticationProvider::new(
    JwksAuthenticationConfig::new(
        "https://identity.example/.well-known/jwks.json",
        "https://identity.example",
        ["graphql-router"],
    )?,
)?;
let schema_credential = std::env::var("PRODUCTS_SCHEMA_AUTHORIZATION")?;

let config = RouterConfig::new("127.0.0.1:4000".parse()?)
    .with_authentication_provider(Arc::new(authentication))
    .with_graphql_path("/graphql")
    .with_subscriptions(SubscriptionConfig::new())
    .forward_header("x-request-id")
    .with_subgraph(
        StaticSubgraph::new(
            "products",
            "http://products:8080/graphql",
            "http://products:8080/sdl",
        )
        .with_protocol_url("http://products:8080/.well-known/graphql-router")
        .with_schema_header("authorization", schema_credential),
    );

graphql_orm_router::run(config)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

The public service exposes the configured GraphQL path plus `/health` and
`/readiness`. Readiness requires an active graph. `prepare()` is also available
for embedders that want to inspect the process-local graph version, SHA-256
fingerprint, and composition warnings before serving.

The standalone executable consumes strict JSON and validates the complete graph
without binding when `--check` is supplied:

```text
graphql-orm-router --config examples/router.example.json --check
graphql-orm-router --config examples/router.example.json
```

It handles `SIGTERM`, `SIGINT`, and `SIGQUIT` with a bounded graceful drain.
The file format, environment-only secret rules, production budgets, metrics,
reload/reconnect procedures, and residual deployment responsibilities are in
the [component documentation](docs/README.md). Initial adopters should also read
the [migration guide](MIGRATION.md).

The public WebSocket gateway preserves GraphQL operation IDs, keeps an
operation-scoped upstream failure from closing sibling work, and treats
one-shot `next`/`complete` independently from long-lived subscriptions. Public
termination is first-cause-wins and closes the private bridge. A bounded
upgrade-attempt token bucket contains reconnect storms; clients must still use
jittered backoff and must not automatically replay an uncertain mutation. The
complete serialized WebSocket message remains limited to 64 KiB, so bulk
payloads require an application HTTP or chunking contract.

## Graph lifecycle

Every enabled SDL and protocol endpoint is polled with its last ETag. Poll
interval, fetch timeout, SDL body limit, retry count, and retry delay are
bounded configuration. Router-relevant SDL is parsed and deterministically
sorted before hashing; authorization metadata is canonicalized separately, and
advertised deployment endpoints are excluded from the admission fingerprint.
An unchanged accepted or rejected fingerprint skips composition.

A changed input is composed with every other active subgraph's last-known-good
input. Federation composition, executable runtime construction, subscription
validation, and authorization-catalog binding must all succeed before one
atomic publication. Failed fetches and rejected candidates retain the exact
active executable graph; disappearance is health failure, never implicit
removal. Refreshes and explicit removals are serialized so cancellation or a
slower older attempt cannot publish stale state.

`PreparedRouter::handle()` supplies process-local `refresh`, explicit
`remove_subgraph`, and safe `status` operations. Status includes the active
graph identity and deterministic registered, candidate, active, unhealthy,
rejected, or disabled subgraph state without SDL, credentials, or downstream
error bodies. Successful removal is intentionally process-local: static
configuration restores the source after restart.

## Dynamic registration and administration

`AdminConfig` opens a distinct administrative listener only when an
authentication provider is configured. Its status, refresh, registration, and
removal routes each require an exact configurable scope. `TrustedSubgraph`
binds one authenticated service subject to one subgraph ID/name, exact metadata
URL, GraphQL origin, SDL origin, and router-owned schema-fetch headers. Neither
the registration bearer nor client credentials are forwarded to metadata or
SDL endpoints.

Dynamic network access is deny by default. `NetworkPolicy` requires exact
hosts and ports plus post-resolution CIDRs, validates every DNS answer under a
timeout and address-count bound, rejects redirects, bypasses ambient proxies,
pins metadata/SDL resolution, verifies the connected peer, and rejects
loopback, private, and link-local ranges unless each range is explicitly
enabled and allowlisted. Because the private Hive execution client cannot
consume the router's pinned resolver result, dynamically registered GraphQL
and WebSocket origins must use an IP-literal host; this closes the second DNS
lookup rebinding window. Static deployment-owned destinations are unaffected.

Administrative status is deliberately safe: it reports graph identity,
composition time, source/fingerprint/health/admission state, and sanitized
errors, but not endpoints, SDL, headers, tokens, keys, or private variables.
Registration and removal are process-local. On restart, static sources rebuild
from configuration and every dynamic service must authenticate and register
again; no status response represents this state as durable or shared.

`RequestLimits` bounds public request bodies and headers, parser tokens,
selection depth, aliases, directives, and normalized field cost. Subscription
connection, operation, message, buffer, and fan-out limits remain under
`SubscriptionConfig`. Public and downstream request deadlines, per-host
connection pools, and the graceful drain window are independently bounded.
Rejections happen before downstream GraphQL work.

`RouterHandle::metrics()` returns an engine-neutral process-local snapshot.
When administration is enabled, `GET /_router/metrics` exposes the same core
counters and gauges under its own exact scope. An optional separate Prometheus
listener exposes the private engine's richer execution and subscription
metrics, including lagged and dropped event counters; it is disabled by default
and deployment network policy owns access.

## Authentication boundary

`JwksAuthenticationProvider` is an RS256 resource-server implementation. It
requires HTTPS except for an explicit loopback-development option, performs a
bounded initial JWKS load before readiness, rotates public verification keys in
the background, and rejects stale cache state. Signature, key ID, issuer,
audience, expiry, not-before, and an injectable clock are validated. The OAuth
`scope` claim is a space-delimited string; the legacy `scopes` array is rejected
unless `LegacyScopeClaims::Accept` is explicitly configured, and conflicting
forms are never merged.

The optional `auth-agql` feature adapts the exact-pinned
`agql_auth::AccessTokenValidator` and its configured scope matcher in one
direction. Standard/legacy scope policy is configured directly on that
validator, and the adapter consumes only its verified normalized principal; it
does not decode the JWT payload again. The router does not expose or construct `AuthService`, accept private
keys, issue or refresh tokens, own sessions, or perform private-key decryption.
An identity service may continue to use those issuer-side responsibilities from
`agql-auth` independently.

Authorization preflight enumerates every selected root field across aliases,
fragments, directives, defaults, and multiple selections after GraphQL
validation. It supports authenticated, fixed any/all-scope, and documented
scalar argument-template requirements. Variable-backed templates are completed
with the current operation's values before downstream execution; WebSocket
operations never reuse variables from another operation on the connection.
Allow only permits normal downstream execution, where subgraph guards and data
policy remain authoritative. Only a successfully validated original bearer
credential is propagated to GraphQL destinations.

## Subscriptions

When `SubscriptionConfig` is present, the same public GraphQL path accepts the
`graphql-transport-ws` subprotocol. Subscriptions require authentication and a
protocol descriptor that declares subscription ownership. The client supplies
one bearer credential in `connection_init`, either directly or under a
`headers` object:

```json
{
  "type": "connection_init",
  "payload": { "authorization": "Bearer <access-token>" }
}
```

The router verifies that credential before acknowledging the connection,
authorizes every operation and its own variable map against the selected
immutable graph, and forwards only the approved credential. A usable expiry is
mandatory. Tokens cannot be replaced or refreshed in-place: at expiry the
connection closes with code `4401`, and the client must reconnect and
authenticate again.

Public connection count, operations per connection, client-message size,
upstream buffering, and downstream fan-out are bounded. The default upstream
transport is `graphql-transport-ws` at the deployment-owned GraphQL endpoint;
`StaticSubgraph::with_subscription_websocket_path` can override only its path.
Subscription deduplication is disabled until an explicitly tested identity
policy is selected. Delivery is process-local and ephemeral: the router does
not persist or replay missed events, and ordinary queries remain the recovery
path after disconnect or lag. On graph retirement the selected operation
receives `SUBSCRIPTION_SCHEMA_RELOAD`; clients reconnect rather than migrating
silently.

The maintained tests prove deterministic Federation v2 composition,
Hive query-plan construction for cross-subgraph queries and mutations,
last-known-good rejection, atomic owner replacement, and graph-retirement
signalling. A test-owned loopback Hive process additionally proves HTTP entity
and mutation routing, post-coercion root denial before downstream connection,
valid and invalid replacement, and completion of an old in-flight request. A
second loopback proof exercises Hive's `graphql-transport-ws` endpoint:
a test-owned upstream SSE event, retirement's reload error and completion,
rejection of new work on the retained connection, and fresh-connection
selection of the replacement graph. The public static-server harness also
proves credentialed SDL retrieval, custom-path HTTP queries, entity resolution,
mutation ownership, downstream error paths, header allowlisting, probes, and
rejection before bind for an invalid complete graph.
Authentication coverage adds a deterministic loopback issuer/JWKS fixture,
rotation and outage behavior, standard/legacy scope migration, exact and
configured hierarchical matching, stale metadata admission, and downstream
denial/credential-isolation evidence.
The authenticated public WebSocket harness additionally proves pre-ACK
failure, connection-init timeout, scope denial before upstream open, a real
upstream WebSocket subscription, filtered arguments, multiple clients,
connection and operation bounds, upstream failure isolation, disconnect and
fresh reconnect, approved bearer propagation, and expiry closure.
Lifecycle coverage adds conditional polling, bounded retry and outage recovery,
canonical no-op detection, incompatible candidate retention, explicit removal,
cancellation and stale-refresh serialization, in-flight HTTP pinning, and an
authenticated graph-plus-policy reload that retires an existing subscription
and selects the replacement only after reconnect.
The bounded hardening campaign additionally exercises 24 consecutive atomic
reloads, downstream timeout/recovery, JWKS outage/rotation, repeated WebSocket
churn, bounded lag, executable HTTP/WebSocket startup, and graceful signal
shutdown with listener release.

Federation engine and composition types are private implementation details and
must not appear in the eventual public API. The crate declares Rust 1.90 and is
tested on both 1.90.0 and stable. ADR-0008 accepts this private engine boundary
with explicit mitigations: Hive JWT and object-storage configuration are not
exposed, authentication remains public-key resource-server validation, and
artifact-specific distribution review remains a release gate.
