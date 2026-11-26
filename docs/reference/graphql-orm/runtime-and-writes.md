---
title: "Runtime, Writes, And Policies"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-25
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

Use `nested_create` on an unconditional collection relation and opt the child
entity in with `nested_create_keys`. For example, an `Order` can declare this
relation to `Line`:

```rust,ignore
#[relation(
    target = "Line",
    from = ["id", "tenant"],
    to = ["order_id", "tenant"],
    multiple,
    nested_create,
    emit_fk = false
)]
lines: Vec<Line>,

// On Line's entity declaration, alongside its table and other settings:
#[graphql_entity(nested_create_keys = ["order_id", "tenant"])]
```

The generated `createOrder` takes `CreateOrderWithChildrenInput!`. It contains
the ordinary parent fields and an optional `lines: [NestedCreateLineInput!]`.
The child input excludes all bound columns. The standalone `createLine` keeps
its ordinary input. When the relation target is imported from another module,
include its generated nested input type in the import. Under default naming,
a call looks like this:

```graphql
mutation {
  createOrder(input: {
    id: "order-1"
    tenant: "west"
    lines: [{ id: "line-1" }, { id: "line-2" }]
  }) {
    success
  }
}
```

Bind the parent's primary key and every stored tenant or owner field needed to
keep children in the parent's scope. `from` uses the parent's Rust field names.
`to` uses the child's physical column names in the same order.
`nested_create_keys` uses the child's corresponding Rust field names.
Declared INSERT RLS tenant and owner columns must participate, and where both
entities declare a tenant or owner column the bindings must pair them.
The ORM checks that input transforms and database writes preserve these
bindings. The normal row policies still evaluate every written row.

There is a total limit of 50 child rows across the lists in a single create.
Omitted, null and empty lists produce no child writes. Invalid typed input,
a count over the limit, and repeated typed child primary keys for the same
table fail before any row is written. The database also enforces its own
constraints and collation rules inside the transaction.

The ORM prepares the parent and then each child in relation declaration order
and list order. Each row goes through its own generated create authentication,
scopes, assurance coordinate, entity and field authorization and input
transforms. Child policies and hooks see the child's create coordinate and
its inputs, with the bound relation values supplied by the server and the
original request principal and data. When the host enables
`propagate_request_authority_to_hooks`, the parent and child hooks can present
that request to entity and row policies for their own transaction operations.
A field write denial refuses a call with children even if the host permits
legacy omission on a standalone create. The write transaction
runs each row's Before hook, inserts the row, checks the stored image and its
row policy, runs the After hook and queues its Changed events with ordinary
propagation. The parent row goes first, followed by the children in preparation
order. A failure before commit rolls back the writes and drops the queued
events and deferred actions without advancing Changed replay. On success,
the transaction releases Changed
events in queue order, then executes deferred actions in registration order.
Use transactional writes or `MutationContext::defer` for hook side effects.
Input transforms run during preparation and should only prepare or validate input.

The catalogue and the parent mutation's `@requiresScopes` and
`@authenticated` directives describe the parent only, so router prechecks and
host extensions that filter by root field can see weaker requirements than
the ORM enforces for the children at runtime. An opted-in parent can create a
child whose own create root is excluded from the schema.

Nested create requires writable, single-primary-key GraphQL entities with
generated create enabled. Bound fields are non-nullable writable relation
keys. Child bindings must be ordinary public create fields other than the
primary key, and the typed child input needs at least one remaining public
field. Append-only and dependent-delete entities are rejected. Parent upsert
and AI create policies are incompatible with the expanded input.
Principal-bound upsert keys are incompatible with either nested-create
declaration. Participants
use transactional mutation hooks and deferred actions in place of
`notify_with`. External SQL Server tables require the usual
`external_writable` policy. The parent and child must use the same database
backend. A mixed-backend declaration can report a Rust type mismatch on the
generated database or mutation context argument. Nesting creates one level of
children.

