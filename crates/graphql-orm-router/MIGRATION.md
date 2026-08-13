---
title: graphql-orm-router migration guide
kind: reference
status: active
owner: graphql-orm-router-maintainers
last_reviewed: 2026-08-13
review_by: 2027-02-07
supersedes: []
---

# graphql-orm-router migration guide

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
It resolves `agql-auth` 0.15.0 at exact revision
`e841ffd382082ad7419be259fe957f949b956ff7`; hosts with a direct dependency
must use the same source and revision. Configure legacy acceptance directly
with `agql_auth::AccessTokenValidatorBuilder::legacy_scope_claims` before
wrapping the validator in `AgqlAuthenticationProvider::new`.

Subscriptions move to `graphql-transport-ws` with a bearer in
`connection_init`. Tokens are not refreshed in place. Clients must reconnect
at expiry and after `SUBSCRIPTION_SCHEMA_RELOAD`, and must query authoritative
state after a delivery gap.

See the [operations runbook](docs/operations.md) for rollout budgets and the
[schema evolution guide](docs/schema-evolution.md) for expand/contract rules.
