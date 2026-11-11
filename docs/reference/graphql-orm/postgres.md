---
title: "PostgreSQL RLS"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-11
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
  `search_path = pg_catalog, pg_temp` and schema-qualified types, so neither the deleting session's
  `search_path` nor a `pg_temp` table or type of the same name can redirect it, and it raises instead of deleting the base row when the search
  table is missing. Dropping or rebuilding the search index drops the trigger; re-applying the
  schema target recreates it. The runtime role needs DML on the search table, as before.
- **Change journal.** `__graphql_orm_change_log` gets RLS enabled (not forced) with no policies,
  and every table or column privilege other than `SELECT` held by any role other than its owner,
  `PUBLIC` included, is revoked. If one cannot be revoked, the statement raises and the plan rolls
  back. A role subject to RLS can neither read nor write entries. The plan drops the `INSERT ... WITH CHECK
  (true)` policy that 0.120.0 to 0.124.x created, and `ensure_change_journal_table` drops it when
  the caller owns the journal. Create the journal with the schema owner, not the runtime role, or
  the runtime role owns it and validation reports it as not enforced. `SELECT` grants survive
  re-application: they cannot write or add triggers, and with no policies they read nothing unless
  the role also has `BYPASSRLS`, as a dedicated backup export role does. See
  [Change journal recorder](#change-journal-recorder).

Validation reports a missing or altered policy, any additional policy on these tables (an extra
permissive policy would widen access), any policy at all on the change journal, and a trigger that
is missing, does not fire in ordinary sessions (for example `ENABLE REPLICA`), has the wrong type
or arguments, targets another table, or calls a modified `graphql_orm.delete_search_document()`.
Dependent deletes accept only a trigger that passes the same checks.

### Change journal recorder

With the `change-journal` feature, the database, not the runtime role, records journal entries.
`plan_schema_target` under `Managed` (and `PlanOnly` for preview), or
`Database::ensure_change_journal_recorder(&entities)`, creates:

- the journal, if it is missing, secured as above;
- `graphql_orm.record_change_journal_entry()`, a `SECURITY DEFINER` trigger function with
  `search_path = pg_catalog, pg_temp` and schema-qualified names and types. `EXECUTE` is revoked
  from `PUBLIC` and from every other role except its owner;
- an `AFTER INSERT OR UPDATE OR DELETE ... FOR EACH ROW` trigger named
  `graphql_orm_change_journal` on every single-key table in the target. Its arguments record the
  journal's schema and the table's own schema and name, all resolved when the trigger is created
  (never a `pg_temp` schema), plus the entity name, the declared table name, the key column and the
  key encoding (`bin:<hex>` for `bytea` keys, otherwise the key's text form).

The plan also drops any trigger that calls the recorder under a name other than
`graphql_orm_change_journal` on a table the applying role acts as owner of; graphql-orm never
installs one, and validation reports any it cannot drop. Partition clones go with their parent,
which relies on `pg_trigger.tgparentid`: the journal requires PostgreSQL 13 or later.

Nothing drops a correctly named recorder trigger on a table outside the target. Another schema
target in the same database may manage that table, so validation reports it as a `Warning`, not
an `Error`. Because the recorder binds the declared table to the table it fires on, such a trigger
can only record that table's own changes under its own name. If no schema target manages the
table, the trigger is unexpected: an operator has to drop it.

The declared table is the entity's table path as written, such as `users`, `"Users"`,
`app."Orders"` or `app."a.b"`. When a trigger is installed, the path is resolved with the ORM's
PostgreSQL identifier rules: split at unquoted dots, quoted parts unescaped, unquoted parts taken
literally. A qualified path must name the table's schema and name exactly. A bare name must name a
table in the journal's schema, which is where the ORM's unqualified names and the journal itself
resolve; qualify entity tables that live elsewhere. Installation fails otherwise.

The recorder runs as its owner, which is also the journal owner. It records only while the
transaction-local setting `graphql_orm.change_journal` is `on`. ORM write transactions on a
journal-enabled `Database` set it, and restore imports set it `off`, even when a role or database
default turns it on. The setting only selects transactions: the entry's key comes from the changed
row and its action from `TG_OP`, so a session that sets it can at most journal its own real
changes. The recorder raises instead of recording unless all of these hold:

- it fires as an `AFTER` row trigger named `graphql_orm_change_journal`;
- the table it fires on is the schema and name baked into its arguments, is not temporary, and is
  owned by the recorder's owner;
- the declared table it records, resolved with the same rules, names that table: a qualified path
  its schema and name exactly, a bare name that table in the journal's schema;
- the key column is the table's sole primary-key column;
- the journal exists in the recorded schema as an ordinary table, is owned by the recorder's
  owner, carries no non-internal trigger or rule, and grants `TRIGGER` to no other role.

The recorder locks the journal before checking for triggers, so no trigger can be added after the
check until the transaction ends. Under `REPEATABLE READ` or `SERIALIZABLE`, the trigger lookup
reads the transaction's snapshot, so it can miss a trigger committed after that snapshot was taken
and before the lock. The `TRIGGER` refusal narrows this to triggers added by a role that held
`TRIGGER` at that moment and no longer does, or by the owner.

The entity name in the arguments is not tied to the table by the database. Validation compares it
with the target for every managed table, and reports recorder triggers on other tables.

A trigger on the journal would run as the journal owner whenever the recorder appends, which is
why the journal must carry none and why no other role may hold `TRIGGER` on it. A role with
`EXECUTE` on the recorder and `TRIGGER` on a table could attach the recorder to its own trigger.
The recorder refuses such triggers, and the ORM check and validation also report the grant. Keep
`EXECUTE` on the recorder, and every journal privilege beyond `SELECT`, away from other roles.
That covers table and column grants, and roles reachable through `NOINHERIT` membership.
Composite-key tables get no trigger; the ORM refuses to journal them.

Partitioned tables are not supported for the journal. A partitioned journal, or a recorder trigger
on a partitioned entity table (whose partitions fire it under their own names), makes journaled
writes fail closed.

Before relying on the recorder, each ORM write transaction checks once per table that:

- the setting is `on` and `session_replication_role` is not `replica`;
- the table carries an enabled, unconditional trigger of the exact type that calls the unmodified
  recorder owned by the table owner, and is the table's only trigger calling it;
- the trigger's arguments name the table, the write's entity and key, and a journal owned by that
  owner with no trigger or rule;
- the current role, unless it acts as the journal owner, holds no table or column privilege on the
  journal beyond `SELECT` and no `EXECUTE` on the recorder. This covers grants to `PUBLIC` and to
  any role it is a member of, including `NOINHERIT` memberships.

A role that acts as the journal owner, by owning it, being a member of its owner or being a
superuser, passes the privilege part of the check, because it can already do anything those
privileges would allow. Otherwise the write fails and rolls back, with an error naming the reason.
`MutationContext::check_change_journal_recorder` runs the same check for one entity.

`validate_target` reports the same conditions, plus:

- a missing journal, or a missing or modified recorder function;
- a current role that acts as the journal owner, whether or not the target has RLS entities;
- any role other than the owner holding a table or column privilege on the journal beyond
  `SELECT`, or `EXECUTE` on the recorder;
- any trigger or rule on the journal;
- as an `Error`, any trigger that calls the recorder under another name; as a `Warning`, any
  correctly named recorder trigger on a table that is not a single-key table of the target.

Validation identifies tables by quoted schema and name, so `evil.users` and `public.evil_users`
are reported separately.

Run it with the runtime pool. With the owner pool it always reports that the role acts as the
journal owner.

Static dependent deletes exempt the recorder trigger from their trigger refusal when it:

- is named `graphql_orm_change_journal`, fires in ordinary sessions, and is an unconditional
  `AFTER INSERT OR UPDATE OR DELETE` row trigger with seven arguments;
- calls `graphql_orm.record_change_journal_entry()` with the exact body and `search_path`, as
  `SECURITY DEFINER`, owned by the table owner.

They do not inspect its arguments, the journal or privileges. The recorder only appends to the
journal, which holds no foreign keys or entity rows. On a journal-enabled `Database` each journaled
dependent row still goes through the ORM check above, and the recorder's own checks run on every
row.

Apply the plan as the role that owns the entity tables and the journal. The runtime role needs no
privilege on the journal and no `EXECUTE` on the recorder.

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
and `EXECUTE` on the helper functions used by its policies. It needs no privilege
on `__graphql_orm_change_log` or `graphql_orm.record_change_journal_entry()`. If default function
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