A custom root remains available for other transaction shapes. It can open
`Database::transaction` and insert rows with `tx.insert`:

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
// `NestedCreate` carries `#[graphql_orm_root_fields]` before `#[Object]`.
// schema_roots! { ..., generated_mutations: "none", extra_mutation_types: [NestedCreate], ... }
```

The host authorizes the caller in a custom root field, as for any hand-written resolver. The
compiled, executed version, including the rollback of a refused second row, is
`a_nested_create_is_one_transaction` in `tests/link_relation_patterns.rs`.

### Ordering precedence in Rust queries

A generated `<Entity>OrderByInput` has one optional direction per orderable field. One value with
several fields set sorts in the entity's field **declaration** order, not in the order the fields
are written, because a Rust struct literal carries no field order:

```rust,ignore
// `id` is declared before `expires_at`: this is ORDER BY id ASC, expires_at ASC.
query.order_by(&ThingOrderByInput { expires_at: Some(Asc), id: Some(Asc), ..Default::default() })
```

State precedence explicitly instead. Every generated query builder (`EntityQuery`,
`Entity::query(..)`, `tx.query::<T>()`, repository-only and projection builders, repository
search) has `order_by_fields`, which takes `(field, direction)` pairs of the generated
`<Entity>OrderField` enum; the first pair is the primary sort key:

```rust,ignore
use ThingOrderField as Field;
query.order_by_fields([(Field::ExpiresAt, Asc), (Field::Id, Asc)])
// equal to one call per field, which also keeps call order:
query.order_by(&Field::ExpiresAt.order(Asc)).order_by(&Field::Id.order(Asc))
```

Each pair is applied as one `order_by` call, so the same orderable-field and read-policy checks
apply, and it composes with other ordering calls in call order. For keyset pages,
`<Entity>KeysetOrderByInput::ordered([(Field::ExpiresAt, Asc), (Field::Id, Asc)])` builds the
ordered list `keyset_page_ordered` takes; the cursor's key order follows the list and the primary
key is still appended as the tiebreak. `<Entity>OrderField` is a Rust type only. GraphQL clients
already pass a list of single-field order inputs for an ordered sort, and the generated SDL is
unchanged.

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

Only a typed `AuthSubject` authenticates a request. Other context data, including a bare `String`,
is never read as a subject; attach `AuthSubject::new(id)` for a subject with only an id.
`ctx.auth_user()` remains as a deprecated alias; use `ctx.auth_user_id()` or
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
deny. An empty scope list denies that access kind. Admitting any authenticated subject is explicit:
`ScopeEntityPolicy::authenticated()`, or `ScopeRequirement::AnyAuthenticated` through
`with_read` or `with_write`. `ScopeEntityPolicy::optional` denies a request without a subject.

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

A generated write runs in this order, after its entity and row policy checks (the
[generated write pipeline](strict-authorization.md#the-generated-write-pipeline) fixes the order of
those):

1. `WriteInputTransform`, for creates, updates and upserts. Deletes have no transform.
2. The `Before` hook, inside the transaction.
3. The write, then `RowPolicy::can_write_row` on the row as written, for creates, updates and upserts.
4. The `After` hook, inside the transaction.
5. Commit. An error from either hook rolls back instead.
6. Queued `Changed` events.
7. Actions registered with `MutationContext::defer`, in registration order.
8. For generated GraphQL mutations, the entity's `notify_with` handler. Single-row writes call it once;
   bulk updates and deletes call it once per row, after the whole bulk transaction commits.

A hook error reaches a repository or `Database::transaction` caller as an
`OrmPublicError`. It keeps its code and message only when it is a deliberate
public refusal: an error whose `code` extension is an ORM public code, such as
`OrmPublicError::with_message(OrmErrorCode::Conflict, "...").into_graphql_error()`,
or an `OrmPublicError` returned with `?`. Any other hook error, including one
with an application-defined code, reads as `INTERNAL_ERROR` with the default
message, and its text stays a server-side diagnostic. GraphQL clients of
generated mutations see a deliberate public refusal with the same code and
message. They still see a refusal with an application-defined `code` as the
hook wrote it, as does `into_graphql_error()` on the repository error. A
refusal with no `code` extension reads as `INTERNAL_ERROR` with the default
message for GraphQL clients too, unless the schema's `Database` opted in with
`DatabaseBuilder::expose_uncoded_callback_messages(true)`. `INTERNAL_ERROR`
always has the default message, on every surface. Entity and row policy refusals
on repository paths keep their public code in the same way, so an entity write
denial reads as `FORBIDDEN`.

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
is unchanged; generated GraphQL bulk writes also have the
[bulk write bound](#bulk-write-bound). `InternalMutationHook` means
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

A hook sees the event, not the mutation's arguments. A generated upsert can carry one typed
[transient input](#transient-mutation-input) for its transform and hooks. Other generated mutations
have none yet; [#221](https://github.com/Dastari/graphql-orm/issues/221) tracks them.

### Transaction extensions

The code that begins a transaction can attach typed values to it, and the mutation hook reads them
for that transaction's writes. Use this when a hook must admit a write for one reviewed workflow
step that it refuses for every other writer:

```rust,ignore
db.transaction(TransactionMode::StateMachine, |tx| {
    Box::pin(async move {
        tx.insert_extension(ReviewedStep { ticket })?;
        tx.update_by_id::<Ledger>(&id, input).await?;
        Ok(())
    })
})
.await?;

// In MutationHook::on_mutation:
let reviewed = hook_ctx.extension::<ReviewedStep>();
```

- `insert_extension::<T>(value)` stores one value per type and returns the one it replaces;
  `extension::<T>()` borrows it; `remove_extension::<T>()` takes it back.
- The value lives in that `MutationContext`. Every write of the transaction sees it: single-row
  and bulk writes (once per row's hook call) and the nested writes a hook makes.
- It is dropped before the commit and before a rollback. A deferred post-commit action, a queued
  event, another transaction, another `Database` handle or clone, and a generated GraphQL mutation
  never see one. Generated resolvers open their own transaction and attach nothing, and no request
  input can set one.
- Only the transaction's owner sets or removes extensions: inside a mutation hook both calls are
  refused with `CONFLICT`, so a hook cannot grant itself a capability.
- A transaction with no extension allocates nothing for the feature, and hooks that never call
  `extension` behave as before.

To present an authority to **policies** rather than to the hook, see
[repository / system access](strict-authorization.md#repository-system-access).

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
  `report_post_commit_error`, or to a `tracing` error event on the `graphql_orm::post_commit` target if
  none is installed. The remaining actions of that commit
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

## Principal-bound upserts

Use these three opt-in options when exactly one current row belongs to each authenticated caller,
and the caller must not choose which row that is. All three apply to the generated GraphQL upsert
only. Repository and transaction writes are unchanged.

```rust,ignore
pub struct MachineIdentity;

