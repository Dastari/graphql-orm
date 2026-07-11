# `agql-auth` Bridge Guide

`graphql-orm` never depends on `agql-auth` by default. Enable the optional
`auth-agql` feature for a one-way adapter.

## Dependency

```toml
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.4.3", features = ["sqlite", "auth-agql"] }
# Host applications may depend on agql-auth directly as well. The optional
# graphql-orm auth-agql feature pins the exact upstream release:
# git = "https://github.com/Dastari/agql-auth.git"
# rev = "5e7f230b96350f55496477c11f8a0505e6438779"
# version = "0.7.0"
agql-auth = { git = "https://github.com/Dastari/agql-auth.git", rev = "5e7f230b96350f55496477c11f8a0505e6438779", version = "0.7.0" }
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

Raw JWTs, API tokens, cookies, and authorization headers are never copied.

## Policy Decisions Stay Host-Owned

The bridge only maps identity. Scope hierarchies, product scopes, and business
authorization remain in host policies or `agql-auth` guards. `ScopeEntityPolicy`
in `graphql-orm` continues to use exact string matching.
