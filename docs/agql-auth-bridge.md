# `agql-auth` Bridge Guide

`graphql-orm` never depends on `agql-auth` by default. Enable the optional
`auth-agql` feature for a one-way adapter.

## Dependency

```toml
graphql-orm = { version = "0.3", features = ["sqlite", "auth-agql"] }
agql-auth = "0.7"
```

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