impl PrincipalKeyBinding for MachineIdentity {
    type Key = String;

    fn bind(ctx: &async_graphql::Context<'_>) -> async_graphql::Result<String> {
        // Fails with UNAUTHENTICATED when the request has no verified subject.
        Ok(ctx.auth_subject()?.id)
    }
}

#[derive(async_graphql::InputObject)]
pub struct SnapshotProof {
    pub sequence: i64,
    pub digest: String,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "status_snapshots",
    plural = "StatusSnapshots",
    upsert = "machine_key",
    upsert_bound_key(field = "machine_key", binding = "MachineIdentity"),
    transient_input(type = "SnapshotProof", on = "upsert", name = "proof"),
    upsert_acknowledge = true,
    mutation_shapes = "upsert"
)]
pub struct StatusSnapshot {
    #[primary_key]
    #[sortable]
    pub id: String,
    #[graphql_orm(private)]
    #[unique]
    pub machine_key: String,
    pub sequence: i64,
    pub digest: String,
    pub payload: String,
}
```

The schema gets `upsertStatusSnapshot(input: CreateStatusSnapshotInput!, proof: SnapshotProof!)`.
`CreateStatusSnapshotInput` has no `machineKey` field.

### Principal-bound conflict keys

`upsert_bound_key(field = "...", binding = "path::Type")` names one member of the `upsert` conflict
key and a type that implements `PrincipalKeyBinding`. Repeat it to bind several members; the other
members stay ordinary public input fields.

The generated resolver runs in this order:

1. Generated input validation, resolver authentication, operation scopes and operation assurance.
2. `PrincipalKeyBinding::bind`. An error refuses the mutation here with a fixed `FORBIDDEN`.
   Nothing has been read or written, and no policy, transform or hook has run.
3. The entity write policy, then the transaction.
4. `WriteInputTransform::before_upsert`. The bound key is already in the input, and the resolver
   writes it again afterwards, so a transform cannot replace it.
5. The locking conflict lookup by the bound key, then the
   [exact key check](#keys-the-database-compares-equal) on an existing row.
6. The row write policy on an existing row, the field write policies, the `Before` hook, the
   write, the exact key check and the row write policy on the row as written, the `After` hook and
   the commit, as for [every generated write](#hooks).

Every write authorization of an ordinary generated upsert still applies: the resolver auth mode,
`operation_authorization` scopes, operation assurance, entity, row and field write policies, the
`AuthorizationMode`, and PostgreSQL RLS.

Write `bind` so that it derives the key only from verified request data: the authenticated subject,
or a value your own middleware attached to the request after verifying it. Never read it from an
argument, a header the client controls or an unverified claim. Return an error when the request
carries no such value. `bind` is synchronous and gets no database handle, so resolve anything it
needs before the request executes.

Every binding error reaches the client as the same refusal: code `FORBIDDEN`, message
`the request principal could not be bound` (`PRINCIPAL_KEY_BINDING_REFUSAL`). The error's own code,
message and extensions are discarded, also under
`DatabaseBuilder::expose_uncoded_callback_messages(true)`, and the ORM does not log them. Log a
diagnostic yourself inside `bind` if you need one.

Two principals must never produce the same key. If subject IDs are unique only within a tenant or
an issuer, encode that tenant or issuer into the key, for example `"{issuer}|{subject}"` with a
separator neither part can contain.

#### Keys the database compares equal

The conflict lookup and the unique index use the database's equality, which can treat different
keys as one: SQL Server's default collations ignore case, and so does a SQLite `NOCASE` column.
`Alice` could then reach the row of `alice`.

The generated upsert closes this itself. It compares every stored bound member with the bound
value using the key type's exact Rust equality:

- after the locked lookup, before any policy callback, hook or statement;
- again on the row as written, where it also refuses an insert that landed on a row the lookup did
  not return.

A mismatch is refused with `FORBIDDEN` and the default message, the same refusal as a row write
denial. Nothing is written, and the transaction cannot commit. The three ownership refusals (an
existing row with a different exact key, a written row with a different exact key, and a written
row that cannot be read back by the bound key) are one identical response, including
`extensions.outcome: "refused"` where the host enables outcomes.

**Prerequisite: identity-preserving equality.** Store bound-key columns so that the database's
equality preserves identity: a binary or case- and accent-sensitive collation on SQL Server (for
example `Latin1_General_100_BIN2`), no `NOCASE` on SQLite, and a deterministic collation on
PostgreSQL. With such a column the check never fires, and an upsert tells a caller nothing about
any other principal.

Without it, the check still keeps principals apart, but it cannot hide that a key is taken. A
caller whose key is collation-equal to an occupied key is refused, where an unused key would have
inserted. That difference reveals that some principal holds an equivalent key. The guarantee that
an upsert reveals nothing about other principals therefore holds only on identity-preserving
columns; on any other column the check is a safety net that locks the second principal out.

**PostgreSQL RLS.** If a policy hides the conflicting row from the caller, the lookup returns no
row and the upsert takes its insert branch. PostgreSQL then refuses the `ON CONFLICT DO UPDATE`
because the existing row fails the policy, and the ORM reports its `insufficient_privilege` error
as `FORBIDDEN`. If the database let the statement through, the check on the written row refuses
it. Either way nothing is committed. Keep the bound row visible to its own principal.

#### What stops a client from setting or replacing the key

- **No input.** The key column must be `#[graphql_orm(private)]` with no other surface, so it is
  absent from `Create<Entity>Input`, `Update<Entity>Input`, `WhereInput`, `OrderByInput`, the
  object, `Changed` events, the semantic, resolver and client-metadata catalogues, and the operation
  catalogue's fields, arguments and types. A request that names it fails schema validation. The
  operation catalogue's host-side `schema_signature` records the field as private, as for every
  private field.
