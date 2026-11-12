---
title: "Runtime, Writes, And Policies"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-11
review_by: 2027-02-01
supersedes: []
---

# Runtime, Writes, And Policies

Generated code targets the `graphql-orm` runtime. The runtime owns database handles, backend traits, filters, pagination, relation loading, write hooks, field policies, row policies, subscriptions, backups, and schema management.

## Database Handle

Use the backend-specific `Database::connect_*` helpers for normal application setup:

```rust
let database =
    Database::<SqliteBackend>::connect_sqlite("sqlite://app.db")
        .await?
        .with_schema_policy(SchemaPolicy::Managed);
```

Postgres uses `Database::<PostgresBackend>::connect_postgres(database_url)`. MSSQL uses
`Database::<MssqlBackend>::connect_ado(connection_string)` and defaults to the read-only external
schema policy. Deliberate SQL Server DML uses
`Database::<MssqlBackend>::connect_ado_external_writable(connection_string)`.

For in-memory SQLite URLs (`sqlite::memory:`, `sqlite://:memory:` and
`mode=memory`), ORM-owned helpers default to one pooled connection, disable idle
and maximum-lifetime retirement, and set at least one minimum connection. An
additional unleased keeper connection holds the shared-cache memory database for
the pool lifetime. If cancellation, a failed release ping or explicit connection
closure replaces a pooled connection, its replacement sees the existing schema
and rows. Explicit pool sizes use the same database. Memory URLs use shared cache
internally, but each pool gets a unique internal database name unless the URL
explicitly requests `cache=shared`. Only that opt-in retains a named URL's name
and permits sharing between pools. Omitting `cache` and specifying `cache=private`
both preserve isolation; anonymous databases also receive unique names. This costs
one additional SQLite connection per memory pool. Closing the pool still
refuses acquisitions normally, and process exit loses memory data. These guarantees
do not apply to caller-built raw pools.

Use `Database::new(pool)` for compatibility or `Database::builder(pool)` when the application
intentionally owns raw driver-specific pool setup.

```rust
let database = Database::builder(pool)
    .schema_policy(SchemaPolicy::Managed)
    .default_page_limit(Some(1000))
    .max_page_limit(Some(1000))
    .change_journal_enabled(true)
    .build();
```

`Database::new` does not mutate schemas. Schema mutation only happens through explicit calls on `database.schema()`.

## Write Support

For atomic multi-entity state machines, versioned CAS, and immutable ledgers, see
[Portable Persistence Primitives](../../architecture/portable-persistence.md).

SQLite and Postgres support generated write helpers when the entity and root derive allow them.
MSSQL supports the applicable generated entity DML, transactions, hooks, policies, events, and
subscriptions only through an explicitly writable Tiberius pool paired with
`schema_policy = "external_writable"`. It does not support migration application, managed search,
backup/restore, or runtime-schema writes. `Database::<MssqlBackend>::connect_ado` remains physically
read-only.

For external databases, prefer explicit policy:

```rust
let database = Database::builder(pool)
    .schema_policy(SchemaPolicy::ExternalReadOnly)
    .build();
```

`ExternalReadOnly` rejects entity writes and schema application. `ExternalWritable` can allow entity writes on write-capable backends while still rejecting schema application.

```rust
let database = Database::<MssqlBackend>::connect_ado_external_writable(connection_string).await?;
```

The SQL Server transaction pins one Tiberius client. Successful commit or rollback returns it to the
pool; protocol failure, cancellation, or drop discards it so a client with indeterminate transaction
state cannot be reused. SQL Server upserts use locked update/insert statements rather than `MERGE`.

Generated row-policy decisions for update, delete, replace, and upsert share the pinned mutation
transaction with their DML. Input transforms and before hooks receive the locked preimage. Bulk
update/delete helpers execute only against the exact authorized key set and validate one affected
row per key; they never re-run the original broad predicate after policy evaluation. This contract
applies equally to generated GraphQL mutations and repository helpers.

## Repository Helpers

Generated repository helpers use `graphql_orm::Result<T>` so application crates can keep SQLX out of
public signatures. Existing raw-pool helpers are still available for compatibility, but new service
code should prefer `Database`-first helpers:

