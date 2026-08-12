---
title: graphql-orm-router configuration reference
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-08
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router configuration reference

The executable accepts one strict, UTF-8 JSON document no larger than 1 MiB.
Unknown fields fail startup. Use
[`examples/router.example.json`](../examples/router.example.json) as the
complete production-shaped example.

Run it with:

```text
graphql-orm-router --config /etc/graphql-orm/router.json
graphql-orm-router --config /etc/graphql-orm/router.json --check
```

`GRAPHQL_ORM_ROUTER_CONFIG` may replace `--config`.
`GRAPHQL_ORM_ROUTER_LISTENER` and
`GRAPHQL_ORM_ROUTER_ADMIN_LISTENER` override the corresponding socket
addresses. `--check` loads environment secrets, initializes JWKS, fetches every
SDL and descriptor, composes the graph, and constructs the executable runtime;
it does not bind a listener.

## Programmatic configuration types

The strict JSON file is represented by `RouterFileConfig`; applications using
the library directly build `RouterConfig` (also exposed as `RouterBuilder`).
The public field-level source contracts are:

| Type | Responsibility |
| --- | --- |
| [`RouterConfig`](../src/config.rs) | Static listener, subgraphs, timeouts, and fail-closed builder validation. |
| [`SubscriptionConfig`](../src/config.rs) | WebSocket enablement, connection, operation, queue, and message limits. |
| [`RequestLimits`](../src/config.rs) | HTTP body/header/parser/depth/alias/directive/cost ceilings. |
| [`RouterTelemetryConfig`](../src/config.rs) | Log and optional Prometheus listener settings. |
| [`AdminConfig`](../src/config.rs) | Separate authenticated administration listener and scopes. |
| [`JwksAuthenticationConfig`](../src/jwt.rs) | RS256 public-key verification, issuer/audience, cache, and bounded JWKS fetch. |
| [`NetworkPolicy`](../src/network.rs) | Dynamic destination host/port/CIDR/DNS policy. |
| [`RouterFileConfig`](../src/file_config.rs) | Strict file representation and environment-secret mapping. |

The defaults and hard ceilings below are applied by the file loader and
programmatic validation. Do not treat an unlisted builder field as a promise of
an unbounded value.

## Top-level fields

| Field | Default or rule |
| --- | --- |
| `listener` | Required socket address; production commonly uses `0.0.0.0:4000` behind a TLS proxy. |
| `graphqlPath` | `/graphql`; absolute, non-root, without query or fragment. |
| `anonymousDevelopment` | `false`; mutually exclusive with authentication and unsuitable for production. |
| `authentication` | Required unless anonymous development is explicitly enabled. |
| `subgraphs` | At least one static source is required. |
| `forwardedHeaders` | Empty. Sensitive, hop-by-hop, cookie, and authorization names are rejected. |
| `schemaFetchTimeoutMs` | 10000. |
| `maxSdlBytes` | 2097152 per SDL or protocol response. |
| `schemaPollIntervalMs` | 30000. |
| `schemaRefreshAttempts` | 2; valid range 1–10. |
| `schemaRefreshRetryDelayMs` | 100 and no greater than the poll interval. |
| `publicRequestTimeoutMs` | 60000; valid through 300000. |
| `subgraphRequestTimeoutMs` | 30000; valid through 300000. |
| `maxSubgraphConnectionsPerHost` | 100; valid range 1–10000. |
| `gracefulShutdownTimeoutSeconds` | 10 whole seconds; valid range 1–60. |
| `requestLimits` | Production-bounded defaults described below. |
| `subscriptions` | Disabled; enabling it requires authentication. |
| `admin` | Disabled; enabling it requires authentication and a distinct listener. |
| `telemetry` | JSON/info logs; metrics exporter disabled. |

Durations must be nonzero. Values over their hard safety ceilings fail before
binding.

## Authentication

`authentication` requires `jwksUrl`, `issuer`, and a non-empty `audiences`
array. JWKS uses HTTPS. Plain HTTP is accepted only for loopback when
`allowInsecureLoopbackJwks` is explicitly true. Optional bounds are
`cacheTtlSeconds`, `refreshIntervalSeconds`, `requestTimeoutMs`,
`maxJwksBytes`, and `leewaySeconds`. `acceptLegacyScopes` defaults false.