- **One value on both branches.** The resolver overwrites the field with the bound value before the
  conflict lookup, so the insert branch stores it and the update branch can only match the caller's
  own row. An upsert never updates its conflict columns.
- **No other generated path.** The generated update and bulk update cannot write the column: their
  GraphQL input does not contain it, and the resolver clears it from the Rust input after
  `WriteInputTransform::before_update`, so a transform cannot assign it either. A generated create
  has no binding, so declaring `upsert_bound_key` on an entity that also generates `create` is a
  compile error: select the mutations you want with `mutation_shapes`, without `create`. Taking
  part in a nested create is refused for the same reason.
- **One writer per column.** The check is per physical column. A second field that maps the same
  column with `db_column`, public or private, is a compile error. So is a bound key on a column
  that generated writes maintain themselves: `created_at`, `updated_at` or a version column.

These are also compile errors: a bound field that is not a member of the conflict key; a nullable
field (a `NULL` key never conflicts); a primary-key, version, `write = false`, JSON, spatial or
read-transformed field; a field bound twice; `upsert_bound_key` without `upsert`, on a
`repository_entity`, or on an entity whose GraphQL upsert is not generated or not selected; and a
binding whose `Key` is not the field's type. `Key` must implement `Clone` and `PartialEq`.

The guarantee covers generated GraphQL surfaces. These remain yours to review:

- Repository and transaction code is trusted and sets the key itself through the Rust
  `Create<Entity>Input` and `Update<Entity>Input`. A repository update transform can still assign
  the column; only generated GraphQL updates clear it.
- A generated update or delete addresses a row by primary key, not by the bound key. Restrict them
  with a row write policy on the private column, or leave them out of `mutation_shapes`.
- Handwritten resolvers, raw SQL and runtime GraphQL collections over the same table are outside
  this check.

### Transient mutation input

`transient_input(type = "path::Type", on = "upsert")` adds one typed argument to the generated
upsert. `type` is any `async_graphql::InputType`. `name` sets the argument name before the schema's
argument case is applied (default `transient`). `required = false` makes the argument nullable.
`on` is required and accepts `upsert` only.

The request value is bounded. `max_bytes` (default 65536, at most 1048576) limits the bytes of its
strings, enum names, binary values and object keys, counting eight per number. `max_nodes`
(default 1024, at most 65536) limits the number of values, list items and object members. A larger
value is refused with `transient input exceeds its size limit` before it is coerced into your type
and before the resolver, the binding or any policy runs. This holds for literals and for variables.
The bound applies to the parsed request, so keep your HTTP body limit as the first line of defence.

A value that does not coerce into your type is refused with `transient input is not a valid value`.
Your type's own coercion error is discarded, because it can quote the value it rejected. An
operation that declares a default value for a variable used in the transient argument is refused
with `INVALID_INPUT`: a default is part of the query text.

The resolver stores the value on the upsert's own transaction before the input transform runs:

```rust,ignore
// In WriteInputTransform::before_upsert_with_context:
let proof = write_ctx.transient_input::<SnapshotProof>();

// In MutationHook::on_mutation, in both phases:
let proof = hook_ctx.transient_input::<SnapshotProof>();
```

- The transform sees it together with the input it shapes. The `Before` hook sees it together with
  `event.before::<T>()`, the existing row read under the write lock, so that is where to compare a
  proof with stored state.
- It is not a column, so it is never stored. It is not part of `MutationEvent`, `Changed` events,
  the change journal, `notify_with` or the mutation result. It is dropped with the transaction,
  before the commit, so deferred post-commit actions do not see it.
- In catalogues it appears only as the upsert's argument and its input type.
- The argument carries the schema directive `@transientInput`, because AI tool arguments are
  retained with the run. The marker covers only the field argument the ORM generates. Arguments of
  executable directives, and arguments you declare yourself on handwritten fields, are not marked
  and not checked:
  - The tool-profile compiler refuses any argument plan that maps such an argument. An upsert with
    a required transient input therefore has no profile; with an optional one, a profile never
    sends it.
  - A loaded manifest is checked again against the owning subgraph's SDL
    (`validate_against_finished_schema`, `AiGraphqlToolManifestSet::aggregate`,
    `register_into_with_finished_schema`), so one written by an older compiler cannot map it.
    `AiGraphqlToolManifest::register_into` and `from_extension_payload` do not see the SDL: a
    manifest that this process did not compile must go through one of the three checks first.
  - The typed mutation capability catalogue fails to compile when a mutation classified for AI
    execution (`ai_mutations(upsert = "automatic" | "approval_required")`) takes one. Leave such
    an upsert prohibited, which is the default.