```rust
let database = Database::<SqliteBackend>::connect_sqlite("sqlite://app.db").await?;

let users = User::find_many(
    &database,
    UserWhereInput {
        active: Some(BoolFilter { eq: Some(true) }),
        ..Default::default()
    },
)
.await?;

let total = User::count_all(&database).await?;
```

Inside `Database::transaction`, `tx.query::<T>().count()` and `.exists()` retain
repository entity authorization. An installed row policy must declare current
`ReadVisibility::Unrestricted` or `Complete` SQL visibility; complete predicates
are intersected with caller filters before the count on the same pinned
transaction. Callback-only/prefilter policies and residual query filters under
an installed row policy are rejected. The SQL count does not materialize entity
rows or invoke per-row callbacks. With no row policy, existing residual-filter
behavior remains available.

Write-capable entities generate:

- `insert(&database, input)` and `insert_many(&database, inputs)`
- `update_by_id(&database, id, input)` and `update_where(&database, where, input)`
- `delete_by_id(&database, id)`, `delete_where(&database, where)`, and explicit `delete_all(&database)`
- `replace_all(&database, inputs)` for transactional service cache replacement
- `upsert(&database, input)` and `upsert_many(&database, inputs)` when `#[graphql_entity(upsert = "...")]` is configured

`delete_where` still rejects an empty filter. Use `delete_all` when a table-wide delete is intended.
`replace_all` deletes existing rows and inserts the provided inputs in one transaction using the
normal ORM write path, so hooks, policies, search maintenance, subscriptions, and change events are
preserved.

Top-level upsert helpers choose `TransactionMode::StateMachine` because an absent conflict key has no
row to lock. The transaction-bound `MutationContext::upsert` rejects `TransactionMode::Default`;
callers composing an upsert with other work must choose `StateMachine` explicitly. Update and delete
helpers lock an existing authoritative preimage and keep their normal source signatures.

### Creating a parent with child rows

