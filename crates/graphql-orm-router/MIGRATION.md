---
title: graphql-orm-router migration guide
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-10-31
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router migration guide

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
