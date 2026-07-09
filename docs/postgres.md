# PostgreSQL RLS

PostgreSQL support includes optional row-level security metadata for generated entities. RLS is a
database-level defense in depth. It does not replace GraphQL authorization: generated resolvers still
enforce their selected generated auth mode and still evaluate configured entity, row, and field
policies.

## Entity Attribute

Enable RLS with one entity-level `#[graphql_rls(...)]` attribute:

```rust
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "postgres", table = "users", plural = "Users")]
#[graphql_rls(
    force = true,
    select(scope = "users.read", tenant = "tenant_id"),
    insert(scope = "users.write", tenant = "tenant_id", owner = "created_by"),
    update(scope = "users.write", tenant = "tenant_id", owner = "owner_id"),
    delete(predicate = "graphql_orm.has_scope('users.delete') AND owner_id = graphql_orm.current_user_id()")
)]
pub struct User {
    #[primary_key]
    pub id: String,
    pub tenant_id: String,
    pub owner_id: String,
}
```

Attribute presence enables table RLS. `force = true` is the default. `force = false` enables RLS
without `FORCE ROW LEVEL SECURITY`.

Supported operations are `select`, `insert`, `update`, and `delete`. If an operation has
`predicate`, that predicate is used exactly. Otherwise the generated predicate combines configured
conditions in deterministic order with `AND`:

- scope: `graphql_orm.has_scope('<scope>')`
- tenant: `<tenant_column> = graphql_orm.current_tenant_id()`
- owner: `<owner_column> = graphql_orm.current_user_id()`

If an operation has no predicate, scope, tenant, or owner, no policy is generated for that operation.
Empty `#[graphql_rls]` is valid: it enables and optionally forces RLS but creates no permissive
operation policies, so PostgreSQL denies rows by default. Mixing `predicate` with generated fields
on the same operation is a compile error.

Tenant and owner values are database column names. For non-text columns or advanced semantics, use a
custom predicate with explicit casts. Scope checks are exact string matches unless a custom predicate
implements different behavior. `graphql-orm` does not add a hardcoded admin or global bypass; model
that with custom predicates or broad application scopes.

PostgreSQL RLS helpers intentionally do not implement wildcard or hierarchical scope logic. Keep
hierarchical matching in the GraphQL/application policy layer and pass only exact scopes into RLS.

`#[graphql_rls]` is PostgreSQL-only. SQLite and MSSQL builds fail clearly when an entity opts into
RLS. In multi-backend builds, RLS entities must declare `backend = "postgres"`.

## Schema Management

`schema_roots!` emits:

```rust
graphql_orm_schema_target() -> SchemaTarget
```

Use this target for RLS-aware planning, validation, and application:

```rust
let target = graphql_orm_schema_target();

let report = database.schema().validate_target(&target).await?;
let plan = database
    .schema()
    .plan_schema_target("2026-06-29-rls", "enable RLS", &target)
    .await?;
database
    .schema()
    .apply_schema_target(&plan, ApplyOptions::default())
    .await?;
```

Policy behavior:

- `Managed`: create helper functions, enable/force RLS, and create deterministic named policies.
- `ValidateOnly`: validate table schema plus enabled/forced RLS flags and expected policies where feasible.
- `PlanOnly`: preview table and RLS SQL, but reject application.
- `ExternalReadOnly` and `ExternalWritable`: do not create, plan, or validate RLS policies.

The table-only APIs, including `SchemaModel`, `TableModel`, `plan_migration_to_entities`, and
`apply_migration`, remain unchanged.

## Generated SQL

Managed plans create helper functions under the `graphql_orm` schema:

- `graphql_orm.current_user_id()`
- `graphql_orm.current_subject()`
- `graphql_orm.current_tenant_id()`
- `graphql_orm.current_roles()`
- `graphql_orm.current_scopes()`
- `graphql_orm.claims()`
- `graphql_orm.has_scope(scope text)`

Request settings use these keys:

- `app.user_id`
- `app.subject`
- `app.tenant_id`
- `app.roles`
- `app.scopes`
- `app.claims`

Policy names are deterministic and quoted. Schema-qualified table paths are quoted in generated
entity metadata and used as the `ON <table>` target. Policies are rendered with
`DROP POLICY IF EXISTS ... ON ...` followed by `CREATE POLICY ...` for create-or-replace behavior.

Operation SQL:

- `select`: `FOR SELECT USING (<predicate>)`
- `insert`: `FOR INSERT WITH CHECK (<predicate>)`
- `update`: `FOR UPDATE USING (<predicate>) WITH CHECK (<predicate>)`
- `delete`: `FOR DELETE USING (<predicate>)`

## Request Auth Context

Attach `DbAuthContext` to each async-graphql request when database RLS should apply:

```rust
let subject = AuthSubject::from_parts(
    identity.user_id.to_string(),
    identity.roles.clone(),
    identity.scopes.clone(),
    identity.tenant_id.clone(),
);
let request = request
    .data(subject.clone())
    .data(DbAuthContext::from_subject(&subject));
```

Generated resolvers look for this context. If it is present and the backend is PostgreSQL, list
queries, single lookups, mutations, and relation preload queries run with transaction-local auth
settings. If it is absent, existing behavior is preserved.

Connection pooling safety depends on transaction-local `set_config(..., true)`: settings are visible
inside the query or mutation transaction and are cleared on commit or rollback. Relation loaders add a
canonical auth-context key to batching keys so concurrent requests with different auth contexts do not
share a batch.
