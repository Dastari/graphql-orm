---
title: "agql-auth Bridge Guide"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-08-13
review_by: 2027-02-01
supersedes: []
---

# `agql-auth` Bridge Guide

`graphql-orm` never depends on `agql-auth` by default. Enable the optional
`auth-agql` feature for a one-way adapter.

## Dependency

```toml
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.23.0", features = ["sqlite", "auth-agql"] }
# Host applications may depend on agql-auth directly as well. The optional
# graphql-orm auth-agql feature pins the exact upstream release:
# git = "https://github.com/Dastari/agql-auth.git"
# rev = "e6439aa034babb6827e9253977f760667ea6b7eb"
# version = "0.16.0"
agql-auth = { git = "https://github.com/Dastari/agql-auth.git", rev = "e6439aa034babb6827e9253977f760667ea6b7eb", version = "0.16.0" }
```

Both projects are intentionally Git-only. Cargo's crates.io packaging flow cannot package
`graphql-orm` because the optional `agql-auth` dependency is Git-sourced; this is expected and is
not a supported release path.

## Conversion

```rust
use graphql_orm::graphql::auth_agql::{
    auth_bundle_from_principal, auth_subject_from_principal, db_auth_context_from_principal,
};
use agql_auth::AuthPrincipal;

fn inject(request: async_graphql::Request, principal: AuthPrincipal) -> async_graphql::Request {
    let (subject, db_auth) = auth_bundle_from_principal(&principal);
    request.data(subject).data(db_auth)
}
```

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

Update any direct `agql-auth` dependency to the exact 0.16 revision above. `AuthSubject` and
`DbAuthContext` gained organization, correlation, and assurance fields; applications constructing
either with struct literals must add the fields or use their builders/`Default` update syntax.
The bridge preserves valid 0.8+ session assurance, active scope, correlation,
actor, safe token metadata, and the documented string `policy_version` instead
of retaining only the older role/scope/tenant subset.

## Migrating from an Earlier Bridge Release

Update any direct `agql-auth` dependency to the exact 0.16.0 revision above at
the same time as `graphql-orm`. This prevents Cargo from resolving separate
package/type universes. Version 0.22.0 keeps the identity, role, scope, tenant,
organization, actor, correlation, token-reference, and policy-version mappings,
but hardens the public bridge projection: arbitrary custom claims are no longer
copied, and malformed or token/session-inconsistent assurance is omitted. If a
host consumed arbitrary `AuthSubject.claims.additional` values, move that data
through an explicit application-owned request type instead of the ORM bridge.

Direct users of `agql-auth` must also review its migration through 0.16. Version
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