Where the value can still be seen:

- **Redacted.** The argument is marked `secret`. async-graphql's rendered execution document
  (`ExtensionContext::stringify_execute_doc`, which its `Logger` extension records) shows
  `"<secret>"` in place of the whole value, whether it was sent as a literal or as a variable.
- **Not redacted: the raw request.** `Debug` of an `async_graphql::Request`, its `query` text and
  its `variables`, and anything your HTTP layer logs before the schema runs, still contain the
  value. Do not log raw requests for a schema that accepts transient input.
- **Not redacted: variable defaults in a logging extension.** async-graphql's opt-in `Logger`
  extension, and any extension that prints the rendered document, prints variable definitions with
  their default values verbatim. Only the argument is redacted. The upsert refuses such an
  operation, but the extension has already printed the document by then. Enable these extensions
  only where their output is treated like the raw request.
- **Not redacted: async-graphql's validation errors.** async-graphql validates arguments and
  variable defaults against the schema before the generated argument type runs. Its messages can
  quote the rejected value. An invalid enum value is quoted, including one nested inside an input
  object. So an error for a malformed transient input can echo the caller's own submitted value
  back in the response, and into anything that logs response errors. The ORM does not rewrite
  these messages. Avoid enum fields in a transient input type if that matters to you.
- **Not redacted: your own callbacks.** The ORM never formats the value into an error or a log
  line. An error that your transform or hook builds from it is your text: with a public code it
  reaches the client, and without one it is a server-side diagnostic that
  `GRAPHQL_ORM_LOG_INTERNAL_ERRORS` writes to the log.
