---
title: "PostgreSQL RLS"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-10
review_by: 2027-02-01
supersedes: []
---

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

- `Managed`: create helper functions, enable/force RLS, and create deterministic named policies,
  including on the ORM-managed tables listed under [ORM-managed tables](#orm-managed-tables).
- `ValidateOnly`: validate table schema plus enabled/forced RLS flags and expected policies where feasible.
- `PlanOnly`: preview table and RLS SQL, but reject application.
- `ExternalReadOnly` and `ExternalWritable`: do not create, plan, or validate RLS policies.

The table-only APIs, including `SchemaModel`, `TableModel`, `plan_migration_to_entities`, and
`apply_migration`, remain unchanged. `build_rls_policy_plan` still renders entity tables only;
`plan_schema_target` uses `build_schema_target_rls_plan`, which also covers ORM-managed tables.

## Enforcement Validation

PostgreSQL skips every policy for a superuser or a `BYPASSRLS` role, and for the owner of a table
without `FORCE ROW LEVEL SECURITY` (`force = false`). For a target with RLS entities,
`validate_target` therefore reports `Error` diagnostics when the database handle's connection role
would not be subject to RLS: one diagnostic for a superuser or `BYPASSRLS` role, otherwise one per
RLS entity table, search document table or change journal where `row_security_active(table)` is
false. Run it, or the narrower startup check, with the runtime pool and refuse to serve on errors:

```rust
let diagnostics = database.schema().validate_rls_enforcement(&target).await?;
if diagnostics
    .iter()
    .any(|diagnostic| diagnostic.severity == SchemaDiagnosticSeverity::Error)
{
    return Err("runtime database role bypasses row-level security".into());
}
```

`validate_rls_enforcement` applies under every schema policy, including the external ones, and
returns nothing for targets without RLS entities. Validating with a superuser migration pool now
reports these errors; validate the schema owner's work with the runtime role instead.

## ORM-Managed Tables

`build_schema_target_rls_plan` (used by `plan_schema_target`) secures the tables the ORM writes on
behalf of RLS entities, and `validate_target` reports drift on them:

- **Search documents.** For each RLS entity with a PostgreSQL search index, the
  `__graphql_orm_search_<table>` table gets `ENABLE` and, when the entity forces RLS, `FORCE ROW
  LEVEL SECURITY`, plus `SELECT`, `INSERT`, `UPDATE` and `DELETE` policies that expose a document
  only while its base row is visible:
  `EXISTS (SELECT 1 FROM <base> b WHERE b.<pk> = (<search>.entity_pk)::<pk type>)`. Binary
  (`Vec<u8>`) keys compare against the ORM's `bin:<hex>` document key (search itself does not yet
  match binary keys: [#560](https://github.com/Dastari/graphql-orm/issues/560)). A
  `graphql_orm_search_delete` `BEFORE DELETE` trigger on the base table removes the document while
  the base row is still visible, including for bulk, cascaded and raw SQL deletes. The plan
  resolves the search table when it creates the trigger and records its schema and name in the
  trigger arguments. `graphql_orm.delete_search_document()` runs as the invoker with
  `search_path = pg_catalog`, so neither the deleting session's `search_path` nor a `pg_temp` table
  of the same name can redirect it, and it raises instead of deleting the base row when the search
  table is missing. Dropping or rebuilding the search index drops the trigger; re-applying the
  schema target recreates it. The runtime role needs DML on the search table, as before.
- **Change journal.** An existing `__graphql_orm_change_log` gets RLS enabled (not forced) with
  a single `INSERT ... WITH CHECK (true)` policy. A role subject to RLS can append journal entries
  but cannot read, update or delete them; only the table owner or a `BYPASSRLS` role can read it.
  `ensure_change_journal_table` applies the same protection when it creates or opens the table.
  Create the journal with the schema owner, not the runtime role, or the runtime role owns it and
  validation reports it as not enforced.

Validation reports a missing or altered policy, any additional policy on these tables (an extra
permissive policy would widen access), and a trigger that is missing, does not fire in ordinary
sessions (for example `ENABLE REPLICA`), has the wrong type or arguments, targets another table, or
calls a modified `graphql_orm.delete_search_document()`. Dependent deletes accept only a trigger
that passes the same checks.

### Search performance

PostgreSQL evaluates row-security quals before non-leakproof operators, and the full-text match
operator `@@` is not leakproof. With the search document policy in place PostgreSQL can no longer
use the GIN index for a search on an RLS entity: each search scans the search document table and
probes the base primary key per document. In a disposable-container measurement with 200,000
documents across ten tenants, a selective search went from about 8 ms to about 470 ms, and a broad
one from about 85 ms to about 430 ms. Searches on entities without `#[graphql_rls]` are unchanged.
Plan capacity accordingly for large RLS search tables. An index-preserving design is tracked in
[#555](https://github.com/Dastari/graphql-orm/issues/555).

Use a separate runtime database role without superuser, `BYPASSRLS`, or inherited
table-owner privileges. The schema owner applies the plan; the runtime role needs
only its intended table privileges, `USAGE` on the table and `graphql_orm` schemas,
and `EXECUTE` on the helper functions used by its policies. If default function
execution grants have been revoked, grant those functions explicitly. A new helper
schema does not automatically grant `USAGE` to another role. Do not solve missing
privileges by giving the runtime role ownership or RLS bypass.

For a scope-and-tenant read policy, an administrator can provision a dedicated
role with grants such as:

```sql
GRANT USAGE ON SCHEMA public, graphql_orm TO application_reader;
GRANT SELECT ON public.users TO application_reader;
GRANT EXECUTE ON FUNCTION graphql_orm.has_scope(text),
    graphql_orm.current_scopes(), graphql_orm.current_tenant_id()
    TO application_reader;
```

Adjust the role, tables and helpers to the application's actual policy. Verify
visible and hidden rows using that restricted role, including relationship
queries; tests run as a superuser cannot establish RLS enforcement.

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
settings.

If it is absent, a generated resolver that reaches an entity with `#[graphql_rls]` fails closed
with `AUTHORIZATION_MISCONFIGURED` before any database work, under the default
`AuthorizationMode::DeclaredPoliciesRequired` and the stricter
`ExplicitPolicyForAllExposedOperations`. The check covers every RLS entity a generated request
reaches:

- the root entity of queries, search, aggregates, mutations, subscriptions, and federation entity
  and node lookups;
- relation fields whose target is an RLS entity;
- relation filters (`where: { children: [...] }`), including nested ones, and relation count
  ordering (`order_aggregate`) whose target is an RLS entity, even from a non-RLS root;
- every entity a static dependent delete reads or writes.

Entities without `#[graphql_rls]` are unaffected. The check applies to generated static entities
only; runtime GraphQL collections do not carry RLS metadata
([#559](https://github.com/Dastari/graphql-orm/issues/559)). Two explicit exceptions remain:

- `SystemAccess::new("<name>")` in the request data is the named trusted bypass for an internal
  schema that deliberately runs without a principal. It only skips this ORM-side check. Under a
  correctly validated runtime role the database still applies RLS, and the generated predicates,
  which see no tenant or user, hide the rows. Use it with a pool whose role is meant to see them.
- `AuthorizationMode::LegacyPermissive` keeps the earlier behaviour of running without settings.

### Dependent deletes

Static dependent deletes issue `SET LOCAL row_security = off`, so that PostgreSQL refuses rather
than silently hides dependents. On a role subject to RLS, a dependent delete of an RLS entity
therefore fails, and it succeeds only for a role that bypasses RLS. Such a role is exactly what
[Enforcement Validation](#enforcement-validation) reports as an `Error`. Run dependent deletes
through a separate `Database` whose pool connects as a dedicated `BYPASSRLS` role, used only by a
host-controlled delete schema. On that path:

- tenant isolation comes only from the ORM: authorize the root row with entity and row policies,
  because the bypass role reads every tenant's rows;
- attach `DbAuthContext` (or `SystemAccess`) to the request so the fail-closed check passes;
- run `validate_rls_enforcement` against the runtime pool, not the delete pool, whose `BYPASSRLS`
  error is expected.

Repository and lower-level APIs (`find_by_key`, `fetch_rows_with_auth(..., None)`,
`MutationContext::begin`) are trusted host paths and still run without settings. Without settings
the generated predicates see no tenant or user and match no rows, which fails closed only when the
pool's role is subject to RLS. See [Enforcement Validation](#enforcement-validation).

Connection pooling safety depends on transaction-local `set_config(..., true)`: settings are visible
inside the query or mutation transaction and are cleared on commit or rollback. Relation loaders add a
canonical auth-context key to batching keys so concurrent requests with different auth contexts do not
share a batch.
