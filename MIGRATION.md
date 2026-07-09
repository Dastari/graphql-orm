# Migration Guide

## 0.2.21 Auth Bridge

### Structural Changes

`AuthExt::auth_user()` is deprecated but still available. Migrate call sites based on what they need:

```rust
// Before
let user_id = ctx.auth_user()?;

// After: id only
let user_id = ctx.auth_user_id()?;

// After: id, roles, scopes, tenant id
let subject = ctx.auth_subject()?;
```

Applications can keep injecting the legacy `String` user id while migrating. `graphql-orm` upgrades it
to `AuthSubject { id, roles: [], scopes: [], tenant_id: None }`. New code should inject
`AuthSubject` directly.

If a downstream crate implemented `AuthExt` itself, add implementations for `auth_user_id`,
`auth_subject`, and `auth_subject_opt`. Most applications only use the built-in implementation for
`async_graphql::Context<'_>` and do not need to change anything beyond call-site names.

```rust
let request = request.data(AuthSubject {
    id: user.id.to_string(),
    roles: user.roles.clone(),
    scopes: user.scopes.clone(),
    tenant_id: user.tenant_id.clone(),
});
```

`DbAuthContext::from_subject(&subject)` can mirror the same principal into PostgreSQL RLS settings:

```rust
let request = request
    .data(subject.clone())
    .data(DbAuthContext::from_subject(&subject));
```

### Generated Resolver Auth Modes

Generated resolvers keep the previous fail-closed default: if no `auth` setting is present, they
require an auth subject before database access. This preserves the old generated `ctx.auth_user()?`
gate.

Use `auth = "none"` for public generated resolvers:

```rust
#[graphql_entity(table = "pages", plural = "Pages", auth = "none")]
pub struct Page {
    // fields...
}
```

Use `auth = "optional"` when a schema should read a subject if present but leave allow/deny decisions
to `EntityPolicy`, `RowPolicy`, or `FieldPolicy`:

```rust
schema_roots! {
    auth: "optional",
    query_custom_ops: [],
    entities: [Record],
}
```

Use `auth = "required"` explicitly for new private schemas or entities:

```rust
schema_roots! {
    auth: "required",
    query_custom_ops: [],
    entities: [Ticket, Session],
}
```

Entity-level `auth` overrides the schema-root mode.

### ScopeEntityPolicy

`ScopeEntityPolicy` is exact-match only:

```rust
let mut database = Database::new(pool);
database.set_entity_policy(ScopeEntityPolicy::new(
    &["tickets.read"],
    &["tickets.write"],
));
```

`require_auth: true` returns an unauthenticated GraphQL error when no subject exists. A subject that
lacks the required exact scope returns `Ok(false)` from the policy and is denied by the generated
access path.

### Behavioral Notes

- No JWT, OIDC, cookie, wildcard, or product-specific scope logic was added to `graphql-orm`.
- PostgreSQL RLS helper functions still use exact scope matching.
- The `auth-agql` feature is currently a reserved compile-time feature. The concrete agql-auth
  conversion helpers will be added after the upstream agql-auth 0.7 API is tagged.
