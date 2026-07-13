# `agql-auth` Bridge Guide

`graphql-orm` never depends on `agql-auth` by default. Enable the optional
`auth-agql` feature for a one-way adapter.

## Dependency

```toml
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.7.0", features = ["sqlite", "auth-agql"] }
# Host applications may depend on agql-auth directly as well. The optional
# graphql-orm auth-agql feature pins the exact upstream release:
# git = "https://github.com/Dastari/agql-auth.git"
# rev = "2ab5dc1f963dad401a3393fd3af1392c2bb51e50"
# version = "0.9.0"
agql-auth = { git = "https://github.com/Dastari/agql-auth.git", rev = "2ab5dc1f963dad401a3393fd3af1392c2bb51e50", version = "0.9.0" }
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
| authoritative `session.assurance` | `AuthAssurance` and `DbAuthContext.assurance` |
| assurance context and custom policy metadata | redacted `claims` / `app.claims` |
| custom `policy_version` string | `DbAuthContext.policy_version` |

Raw JWTs, API tokens, cookies, and authorization headers are never copied.

The database context also installs transaction-local `app.organization_id`,
`app.correlation_id`, `app.assurance`, and `app.policy_version` settings on PostgreSQL. Assurance
contains only the accepted authentication timestamp, normalized methods, ACR, policy context, and
MFA decision.

## Migrating from 0.7

Update any direct `agql-auth` dependency to the exact 0.9 revision above. `AuthSubject` and
`DbAuthContext` gained organization, correlation, and assurance fields; applications constructing
either with struct literals must add the fields or use their builders/`Default` update syntax.
The bridge now preserves 0.8 session assurance, active scope, correlation, actor, token metadata,
and custom policy metadata instead of retaining only the older role/scope/tenant subset.

## Migrating from 0.8.0

Update any direct `agql-auth` dependency to the exact 0.9.0 revision above at the same time as
updating `graphql-orm`. This prevents Cargo from resolving separate 0.8.0 and 0.8.1 package/type
universes. The bridge API and mapped authorization data are unchanged.

## Policy Decisions Stay Host-Owned

The bridge only maps identity. Scope hierarchies, product scopes, and business
authorization remain in host policies or `agql-auth` guards. `ScopeEntityPolicy`
in `graphql-orm` continues to use exact string matching.
