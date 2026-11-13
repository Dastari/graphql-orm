---
title: graphql-orm-router migration guide
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-11-12
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router migration guide

## 0.13.0 to 0.14.0: persisted documents

Persisted documents remain disabled by default. Enable `persistedDocuments` or
`RouterConfig::with_persisted_allow_list(path)` (strict) or
`RouterConfig::with_persisted_ids_or_text(path)` (ordinary text also allowed) with a local manifest.
The file configuration requires an explicit `strict` boolean. Registered
documents are capped at 256 KiB; WebSocket forwarding separately checks the actual
serialized operation against its 64 KiB internal budget. Snapshot integrity is
checked on every candidate generation. Strict deployments require clients to send an id for every operation. See the
[configuration reference](docs/configuration.md#persisted-documents) for exact
hashing, limits, wire forms, and authorization boundaries.

## 0.12.0 to 0.13.0: outbound transport and admin authentication

### Plaintext outbound URLs

Outbound `http` URLs now need a `localhost` or loopback IP-literal host. This
covers static subgraph `graphqlUrl`, `sdlUrl` and `protocolUrl`, byte route
`upstream`, and trusted subgraph `metadataUrl`, `graphqlOrigin` and
`schemaOrigin`. Startup fails with `InvalidConfiguration` naming the field.
Move those endpoints to `https`. Where the network path is protected another
way, such as inside a service mesh, keep plaintext with:

```json
"allowPlaintextHttp": true
```

or `RouterConfig::allow_plaintext_http(true)`.

### Proxy environment variables

The schema and JWKS clients now ignore `HTTP_PROXY`, `HTTPS_PROXY`,
`ALL_PROXY` and `NO_PROXY`. A deployment that reaches subgraph schemas or the
issuer only through such a proxy sets top-level `"useEnvironmentProxy": true`
for schema fetches and `"useEnvironmentProxy": true` inside `authentication`
(and inside `admin.authentication`) for key fetches. Programmatic hosts call
`RouterConfig::use_environment_proxy(true)` and
`JwksAuthenticationConfig::use_environment_proxy(true)`. Deployments without
proxy variables need no change.

### Administrative authentication

A router with `admin` fails validation until the admin listener has its own
provider. Issue administrative tokens for a dedicated audience, then add:

```json
"admin": {
  "listener": "0.0.0.0:4001",
  "authentication": {
    "jwksUrl": "https://identity.example/.well-known/jwks.json",
    "issuer": "https://identity.example",
    "audiences": ["graphql-router-admin"]
  },
  "network": {}
}
```

Programmatic hosts pass a second provider to
`AdminConfig::with_authentication_provider`. Its audiences must not overlap the
public provider's. Operators and dynamic services that registered with public
tokens must request tokens for the admin audience. A custom
`AuthenticationProvider` may implement the new `accepted_audiences` method so
validation can compare audiences; the default `None` leaves that to the
operator. The router does not check `AgqlAuthenticationProvider` audiences,
because the pinned `agql-auth` validator does not expose them: build the admin
validator with an audience the public validator does not accept. Never issue a
token whose `aud` lists both a public and an administrative audience; both
providers accept it.

### Static subgraph removal

`DELETE /_router/subgraphs/{name}` now answers 409 `REMOVAL_REJECTED` for a
static subgraph. To retire one, remove it from configuration and restart, or
call `RouterHandle::remove_subgraph` from the embedding host.

### JWKS key handling

No change is needed. JWKS documents that mix RS256 signing keys with other
keys now load instead of failing, and an unknown key ID requests one
background refresh per 30-second cooldown, at most one day. Tune it with
`unknownKeyRefreshCooldownSeconds` or
`JwksAuthenticationConfig::with_unknown_key_refresh_cooldown`.

## 0.11.0 to 0.12.0: opt-in byte routes

No change is needed. Byte routes are off until `byteRoutes` or
`RouterConfig::with_byte_route` lists one, and a configuration without them
serves exactly the same paths as before. Validation rejects a byte route that
overlaps the GraphQL path, `/health`, `/readiness`, or another byte route, and
an authenticated byte route on a router without an authentication provider.
When byte routes are configured, the admin `GET /_router/metrics` response gains
six `router_byte_route_*` keys, each an object keyed by route prefix. They are
absent when no byte route is configured, so that response is unchanged for
existing deployments. Metrics consumers that reject unknown keys must accept
them before enabling a byte route.

To move a download that currently bypasses the router, list its path prefix
and upstream base URL, keep the route authenticated unless the link is meant to
work without sign-in, and set `maxResponseBytes` above the largest expected
file. The upstream still receives the verified `Authorization` header and must
make its own authorization decision. See the
[byte route reference](docs/configuration.md#byte-routes).

## 0.10.0 to 0.11.0

### agql-auth 0.26.0

The `auth-agql` public constructors take types from published Auth 0.26.0.
Replace a host's `v0.24.0` selector with:

```toml
agql-auth = { git = "https://github.com/Dastari/agql-auth.git", tag = "v0.26.0", version = "0.26.0" }
```

Use exactly that source for the host, ORM, AI and router, refresh locks, and
rebuild hosts. The tag peels to `7d016f6a6bc3e697382e2e05865f596862189240`.
Hosts built without `auth-agql` are unaffected. The Auth update itself changes
no router behavior, and no data migration is required.

### Introspection, catalogue and admission settings

**Introspection is off by default.** To keep it, opt in with one line:

```json
"introspection": true
```

or `RouterConfig::with_introspection(true)`. On an authenticated router,
enabled introspection needs a valid bearer: tools that introspect anonymously
must send one, or they receive HTTP 401 `UNAUTHENTICATED`. With introspection
off, `__schema` and `__type` return HTTP 403 `INTROSPECTION_DISABLED`. Root
`__typename` keeps working either way.

**Every client root field needs authorization metadata.** Descriptors must now
describe underscore-prefixed root fields as well. Only Federation's
`_entities` and `_service` are left out. Preflight refuses an undescribed root
field with `FORBIDDEN` instead of skipping it.

**Scope-template values are one plain segment.** A value substituted into a
scope template must be 1–256 ASCII letters, digits, `-` or `_`. If an argument
used in a template can carry other characters, such as `.`, `:`, `/`, `=`,
`+` or `*`, change the template to a fixed scope, or use `SubgraphOnly`
authorization for that field. With the `auth-agql` file matcher, a `separator`
made of a letter, a digit, `-` or `_` is refused, and so is a `wildcard` made
only of those characters.

**WebSocket budgets are split.** `maxConnections` now counts authenticated
connections only. The new `subscriptions.maxPendingConnections` limits sockets
waiting for `connection_init`. By default it is
`min(ceil(maxConnectionAttemptsPerSecond × connectionInitTimeoutMs / 1000),
maxConnections)`, which is 384 with the defaults. Most deployments need not set
it. A client that authenticates while `maxConnections` is full now closes with
`1013`. Before, its upgrade was refused with 503. Reconnect with backoff as for
503.

**Init timeout.** The default `connectionInitTimeoutMs` is now 3000 (was 5000).
Clients must send `connection_init` within 3 seconds, or set the old value
explicitly.

**WebSocket metrics.** `router_websocket_connections` now counts authenticated
connections only. Add `router_pending_websocket_connections` to dashboards that
tracked every open socket.

**Programmatic hierarchical matchers.** Prefer
`AgqlScopeMatcher::hierarchical(matcher)?` to
`AgqlScopeMatcher::new(Arc::new(matcher))`. It refuses a separator or wildcard
that a scope-template value could contain.

**Administrative listener port.** `admin.listener` must use a different port
from the public listener, even on a different address.

No data migration is needed.

## 0.9.1 to 0.10.0: subscriptions are WebSocket-only

Public HTTP subscription requests are refused. This covers SSE
(`text/event-stream`), incremental-delivery and Apollo multipart
(`multipart/mixed`), and GET. The response is HTTP 400 with a
`SUBSCRIPTIONS_NOT_SUPPORTED` GraphQL error, sent as plain JSON before any
stream opens. Move subscription clients to `graphql-transport-ws` on the public
GraphQL path, with the bearer in `connection_init`. HTTP gave no mandatory
bearer, expiry teardown, attempt limit or continuity contract, so it has no
supported equivalent. Send one operation per document, or name a query or
mutation with `operationName`: a document is refused when any operation it
could select is a subscription. Queries and mutations over HTTP are unchanged,
including with a streaming-capable `Accept` header. No data migration or public
Rust API change is needed.

Routers without `subscriptions` configured have no WebSocket gateway. They also
answer every HTTP subscription with HTTP 400 and `SUBSCRIPTIONS_NOT_SUPPORTED`,
but the message is `subscriptions are not enabled on this router` and names no
endpoint. Before 0.10.0, Hive answered these requests with 415 or 406, or with a
one-event 200 stream when only a streaming `Accept` was offered. Clients that
branch on those statuses should match the 400 status and the error code instead.

## 0.9.1: introspection declaration order

Introspection now preserves source order for input fields, object/interface
fields, enum values, and field/directive arguments. Composite-key clients can
read the declared key input order directly. For declarations shared across
subgraphs, the first occurrence in lexicographic subgraph-name order determines
member order; later subgraphs append unseen members. Composition still decides
which members survive. No data migration is needed.

## 0.8.3 to 0.9.0: subscription continuity

Handle terminal `SUBSCRIPTION_CONTINUITY_LOST` by backing off with jitter,
resubscribing and refetching authoritative state. Do not wait for `complete`.
Router-owned overflow affects only the identified subscription; socket pressure
affects that connection's subscriptions. Hive 0.2.19 exposes no operation identity
in its loss counters, so its fallback may terminate unaffected subscriptions on
the runtime. It never terminates queries or mutations. New subscriptions are
rejected until one second of quiet after the last Hive loss to coalesce sustained
bursts safely. The public connection stays usable. See the operations runbook for
the residual cross-subscriber DoS risk and mitigations. No protocol change is
required.

## 0.8.2 to 0.8.3: terminal WebSocket errors

Treat an operation `error` as terminal immediately; do not wait for `complete`.
For `SUBSCRIPTION_SCHEMA_RELOAD`, open a fresh connection and resubscribe.
A client ID may be reused after `error`, although retired connections still
reject new operations with `SERVICE_UNAVAILABLE`. GraphQL errors inside `next`
remain execution results and can be followed by `complete`. No data migration
or public Rust API change is needed.

## 0.8.1 to 0.8.2: owned wire-test listener addresses

No caller changes or data migration are needed. Wire-test children bind port zero
and report addresses after owning their listeners. The additive `run_with_listener`
API and Unix `--listen-stdin` command option transfer an already-bound public
listener without rebinding. Its actual address replaces the configured public
address before preparation; default startup and administration are unchanged.

## 0.8.0 to 0.8.1: subscription error frame test

No change is required. The release adds a test only.

## 0.7.0 to 0.8.0: opt-in global node lookup

No change is required. To serve `node(NodeId)` and `nodes(NodeIds)`, opt subgraph entities in with
`#[graphql_entity(node_id)]` (graphql-orm 0.86.0) and enable the router lookup with
`RouterConfig::with_node_lookup(NodeLookupConfig::new())` or `"nodeLookup": {}` in the file
configuration. Rename any configured subgraph called `graphql-orm-node`, which is now reserved.
Do not hand-write `interface Node`, `implements Node` or `node` resolvers in subgraphs.

## 0.6.7 to 0.7.0: canonical Auth 0.24 types

The `auth-agql` public constructors take types from published Auth
0.24.0. Replace direct `v0.23.0`/other Auth selectors with:

```toml
agql-auth = { git = "https://github.com/Dastari/agql-auth.git", tag = "v0.24.0", version = "0.24.0" }
```

Use exactly that source for the host, ORM, AI and router. Old-source
`Arc<AccessTokenValidator>` and `Arc<dyn ScopeMatch>` values cannot cross this
public type boundary. Refresh locks and rebuild hosts; the independent
[consumer fixture](../graphql-orm/tests/fixtures/graphql-features/Cargo.toml)
compiles both constructors using the host's canonical Auth types. This is an
intentional pre-1.0 dependency/type-universe break from Auth 0.23. The published
tag peels to `1f4b4bfb6d052d04e6c80b80cb7aa3d0c2ebaacd`, unchanged from the
reviewed revision selector. Router logic, TLS/provider behavior and the Datadog constraint are
unchanged. No data migration is required.

## 0.6.6 to 0.6.7: preparation ownership and fresh consumer dependencies

No breaking changes or application, protocol, configuration or data migration
are required. The patch records a private wire-test ownership correction and a
valid-candidate control; production preparation and listener binding are unchanged.
Fresh consumers now resolve Datadog OpenTelemetry `=0.5.2` with default features
disabled, matching pinned Hive `0.2.19` and its OpenTelemetry SDK `0.32` types.
Consumers that explicitly selected Datadog `0.5.3` must align that dependency;
its SDK `0.33` types cannot be exchanged with this Hive version. No public router
feature or behavior changes. Semver baseline generation applies this same narrow
constraint to historical Hive `=0.2.19` manifests, preserving historical source,
version and feature definitions; fresh consumer validation uses no input lockfile.
No executable is distributed or activated by this source change.

## 0.6.5 to 0.6.6: documentation package identity

No API, configuration, SDL or data migration is required. This package bump
records the README Auth `v0.23.0` selection note added after the immutable
0.6.5 tag. Direct Auth consumers still select the same published tag as the
workspace; see the [workspace migration](../../MIGRATION.md#agql-auth-0230-selection).
No router executable is distributed or activated by this source change.

## 0.6.4 to 0.6.5: test startup diagnostics

No application, protocol or data migration is required. The package identity
records a test-only diagnostic correction. No executable is distributed by this
source change; binary publication retains its separate approval boundary.

## 0.6.3 to 0.6.4: TLS certificate distrust patches

Refresh the consuming lockfile to `rustls-webpki` 0.103.16 or later in its
0.103 series and `webpki-roots` / `webpki-root-certs` 1.0.10 or later.
Rebuild consuming binaries with the reviewed lock. The 0.26.11 compatibility
root packages re-export their resolved 1.x parent; retaining an older parent
retains the affected trust anchors. Certificate paths through the distrusted
emSign intermediates now fail verification. Do not restore those paths through
certificate-verification bypasses or by pinning an older root bundle.

No Rust API, SDL, configuration format, database or stored-data migration is
added. Source release does not authorize a router binary distribution: renew
its lock-bound notice and distribution evidence before attaching an artifact.

## 0.6.2 to 0.6.3: signed catalogue transport bound

The default role-catalogue response limit is now 2 MiB. Explicit `maxBodyBytes`
values continue to apply; review any previous 1 MiB override before accepting
larger complete signed envelopes. This HTTP limit is separate from Auth
structural JWS limits, JWKS limits and GraphQL request limits. Use the coordinated
Auth signature-envelope correction; increasing a transport limit cannot repair
an older verifier. No SDL, database or stored-data migration is added.


## 0.6.1 to 0.6.2: maintained workspace package identity

Select the reviewed coordinated workspace revision for all companion packages,
regenerate the consuming lockfile and align direct Auth dependencies with
published `v0.22.0`. This patch gives maintained dependency guidance a new
immutable package identity; the Rust implementation and feature selection are
unchanged. Native compatibility checks remain required for the complete
consumer graph. No stored-data or schema migration is added.

## 0.6.0 to 0.6.1: published Auth selector

When enabling `auth-agql`, use `tag = "v0.21.3"` for every direct
`agql-auth` dependency from `https://github.com/Dastari/agql-auth.git`.
The shared dependency retains the same reviewed commit
`59c0a38f82e8c75459474238799a81f29d4ffa58`. Refresh the lockfile and check that
the selected graph contains one Auth source. A revision and a tag are distinct
Cargo sources even at the same commit, which can split validator types.
No Rust API, clock, token, GraphQL, descriptor or stored-data migration is added.
The fallible-clock contract and host migrations from 0.6.0 remain required.

## 0.5.4 to 0.6.0: coherent fallible authentication clocks

The agql adapter now observes the same injected clock as its access-token
validator. `AgqlAuthenticationProvider::new(validator)` is unchanged. Adopt the
reviewed auth revision exposing `AccessTokenValidator::current_time`; retain its
full locked commit in the release manifest. No token, GraphQL, descriptor or
stored-data migration is required. Standard system-clock configurations retain
their behavior; injected clocks no longer silently use a different wall clock.

Custom implementations overriding `AuthenticationProvider::current_time` must
change their return type from `SystemTime` to
`Result<SystemTime, AuthenticationError>`. For an infallible host clock, wrap the
same clock observation in `Ok`:

```rust,ignore
fn current_time(&self) -> Result<SystemTime, AuthenticationError> {
    Ok(self.clock.now())
}
```

For a fallible host clock, preserve the error and use a safe unavailable category:

```rust,ignore
fn current_time(&self) -> Result<SystemTime, AuthenticationError> {
    self.clock.now().map_err(|_| {
        AuthenticationError::unavailable("authentication clock unavailable")
    })
}
```

Providers using the default system clock need no override. Observe the same clock
used for credential validation exactly once per decision; do not return an
unrelated fallback clock or panic on conversion. The agql adapter preserves signed
nanoseconds using checked `SystemTime` arithmetic. A platform that cannot represent
a supplied clock timestamp returns unavailable. WebSocket initialization closes
with code 1011 for a clock failure, rather than labeling a server fault as invalid
client credentials; no upstream subscription is opened.

The gateway samples time at admission and schedules the remaining lifetime.
It does not poll token status, track later arbitrary clock jumps or promise
immediate revocation. Authoritative subgraphs must continue to validate the
forwarded credential and enforce current authority/bindings/row policy, including
per-event subscription checks when required.

## 0.5.3 to 0.5.4

No code, configuration or stored-data migration is needed. Rebuild to retain
warnings-denied compilation on current Rust without raising the Rust 1.90 MSRV.
Metrics preserve their atomic saturating decrement and relaxed ordering.

## Workspace release dependency alignment

For workspace releases selecting `agql-auth` v0.19.1, direct consumers must use
`tag = "v0.19.1"`, locked to `96bafbf21adbc7ad963729e1e981feaba5debe90`.
A tag and a revision selector produce different Cargo sources even at the same
commit. The MIT-licensed library source is unchanged from the historical revision below;
no configuration or stored-data migration is needed. The workspace lockfile
also updates h2 and rustls to their compatible security fixes; rebuild router
binaries from the reviewed lockfile.

## 0.5.2 to 0.5.3

Rebuild the router from the reviewed lockfile to adopt Hive 0.2.19 and the
`domain` 0.12.3 security fixes for RUSTSEC-2026-0310. No stored-data migration is
required. Public
configuration, authentication, descriptor and schema contracts remain unchanged.
The private adapter now attaches execution settings to each immutable graph,
so configured forwarded headers, timeouts and subscription transports survive
reloads together with the selected schema and authorization snapshot.

Cancelling a quiet subscription with GraphQL Complete or closing its public
WebSocket now releases the upstream subscription before another event arrives.
Clients should still authenticate each new connection and retain jittered
reconnect backoff; cancellation never permits replay of an uncertain mutation.
Run authenticated quiet-cancellation, schema-retirement and reconnect acceptance
checks before activation. A source dependency update does not replace an
independently pinned router executable: publish and adopt a reviewed binary
artifact through the canonical release process.

## 0.5.1 to 0.5.2

No configuration, schema, token, descriptor, or stored-data migration is
required. The release adds test coverage only. Subgraphs that newly advertise a
resolvable Federation `@key` continue to need no descriptor entry for
`_entities` or `_service`; those fields remain outside the router's root-field
authorization contract, so a field reached by following a key is authorized
solely by the subgraph that owns the entity.

## 0.5.0 to 0.5.1

No configuration, schema, token, descriptor, or stored-data migration is
required. Rebuild consumers at the reviewed 0.5.1 monorepo revision and run
their authenticated WebSocket acceptance tests. The router now sends only a
bounded scalar authorization projection beside the ordinary variables object;
large data variables are no longer duplicated on the private subscription
transport. Variable-dependent authorization still runs before subgraph work
and fails closed when a required value cannot fit that projection.

## 0.4.0 to 0.5.0

Align direct `agql-auth` consumers to 0.19.0 at reviewed merged revision
`1d2e9fe2e1576105212a7b340a11abf8cad0382d` so the process resolves one matcher
type universe.

Existing router configuration needs no change. Omitted `scopeMatcher`,
`kind: "exact"`, and hierarchical matchers without the new field preserve their
previous decisions. To let caller-supplied super-scopes satisfy exact-only
requirements, build with `auth-agql` and explicitly set:

```json
{
  "scopeMatcher": {
    "kind": "hierarchical",
    "superScopes": ["root.admin", "operations.breakglass"],
    "allowSuperScopesForExactOnly": true,
    "exactOnlyScopes": ["payments.credentials.release"]
  }
}
```

The option recognizes only exact membership in `superScopes`. Direct exact
grants continue to satisfy exact-only requirements, while universal, trailing,
and segment wildcard matches remain denied. Disable or omit the option to
restore the previous behavior. No descriptor, GraphQL schema, token wire,
role, or stored-data migration is implied.

## 0.3.1 to 0.4.0

Align direct `agql-auth` consumers to 0.18.0 at merged revision
`527d15d28e3c295a6f6b5e6d74559a7aecdc1322`. Issuers place catalogue-backed
grants in the distinct `authorization_roles` claim; ordinary application
`roles` are no longer an expansion input.

Deploy resource-server support before minting that claim. Configure a literal
`url` or `urlFromEnv`, and load any request credentials with
`requestHeadersFromEnv`. `maximumSignedLifetimeSeconds` and
`clockSkewLeewaySeconds` validate newly fetched signatures independently of
the local `cacheTtlSeconds` refresh age. Optional `retryBackoffSeconds` and
`maximumRetryBackoffSeconds` bound lazy-fetch retries. The role catalogue is no longer a
startup readiness gate. Until the lazy fetch succeeds, role-bearing requests
fail closed while direct-scope-only requests continue. After one successful
verification, refresh failures preserve the last-good snapshot and surface
warning/metric signals.

No schema or stored-data migration is implied. Roll back only after restoring
expanded direct scopes and allowing typed compact tokens to expire.

## 0.3.0 to 0.3.1

Align direct `agql-auth` consumers to 0.17.1 at reviewed full revision
`b792241b7d9bb46bad81dde4534ae5b39857f614` and rebuild the router from the
same reviewed monorepo revision. Existing configuration, catalogue expansion,
scope matching, and accepted token semantics are unchanged. No descriptor,
GraphQL schema, stored-data, or router configuration migration is required.

## 0.2.0 to 0.3.0

Align direct `agql-auth` consumers to 0.17.0 at the reviewed full revision.
Existing configurations remain unchanged when `roleScopeCatalogue` is omitted.

To adopt compact role grants, first publish a signed
`agql_auth::SignedRoleScopeCatalogue`, then build the router with `auth-agql`
and configure `authentication.roleScopeCatalogue` with its HTTPS URL, exact
signature audience, cache TTL, and body limit. The authentication refresh
interval must be shorter than the catalogue TTL. Run `--check` while the
catalogue and JWKS endpoints are available; initial verification is a readiness
gate.

Only after every resource server expands the same verified role IDs should an
issuer remove expanded scopes from access tokens. A role-bearing token is
denied when the catalogue is absent, forged, expired, stale, or invalid. A
failed refresh preserves a still-current snapshot. Roll back by restoring
expanded scope issuance and removing `roleScopeCatalogue` after all compact
tokens expire.

## 0.1.4 to 0.2.0

Replace the reviewed full-revision pin and rebuild the router. Hosts that also
depend directly on `agql-auth` must align to version 0.16.0 at the exact
reviewed revision so Cargo resolves one auth type universe.

Omitted `scopeMatcher` configuration remains exact and needs no migration.
To opt into the generic hierarchy, build with `auth-agql` and add
`{"scopeMatcher":{"kind":"hierarchical"}}`; then explicitly review
separator, wildcard, universal-wildcard, `superScopes`, `exactOnlyScopes`, and
`exactOnlyScopePatterns` policy. Exact-only requirements are checked before a
super scope or wildcard, including rendered resource-qualified requirements.

No descriptor, GraphQL schema, token wire, or stored-data migration is
required. Run `--check` against the complete graph and exercise both ordinary
and exact-only authorization cases before deployment.

## 0.1.3 to 0.1.4

Replace the reviewed full-revision pin and rebuild the router. When a host
also depends directly on `agql-auth`, align it to version 0.15.0 at exact
revision `e841ffd382082ad7419be259fe957f949b956ff7` so Cargo resolves one auth
type universe.

The `auth-agql` feature remains a validator and scope-matcher adapter only. It
does not construct an issuer, session resolver, or delegated token. No router
configuration, descriptor, GraphQL schema, token wire, or stored-data
migration is required.

## 0.1.2 to 0.1.3

Replace the reviewed full-revision pin and rebuild the router. Protocol crate
0.2.0 adds optional fingerprinted descriptor extensions while keeping
protocol wire major 1. Existing subgraphs may continue omitting `extensions`.
No configuration, GraphQL schema, token, or stored-data migration is required.

If a subgraph begins advertising an extension, its payload participates in
canonical candidate identity and any change follows the ordinary complete
composition/last-known-good path. The router deliberately does not interpret
extension payloads or derive authorization from them.

## 0.1.1 to 0.1.2

Replace the exact full-revision pin and rebuild the router. No descriptor,
schema, token, or stored-data migration is required. Existing configurations
gain a default limit of 128 public WebSocket upgrade attempts per second with
one second of burst capacity. Set
`subscriptions.maxConnectionAttemptsPerSecond` explicitly when measured
startup bursts require another positive budget. Limited attempts return HTTP
429 with `Retry-After: 1`; active-connection saturation continues to return
HTTP 503.

Clients must use jittered backoff for either response and must not blindly
replay a mutation when the socket was lost after `subscribe` but before its
terminal `complete`: the write may already have committed. Query authoritative
state or use an application idempotency contract. A complete one-shot
operation can safely retire its ID while long-lived sibling subscriptions stay
active.

The public transport still has a 64 KiB serialized-message ceiling. Move bulk
payloads to bounded HTTP operations or an application-level chunk protocol;
raising retry frequency cannot make an oversized WebSocket operation valid.

## 0.1.0 to 0.1.1

Replace the exact full-revision pin and rebuild the router. No configuration,
descriptor, schema, token, client-protocol, or stored-data migration is
required. Variable-backed scope templates on HTTP and
`graphql-transport-ws` operations now use the current operation's values and
deny before downstream execution when the rendered scope is absent.

This patch does not weaken the subgraph boundary: resolver authorization and
database policy remain authoritative. Existing clients continue sending
standard GraphQL variables and must not depend on router-private GraphQL
extension names.

## Initial adoption

1. Expose Federation-compatible SDL and a protocol v1 descriptor from every
   subgraph. Keep direct subgraph guards authoritative.
2. Configure static sources and run the executable with `--check` until full
   composition, authorization binding, and runtime construction pass.
3. Shadow representative HTTP and WebSocket operations and compare data,
   GraphQL errors, scope decisions, and reconnect behavior.
4. Route clients behind a reversible deployment switch. Retain the previous
   entry point through the agreed observation window.
5. Remove superseded federation or event infrastructure only after ownership
   checks and an exercised rollback.

Fixed generated policies emit standard Federation `@authenticated` and
`@requiresScopes` metadata where representable. Argument-templated and custom
policies remain in protocol metadata or subgraph-only policy; no SDL string
rewriting is required. The router preflight never replaces the resolver guard.

New access tokens should use the OAuth space-delimited `scope` claim. The
legacy `scopes` array is accepted only with `acceptLegacyScopes: true`; a token
containing conflicting forms is rejected. The optional `auth-agql` feature is
a validation/matching adapter only and introduces no issuer responsibilities.
It resolves `agql-auth` 0.16.0 at exact revision
`3bc38cd94794f1e868a9cc3a5551047b95a32105`; hosts with a direct dependency
must use the same source and revision. Configure legacy acceptance directly
with `agql_auth::AccessTokenValidatorBuilder::legacy_scope_claims` before
wrapping the validator in `AgqlAuthenticationProvider::new`.

Subscriptions move to `graphql-transport-ws` with a bearer in
`connection_init`. Tokens are not refreshed in place. Clients must reconnect
at expiry and after `SUBSCRIPTION_SCHEMA_RELOAD`, and must query authoritative
state after a delivery gap.

See the [operations runbook](docs/operations.md) for rollout budgets and the
[schema evolution guide](docs/schema-evolution.md) for expand/contract rules.
