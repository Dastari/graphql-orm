---
title: "agql-auth Bridge Guide"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-16
review_by: 2027-02-01
supersedes: []
---

# `agql-auth` Bridge Guide

`graphql-orm` never depends on `agql-auth` by default. Enable the optional
`auth-agql` feature for a one-way adapter.

## Accepted request and current authority

Enable `auth-agql` and use the same `agql-auth` selector as the workspace:
`git = "https://github.com/Dastari/agql-auth.git", tag = "v0.21.3", version = "0.21.3"`.
The published tag is locked to reviewed commit
`59c0a38f82e8c75459474238799a81f29d4ffa58`. Matching only the commit is not
enough: Cargo treats a revision and a tag as different sources, which can split
principal types. Refresh the host lockfile and verify one Auth source across
the selected graph. Both projects are Git-distributed; the release manifest
retains the full locked revision and Auth tag.

Authenticate the credential before constructing a bundle. Signature/secret,
issuer, audience and transport proof validation belong to the trusted host
transport. Construction and conversion alone do not authenticate anything.

```rust
use graphql_orm::prelude::*;
use agql_auth::AuthPrincipal;

fn inject(request: async_graphql::Request, authenticated: AuthPrincipal)
    -> async_graphql::Result<async_graphql::Request>
{
    AgqlAcceptedRequest::new(authenticated).inject(request)
}
```

The immutable bundle retains the complete accepted principal, alongside coherent
`AuthPrincipal`, matching `AuthUser` or `ApiTokenPrincipal`, `AuthSubject` and
`DbAuthContext` copies. Install it per request or authenticated connection-init,
never on a globally shared schema. `from_context` denies missing, mismatched,
shadowed conflicting, opposite actor-kind or schema-only identity. Do not add a
second independently constructed identity copy. Injection returns an error
before mutation when preexisting copies conflict. For reauthentication, create
fresh request/connection data and install the entire new bundle; do not replace
individual copies or mutate in-flight operations. Existing streams retain their
accepted credential and must reauthenticate/reconnect when it changes. Safe database projections omit
token bindings; the original principal remains separately available.

For selected sensitive entities, wrap the business policy:

```rust
let enforcement = std::sync::Arc::new(AgqlCurrentAuthorityEnforcement::new(
    current_principal_resolver, token_status_checker, clock, binding_authorizer,
));
database.set_entity_policy(AgqlCurrentAuthorityPolicy::new(
    enforcement.clone(), business_policy,
));
let schema = schema_builder(database).data(enforcement).finish();
```

All four dependencies are required trusted host implementations. A checker that
always returns active does not implement immediate revocation. The resolver must
read authoritative enabled/deleted, session, membership and grant state on every
invocation, and fail closed when that state is unavailable. The wrapper calls
`revalidate_sensitive_principal` for every operation; it does not cache a decision
across aliases, fields, requests or subscription events. If current authority
narrows any accepted grants (including typed authorization roles), or its safe
projection changes, permission is denied. Obtain a fresh accepted credential;
parallel GraphQL fields never mutate their shared identity.

The mandatory `AgqlBindingAuthorizer` receives the complete accepted principal,
actual context, entity, policy key, operation kind and execution surface. It must
explicitly authorize purpose, audience, issuer contract, resource, exact operation,
tenant/business scope and sender confirmation wherever applicable. Unsupported
restrictions must deny. User JWT issuer/audience are validated by the transport
and are not retained in `AuthUser`; supply trusted request evidence when the
callback needs those facts. API token audience is retained. Never substitute
client arguments for verified sender proof or transport evidence. Coarse entity
checks cannot establish row/resource fencing for list results: install host row
policies or deny such bound credentials on unsupported routes.

The wrapper checks expiry with its trusted clock after asynchronous status and
resolver calls, after binding authorization and after the inner business policy.
A delegated credential also retains the authoritative parent expiry deadline.
Each decision observes authoritative state at evaluation. Revocation racing a
later committed write cannot be atomically undone by this adapter; hosts requiring
that guarantee must fence the database write using their authoritative session
version/transaction policy.

Generated roots, relationship loads and Federation lookups call the entity policy.
Subscriptions repeat this entity policy for each event using the original accepted
request. Deleted snapshots additionally require an explicit host
`RowPolicy::can_read_deleted_preimage` grant; wrapping an entity policy does not
install or replace that row policy. Repository
convenience calls with `ctx=None` deny under this wrapper. Custom resolvers can use
`AgqlCurrentAuthorityGuard::new(entity, policy_key, kind, surface)` with the shared
enforcement installed as trusted schema configuration, or call `enforce` directly.
These checks do not replace business, row or field authorization. Custom SQL,
raw pools, direct loaders and `EntityQuery` are trusted host boundaries and must
perform both immediate checks and business authorization explicitly.

`DeclaredPoliciesRequired` is the database default. It requires providers for
marked policy keys; it does not protect unmarked operations or create tenant
isolation. Sensitive applications should select
`ExplicitPolicyForAllExposedOperations` and install appropriate entity, row and
field policies. See [strict authorization](strict-authorization.md).

## Safe projection

The lower-level conversion functions remain useful for trusted non-sensitive
integrations. Supplying only their two projected values does not satisfy the
accepted-request/current-authority contract above.

Mapped fields:

| `agql-auth` | ORM |
| --- | --- |
| principal subject | `AuthSubject.id` / `DbAuthContext.subject` |
| user id | `AuthSubject.user_id` / `DbAuthContext.user_id` |
| roles / scopes | roles / scopes |
| `token_claims.tenant_id` | tenant id |
| `token_claims.jti` / API token id | token reference |
| session id | session reference |
| actor (`token_claims.actor.sub`) | `actor_id` |
| organization / correlation id | typed subject/database fields and redacted claims |
| authoritative, structurally consistent `session.assurance` | `AuthAssurance` and `DbAuthContext.assurance` |
| assurance context | distinct `AuthAssurance.context` / `DbAuthContext.assurance.context` |
| custom `policy_version` string | `DbAuthContext.policy_version` |