- It is kept apart from [transaction extensions](#transaction-extensions): `extension::<T>()` never
  returns a request's transient input, and extensions still cannot be set by request input.

Trusted code that owns a transaction can supply the same value, so one hook serves both paths:

```rust,ignore
db.transaction(TransactionMode::StateMachine, |tx| {
    Box::pin(async move {
        tx.set_transient_input(SnapshotProof { sequence, digest })?;
        tx.upsert::<StatusSnapshot>(input).await?;
        Ok(())
    })
})
.await?;
```

`set_transient_input` and `take_transient_input` are refused with `CONFLICT` inside a mutation
hook. A value set this way stays until it is taken or the transaction ends, and every write of that
transaction sees it, including the nested writes a hook makes. `Entity::upsert(&db, input)` opens
its own transaction and carries none, so a hook that requires one refuses that call.

### Acknowledging a retry without writing

With `upsert_acknowledge = true`, the `Before` hook of the generated GraphQL upsert may call
`hook_ctx.acknowledge_without_write()` on the update branch:

```rust,ignore
let Some(current) = event.before::<StatusSnapshot>()? else { return Ok(()) };
if proof.sequence < current.sequence {
    return Err(OrmPublicError::with_message(OrmErrorCode::Conflict, "stale snapshot").into_graphql_error());
}
if proof.sequence == current.sequence && proof.digest == current.digest {
    hook_ctx.acknowledge_without_write()?; // an idempotent retry
}
```

The upsert then runs no statement, no `After` hook, no row post-image check, no `Changed` event, no
change-journal entry and no `notify_with` call. It commits the transaction, which keeps anything the
hook itself wrote, and returns `success: true`, `action: null` and the existing row, released
through the read policy like any mutation result. A refusal from the hook rolls back as usual, and
a request the hook neither refuses nor acknowledges is written and delivered as before.

`acknowledge_without_write` is refused with `CONFLICT`, which fails the mutation, everywhere else:
on the insert branch, in the `After` phase, in an input transform, in repository and transaction
writes, in other mutations, on an entity without `upsert_acknowledge`, and in the nested writes a
hook makes. The refused call also marks the transaction refused. A hook that ignores the error
cannot let the write through: the commit rolls back and the mutation fails with `FORBIDDEN`. Call
`can_acknowledge_without_write()` first in a hook shared by several paths.

Concurrent upserts for one key are ordered by the transaction: `BEGIN IMMEDIATE` on SQLite,
`SERIALIZABLE` on PostgreSQL and SQL Server, with the lookup locking an existing row. A request that
loses a race is refused by your hook against the row the winner wrote, or by the database: a
serialization failure or deadlock is a retryable `SERVICE_UNAVAILABLE`, and a uniqueness violation
is `CONSTRAINT_VIOLATION`. It never creates a second row.

## Search Document Maintenance

Entities with `#[graphql_orm(searchable(...))]` or `#[graphql_orm(search_json(...))]` maintain local
search documents through generated ORM writes once the managed search structures exist.

- create/update/upsert refresh the entity document
- delete removes the entity document
- `Entity::search_db(&database, search_input)` builds a policy-aware search: entity access, every search source, and row and field policies on each hit (`Entity::search(pool, …)` is the trusted, unchecked builder)
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

- default `default_limit`: `Some(50)` (`PaginationConfig::SECURE_DEFAULT_LIMIT`)
- default `max_limit`: `Some(100)` (`PaginationConfig::SECURE_MAX_LIMIT`)
- omitted `page.limit`: uses `default_limit`
- explicit limits above `max_limit`: clamp to `max_limit`
- explicit negative limits: clamp to `0`

GraphQL clamping is visible to the client: every connection reports the remaining rows through
`pageInfo.hasNextPage` (and `totalCount` when selected), so a client that asked for more than the
maximum can page on. `PaginationConfig::legacy()` restores the former `1000`/`1000` limits.

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

### Bulk write bound

Generated GraphQL `updateMany` and `deleteMany` select and lock their target rows before they
write. `PaginationConfig::max_bulk_write_rows`, default `10_000`
(`PaginationConfig::DEFAULT_MAX_BULK_WRITE_ROWS`), bounds the rows one bulk mutation field may
write:

- **What it counts.** The rows the caller is allowed to write: matches of the filter after
  visibility. A row the caller can neither read nor write is never counted. For a fixed set of
  rows the caller may write, the payload is therefore the same whether or not other, hidden rows
  match the filter.
- **Over the bound.** The write is refused before any row changes. The payload reports
  `success: false` and `errorCode = "PAGE_LIMIT_EXCEEDED"`; the message names no row count. A bulk
  write is never truncated to the bound, and one at or under it returns what an unbounded one
  returns. Target rows are locked and written in primary-key order.
- **How targets are selected.** When no row policy is installed and the filter is rendered
  completely in SQL, every match is a target: one locking read with a look-ahead row decides the
  bound. When the row policy decides in Rust (any installed `RowPolicy`: its callbacks classify
  every candidate, with or without a
  [`write_target_prefilter`](strict-authorization.md#row-policy-on-generated-writes)), or the
  filter has a part evaluated in Rust (SQLite spatial predicates), candidates are read in pages of
  500, the Rust predicate and the row policy are applied per page, and only targets count against
  the bound.
- **Scan budget.** That paged scan reads at most `PaginationConfig::max_scanned_rows` candidates
  (default `10_000`), plus one look-ahead row: the same budget, configuration and refusal as a
  [visible-row read](#repository-limits-and-offsets-count-visible-rows). A scan that would need
  more candidates fails with `PAGE_LIMIT_EXCEEDED` and the scan-budget message, before any row
  changes. Whether the budget runs out can depend on rows the caller cannot see; that is the
  documented residual of the budget, and it is the only way hidden rows can affect a bulk write.
  A `write_target_prefilter`, or without one a SQL predicate from the row
  policy's `read_visibility` (the default; `DatabaseBuilder::narrow_bulk_writes_to_read_visibility(false)`
  turns it off) (see [row policy on generated
  writes](strict-authorization.md#row-policy-on-generated-writes)), removes hidden rows in SQL, so
  they are not counted against the budget. Set the budget in trusted code.
- **Per field.** The bound applies to each bulk mutation field. A request with several aliased
  bulk mutations may write up to the bound in each; there is no request-wide budget. Limit
  aliases, complexity or depth at the schema if a request-wide ceiling is needed.

```rust
let database = Database::builder(pool)
    .max_bulk_write_rows(Some(50_000)) // `None` removes the bound
    .build();
```

`PaginationConfig::with_max_bulk_write_rows` sets it on a config value, and
`PaginationConfig::unbounded()` and `unbounded_pagination()` remove it together with the page
limits and the scan budget. Repository `update_where`, `delete_where` and their transaction forms
are trusted code and are not bounded by this setting; `update_where_bounded` and
`delete_where_bounded` take an explicit ceiling there. The 1,000-row `notify_with` bound is
separate and also counts only the rows the caller may write.

Run the cross-backend regression suite for the bound, the write-target prefilter, bare-pool
inserts, scoped system access, transaction extensions and ordered sorts through
`scripts/run-owned-database-lanes.sh bulk-write-targets`; PostgreSQL and SQL Server use test-owned
disposable containers.

### Bounded set-based deletes

Implement `SetDeleteEntity` on a generated mutable model to use
`tx.delete_keys_set_bounded::<Model>(&keys, limit)` or
`tx.delete_where_set_bounded::<Model>(filter, limit)` inside a
`Database::transaction(TransactionMode::StateMachine, ...)` callback.
In a build with multiple backends, specify the model's backend in the impl
for `SetDeleteEntity<Backend>`.

```rust
impl SetDeleteEntity for WorkItem {}

let limit = MutationLimit::new(100)?;
let outcome = database
    .transaction(TransactionMode::StateMachine, |tx| {
        Box::pin(async move {
            tx.delete_keys_set_bounded::<WorkItem>(&keys, limit)
                .await
                .map_err(OrmPublicError::from)
        })
    })
    .await?;
```

- Pass unique, complete typed primary keys, or a non-empty typed scalar
  filter such as an equality filter. A missing key or multiple keys matching
  one row returns `CONFLICT`. Empty key sets, empty filters, relation
  filters and predicates that require Rust evaluation are refused.
- `LimitExceeded` leaves the transaction usable and runs no DELETE or
  hooks. A filter that matches zero rows returns `Applied { affected: 0 }`
  without a DELETE. Larger key sets that cannot fit in one statement are
  refused. The filter path uses an independent one-row look-ahead and ignores
  repository page limits. If the caller sets a limit above the statement's
  key capacity and that look-ahead finds more matches than fit, the operation
  returns `INVALID_INPUT` and prevents commit. For example, SQL Server allows
  2,000 single-column keys per statement.
- The ORM checks entity write access and calls the row write policy for
  every selected row. Any denied row refuses the batch. The database
  principal must be able to select the entire requested scope. Database RLS
  can hide rows from selection, so a filter operation can only bound and
  authorize the matches visible to that principal. On SQL Server, the
  `UPDLOCK, HOLDLOCK` selection locks the rows read by a scan, including
  rows hidden from the caller. The `mssql_snapshot_bulk_write_targets`
  option does not apply.
- Adopt this API only if the row policy can authorize the batch against the
  pre-delete state. All Before hooks run before the DELETE and all After
  hooks run after it, each in complete primary-key order. Hooks can write
  other rows in the same transaction. Before hooks must leave the targets
  and match set intact. The ORM revalidates them if a hook is configured.
- The DELETE binds the selected primary keys and must affect exactly the
  selected row count. Errors, vetoes and cancellation prevent commit even
  if the caller catches an error. The ordinary generated Changed
  notifications are queued after the After hooks and emitted at commit.
- The runtime refuses append-only and read-only entities with `FORBIDDEN`,
  composite keys without an entity policy with `AUTHORIZATION_MISCONFIGURED`,
  and enabled generated search indexes with `CONFLICT`. Entities with
  ORM-managed dependent deletes lack the generated adapter.
  Opt in only after checking that foreign keys and triggers cannot delete,
  rewrite or redirect other rows as a side effect of deleting targets.
  Database cascades and other database-side effects fall outside the
  target-row count, authorization, hooks and notifications.

Run the regression on all three backends with
`scripts/run-owned-database-lanes.sh bounded-set-delete`.

### Rust repository limits never truncate

Rust repository builders return `Vec`s with no page info, so they never reduce a limit silently.
This applies to `Entity::query(&db)` (`RepositoryQuery`), transaction `tx.query::<T>()` and
`tx.project::<P>()`, projection `P::query(&db)`, `search_db`, write-input `write_ctx.query::<T>()`,
the pool-bound `Entity::query(pool)` (`FindQuery`) and `EntitySearchQuery::limit`:

- `limit(n)` is exact. A limit above `max_limit` fails with `PAGE_LIMIT_EXCEEDED`, and a negative
  limit with `INVALID_INPUT`, when the query runs. `PaginationConfig::checked_explicit_limit`
  applies the same check in host code. The pool-bound builder checks against
  `PaginationConfig::DEFAULT_MAX_LIMIT`, because it cannot see a `Database` configuration.
- Without a limit, `fetch_all` on `RepositoryQuery`, `tx.query::<T>()`, and the projection
  builders reads at most `max_limit` rows and fails with `PAGE_LIMIT_EXCEEDED` when more rows
  match. `default_limit` does not apply to these reads. Repository-only `find_all` and
  `find_many` use this path. `RepositoryQuery::fetch_first` scans at most `max_limit` rows and
  does not fail on more matches; `fetch_optional_one` always reads two rows to prove uniqueness.
- Without a limit, the write-input, pool-bound and search builders stay unbounded, as before.
  GraphQL-enabled `find_all` and `find_many` also stay unbounded.
- `paginate(PageInput)` on `tx.query::<T>()` and `write_ctx.query::<T>()` is shorthand for
  `limit` plus `offset` and is not clamped. `EntityQuery::paginate` (read through
  `fetch_connection*`) and `EntitySearchQuery::paginate` are exact or refused in the same way:
  a `page.limit` above the maximum, or above `EntityQuery::with_max_limit(n)`, fails with
  `PAGE_LIMIT_EXCEEDED`. Generated resolvers apply the GraphQL page semantics above to the
  client's page before they call `paginate`.

So a `limit(bound + 1)` overflow check either sees the extra row or gets an error. When trusted
code needs a bound above the maximum, raise the maximum for that one read with `with_max_limit`.
It is available on `Entity::query(&db)`, `tx.query::<T>()`, `P::query(&db)`, `tx.project::<P>()`,
`write_ctx.query::<T>()` and the pool-bound `Entity::query(pool)`:

```rust
let rows = tx
    .query::<Membership>()
    .filter(filter)
    .with_max_limit(bound + 1)
    .limit(bound + 1)
    .fetch_all()
    .await?;
if rows.len() as i64 > bound {
    return Err(too_many());
}
```

The read stays bounded: a `limit` above the override still fails with `PAGE_LIMIT_EXCEEDED`, and an
unlimited `fetch_all` fails closed above it. The database-wide maximum that GraphQL clients see is
unchanged. Pass only a constant or a value the application has validated, never an unchecked
client value. To raise the maximum for every read on a handle instead, use
`database.clone().with_pagination_config(database.pagination_config().with_max_limit(Some(n)))`.
Or read page by page with `limit` and `offset`.

The fail-closed check of an unlimited `fetch_all` counts matching rows before row policy is applied.
It fails safe: a caller can be refused because of rows it cannot see. Its `PAGE_LIMIT_EXCEEDED`
message therefore names no row count.

### Repository limits and offsets count visible rows

When a row policy is installed and `RowPolicy::read_visibility` returns `CallbackOnly` or
`Prefilter` for the Repository surface, `can_read_row` decides rows in Rust. `Entity::query(&db)`
and `tx.query::<T>()` resolve the decision once and add its `Prefilter` or `Complete` predicate to
the base query, so both the initial SQL window and the scan carry it, and `Complete` keeps one SQL
query. The search executors do not receive the predicate, so `search_db` treats `Complete` like
`Prefilter`. `limit(n)` and `offset(k)` on
`Entity::query(&db)`, `tx.query::<T>()` (`fetch_all`, `fetch_one`) and `search_db` then count
visible rows, and a page is either complete or the read fails:

- The read fetches batches of candidate rows in the query's order, checks each row, skips `k`
  visible rows and returns up to `n`. A short page means no more visible rows match. `fetch_first`
  and `fetch_optional_one` use the same window. `limit(0)` returns an empty page without reading,
  with or without a row policy, after authorization, the read decision and limit validation.
- A primary-key tiebreak is appended to the order unless a term already orders directly by each
  key column. A term that only mentions a key column, such as `CASE WHEN id > 0 THEN 0 END`, does
  not count.
- `PaginationConfig::max_scanned_rows` (default `PaginationConfig::DEFAULT_MAX_SCANNED_ROWS`,
  `10_000`) bounds visible-row scans: the candidate rows a scan takes from the database, plus one
  look-ahead row that shows whether more remain. Skipped rows count. It does not bound, or refuse,
  a read that needs no scan. When the budget runs out before the page is
  complete and more rows remain, the read fails with `PAGE_LIMIT_EXCEEDED`; the message names no
  row count. A zero budget still reads the look-ahead row, so an empty result is not refused.
  Raise the budget for one read with `with_max_scanned_rows(n)`, for a handle with
  `PaginationConfig::with_max_scanned_rows`, or remove it with `None`.
- Candidates are the rows the SQL filter selects. Residual in-memory predicates, such as SQLite
  spatial checks, are evaluated on each candidate, so they do not make the read load every
  candidate. Fallback (Rust-scored) search must rank every candidate, so a scanning fallback search
  reads at most `max_scanned_rows + 1` candidates once, fails with `PAGE_LIMIT_EXCEEDED` when more
  exist, and pages the ranked hits in memory. Native search scans ranked hits in SQL batches.
- A scan covers unlimited reads too. Without a limit, `fetch_all` skips `k` visible rows and
  returns every visible row after them, reading at most `max_scanned_rows + 1` candidates in all;
  it fails with `PAGE_LIMIT_EXCEEDED` if more remain. On `Entity::query(&db)` and
  `tx.query::<T>()` the fail-closed `max_limit` check above still applies, counting the matching
  rows after the skipped ones before row policy, and whichever bound is reached first refuses the
  read. `search_db` has no `max_limit` refusal for unlimited searches: under a residual decision
  only the candidate budget bounds it, and an unlimited search that needs no scan is unbounded, as
  before.
- With an `Unrestricted` decision (or `Complete` outside `search_db`) a read is one SQL query, as
  with no row policy, and the budget does not apply to it; fallback search still loads its
  candidates as before. Only if the row callback refuses a row of a bounded page does the read
  rescan. The rows that first query read count against the rescan's budget, and the rescan is a
  scan as above.

Continuation uses SQL offsets over separate reads. Rows inserted, deleted or re-sorted between
batches can be skipped or returned twice. Inside a transaction, what the batches see depends on
its isolation level; `TransactionMode::StateMachine` raises it. Read inside such a transaction when
a page must be stable.

These reads still depend on rows the caller cannot see. Whether the budget runs out, how long the
read takes, and whether an unlimited read is refused all vary with hidden rows. Set
`max_scanned_rows` and decide whether to read without a limit in trusted code, not from untrusted
input.

Reads without a per-row Rust check keep one SQL `LIMIT`/`OFFSET` query: no row policy is
installed, the decision is `Unrestricted`, or (outside `search_db`) it is `Complete`. If the row callback still refuses a row of such a
bounded page, the read rescans as above, so the page is still complete. Projections refuse
residual decisions, and `write_ctx.query::<T>()` and the pool-bound `Entity::query(pool)` apply no
row policy, so their limits stay in SQL. GraphQL connections and search roots use their own
authorized pagination.

The generated `search_similar(pool, field, query, threshold, filter, limit)` helper scores five
candidates per requested result. The candidate bound is that read's own maximum, so it is not
reduced to the page maximum. `limit` may be at most `SIMILAR_SEARCH_MAX_LIMIT` (`1_000`); a larger
limit fails with `PAGE_LIMIT_EXCEEDED` and a negative one with `INVALID_INPUT`. Without a limit,
every matching row is scored, as before. Bounded `update_where`/`delete_where`, retention
purges and dependent deletes use their own `MutationLimit` look-ahead and are not capped by
`max_limit`. Keyset helpers called from Rust refuse a page size above the maximum with
`PAGE_LIMIT_EXCEEDED`; called with a GraphQL context they reduce it, or refuse it when
`refuse_oversized_page_limit` is set. Either way more rows are reported through
`pageInfo.hasNextPage`.

Host code that receives a client's `PageInput` should resolve it with
`database.client_pagination_config().resolve_client_page(...)` before paginating, so the client
page is reduced, or refused when the host opted in. The compatibility helper `PageInput::limit()` is
deprecated because it only has access to the default `100` cap and cannot observe per-application
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