No generated mutation creates a parent together with typed child rows
([#353](https://github.com/Dastari/graphql-orm/issues/353) asks for one). Write a custom root
field that opens `Database::transaction` and inserts every row with `tx.insert`. Entity write
access, row and field policies, hooks and input transforms apply to each row, and an error from
any insert rolls the whole call back: no row is kept and no `Changed` event is released.

```rust,ignore
#[Object]
impl NestedCreate {
    async fn create_owner_with_links(
        &self,
        ctx: &Context<'_>,
        id: String,
        label: String,
        links: Vec<NestedLinkInput>,
    ) -> async_graphql::Result<ProbeOwner> {
        let db = ctx.data::<Database>()?;
        let owner_key = id.clone();
        db.transaction(TransactionMode::Default, move |tx| {
            Box::pin(async move {
                let owner = tx.insert::<ProbeOwner>(CreateProbeOwnerInput { id, label }).await?;
                for link in links {
                    tx.insert::<ProbeLink>(CreateProbeLinkInput {
                        id: link.id,
                        owner_id: owner_key.clone(),
                        target_kind: link.target_kind,
                        target_id: link.target_id,
                    })
                    .await?;
                }
                Ok(owner)
            })
        })
        .await
        .map_err(|error| async_graphql::Error::new(format!("{error}")))
    }
}
// schema_roots! { ..., generated_mutations: "none", extra_mutation_types: [NestedCreate], ... }
```

The host authorizes the caller in a custom root field, as for any hand-written resolver. The
compiled, executed version, including the rollback of a refused second row, is
`a_nested_create_is_one_transaction` in `tests/link_relation_patterns.rs`.

## Field Controls

Fields can be marked read-only or excluded from specific generated surfaces.

```rust
#[graphql_orm(db_column = "CreatedAt", write = false)]
pub created_at: String,

#[graphql_orm(filter = false, order = false)]
pub internal_note: Option<String>,
```

Use these flags for legacy columns, generated columns, audit fields, and database-managed values.

## Policies

Runtime policies let applications enforce access control around generated operations.

- `RowPolicy` constrains read queries, the existing rows generated writes may target, the rows
  they write and the rows their payloads return. See
  [row policy on generated writes](strict-authorization.md#row-policy-on-generated-writes).
- `FieldPolicy` can hide or reject field access.
- `EntityPolicy` can allow or reject entity-level operations.
- `WriteInputTransform` can normalize write inputs before generated SQL runs.

Policy APIs are installed on `Database` and are evaluated by generated code.

```rust
let database = Database::with_row_policy(pool, MyRowPolicy);
```

For new code that also needs schema policy configuration, prefer the builder plus setter methods or compose your own construction helper.

Field-level write denials have a narrower contract than entity or row denials. For optional create
fields, a denied write policy drops that field from the generated insert so database defaults or
`NULL` semantics can apply. For update inputs, a denied field is skipped and the rest of the update
can continue. Required create fields still fail when denied because generated code cannot synthesize
a safe value. Entity-level and row-level write denials remain hard errors.

## Auth Subjects And Resolver Modes

`graphql-orm` understands a small project-agnostic auth shape:

```rust
let subject = AuthSubject {
    id: user.id.to_string(),
    roles: user.roles.clone(),
    scopes: user.scopes.clone(),
    tenant_id: user.tenant_id.clone(),
};

let request = request.data(subject);
```

For compatibility, applications can still attach a `String` user id. `ctx.auth_subject()` first reads
an `AuthSubject`, then upgrades a legacy `String` into a subject with empty roles, scopes, and tenant
id. `ctx.auth_user()` remains as a deprecated alias; use `ctx.auth_user_id()` or
`ctx.auth_subject()` in new code.

Generated resolvers use an explicit auth mode. The default preserves the older fail-closed generated
`ctx.auth_user()?` behavior. Opt out when a schema is intentionally public:

```rust
#[graphql_entity(table = "pages", plural = "Pages", auth = "none")]
pub struct Page {
    // fields...
}

schema_roots! {
    auth: "optional",
    query_custom_ops: [],
    entities: [Record],
}
```

Modes are:

- `required`: require an auth subject before generated resolver database work.
- `optional`: read a subject if present, then let entity, row, and field policies decide.
- `none`: do not read auth in generated resolvers.

Entity-level `auth` overrides the schema-root mode.

For scope-only entity gates, use `ScopeEntityPolicy`:

```rust
let mut database = Database::new(pool);
database.set_entity_policy(ScopeEntityPolicy::new(
    &["records.read"],
    &["records.write"],
));
```

`ScopeEntityPolicy` matches exact scope strings only. With `require_auth: true`, missing auth returns
an unauthenticated GraphQL error. A present subject without the required scope is a normal policy
deny.

PostgreSQL RLS support is defense in depth, not a replacement for GraphQL authorization. Generated
resolvers still enforce the selected auth mode and still evaluate configured entity, row, and field
policies. Keep root field, mutation, and subscription authorization in the GraphQL layer.

When a request carries `DbAuthContext`, generated PostgreSQL resolvers run database work through
transaction-local settings so RLS policies can read the authenticated user, tenant, roles, scopes,
and claims:

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

If `DbAuthContext` is absent, generated resolvers use the same execution paths as before. Relation
preload batching includes the canonical auth context in loader keys so requests with different
database auth contexts do not batch together. PostgreSQL settings are applied with transaction-local
`set_config(..., true)` and are cleared by commit or rollback before pooled connections are reused.

## Hooks

Write-capable backends run mutation hooks around generated creates, updates, deletes, and upserts.

Hooks are intended for application behavior such as audit rows, validation, downstream notifications, and change journals. Keep database schema changes in the schema manager instead of mutation hooks.

One `MutationHook` is installed per `Database`, with `set_mutation_hook`, and receives every entity's
events. `on_mutation` gets the GraphQL context if there is one, a transaction-bound `MutationContext`,
and a `MutationEvent` holding `phase`, `action`, `entity_name`, `id`, `changes` and the typed rows from
`event.before::<T>()` and `event.after::<T>()`.

A generated write runs in this order, after its entity and row policy checks:

1. `WriteInputTransform`, for creates, updates and upserts. Deletes have no transform.
2. The `Before` hook, inside the transaction.
3. The write, then `RowPolicy::can_write_row` on the row as written, for creates, updates and upserts.
4. The `After` hook, inside the transaction.
5. Commit. An error from either hook rolls back instead.
6. Queued `Changed` events.
7. Actions registered with `MutationContext::defer`, in registration order.
8. For generated GraphQL mutations, the entity's `notify_with` handler. Single-row writes call it once;
   bulk updates and deletes call it once per row, after the whole bulk transaction commits.

| Write path | Hooks | GraphQL context in the hook | `Changed` |
| --- | --- | --- | --- |
| Generated GraphQL mutation, including bulk | Per row | Yes | Yes |
| Repository helpers, including `insert_many`, `update_where`, `delete_where`, `delete_all`, `replace_all`, `upsert` and `compare_and_swap` | Per row | No | Yes |
| `Database::transaction` | Per row | No | After commit |
| Through `MutationContext` in a hook | Per row | No | After commit |
| Database foreign-key actions, raw SQL, and any other write outside the ORM | No | | No |

Generated writes run `WriteInputTransform` once per input row. Repository `insert`
uses the transaction helper's single call. Bulk updates pass every locked existing
row to `before_update`, each with a fresh clone of the original input. Different
rows may produce different values; the ORM retains a separate SQL plan per row.
The explicit legacy field-policy opt-out still omits a field for the entire
unbounded bulk operation if any row denies it, without repeating callbacks.
For unbounded bulk updates, all transforms and field checks finish before mutation
`Before` hooks, then
all row writes run, followed by `After` hooks and commit. A refusal rolls back the
whole write, including writes made through the transform's transaction context.
Bounded updates retain their per-row transform/`Before`/write/`After` order.
External side effects cannot be rolled back; use post-commit actions for those.
There is no transform for deletes or for raw pool/SQL writes.

This costs one transform invocation and one retained input/SQL plan per matched
row. Bulk writes already execute by locked primary key, so it adds no update SQL
round trips. The existing 1,000-row bound for GraphQL `notify_with` bulk writes
is unchanged; other bulk writes gain no new bound. `InternalMutationHook` means
a write made while a mutation hook is executing; ordinary transaction helpers
report `Repository`.

Transform invocation audit (N matched/input rows; successful ORM writes using a
Database, not a raw pool). Upserts use `before_upsert` in either branch, not an
additional create/update transform. Before counts are from the pre-0.93 generated
paths; after counts are pinned by `bulk_refusals` and the skill contract suite.

| Write path | Before | After |
| --- | ---: | ---: |
| GraphQL create | 1 | 1 |
| Repository insert | 2 | 1 |
| GraphQL / repository single update | 1 | 1 |
| GraphQL upsert, insert / update branch | 1 / 1 | 1 / 1 |
| Repository upsert, insert / update branch | 1 / 1 | 1 / 1 |
| Repository insert_many | N | N |
| Repository upsert_many, mixed branches | N | N |
| Repository replace_all (inserted rows) | N | N |
| GraphQL / repository bulk update | 1 (first row) | N |
| Repository bounded update | N | N |
| MutationContext insert / single update | 1 / 1 | 1 / 1 |
| MutationContext upsert, insert / update branch | 1 / 1 | 1 / 1 |
| MutationContext bulk update | 1 (first row) | N |
| MutationContext bounded update | N | N |
| Deletes / empty bulk match | 0 | 0 |

Additional source audit: conditional update, compare-and-swap, and
insert-if-absent each retain one transform call per attempt (including attempts
that do not write). Composite-key helpers have no duplicate wrapper/helper call.

A hook sees the event, not the mutation's arguments. Generated mutations have no typed transient input;
[#221](https://github.com/Dastari/graphql-orm/issues/221) tracks that capability.

### After-commit work

`MutationContext::defer` is the after-commit callback. There is no other name for it and no second
API: it takes `FnOnce(Database<B>) -> impl Future<Output = Result<(), E>>` where `E: Display`, and
registers it on the transaction.

```rust
// In a `MutationHook::on_mutation`, or in a `Database::transaction` callback.
// 1. The durable job commits or rolls back with the write.
hook_ctx
    .insert::<Job>(CreateJobInput { kind: "reindex".into(), subject: id.to_string() })
    .await
    .map_err(|error| async_graphql::Error::new(error.to_string()))?;
// 2. After commit, wake whatever processes the job rows.
let wake = worker_wake.clone();
hook_ctx.defer(move |_db| async move {
    wake.notify_one();
    Ok::<(), String>(())
});
```

The job row is the guarantee. The callback is only a hint that a row is ready, so a worker should also
poll for pending jobs on a timer and on startup, which covers a lost wake-up.

Guarantees, each pinned by `crates/graphql-orm/tests/after_commit_defer.rs` on SQLite, PostgreSQL and
MSSQL:

- **Available on every commit path.** Hooks reach it on generated GraphQL create, update, delete,
  upsert, `update_many` and `delete_many`; on repository helpers including `insert_many`,
  `update_where`, `delete_where`, their bounded forms and `delete_all`; and on writes inside
  `Database::transaction`. A transaction callback can also call `tx.defer(...)` directly.
- **Only after a successful commit.** The action runs once per registration and sees committed data:
  a read through the `Database` it receives, taken from the pool rather than the transaction, returns
  the new rows, and a deleted row is gone.
- **Never after a rollback.** A hook refusal (including after the same hook deferred an action), a
  database failure such as a unique-constraint violation, and a callback error all discard the
  registered actions, and so does a failed commit. A bulk write is one transaction, so a refusal or
  failure on a later row discards the actions registered for earlier rows.
- **Failures are contained.** An action that returns `Err` doesn't change the committed result: a
  GraphQL mutation still returns `success: true`, a repository call returns `Ok`, and `transaction`
  returns the callback's value. The error text goes to `PostCommitErrorHandler` through
  `report_post_commit_error`, or to stderr if none is installed. The remaining actions of that commit
  still run.
- **Order.** After the commit: `Changed` events for every row, then every deferred action, then
  `notify_with`. For a bulk GraphQL write that is all rows' events, all rows' actions, then all rows'
  handler calls. Repository helpers and `Database::transaction` have no `notify_with`, so their order is
  events and then actions.
- **Awaited, at most once, not persisted.** Actions run one at a time before the write returns, so a
  slow action delays the response. They are not retried, and nothing records them. The callback isn't
  durable: a crash after commit loses it. That is why the durable job goes in the transaction and the
  callback only wakes the worker.

Don't spawn a delayed task by hand from a hook, such as `tokio::spawn` with a sleep, to approximate
this. A hook runs before the commit and can't know whether the transaction will commit, so no fixed
delay is right: a short one can fire before the commit and find no job row, or after a rollback for a
write that never happened, and a long one adds latency to every write. `defer` runs after the commit
decision and only on commit.

Runtime collections use the separate `RuntimeMutationHook`; see
[runtime mutations](runtime-mutations.md).

## Search Document Maintenance

Entities with `#[graphql_orm(searchable(...))]` or `#[graphql_orm(search_json(...))]` maintain local
search documents through generated ORM writes once the managed search structures exist.

- create/update/upsert refresh the entity document
- delete removes the entity document
- `Entity::search_db(&database, search_input)` builds a search query without exposing the raw pool type
- `Entity::rebuild_search_index(&database)` refreshes all documents for that entity
- `Entity::rebuild_search_document(&database, &id)` refreshes one entity

JSON search paths are extracted from the Rust entity value before the backend-specific search
document is persisted. This keeps Postgres shadow-table/`tsvector` search and SQLite FTS5 search
backend-agnostic for application code. Missing JSON paths, null values, non-string scalars, and empty
wildcard matches contribute empty text rather than failing a write or rebuild.

Writes made outside `graphql-orm` can leave native search structures stale. Run a rebuild after
external imports, manual SQL updates, or relation-data changes that should be reflected in a
denormalized search document.

Search resolvers still enforce entity and row policies before returning results. Snippets/highlights
are intentionally not generated in this pass because denormalized documents can include protected
source fields.

Postgres and SQLite FTS5 search resolvers push native search, ranking, count, limit, and offset into
SQL when the native search structures are available. PostgreSQL requests that carry `DbAuthContext`
still use the native search SQL inside an auth-context transaction, so database RLS policies compose
with indexed search. If native SQLite FTS structures are missing and fallback is enabled, the runtime
can fall back to deterministic Rust scoring; other native search errors are returned instead of being
silently swallowed.

## Pagination

Generated connections use offset-style cursors. Cursors are intentionally simple and are not stable
under concurrent inserts or deletes ahead of the current offset. Native SQL paths request count and
page rows through the same backend pair-fetch API so the result is taken from a consistent execution
context where the backend supports it.

`PageInput` clamps negative offsets to `0`. Generated GraphQL list, search, and relation connection
resolvers use `Database` pagination config to resolve limits:

- default `default_limit`: `Some(1000)`
- default `max_limit`: `Some(1000)`
- omitted `page.limit`: uses `default_limit`
- explicit limits above `max_limit`: clamp to `max_limit`
- explicit negative limits: clamp to `0`

Configure this per runtime handle:

```rust
let database = Database::builder(pool)
    .default_page_limit(Some(250))
    .max_page_limit(Some(5_000))
    .build();
```

For export or sync services that intentionally allow unbounded generated connections:

```rust
let database = Database::builder(pool)
    .unbounded_pagination()
    .build();
```

Repository-style `fetch_all` paths remain intentionally unbounded unless the caller supplies
pagination. The default limit is applied to connection-style APIs, not to every low-level helper.

Host code that handles `PageInput` directly should call `limit_with_config(database.pagination_config())`
or `database.pagination_config().resolve_page(...)`. The compatibility helper `PageInput::limit()` is
deprecated because it only has access to the default `1000` cap and cannot observe per-application
settings such as `.max_page_limit(Some(5_000))`.

When a query includes predicates that must run in Rust, such as SQLite spatial topology checks, the
runtime now pushes the SQL-safe prefix of the filter first and applies only the residual predicate in
memory. The in-memory connection path fetches the filtered candidate set once, counts it, then slices
the requested page.

## Subscriptions

Generated subscriptions are available for write-capable entities when roots include subscription
support. An `ExternalWritable` MSSQL entity participates in the same in-process change-event path;
an external write made outside the ORM is not automatically observed.

The runtime uses request-local event senders attached to `Database`. Subscriptions observe changes emitted by generated write paths.

`#[relation(propagate_change = "up")]` also delivers a `PROPAGATED` event to the parent's `Changed`
subscribers when a child is created, updated or deleted. Delete release differs:

- A direct `DELETED` event reaches a GraphQL subscriber only when the host grants
  `RowPolicy::can_read_deleted_preimage`, which denies by default. See
  [subscription release authorization](entities-and-relations.md#subscription-release-authorization).
- A child delete captures the parent tuple inside the transaction and authorizes against
  the current parent. No child read grant is required; public source identity is null.
  See [subscription release authorization](entities-and-relations.md#subscription-release-authorization).

## Relation Delete Policy

Relations can describe delete behavior for managed schemas and generated writes. Existing external schemas can disable physical foreign-key emission with `emit_fk = false`.

```rust
#[graphql(skip)]
#[relation(target = "Customer", from = "customer_id", to = "CustomerId", emit_fk = false)]
pub customer: Option<Customer>,
```

For external schemas, `emit_fk = false` means the relation is a GraphQL/runtime mapping only. It does not claim ownership of the physical database constraint.

`on_delete` selects a physical foreign-key action: `restrict` (the default), `cascade` or `set_null`.
The database changes the dependent rows, so the ORM doesn't observe those writes. Dependent rows get no
hooks, no `Changed` events, no managed-version advance and no search-document update
([#283](https://github.com/Dastari/graphql-orm/issues/283)). To run dependent hooks and events, delete
the dependents through `MutationContext` in a `Before` hook. Runtime collections can opt in to
ORM-performed dependent changes with `RuntimeMutationRequest::with_dependent_changes`.

## Computed Fields

Use normal `async-graphql` complex object methods for computed fields.

```rust
#[ComplexObject]
impl Job {
    async fn display_name(&self, ctx: &Context<'_>) -> async_graphql::Result<String> {
        let database = ctx.data::<Database<MssqlBackend>>()?;
        Ok(format!("{} ({})", self.name, self.job_id))
    }
}
```

If a computed field needs database access, use request-scoped `DataLoader` or another batching mechanism. Generated relation fields already use the relation batching runtime; custom computed fields should follow the same rule to avoid N+1 behavior.

Transaction count regressions run on memory SQLite and individually owned
PostgreSQL containers (never an ambient application database):

```bash
cargo test -p graphql-orm --locked --no-default-features --features sqlite --test repository_counts
cargo test -p graphql-orm --locked --no-default-features --features postgres --test repository_counts -- --test-threads=1
```
