# `agql-auth` Bridge Guide

`graphql-orm` never depends on `agql-auth` by default. Enable the optional
`auth-agql` feature for a one-way adapter.

## Dependency

```toml
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.15.0", features = ["sqlite", "auth-agql"] }
# Host applications may depend on agql-auth directly as well. The optional
# graphql-orm auth-agql feature pins the exact upstream release:
# git = "https://github.com/Dastari/agql-auth.git"
# rev = "3f3b0c5365adfbe436514a681d977b600991b797"
# version = "0.12.0"
agql-auth = { git = "https://github.com/Dastari/agql-auth.git", rev = "3f3b0c5365adfbe436514a681d977b600991b797", version = "0.12.0" }
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

The database context also installs transaction-local `app.organization_id`,
`app.correlation_id`, `app.assurance`, and `app.policy_version` settings on PostgreSQL. Assurance
contains only the accepted authentication timestamp, normalized methods,
standard scalar ACR, separate policy context, and exact host MFA decision. The
bridge requires session MFA state plus access-token `auth_time`, AMR, and scalar
ACR to be structurally consistent with the `SessionAssurance`; malformed,
missing, or inconsistent assurance is omitted rather than repaired.

## Migrating from 0.7

Update any direct `agql-auth` dependency to the exact 0.12 revision above. `AuthSubject` and
`DbAuthContext` gained organization, correlation, and assurance fields; applications constructing
either with struct literals must add the fields or use their builders/`Default` update syntax.
The bridge preserves valid 0.8+ session assurance, active scope, correlation,
actor, safe token metadata, and the documented string `policy_version` instead
of retaining only the older role/scope/tenant subset.

## Migrating from an Earlier Bridge Release

Update any direct `agql-auth` dependency to the exact 0.12.0 revision above at
the same time as `graphql-orm`. This prevents Cargo from resolving separate
package/type universes. Version 0.15.0 keeps the identity, role, scope, tenant,
organization, actor, correlation, token-reference, and policy-version mappings,
but hardens the public bridge projection: arbitrary custom claims are no longer
copied, and malformed or token/session-inconsistent assurance is omitted. If a
host consumed arbitrary `AuthSubject.claims.additional` values, move that data
through an explicit application-owned request type instead of the ORM bridge.

Direct users of `agql-auth` must also review its 0.10→0.12 migration. Version
0.11 replaces split durable rate-limit writes with the versioned atomic
`AuthRateLimitStore` contract. `graphql-orm` does not implement that store and
does not add a split or synthetic ORM-backed implementation. Version 0.12 adds
the state-bound `OidcIdTokenClaimRequest::EssentialAcrs` request and separate
`OidcAuthorizationOutcome.matched_acrs` provider evidence. Neither OIDC
requests/outcomes nor rate-limit persistence enter the ORM bridge. Applications
that only consume a structurally valid `AuthPrincipal` need no database or data
migration.

An `EssentialAcrs` callback outcome alone never creates ORM assurance. The host
must first verify and locally allowlist the provider evidence, then construct a
session-bound `SessionAssurance`. A mapped value such as
`microsoft-entra/acrs/c1` stays byte-for-byte in `AuthAssurance.context`; it is
never translated into scalar `acr`, AMR, roles, scopes, tenant, or a custom
policy field. Missing scalar ACR or context remains `None`.

## Policy Decisions Stay Host-Owned

The bridge only maps identity. Scope hierarchies, product scopes, and business
authorization remain in host policies or `agql-auth` guards. `ScopeEntityPolicy`
in `graphql-orm` continues to use exact string matching.
