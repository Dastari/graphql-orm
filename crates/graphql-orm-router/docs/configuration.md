---
title: graphql-orm-router configuration reference
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-11-12
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

On Unix, `--listen-stdin` consumes an already-bound TCP listener inherited as file
descriptor 0, without releasing or rebinding its port. The actual socket address
overrides the configured public `listener` before preparation, including
subscription endpoint validation. It cannot be combined with `--check`; invalid
or non-socket stdin fails startup. Administrative listeners retain their normal
configuration. An embedding launcher can pass a `TcpListener` through
`Command::stdin(Stdio::from(OwnedFd::from(listener)))`. Library hosts can use
`graphql_orm_router::run_with_listener(config, listener)` on the owned runtime.
The default command continues to bind the configured address normally.

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
| [`ByteRoute`](../src/byte_route.rs) | One opt-in streamed `GET`/`HEAD` path prefix mapped to one static upstream. |
| [`JwksAuthenticationConfig`](../src/jwt.rs) | RS256 public-key verification, issuer/audience, cache, and bounded JWKS fetch. |
| [`RoleScopeCatalogueConfig`](../src/jwt.rs) | Optional signed role-to-scope catalogue retrieval and cache bounds. |
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
| `introspection` | `false`. `true` enables `__schema` and `__type`; with authentication configured they also need a valid bearer. The depth limit applies either way. |
| `authentication` | Required unless anonymous development is explicitly enabled. |
| `scopeMatcher` | Omitted means exact-string matching. `kind: "hierarchical"` requires the `auth-agql` feature. |
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
| `admin` | Disabled; enabling it requires authentication and a listener port distinct from the public port. |
| `telemetry` | JSON/info logs; metrics exporter disabled. |
| `nodeLookup` | Disabled. `{}` enables the router-hosted node lookup; `maxNodes` defaults to 100, valid range 1–10000. |
| `byteRoutes` | Empty, so no byte route is served. See [Byte routes](#byte-routes). |

Durations must be nonzero. Values over their hard safety ceilings fail before
binding.

## Node lookup

`nodeLookup` (programmatically `RouterConfig::with_node_lookup(NodeLookupConfig)`)
adds `Query.node(NodeId: ID!): Node` and `Query.nodes(NodeIds: [ID!]!): [Node]!`.
Each candidate graph is scanned for object types with `NodeId: ID!` and a
resolvable `@key(fields: "NodeId")`, which graphql-orm generates for
`#[graphql_entity(node_id)]`. When at least one exists, the router adds the
reserved subgraph `graphql-orm-node` to the same atomic candidate. That subgraph
owns `interface Node @key(fields: "NodeId")` and has one
`@key(fields: "NodeId", resolvable: false)` stub per type. It is never contacted
over the network: the router answers its fetches in-process from the graph each
request is pinned to.

The router decodes only the typename and returns `{ __typename, NodeId }`. The
planner fetches every other field from the owning subgraph through `_entities`,
one fetch per subgraph for any number of identities. The router authorizes no
record: an unreadable row is `null` exactly like a missing one. A malformed
identity, or one whose typename is not a composed `Node` type, is `null`. A
`nodes` list longer than `maxNodes` fails with `INVALID_INPUT`. With
authentication, `node` and `nodes` require an authenticated caller.

Subgraphs must not declare `interface Node` themselves: `graphql-composition`
0.12.2 requires every subgraph that defines it to define every implementation.
`@interfaceObject` is not used.

## Authentication

`authentication` requires `jwksUrl`, `issuer`, and a non-empty `audiences`
array. JWKS uses HTTPS. Plain HTTP is accepted only for loopback when
`allowInsecureLoopbackJwks` is explicitly true. Optional bounds are
`cacheTtlSeconds`, `refreshIntervalSeconds`, `requestTimeoutMs`,
`maxJwksBytes`, and `leewaySeconds`. `acceptLegacyScopes` defaults false.

`roleScopeCatalogue` is optional and requires `auth-agql`. Select exactly one
of literal `url` or `urlFromEnv`. Request headers are loaded only through
`requestHeadersFromEnv`; values never appear in JSON or diagnostics. Optional
bounds are `cacheTtlSeconds`, `maximumSignedLifetimeSeconds`,
`clockSkewLeewaySeconds`, `retryBackoffSeconds`,
`maximumRetryBackoffSeconds`, and `maxBodyBytes`. The default response bound is
2 MiB for the complete signed JSON envelope, including the catalogue repeated
inside its JWS. An explicit lower limit still applies. This is separate from
the JWKS and public GraphQL request-body limits. Its URL is HTTPS-only except for
explicit loopback development. The literal `allowInsecureLoopbackHttp` flag or
`allowInsecureLoopbackHttpFromEnv` can opt into that development exception,
but both cannot be set together.

JWKS remains the startup authentication dependency. The role catalogue is
fetched lazily on the first authorization-role-bearing request and retried with
bounded exponential backoff as well as by the ordinary authentication refresh
loop. A fetch verifies the RS256 signature
with the JWKS cache and binds the exact catalogue, issuer, audience, purpose,
and independently configured signed lifetime. `cacheTtlSeconds` is only the
soft refresh age; it does not cap issuer lifetime.

When configured, the distinct typed `authorization_roles` claim expands
through the last signature-verified catalogue and is unioned with direct token
scopes. Application `roles` are not expanded. Unknown authorization roles fail
explicitly and request an immediate refresh. A failed refresh preserves and
continues serving the last verified snapshot past its soft age or signed
expiry, increments `role_scope_stale_serve_total`, and emits rate-limited warning
logs. A role-bearing credential fails closed only when no verified snapshot is
available or its role remains unknown; direct-scope-only credentials do not
depend on the optional authority source.

The router validates RS256 public keys only. Configuration has no private-key,
token-signing, session, refresh-token, or RSA-decryption field.

Scope matching is a separate resource-server policy. Omission or
`{"kind":"exact"}` preserves exact-string compatibility. With the
`auth-agql` feature, `kind: "hierarchical"` accepts these optional fields:

| Field | Default or rule |
| --- | --- |
| `separator` | `.`; one visible character other than an ASCII letter, digit, `-` or `_`. |
| `wildcard` | `*`; one non-empty segment without whitespace, containing a character other than an ASCII letter, digit, `-` or `_`. |
| `wildcardMatchesMultiSegment` | `true`; a trailing wildcard matches the remaining hierarchy. |
| `allowUniversalWildcard` | `false`; a bare wildcard has no implicit authority. |
| `superScopes` | Empty; each listed grant satisfies ordinary requirements. |
| `allowSuperScopesForExactOnly` | `false`; when enabled, exactly configured super-scopes also satisfy exact-only requirements. |
| `exactOnlyScopes` | Empty; wildcard-derived grants never satisfy a listed requirement. |
| `exactOnlyScopePatterns` | Empty; wildcard-derived grants never satisfy matching resource-qualified requirements. |

Exact-only requirements always reject wildcard-derived matches. Direct exact
grants remain valid. Configured super-scopes remain excluded by default and
become valid only when `allowSuperScopesForExactOnly` is explicitly enabled;
membership in `superScopes` is exact and case-sensitive. Lists are normalized
and deduplicated at startup. The router applies this one matcher to fixed and
rendered operation requirements; resolver guards remain authoritative.

A rendered requirement substitutes each `{argument}` with that argument's
value only when the value is one plain segment: 1–256 ASCII letters, digits,
`-` or `_`. Any other value fails the template, so a separator or wildcard in a
client argument never reaches the matcher. Use a fixed scope or `SubgraphOnly`
authorization for an argument that can carry other characters. Preflight
refuses any client root field the active descriptors do not describe; only
Federation's planner-owned `_entities` and `_service` are outside the
descriptor contract.

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

The default `subscriptions` limits are 1024 process-wide authenticated
connections, 384 process-wide connections waiting for `connection_init`, 128
connection attempts per second with one second of burst capacity, 32
operations per connection, downstream broadcast capacity 32, upstream buffer
capacity 1024, 64 KiB client messages, and a 3000 ms `connection_init`
deadline. Configure the attempt budget with
`maxConnectionAttemptsPerSecond`. Excess attempts return HTTP 429 with
`Retry-After: 1`. All queues are bounded. Delivery is ephemeral and
unreplayed.

Admission uses two separate budgets. An upgrade holds a pending slot
(`maxPendingConnections`) until its `connection_init` bearer validates, the
socket closes, or the init deadline passes. Only a validated connection is
charged against `maxConnections`; if that budget is full at that moment the
socket closes with `1013`. Upgrades are refused with HTTP 503 when either
budget is full. Sockets that never authenticate therefore cannot occupy
authenticated capacity, although many idle upgrades can still delay new
handshakes until the init deadline frees pending slots. The attempt rate and the pending budget
are shared before any credential is known, so apply per-client connection
limits at the proxy or load balancer that sees client addresses.

The three pre-authentication settings work together. Unless
`maxPendingConnections` is set, it defaults to the number of upgrades the
attempt rate admits within one init deadline, rounded up and capped at
`maxConnections`: `min(ceil(maxConnectionAttemptsPerSecond ×
connectionInitTimeoutMs / 1000), maxConnections)`. The defaults give
`min(128 × 3, 1024) = 384`. With that sizing, a stream of upgrades that never
send `connection_init` reaches the attempt rate (HTTP 429) before it can fill
the pending budget. The exception is when `maxConnections` is the smaller
value. Changing the rate or the deadline resizes the default. An explicit
`maxPendingConnections` below that product lets fewer idle upgrades than the
rate admits fill the budget. A shorter deadline frees idle slots sooner.

The administrative metrics report `router_websocket_connections` for
authenticated connections and `router_pending_websocket_connections` for
sockets still waiting for `connection_init`. A socket is counted in exactly one
of them.

The message limit applies to the complete serialized
`graphql-transport-ws` message, including variables and base64 expansion. Bulk
uploads belong on a bounded HTTP endpoint or an application chunk protocol,
not in one WebSocket operation.

## Byte routes

A byte route serves a service's file download through the public listener, for
example an installer or an export too large to send as GraphQL. Byte routes
are off by default and fixed at startup; dynamic registration never adds one.
Each route maps one exact path prefix to one upstream base URL. Programmatic
hosts use `RouterConfig::with_byte_route(ByteRoute::new(prefix, upstream))`.

```json
"byteRoutes": [
  {
    "prefix": "/files",
    "upstream": "http://10.10.0.12:8080/downloads",
    "maxResponseBytes": 134217728
  },
  {
    "prefix": "/shared-files",
    "upstream": "http://10.10.0.12:8080/shared",
    "public": true,
    "allowRange": true
  }
]
```

A request for `/files/reports/q3.csv?v=2` goes to
`http://10.10.0.12:8080/downloads/reports/q3.csv?v=2`. Only the path below the
prefix and the query are copied; the upstream scheme, host, and port come from
configuration alone.

| Field | Default or rule |
| --- | --- |
| `prefix` | Required. An absolute path of non-empty segments using only letters, digits, `-`, `.`, `_` or `~`, with no dot segments and no trailing slash. It must not overlap the GraphQL path, `/health`, `/readiness`, the reserved `/_graphql-orm-router` path, or another byte route. |
| `upstream` | Required `http` or `https` URL with no credentials, query, fragment, dot segments, or empty or percent-encoded path segments. |
| `public` | `false`. When `false`, the route needs a valid bearer and the router must have an authentication provider. |
| `allowRange` | `false`. When `false`, a `Range` request is refused with 416. |
| `allowSetCookie` | `false`. When `false`, upstream `Set-Cookie` headers are dropped. |
| `forwardRequestHeaders` | Empty. Extra request headers to forward. Names containing `_`, and credentials, cookies, range, transport, forwarding, client-address, method-override, and common caller-identity headers are refused. |
| `passResponseHeaders` | Empty. Extra upstream response headers to pass back. Names containing `_`, and `Set-Cookie`, range, transport, redirect, origin-wide, and router-owned headers are refused. |
| `maxResponseBytes` | 67108864 (64 MiB); valid range 1 byte to 1 TiB. |
| `maxConcurrentRequests` | 16; valid range 1–10000. |
| `maxRequestsPerSecond` | 32; valid range 1–10000. The same token bucket as WebSocket attempts, with one second of burst capacity, shared by all clients of the route. Only authenticated requests (or any request on a public route) spend it. |
| `connectTimeoutMs` | 5000; valid through 60000. |
| `idleTimeoutMs` | 30000; valid through 300000. Bounds the wait for response headers after connecting, for each upstream body chunk, and for the client to accept each chunk. |
| `maxDurationMs` | 900000 (15 minutes); at least the idle timeout and at most 24 hours. Bounds the whole request. |

### Matching

A route claims a request whose raw, still percent-encoded path is exactly its
prefix or starts with the prefix followed by `/`. `/files` claims `/files` and
`/files/a.bin`, but not `/files-admin` or `/FILES/a.bin`, which continue to the
GraphQL server and get its 404. Matching is case-sensitive.

Every path a route claims is either relayed or refused with 400
`BYTE_ROUTE_INVALID_PATH`; it never falls through to another handler. A
claimed path is refused when it has an empty segment (`//` or a trailing `/`),
a `.` or `..` segment or any segment made only of dots, a `;` or `\`, a raw
character outside RFC 3986's unreserved characters and `! $ & ' ( ) * + , = :
@`, or an encoded `/`, `\`, `.`, `%`, `;`, or control byte. That refuses
`%2e%2e`, `%2f`, and double-encoding such as `%252e`. A segment that becomes
empty once leading spaces and trailing spaces and dots are removed, such as
`..%20` or `.%20.`, is refused because some servers normalize it to `..`. The percent-decoded
segment must be valid UTF-8, which refuses every overlong encoding (such as
`%C0%AE` or `%E0%80%AE` for `.` and `%E0%80%AF` for `/`), surrogates, and stray
continuation bytes. It must also not contain a character that Unicode
compatibility normalization (NFKC) turns into `.`, `/`, or `\`: the fullwidth
`%EF%BC%8E`, `%EF%BC%8F`, and `%EF%BC%BC`, the small `%EF%B9%92` and
`%EF%B9%A8`, and the leaders `%E2%80%A4`, `%E2%80%A5`, and `%E2%80%A6`. Queries must be
printable ASCII up to 4096 bytes. Other percent-encoded bytes, such as UTF-8
file names, are relayed unchanged.

### Requests

Only `GET` and `HEAD` over HTTP/1.1 or later are accepted. Other methods get
405 with `Allow: GET, HEAD`, and HTTP/1.0 gets 505
`BYTE_ROUTE_HTTP_VERSION_NOT_SUPPORTED`, because HTTP/1.0 marks the end of an
unknown-length body by closing the connection and could not tell an abort from
a complete transfer. A request with `Content-Length` other than 0, or with any
`Transfer-Encoding`, gets 400 `BYTE_ROUTE_REQUEST_BODY_NOT_ALLOWED`. `HEAD`
is forwarded as `HEAD`.

Authentication follows the GraphQL endpoint. A route that is not public needs
`Authorization: Bearer <token>`; a missing bearer gets 401, and an invalid,
non-bearer, or repeated `Authorization` gets 401 `UNAUTHENTICATED` with the
GraphQL error shape. An unavailable provider gets 503. A verified bearer is
forwarded upstream unchanged, as GraphQL forwards it to subgraphs, so the
service still makes its own authorization decision. A public route needs no
bearer and forwards none. If a client sends one anyway, it is verified and
forwarded, and an invalid one gets 401 as on GraphQL. A router without an
authentication provider (anonymous development mode) can serve only public
routes, and there any `Authorization` header gets 401 because the router
cannot verify it; the GraphQL endpoint of such a router ignores the header
instead. At startup,
the router logs one warning naming every public route.

Upstream requests carry only these client headers: `Accept`,
`Accept-Encoding`, `Accept-Language`, `If-Match`, `If-None-Match`,
`If-Modified-Since`, and `If-Unmodified-Since`. They also carry `Range` and
`If-Range` when `allowRange` is set, the verified `Authorization`, and the
route's `forwardRequestHeaders`. Cookies, `Forwarded`, `X-Forwarded-*`,
client-supplied identity and client-address headers, method-override and
URL-rewrite headers, proxy-asserted identity headers (such as
`X-Amzn-Oidc-*`, `X-Ms-Client-Principal*`, `X-Goog-Iap-Jwt-Assertion`,
`Cf-Access-Jwt-Assertion`, `X-Jwt-Assertion`, `X-Consumer-*`,
`X-Ssl-Client-*`, `X-Client-Cert`, and `X-Forwarded-Client-Cert`), router
internal headers, and hop-by-hop headers are never forwarded. Neither is any
header the client names in its `Connection` header (RFC 9110 section 7.6.1).
Configured names must not contain `_`, because CGI-style servers read
`X_User_Id` and `X-User-Id` as the same variable. Do not list an identity header in
`forwardRequestHeaders`: a client can set any header it sends. The upstream
client ignores ambient proxy settings and does not follow redirects.

### Responses

The upstream status and body pass through, including errors. The relayed
`Content-Length` is the length the router's upstream client decodes, never the
raw header, and a response without one is relayed chunked. Only these
response headers pass: `Content-Type`, `Content-Disposition`,
`Content-Encoding`, `Content-Language`, `Cache-Control`, `Expires`, `ETag`,
`Last-Modified`, `Vary`, `Age`, and `Retry-After`. With `allowRange`,
`Content-Range` and `Accept-Ranges` also pass, and with `allowSetCookie`,
`Set-Cookie` does. The route's `passResponseHeaders` are added to the list. Headers that would
apply to the router's whole origin cannot be added: `Strict-Transport-Security`,
`Alt-Svc`, `Clear-Site-Data`, `Public-Key-Pins`, `Access-Control-*`, `NEL`,
`Report-To`, `Reporting-Endpoints`, `Accept-CH`, `Critical-CH`,
`Origin-Agent-Cluster`, and `Service-Worker-Allowed`. Byte routes never relay
redirects or linked resources, so `Location`, `Refresh`, and `Link` cannot be
added either; an upstream 3xx passes through without its `Location`.
Every relayed response also gets `X-Content-Type-Options: nosniff`, and, unless
an allowed upstream header supplies one,
`Content-Security-Policy: default-src 'none'; sandbox`. A route without
`allowRange` sends `Accept-Ranges: none`. These stop a relayed file from
running as active content on the router's origin.

The body is streamed with at most one chunk queued between the upstream and the
client. An HTTP/1 upstream response gets 502 `BYTE_ROUTE_UPSTREAM_UNAVAILABLE`
when its framing cannot be relayed exactly:

- a `Transfer-Encoding` other than exactly `chunked`, such as `gzip`, whose
  coded bytes would otherwise pass as plain content;
- both `Transfer-Encoding` and `Content-Length`, where the length is ignored
  when decoding and relaying it would truncate the body;
- an unparseable `Content-Length`;
- a body with neither `Content-Length` nor chunked framing, because a body
  ended only by the upstream closing its connection cannot show truncation. A declared `Content-Length` above `maxResponseBytes` is refused before
any body is sent. A body without a declared length that grows past the limit,
an idle timeout, the request deadline, an upstream read error, and a client
that stops reading all abort the transfer. On HTTP/1 the router closes the
client connection, including a keep-alive one, so a client never sees a
truncated body as complete and never reuses that connection. On
HTTP/2 the router ends the stream with an error when the server next polls the
body; a stalled HTTP/2 stream keeps its connection until the client or a
fronting proxy closes it. An abort also releases the upstream connection and
the route's concurrency slot.

There is no minimum transfer rate. A client that accepts one chunk just inside
every idle timeout keeps a concurrency slot until `maxDurationMs`, and a client
that never reads a small response keeps its socket after the router has
finished with it. Set `maxConcurrentRequests` and `maxDurationMs` from real
transfer times, and use a fronting proxy's per-client connection and send
timeouts for public routes.

Router-generated errors use the GraphQL error JSON shape and carry a
`graphql-orm-router-error` header with the code. That header is never copied
from an upstream, so it tells a router refusal apart from a relayed upstream
error:

| Status | Code | Cause |
| --- | --- | --- |
| 400 | `BYTE_ROUTE_INVALID_PATH` | Unsafe path or query under a prefix. |
| 400 | `BYTE_ROUTE_REQUEST_BODY_NOT_ALLOWED` | The request declared a body. |
| 401 / 503 | `UNAUTHENTICATED` | Missing or invalid bearer, or unavailable provider. |
| 405 | `BYTE_ROUTE_METHOD_NOT_ALLOWED` | Not `GET` or `HEAD`. |
| 416 | `BYTE_ROUTE_RANGE_NOT_ALLOWED` | `Range` on a route without `allowRange`. |
| 429 | `BYTE_ROUTE_RATE_LIMITED` | Route admission rate exceeded; `Retry-After: 1`. |
| 502 | `BYTE_ROUTE_UPSTREAM_UNAVAILABLE` | Connection refused, failed, or timed out; an informational response; HTTP/1 framing that cannot be relayed exactly; or a protocol error. |
| 502 | `BYTE_ROUTE_RESPONSE_TOO_LARGE` | Declared length over `maxResponseBytes`. |
| 503 | `BYTE_ROUTE_BUSY` | All `maxConcurrentRequests` slots in use. |
| 504 | `BYTE_ROUTE_UPSTREAM_TIMEOUT` | Connected, but no response headers within the idle timeout. |
| 505 | `BYTE_ROUTE_HTTP_VERSION_NOT_SUPPORTED` | HTTP/1.0 request. |

Per-route counters appear in `GET /_router/metrics` as
`router_byte_route_requests_total`, `router_byte_route_rejected_total`,
`router_byte_route_upstream_errors_total`, `router_byte_route_aborted_total`,
`router_byte_route_bytes_total`, and `router_byte_route_active_requests`, each
an object keyed by prefix. The keys are absent when no byte route is
configured; the [operations runbook](operations.md#resource-budgets) defines
each counter. Embedding hosts read the same values from
`RouterHandle::byte_route_metrics`.

## Administration and dynamic destinations

`admin.listener` must use a port distinct from the public listener's port, on
any address. Its exact default
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
