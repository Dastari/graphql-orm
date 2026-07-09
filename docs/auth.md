# Auth Integration

`graphql-orm` does not validate JWTs, manage cookies, or define product scope names. It only needs a
request principal shape that generated resolvers and policy hooks can understand.

## AuthSubject

Attach `AuthSubject` to each async-graphql request:

```rust
let subject = AuthSubject {
    id: user.id.to_string(),
    roles: user.roles.clone(),
    scopes: user.scopes.clone(),
    tenant_id: user.tenant_id.clone(),
};

let request = request.data(subject);
```

Generated code and host policies can then use:

```rust
let subject = ctx.auth_subject()?;
let user_id = ctx.auth_user_id()?;
let maybe_subject = ctx.auth_subject_opt();
```

`ctx.auth_user()` remains as a deprecated alias for `ctx.auth_user_id()`. For compatibility,
`auth_subject()` also accepts the legacy `String` user id from context and upgrades it to an
`AuthSubject` with empty roles, scopes, and tenant id.

## Generated Resolver Modes

Generated query, mutation, subscription, and relation resolvers support three auth modes:

- `required`: require an auth subject before database work.
- `optional`: read auth when present and let policies decide.
- `none`: do not read auth in generated resolvers.

Set a schema-root default:

```rust
schema_roots! {
    auth: "required",
    query_custom_ops: [],
    entities: [Ticket, Session],
}
```

Override per entity:

```rust
#[graphql_entity(table = "public_pages", plural = "PublicPages", auth = "none")]
pub struct PublicPage {
    #[primary_key]
    pub id: String,
}
```

The compatibility default remains fail-closed, matching previous generated resolver behavior. Use
`auth = "none"` for public generated schemas.

## ScopeEntityPolicy

`ScopeEntityPolicy` is an exact-scope `EntityPolicy` helper:

```rust
let mut database = Database::new(pool);
database.set_entity_policy(ScopeEntityPolicy::new(
    &["tickets.read"],
    &["tickets.write"],
));
```

Read operations compare against `read_scopes`; create/update/delete operations compare against
`write_scopes`. With `require_auth: true`, missing auth returns an unauthenticated GraphQL error.
When a subject exists but lacks a required scope, the policy returns `Ok(false)`.

Matching is exact. A subject with `tickets.*` does not satisfy `tickets.read` unless the host adds a
separate policy or future auth bridge that deliberately implements hierarchical matching.

## PostgreSQL RLS

When PostgreSQL RLS should receive the same request principal, attach `DbAuthContext` too:

```rust
let request = request
    .data(subject.clone())
    .data(DbAuthContext::from_subject(&subject));
```

RLS helper functions still use exact scope checks. Hierarchical or wildcard semantics should stay in
GraphQL/application policies.

## agql-auth Feature

Enable `auth-agql` for optional converters. See [agql-auth-bridge.md](agql-auth-bridge.md).

```rust
use graphql_orm::graphql::auth_agql::auth_bundle_from_principal;
let (subject, db_auth) = auth_bundle_from_principal(&principal);
```


## Authorization Modes

See [strict-authorization.md](strict-authorization.md).

```rust
let database = Database::new(pool)
    .with_authorization_mode(AuthorizationMode::DeclaredPoliciesRequired);
```

Current default: `LegacyPermissive`. Secure recommended: `DeclaredPoliciesRequired`.

## Expanded AuthSubject

`AuthSubject` now includes optional `user_id`, `claims`, `token_id`, `session_id`,
and `actor_id`. `Debug` redacts claim bodies. Scope comparison is case-sensitive.

## Safe Errors

Missing auth returns `UNAUTHENTICATED` via `OrmPublicError`. See
[error-codes.md](error-codes.md).