The router validates RS256 public keys only. Configuration has no private-key,
token-signing, session, refresh-token, or RSA-decryption field.

For programmatic setup, `JwksAuthenticationConfig::new` requires a JWKS URL,
issuer, and non-empty audiences. It defaults to a 15-minute key cache,
5-minute refresh interval, 5-second request timeout, 1 MiB JWKS body limit,
zero clock leeway, rejected legacy scope arrays, and HTTPS-only verification.
Leeway may not exceed five minutes; insecure HTTP is an explicit loopback-only
development setting. These fields are configured by the `with_*` methods in
the [source type](../src/jwt.rs).

## Static subgraphs and secrets

Each `subgraphs` entry requires `name`, `graphqlUrl`, and `sdlUrl`.
Authenticated deployments also require `protocolUrl`. An optional
`subscriptionWebsocketPath` replaces only the path used for the upstream
WebSocket.

Secret header values cannot appear in JSON. Map a header name to an environment
variable name:

```json
{
  "schemaHeadersFromEnv": {
    "authorization": "PRODUCTS_SCHEMA_AUTHORIZATION"
  }
}
```

A missing, empty, or non-UTF-8 variable fails startup. Schema credentials are
used only for that source's descriptor and SDL fetches. They are never copied
to GraphQL execution. A separately validated original client bearer is the only
authorization credential propagated to an execution destination.

## Request and WebSocket limits

The default `requestLimits` are 1 MiB body, 64 KiB aggregate headers, 10000
parser tokens, depth 20, 50 aliases, 100 directives, and normalized field cost
500. Every value is configurable with its matching `max...` JSON field and
must remain within the crate's hard safety ceiling.

The default `subscriptions` limits are 1024 process-wide public connections,
128 connection attempts per second with one second of burst capacity, 32
operations per connection, downstream broadcast capacity 32, upstream buffer
capacity 1024, 64 KiB client messages, and a 5000 ms `connection_init`
deadline. Configure the attempt budget with
`maxConnectionAttemptsPerSecond`. Excess attempts return HTTP 429 with
`Retry-After: 1`; active-connection saturation returns HTTP 503. All queues
are bounded. Delivery is ephemeral and unreplayed.

The message limit applies to the complete serialized
`graphql-transport-ws` message, including variables and base64 expansion. Bulk
uploads belong on a bounded HTTP endpoint or an application chunk protocol,
not in one WebSocket operation.

## Administration and dynamic destinations

`admin.listener` must be distinct from the public listener. Its exact default
scopes are `router.status`, `router.refresh`, `router.register`,
`router.remove`, and `router.metrics`. `maxRequestBodyBytes` defaults to 16 KiB.

`trustedSubgraphs` binds one authenticated `serviceSubject` to an exact
`subgraphId`, name, metadata URL, GraphQL origin, schema origin, and optional
environment-backed schema headers. Bindings must be unique.

Dynamic `network` policy is deny by default. Configure `allowedHosts`,
`allowedPorts`, and `allowedNetworks`; loopback, private, and link-local ranges
also require their matching explicit boolean. DNS has a timeout and resolved
address-count bound. Dynamic execution GraphQL/WebSocket origins must use an
IP-literal host because the private execution client cannot consume the
router's pinned DNS result.

The programmatic [`NetworkPolicy`](../src/network.rs) defaults to no allowed
hosts/networks, ports 80 and 443 only, loopback/private/link-local denial, a
2-second DNS deadline, and 16 resolved addresses. Hosts, ports, CIDRs, and
special ranges must all be expressly allowed; this type has no credential or
proxy setting.

## Telemetry

JSON `info` logs are the production default. `textLogsForDevelopment` and
`debug` are diagnostic modes; debug engine logs may include GraphQL document
literals and should be enabled only in a controlled environment.

Prometheus is opt-in with a distinct `port` and absolute non-root `path`. That
scrape listener has no application authentication; bind and protect it with
deployment network policy. The authenticated administrative metrics route is
`GET /_router/metrics`.