Raw JWTs, refresh tokens, OAuth state, nonces, authorization codes/URLs,
claims requests, cookies, provider responses, API-token credentials, and
authorization headers are never copied. Arbitrary `token_claims.additional`
members are not bridge output; only the documented string `policy_version` is
retained. Token/session/actor identifiers are references, never credentials.

## Operation Assurance Evaluation

Version 0.17 adds `AgqlAssuranceEvaluator`. It converts a provider-neutral ORM
policy ID into upstream `AssurancePolicyId` / `AssuranceRequirement`, then
evaluates the current accepted user through the host's `AssurancePolicySet` and
injected `Clock`. It never verifies external evidence or calls session step-up.

Install it through `AssuranceEnforcement`; generated mutations call the
enforcement hook automatically and custom fields use
`DeclaredAssuranceGuard`. Denials expose lowercase GraphQL extension key `code`
with `STEP_UP_REQUIRED`, `UNAUTHENTICATED`, or `FORBIDDEN`. API-token
principals cannot satisfy user-session assurance and fail a declared
requirement as `FORBIDDEN`; explicitly exempt machine/service operations still
run their independent authorization checks.

See [Operation assurance](../../architecture/operation-assurance.md). Client manifests and safe
status projections are advisory; the current server evaluation controls
execution.

The database context also installs transaction-local `app.organization_id`,
`app.correlation_id`, `app.assurance`, and `app.policy_version` settings on PostgreSQL. Assurance
contains only the accepted authentication timestamp, normalized methods,
standard scalar ACR, separate policy context, and exact host MFA decision. The
bridge requires session MFA state plus access-token `auth_time`, AMR, and scalar
ACR to be structurally consistent with the `SessionAssurance`; malformed,
missing, or inconsistent assurance is omitted rather than repaired.

## Migrating from 0.7

Update any direct `agql-auth` dependency to the same published tag selector in the workspace manifest. `AuthSubject` and
`DbAuthContext` gained organization, correlation, and assurance fields; applications constructing
either with struct literals must add the fields or use their builders/`Default` update syntax.
The bridge preserves valid 0.8+ session assurance, active scope, correlation,
actor, safe token metadata, and the documented string `policy_version` instead
of retaining only the older role/scope/tenant subset.

## Migrating from an Earlier Bridge Release

Update any direct `agql-auth` dependency to the same published tag selector in the workspace manifest at
the same time as `graphql-orm`. This prevents Cargo from resolving separate
package/type universes. Version 0.22.0 keeps the identity, role, scope, tenant,
organization, actor, correlation, token-reference, and policy-version mappings,
but hardens the public bridge projection: arbitrary custom claims are no longer
copied, and malformed or token/session-inconsistent assurance is omitted. If a
host consumed arbitrary `AuthSubject.claims.additional` values, move that data
through an explicit application-owned request type instead of the ORM bridge.

Direct users of `agql-auth` must also review its migration through the pinned version. Version
0.11 replaces split durable rate-limit writes with the versioned atomic
`AuthRateLimitStore` contract. `graphql-orm` does not implement that store and
does not add a split or synthetic ORM-backed implementation. Version 0.12 adds
the state-bound `OidcIdTokenClaimRequest::EssentialAcrs` request and separate
`OidcAuthorizationOutcome.matched_acrs` provider evidence. Neither OIDC
requests/outcomes nor rate-limit persistence enter the ORM bridge. Applications
that only consume a structurally valid `AuthPrincipal` need no database or data
migration.

Version 0.15 adds existing-session-bound, access-token-only delegation. The
ORM bridge neither issues those tokens nor validates an active-session store;
hosts use agql-auth's `VerifiedActiveUserSessionResolver` and exact delegation
binding directly. The resulting user-shaped principal continues through the
ordinary bridge and resolver authorization without synthesizing a new session.

Version 0.16 adds consumer-supplied exact-only fixed requirements and patterns
to the hierarchical matcher. The ORM bridge continues projecting scopes
without choosing matcher policy; hosts opt into those semantics where they
construct their resource-server matcher.

Version 0.13 adds the policy ID, evaluation state/times, safe session status,
denial categories, and policy-set contract consumed by the operation assurance
evaluator. It does not change ordinary refresh eligibility.

Version 0.14 changes newly issued access JWTs from a `scopes` string array to
the standard space-delimited OAuth `scope` claim. The bridge sees the same
normalized `AuthPrincipal::scopes()` vector and needs no API or data migration.
Hosts that decode JWT payloads directly must follow the upstream rolling
migration: accept both shapes, switch issuers, wait the maximum old
access-token TTL plus leeway, then reject the legacy array. Purpose tokens keep
their separate `scopes` array.

An `EssentialAcrs` callback outcome alone never creates ORM assurance. The host
must first verify and locally allowlist the provider evidence, then construct a
session-bound `SessionAssurance`. A mapped value such as
`microsoft-entra/acrs/c1` stays byte-for-byte in `AuthAssurance.context`; it is
never translated into scalar `acr`, AMR, roles, scopes, tenant, or a custom
policy field. Missing scalar ACR or context remains `None`.

## Policy Decisions Stay Host-Owned

The bridge maps accepted identity and evaluates only explicitly declared
assurance requirements. Scope hierarchies, application scopes, and business
authorization remain in host policies or `agql-auth` guards. `ScopeEntityPolicy`
in `graphql-orm` continues to use exact string matching.
