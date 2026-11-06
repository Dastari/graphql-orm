---
name: graphql-orm-usage
description: Decide how to build on graphql-orm and review services that use it. Load when adding or changing an entity, resolver, relation, subscription, policy, hook or write path in a graphql-orm service, or when reviewing one.
title: Using graphql-orm
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-06
review_by: 2027-02-01
supersedes: []
---

# Using graphql-orm

graphql-orm turns one annotated Rust struct into a database table, a typed repository and a
GraphQL API. Hand-written GraphQL habits work against it: a resolver written beside the generated
ones skips the hooks, policies and events that make every write path behave the same way.

This skill says which approach to choose. The linked reference documents say what each feature
does. Examples use `Order`, `OrderLine` and `Customer`; they aren't a required model.

## What one entity generates

For an entity `Order` with `plural = "Orders"`, in the default naming:

| Surface | Generated |
| --- | --- |
| Queries | `orders`, `order`; `ordersKeyset`, `ordersAggregate` and `ordersSearch` when configured |
| Mutations | `createOrder`, `updateOrder`, `deleteOrder`, `updateOrders`, `deleteOrders`; `upsertOrder` when configured |
| Subscription | `orderChanged` |
| Rust | `Order::insert`, `update_by_id`, `delete_by_id`, `find_many`, `count` and more, plus `CreateOrderInput`, `UpdateOrderInput` and `OrderWhereInput` |

The `resolver-case-*`, `argument-case-*` and `field-case-*` features change these names. Check your
own SDL with `schema.sdl()`.

## Decide in this order

Before writing a resolver, work through these and stop at the first that fits:

1. A generated query, mutation or subscription already does it.
2. A declared relation, filter, sort, aggregate or keyset page provides the data.
3. Validation or input shaping belongs in field validators or the write input transform.
4. Side effects and cleanup belong in a mutation hook on the generated operation.
5. Only then write a custom operation, and only for a genuine action.

A **genuine action** is a command with no entity write at its core: restarting a device, starting a
background job, calling an external provider, an auth flow, or anything with no entity behind it.

**Never hand-write a twin of a generated operation.** If `deleteOrder` exists there is no
`removeOrder`. `deleteOrder` fires a hook that does the domain work, so every write path behaves the
same way. A hand-written twin leaves the generated mutation callable, and it skips that logic.

### Adding an entity

First decide whether it belongs in the API at all:

| Declare | When |
| --- | --- |
| `GraphQLEntity` and `GraphQLOperations` | It's part of the GraphQL API |
| `RepositoryEntity` | Trusted Rust needs typed persistence, as with credentials, worker state and internal records. It has no GraphQL surface to secure |
| `GraphQLSchemaEntity` | A crate owns schema metadata only |

Never expose private persistence through GraphQL just to get a repository. A service needs direct
dependencies on `graphql-orm` with one backend feature, `async-graphql`, `serde` and `tokio`. Then, for
a GraphQL entity:

1. Declare the struct. Mark `#[primary_key]`, and at least one `#[sortable]` field: with none, the
   build fails with `…OrderByInput: InputType is not satisfied`.
2. Add `#[filterable]` to fields clients filter on and `#[sortable]` to fields they sort on.
3. Declare relations, not bare `*_id` fields ([Relationships](#relationships)).
4. Decide who may read and write it ([Authorisation](#authorisation-and-generated-surface)).
5. List it in `schema_roots!`, and remove any generated mutations the application doesn't need. In
   another module, glob-import the entity's module too (`use crate::entities::*;`), because the macro
   refers to generated types such as `OrderQueries`.
6. Add validators, a transform or a hook only if the fields need them.
7. Plan a migration with a new version, passing **every** entity, as
   `&graphql_orm_entity_metadata()`. An entity left out is planned as a `DROP TABLE`.

## Worked example

A delete with child cleanup and a deferred external side effect. This is a complete program, and
its assertions run in this repository's tests. [`examples.md`](examples.md) has two more: a write input
transform, and a custom action with row policies.

```rust
use async_graphql::{Request, SimpleObject};
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use graphql_orm::sqlx::Row;
use graphql_orm::uuid::Uuid;
use std::sync::{Arc, Mutex};

// Relation fields need SimpleObject, #[graphql(complex)] and GraphQLRelations.
#[derive(GraphQLEntity, GraphQLRelations, GraphQLOperations, SimpleObject)]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(complex)]
#[graphql_entity(table = "orders", plural = "Orders", default_sort = "reference ASC")]
struct Order {
    #[primary_key]
    id: Uuid,
    #[filterable(type = "string")]
    #[sortable]
    reference: String,
    invoice_path: String,
    #[graphql(skip)]
    #[relation(target = "OrderLine", from = "id", to = "order_id", multiple)]
    lines: Vec<OrderLine>,
}

#[derive(GraphQLEntity, GraphQLRelations, GraphQLOperations, SimpleObject)]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(complex)]
#[graphql_entity(table = "order_lines", plural = "OrderLines", default_sort = "sku ASC")]
struct OrderLine {
    #[primary_key]
    id: Uuid,
    #[filterable(type = "uuid")]
    order_id: Uuid,
    #[sortable]
    sku: String,
    #[graphql(skip)]
    #[relation(target = "Order", from = "order_id", to = "id", propagate_change = "up")]
    order: Option<Order>,
}

// Every relation target needs this impl for now; see #285.
impl BatchLoadEntity for Order {
    fn batch_column() -> &'static str { "id" }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
        row.try_get("id")
    }
}
impl BatchLoadEntity for OrderLine {
    fn batch_column() -> &'static str { "id" }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> graphql_orm::Result<String> {
        row.try_get("id")
    }
}

// Every entity is listed here, so no generated root or relation loader is left out.
schema_roots! {
    backend: "sqlite",
    schema_policy: "managed",
    query_custom_ops: [],
    entities: [Order, OrderLine],
}

/// Stands in for object storage, a network call or anything else a rollback can't undo.
#[derive(Clone, Default)]
struct InvoiceStore(Arc<Mutex<Vec<String>>>);

struct OrderLifecycle {
    invoices: InvoiceStore,
}

impl MutationHook for OrderLifecycle {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            let deleted = event.before::<Order>().ok().flatten();
            match (&event.phase, event.action, deleted) {
                // In the transaction: rolls back with the delete. Deleting through
                // the ORM runs each line's own hooks and queues its Changed event.
                (MutationPhase::Before, ChangeAction::Deleted, Some(order)) => {
                    hook_ctx
                        .delete_where::<OrderLine>(OrderLineWhereInput {
                            order_id: Some(UuidFilter {
                                eq: Some(order.id),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .await
                        .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                }
                // After commit: a rollback can't leave the invoice deleted.
                (MutationPhase::After, ChangeAction::Deleted, Some(order)) => {
                    let path = order.invoice_path.clone();
                    let invoices = self.invoices.clone();
                    hook_ctx.defer(move |_db| async move {
                        invoices.0.lock().unwrap().push(path);
                        Ok::<(), String>(())
                    });
                }
                _ => {}
            }
            Ok(())
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await?
        .with_schema_policy(SchemaPolicy::Managed);
    let invoices = InvoiceStore::default();
    database.set_mutation_hook(OrderLifecycle { invoices: invoices.clone() });

    let plan = database
        .schema()
        .plan_migration_to_entities("001", "orders", &[Order::metadata(), OrderLine::metadata()])
        .await?;
    database.schema().apply_migration(&plan, ApplyOptions::default()).await?;
    let schema = schema_builder(database.clone()).finish();

    let input = CreateOrderInput { reference: "A-1".into(), invoice_path: "invoices/a-1.pdf".into() };
    let order = Order::insert(&database, input).await?;
    let line = CreateOrderLineInput { order_id: order.id, sku: "SKU-1".into() };
    OrderLine::insert(&database, line).await?;

    // The generated mutation does the domain work. No custom resolver is involved.
    let mutation = format!(r#"mutation {{ deleteOrder(id: "{}") {{ success }} }}"#, order.id);
    let response = schema.execute(Request::new(mutation).data(AuthSubject::new("user-1"))).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);

    assert_eq!(OrderLine::count_all(&database).await?, 0);
    assert_eq!(*invoices.0.lock().unwrap(), ["invoices/a-1.pdf"]);
    Ok(())
}
```

## Lifecycle hooks

A generated write runs in this order. Entity and row policy checks come first, and the
[write-path contract](../../docs/reference/graphql-orm/runtime-and-writes.md#hooks) has the detail.

1. **Write input transform** (`WriteInputTransform`, installed with `set_write_input_transform`).
   Changes or rejects the input ([example](examples.md#a-write-input-transform)). It doesn't run for
   deletes.
2. **`Before` hook.** `MutationHook::on_mutation` inside the transaction. An error rolls back.
3. **The write.**
4. **`After` hook.** Still inside the transaction. An error rolls back.
5. **Commit.**
6. **`Changed` events**, then **deferred actions**, registered with `hook_ctx.defer`.
7. **`notify_with`**, for generated GraphQL mutations only. See [Deferred actions](#deferred-actions).

One `MutationHook` is installed per `Database`, with `set_mutation_hook`. It sees every entity, so
check `event.entity_name` first. `event.before::<T>()` returns `Err` for any other entity, so `?` on it
fails their writes; the worked example uses `.ok().flatten()`.

`MutationEvent` carries `phase`, `action`, `entity_name`, `id`, `changes`, and the typed rows from
`event.before::<T>()` and `event.after::<T>()`. A created row has no `before`, a deleted row has no
`after`, and `after` is empty in the `Before` phase.

`MutationContext` (`hook_ctx`) runs typed reads and writes on the same transaction:
`hook_ctx.query::<T>()`, `insert`, `update_by_id`, `update_where`, `delete_by_id`, `delete_where`.
These writes reach your policies with `ctx = None`, so they need a policy that allows them
([In-process writes](#in-process-writes)).

### Which work goes where

| Work | Where | Why |
| --- | --- | --- |
| Normalise or reject input | Field validators, then the transform | Runs before anything is written |
| Reject a delete | `Before` hook | Transforms don't run for deletes |
| Change other rows | Hook, through `hook_ctx` | Commits or rolls back with the write |
| Stop a process, delete files, call a network service | `hook_ctx.defer` | A rollback can't undo it, and slow work in a transaction holds locks |
| Work that must happen | Insert a job row in the hook, run it from a worker, and wake the worker with `hook_ctx.defer` | Survives a crash and can retry |

### Deferred actions

`MutationContext::defer` is the after-commit callback. Use that name; there is no second API for it.
It takes a closure that receives a `Database` and returns a future of `Result<(), E>` where
`E: Display`. It works from a `MutationHook` on every write path (generated GraphQL single-row and
bulk writes, repository helpers, `Database::transaction`), and a `Database::transaction` callback can
call `tx.defer(...)` directly.

The pattern for work that must happen: **write a durable job row in the transaction, and wake a worker
after commit.** The row commits or rolls back with the write, so it can't describe a write that didn't
happen. The callback only tells the worker a row is ready.

```rust,ignore
// Inside `on_mutation` (or a `db.transaction` callback), for the After phase of a create.
hook_ctx
    .insert::<Job>(CreateJobInput { kind: "reindex".into(), subject: id.to_string() })
    .await
    .map_err(|error| async_graphql::Error::new(error.to_string()))?;
let wake = worker_wake.clone(); // for example an `Arc<tokio::sync::Notify>`
hook_ctx.defer(move |_db| async move {
    wake.notify_one();
    Ok::<(), String>(())
});
```

**The callback isn't durable.** It is held in memory and a crash after commit loses it, which is why
the job row is the guarantee and the worker must also look for pending jobs on startup and on a timer.

Behaviour to design around:

- **Order:** commit, then every queued `Changed` event, then every deferred action in registration
  order, then `notify_with` for generated GraphQL mutations (a bulk write: all events, all actions,
  then one handler call per row). Repository helpers and `Database::transaction` have no
  `notify_with`.
- **Never after a rollback.** A hook refusal, a database failure and a callback error discard the
  registered actions. A bulk write is one transaction, so a refusal on a later row discards the
  actions of earlier rows.
- **At most once, not persisted.** No retry, no record that it was registered.
- **Before the response returns, one at a time.** After commit doesn't mean in the background, so long
  work still delays the client. Start it from a job row.
- **Committed data is visible.** A read through the `Database` the closure receives sees the write.
- **A failure doesn't fail the mutation**, because it has committed, and it doesn't stop the other
  actions. The error goes to the `PostCommitErrorHandler` from `set_post_commit_error_handler`, or to
  stderr if none is set.
- The closure gets a `Database`, not the request. Copy what it needs from the event first.
- A call that times out may still have happened. Give external calls an idempotency key, and
  reconcile an uncertain result rather than retrying it as if it never ran.
- **Don't spawn a delayed task by hand** (`tokio::spawn` plus a sleep) to approximate this. The hook
  can't know whether the transaction will commit, so any delay is racy: it can fire before the commit,
  or after a rollback.

### Which write paths run hooks

| Path | Hooks | `ctx` in the hook | `Changed` |
| --- | --- | --- | --- |
| Generated GraphQL mutation, including bulk | Yes, per row | `Some` | Yes |
| Repository helper: `insert`, `insert_many`, `update_by_id`, `update_where`, `delete_by_id`, `delete_where`, `delete_all`, `replace_all`, `upsert`, `compare_and_swap` | Yes, per row | `None` | Yes |
| `Database::transaction` methods | Yes | `None` | Yes, after commit |
| A write made through `hook_ctx` | Yes, so guard against recursion | `None` | Yes, after commit |
| Database `on_delete` action | **No** | | **No** |
| Raw SQL, or any write outside the ORM | **No** | | **No** |

A hook that reads the request must handle `ctx` being `None`, including for a repository call made by
a custom resolver. Runtime collections use the separate
[`RuntimeMutationHook`](../../docs/reference/graphql-orm/runtime-mutations.md).

### Races and workflows

- When a decision reads and then writes, and another writer could race it, use
  `database.transaction(TransactionMode::StateMachine, |tx| …)`. A caller-owned `Default` transaction
  refuses an upsert with `CONFLICT`. Transactions don't nest.
- On an error where `is_retryable()` is true, rerun the whole callback, never only its last statement.
- Never claim work with a read followed by an unconditional update. Use `compare_and_swap`, or a
  conditional update in one statement. A stale worker must fail at the database, so worker claims check
  the owner, attempt, fencing token, deadline and version together; see
  [fenced leases](../../docs/architecture/schema-modules-and-leases.md#fenced-lease-transitions).

### Transform invocation contract

- Repository insert runs the transform once, inside its transaction.
- Bulk updates run `before_update` once per matched row, with that existing row
  and a fresh clone of the original input. Row-dependent outputs are supported.
- A refusal on any row rolls back the entire write. Bulk GraphQL refusals return
  `success: false`, the authored `error`, and `errorCode`.
- Unbounded bulk transforms finish before mutation `Before` hooks; bounded
  updates retain their per-row hook order. Use post-commit actions for external side effects. Ordinary transaction writes report
  `Repository`; only writes executing inside a hook report `InternalMutationHook`.

### Options a hook can't receive

Hooks see the event, not the mutation's arguments, and a generated mutation has no slot for extra
inputs. `deleteOrder(keepInvoice: true)` can't be expressed. This is a gap, tracked in
[#221](https://github.com/Dastari/graphql-orm/issues/221).

- If the option is a durable fact about the row, store it in a field. Set `retain_invoice` with
  `updateOrder`, and have the hook read it from `event.before::<Order>()`.
- If it's request-wide, such as the actor or a correlation ID, put it in the request data and read it
  with `ctx.data_opt::<T>()` or `hook_ctx.actor::<T>(ctx)`, allowing for `None`.
- Otherwise, say that the gap blocks you. Don't parse the query AST in a hook, and don't add a
  hand-written twin that takes the option.

## Relationships

Generated to-one relation fields are nullable: row denial or SQL visibility
exclusion returns null like an absent target; collections exclude hidden rows.
Entity and relation-field `read_policy` denials remain explicit errors even for
absent targets, and policy failures stay errors. Do not wrap a nullable relation
as non-null unless denied and absent targets have identical unavailability.
Changed snapshots reject nested relations; refresh an authorized query.

Declare relations rather than leaving bare `*_id` fields. A relation gives you a typed GraphQL field,
batched loading, a foreign key in managed schemas, and optional propagation. Declare the inverse when
clients walk both ways.

A relation is a `#[graphql(skip)]` field with a `#[relation(...)]` attribute, as in the worked example.
`target` names the entity, `from` is a Rust field, and `to` is a physical column. Add `multiple` for
a to-many relation, with a `Vec<T>` field. The relation field isn't a column.

- An entity with relation fields needs `GraphQLRelations`, `SimpleObject` and `#[graphql(complex)]`,
  and every target needs a `BatchLoadEntity` impl
  ([#285](https://github.com/Dastari/graphql-orm/issues/285)). The worked example shows both.
- Add `filter` to an unconditional public `multiple` relation to expose a typed list of target
  Where alternatives on the source Where input: `#[relation(target = "OrderLine", from = "id", to = "order_id", multiple, filter)]`.
  Alternatives use authorized EXISTS before paging and counts; an empty list matches nothing.
  Each target and link needs complete SQL read visibility. Related filters are read-only:
  bulk updates/deletes, including bounded and `MutationContext` writes, return `invalid_input`.
  Restart filtered keyset pagination after changing the filter.
- Don't hand-load related rows in resolvers, and don't write loaders to avoid N+1 queries. Generated
  relation fields already batch through `DataLoader`, per layer, and to-many fields take `where`,
  `orderBy` and `page`. See [nested relation batching](../../docs/reference/graphql-orm/entities-and-relations.md#nested-relation-batching).
- Don't use JSON or delimited text where a relation fits. You lose filtering, batching, foreign keys
  and propagation.

### Delete policies

`on_delete` is a **physical foreign-key action**. The database changes the dependent rows, so the
ORM never sees them: no dependent hooks, no dependent `Changed` events, no version bump and no search
update ([#283](https://github.com/Dastari/graphql-orm/issues/283)).

| Policy | Effect | Use when |
| --- | --- | --- |
| None (`restrict`) | The database refuses to delete a parent with children | Children must be handled deliberately |
| `on_delete = "cascade"` | Dependents vanish silently | Children have no hooks, no external state and no live viewers |
| `on_delete = "set_null"` | The foreign key is cleared silently; needs an `Option` field | As above |
| Delete in a `Before` hook | Each child runs its hooks and queues its events | Children have side effects or live viewers |

Prefer a declared policy over manual cleanup when nothing needs to observe the children. Use a hook
when something does. With `restrict`, clean up in `Before`, because the parent's delete would fail
first if you waited for `After`.

## Subscriptions and propagation

- **Use `<entity>Changed`** rather than hand-written signal subscriptions. It rechecks authorization
  for each event and reports lost continuity.
- **Set `propagate_change = "up"`** on relations whose parent views show child state, rather than
  computing parent invalidations in hooks or keeping dependency tables in the frontend. The parent's
  stream gets a `PROPAGATED` event with `sourceEntity`, `sourceId` and `path`, limited to 16 hops and
  256 targets per change.
- **Write through the ORM.** Only ORM writes emit events.
- **Narrow with `key` or `keys`** to the records on screen. `keys` accepts up to
  `Database::subscription_key_limit()`, 500 by default. A longer list fails with `INVALID_INPUT`. To
  change the set, open the new subscription, then close the old one, and refetch the added records.
- **Payloads hold persisted scalars only.** Selecting a relation or computed field fails with
  `INVALID_INPUT`. Use the event to decide what to refetch.
- **Lists still need a snapshot read.** An event says a row changed. It can't tell you whether that
  row now belongs in a filtered or sorted list, so refetch the list.

**Deletes need extra work**, and they catch projects out:

- `DELETED` doesn't reach GraphQL subscribers unless the host grants
  `RowPolicy::can_read_deleted_preimage`, which denies by default.
- Child deletes with `propagate_change = "up"` notify current readable parents, even
  for subscribers unable to read the child. Refresh the parent on `PROPAGATED`/
  `DELETED`; `sourceEntity` and `sourceId` are null to withhold child identity.
  Remove old parent-touch workarounds. Bulk writes emit one event per row/path.


### Continuity

A `Changed` stream isn't endless or lossless. Events are process-local, delivered at most once, never
persisted and never replayed.

| The stream ends with | Meaning |
| --- | --- |
| `SUBSCRIPTION_CONTINUITY_LOST` | The subscriber fell behind the channel, which holds 256 events per entity type by default |
| `SUBSCRIPTION_SOURCE_CLOSED` | The channel closed |

After either, **subscribe again first, then refetch**. Refetching first misses a write that lands
before the new subscription starts. Add jitter, because one bulk write can end every subscriber of a
type at once. See [delivery and continuity](../../docs/reference/graphql-orm/macros-and-attributes.md#delivery-and-continuity).

**Test the path your client really uses.** These errors arrive as a `next` frame with `errors`,
followed by `complete`, not as a protocol `error` frame. Apollo Client 4 (checked at 4.3.5) delivers
them through `next` as `{ error }`, so recovery wired to the `error` callback never runs, and
`errorPolicy: "ignore"` deletes the error entirely. Any client should have a test that feeds it a
continuity error and checks that it resubscribes and refetches.

## Authorisation and generated surface

**Decide access for every entity.** With the defaults, an anonymous request is refused with
`UNAUTHENTICATED`, but **any authenticated subject can call every generated operation** on an entity
with no policy. Nothing separates "open to every user" from "nobody decided"
([#287](https://github.com/Dastari/graphql-orm/issues/287)).

| Layer | Declare | Provider | Installed with |
| --- | --- | --- | --- |
| Authentication | `auth` on the entity or in `schema_roots!`: `required` (default), `optional`, `none` | `AuthSubject` in the request data | |
| Entity | `read_policy`, `write_policy` | `EntityPolicy` | `set_entity_policy` |
| Row | The same entity keys | `RowPolicy` | `set_row_policy` |
| Field | `#[graphql_orm(read_policy, write_policy)]` | `FieldPolicy` | `set_field_policy` |
| Operation | `operation_authorization(categories, all_scopes)` | Scopes on `AuthSubject` | |

- Each provider is a single object for the whole `Database`, and a second `set_*` replaces the first.
  It sees every entity, so dispatch on `entity_name` or the policy key. `ScopeEntityPolicy` ignores
  both, so its scopes apply to every entity, including those with no policy key.
- A declared key without its provider fails with `AUTHORIZATION_MISCONFIGURED`. An entity
  `write_policy` needs **both** an entity provider and a row provider for updates and deletes, and
  Federation lookups of an entity with a `read_policy` need both too.
- For sensitive services use
  `with_authorization_mode(AuthorizationMode::ExplicitPolicyForAllExposedOperations)`, which denies
  when no entity provider is installed.
- On an entity with `SimpleObject`, `read_policy` on a scalar field isn't enforced until you add
  `#[graphql_orm::graphql_policy_object]` **before** the derives. Generated objects enforce it already.
- Installing a `RowPolicy` affects every entity. One that only implements row callbacks makes paged
  reads fail closed until you add `with_authorized_scan_config`, or return a complete
  `read_visibility` predicate; see [bounded scans](../../docs/reference/graphql-orm/pagination-migration.md#opt-in-bounded-scans).

See [strict authorization](../../docs/reference/graphql-orm/strict-authorization.md) and
[policies](../../docs/reference/graphql-orm/runtime-and-writes.md#policies).

### Trust boundaries

- **The host owns identity.** graphql-orm validates no credentials. Authenticate in trusted transport
  code and attach a fresh `AuthSubject` per request. Building one authenticates nothing, so never build
  it from client-supplied headers or arguments.
- **Providers aren't isolation.** Neither authorization mode creates tenant isolation or revocation.
  Tenant isolation needs a row policy, and scopes in a token are a snapshot. Sensitive operations need a
  [current-authority check](../../docs/reference/graphql-orm/agql-auth-bridge.md#accepted-request-and-current-authority)
  that fails closed.
- **Metadata grants nothing.** Operation catalogues, fingerprints, directives and
  `authorization = true` describe a contract and enforce none of it. Assurance requirements do nothing
  until `AssuranceEnforcement` is installed, and they supplement authorization rather than replace it.
- **The router doesn't protect joined fields.** Its gate covers root fields. A joined entity is
  authorized only by the subgraph that owns it, so an entity that isn't universally readable needs its
  own policy there.
- **PostgreSQL RLS is defence in depth.** Keep resolver, row and field policy as well.

### In-process writes

`hook_ctx`, repository helpers and workers all reach policies with `ctx = None` and
`EntityAccessSurface::Repository`. The library can't tell them apart, and the GraphQL caller's authority
doesn't carry into a hook's writes ([#291](https://github.com/Dastari/graphql-orm/issues/291)).

- `ScopeEntityPolicy::new` refuses all of them as `UNAUTHENTICATED`, so a hook's cleanup makes the
  mutation fail. A service with writing hooks needs its own `EntityPolicy`.
- `None` proves nothing about the caller, and it isn't system authority. `SystemAccess` and
  `AccessContext` reach only repository field policies today. So the only available decision is whether
  to allow the `Repository` surface, and allowing it means trusting all in-process code. Make that
  decision explicitly, in the policy, with a comment, and make sure client intent can't reach those
  calls unchecked. [The example](examples.md#a-custom-action-with-row-policies) shows one.

### Custom resolvers don't inherit authorization

A generated resolver checks authentication, entity, row and field policy with the request context. A
custom resolver gets none of that, and repository helpers it calls can't see its caller.

Apply the same checks yourself, passing `Some(ctx)`:

1. `enforce_resolver_auth(ctx, None)?` for authentication.
2. `db.ensure_entity_access(Some(ctx), meta.entity_name, meta.write_policy, EntityAccessKind::Write, EntityAccessSurface::GraphqlMutation).await?`
   for the entity, with `meta = Order::metadata()`.
3. Read the row, then authorize it with `db.ensure_writable_row(Some(ctx), …, &order).await?`.

[A custom action with tenant row policies](examples.md#a-custom-action-with-row-policies) is a complete
program.

Annotate custom roots with `#[graphql_orm_custom_operations]` and list them in `described_*`, so they
appear in the operation catalogue.

### Keep the generated surface intentional

| To limit | Use | Notes |
| --- | --- | --- |
| Queries | `query_shapes = "single,list"` | Also `keyset`, `scan`, `search`, `aggregate`. Omit `keyset` for policies always returning `Prefilter` or `CallbackOnly`; use bounded `scan`. Repository APIs are unaffected |
| Queries | `query_shapes = "single,list"` | Also `keyset`, `scan`, `search`, `aggregate`. Repository APIs are unaffected |
| Some of an entity's mutations | `mutation_shapes = "create,update"` | Also `upsert`, `update_many`, `delete`, `delete_many`. Absent from SDL and catalogues; repository writes and `Changed` stay. Narrows within `generated_mutations`, which wins when it removes the entity. `append_only` entities accept only `create`. Refusing a mutation at run time leaves it in the schema; this removes it |
| All of an entity's mutations | `generated_mutations` with `generated_mutation_allowlist` or `generated_mutation_denylist` in `schema_roots!` | Repository writes and `Changed` stay |
| Update and delete | `append_only = true` | |
| All writes | `schema_policy = "external_read_only"` | `Changed` stays in the schema |
| Who can call an operation | `operation_authorization` | The operation stays in the schema |
| A field | `#[graphql_orm(private)]`, or `read`, `write`, `filter`, `order` and `subscribe` set to `false` | On a `SimpleObject` entity, also add `#[graphql(skip)]` or `graphql_policy_object`, or the field stays readable ([#292](https://github.com/Dastari/graphql-orm/issues/292)). `sensitive` is a classification and hides nothing |

There's no per-mutation selector: you can't keep `createOrder` and remove `deleteOrder`
([#286](https://github.com/Dastari/graphql-orm/issues/286)). Until then, guard the unwanted category
with `operation_authorization`, or a write policy that refuses it. Don't replace it with a twin.

## Features projects miss

| Feature | Declare | Use it for |
| --- | --- | --- |
| Managed version | `#[graphql_orm(version = "managed", default = "0")]` on an `i64` | Rows rewritten while users watch them. Every generated write advances it, `updateOrder(expectedVersion:)` refuses a stale write with `CONFLICT`, and `RecordVersion` lets clients order writes. Bare `version` is compare-and-swap only |
| `NodeId` | `#[graphql_entity(node_id)]` | A global ID for refetching one record |
| `compose_complex_object` | `#[graphql_orm(compose_complex_object)]` with `#[graphql_complex_object]` on your impl | Computed fields on an entity that also has relations |
| Aggregates | `aggregate = true`, or `order_aggregate` on a relation to sort by a count | Totals and grouped metrics, instead of loading rows to count them |
| Keyset pages | `keyset = "created_at desc, id asc"` | Deep or append-heavy lists, where offsets drift |
| Search | `#[graphql_orm(search(index = true))]` with `searchable` fields | Full-text search, instead of `LIKE` in a resolver |
| `schema_roots!` | Every entity listed | Composes all generated roots and registers relation loaders and limits. A hand-built `Schema::build` silently drops any root you forget, and relation fields lose batching |
| Indexes | `index = "customer_id,created_at"` | `#[filterable]` creates a single-column index, but `#[sortable]` doesn't. Add composite indexes for filter-and-sort pairs, and an index for each sort on a large table |
| Filter types | `#[filterable(type = "…")]`: `string`, `number`, `int64`, `uuid`, `boolean`, `date`, `decimal`, `spatial` | `int64` for full-range `i64`. `number` selects `IntFilter` |
| Migrations | `plan_migration_to_entities`, then `apply_migration` | Constructing a `Database` changes nothing. Review the plan. `additive_only: true` in `ApplyOptions` applies additive steps, including nullable or defaulted columns, and rejects a plan with anything else, so it suits service startup |
| Backends | One feature per service, or `backend` on each entity | Features are alternatives. SQL Server's schema stays externally owned: no managed migrations, search, RLS, backup or runtime-schema writes. Check [backend support](../../docs/reference/graphql-orm/backends.md) before relying on a capability |
| Tests | Temporary SQLite, or disposable containers your tests own | Never point tests at an application database. Run each backend you ship as its own explicit feature lane |

An entity with generated relation fields serves both too. On a `#[graphql(complex)]` entity,
including `compose_complex_object`, `GraphQLRelations` emits `NodeId` (with `node_id`) and
`RecordVersion` (with a managed version), and `Changed` payloads carry them. A hand-written
`RecordVersion` field on such an entity still works, and can now be removed.

See the [macro reference](../../docs/reference/graphql-orm/macros-and-attributes.md),
[typed aggregates](../../docs/reference/graphql-orm/typed-aggregates.md),
[runtime search](../../docs/reference/graphql-orm/runtime-search.md) and
[schema management](../../docs/reference/graphql-orm/schema-management.md).

## Companion packages

Only for services that use them:

- **AI.** Mutations are prohibited unless `ai_mutations` or `ai_execution` says otherwise, and that
  classification only describes them: discovery never enables execution. Admit exact tools through
  reviewed profiles, keep `automatic` for bounded low-consequence work, and use supervised approval for
  high-impact actions. Resolver authorization still applies.
- **Restore.** Keep workers, subscriptions and callbacks closed after a restore until reconciliation,
  validation and readiness succeed. Reconcile uncertain external effects, and never replay them as new
  work. See [ADR-0005](../../docs/decisions/ADR-0005-restore-readiness-and-uncertain-effects.md). A
  row-level restore can leave a search index `ready` with stale documents
  ([#280](https://github.com/Dastari/graphql-orm/issues/280)).

## Related

- Pin an immutable release: the full commit SHA from its manifest, or its workspace tag with that commit
  recorded. Never pin a branch. Use one source for `agql-auth` across the host and every package, because
  Cargo treats a revision, a tag and a path as different sources, which splits its types.
- [graphql-orm-client](https://github.com/Dastari/graphql-orm-client) ships a
  `graphql-orm-client-server` skill listing what its normalised store needs from a service. This
  skill agrees with it. For a service with that client, follow that skill too, and add
  `federation_key` to every entity the router serves.
- Check every API here against your pinned version. Issue links show what was missing at the release
  this skill came from.

## Anti-patterns

| Wrong | Why it fails | Right |
| --- | --- | --- |
| Hand-written twin of a generated mutation | The generated mutation stays callable and skips the domain logic | Put the logic in a hook on the generated mutation |
| Custom resolver with a generated name | It compiles, the generated resolver wins, and yours never runs. The only signal is a panic from `graphql_orm_semantic_catalog()` ([#282](https://github.com/Dastari/graphql-orm/issues/282)) | Hook or transform. Pick a distinct name only for a genuine action |
| Validation inside a custom resolver | Generated writes and repository writes bypass it | Field validators or the transform |
| Irreversible or slow side effect in a hook | A rollback can't undo it, and it holds locks | `hook_ctx.defer`, or a job row |
| Treating `defer` as background or guaranteed | It delays the response, and a crash loses it | A job row in the transaction for anything that must happen, with `defer` only to wake the worker |
| A hand-spawned delayed task from a hook | Racy: it can run before the commit or after a rollback | `hook_ctx.defer` |
| Manual child cleanup where nothing observes the children | More code, and easy to miss a path | `on_delete` |
| `cascade` where children have hooks or live viewers | Children vanish with no hooks or events | Delete them in a `Before` hook |
| Hand-computed parent invalidation | Duplicates `propagate_change` and drifts as relations change | `propagate_change = "up"`. Child deletes authorize against the current parent (#290) |
| Hand-written `somethingUpdated` signal subscription | Duplicates `Changed` and loses its authorization and continuity guarantees | `<entity>Changed` with `key` or `keys` |
| Writing around the ORM | No events, hooks, version bump or search update | Repository or `hook_ctx` writes |
| Executing GraphQL strings internally | The compiler can't check them | Typed repository calls |
| Entity with no explicit policy | Default access is decided by accident: any authenticated subject | Declare policies. `auth = "none"` is only for data that's deliberately public, and it disables authentication rather than deciding authorization |
| GraphQL entity just to get a repository | Internal rows gain a public surface | `RepositoryEntity` |
| Building identity from client input | Authenticates nothing | Validate in trusted transport, then attach `AuthSubject` |
| Treating catalogues, directives or `authorization = true` as enforcement | They describe a contract and grant nothing | Enforce in resolvers and policies |
| Read then unconditional update for a claim or transition | Two workers both win | `compare_and_swap`, or a `StateMachine` transaction |
| `ScopeEntityPolicy` with hooks that write | Hook writes fail `UNAUTHENTICATED` | Your own `EntityPolicy` |
| Treating a `Changed` stream as endless or lossless | Missed events leave stale state | Handle continuity errors: subscribe again, then refetch |
| Expecting `DELETED` by default | It's suppressed without `can_read_deleted_preimage` | Grant it deliberately, or refetch |
| Hand-written relation loader, `NodeId` or `node` resolver | Duplicates the library and misses its policy checks | Declare the relation, use `node_id`, or report the gap |

## Review checklist

Run this against any graphql-orm service. The commands use `rg` from the service root.

- [ ] **List custom roots and justify each as a genuine action.** They're every non-generated
      `#[Object]` and every entry in `described_*`, `extra_*` and `query_custom_ops`:
      `rg -n -A14 'schema_roots!' src` and `rg -n '#\[(\w+::)*(Object|Subscription)\b|graphql_orm_custom_operations' src`.
      `schema.sdl()` lists everything that's actually exposed.
- [ ] **Look for names that collide with generated ones.** Add a test that calls
      `graphql_orm_semantic_catalog()`: it panics with `ordering is not canonical` on a collision among
      `described_*` roots. Check `extra_*` roots by eye against the table at the top.
- [ ] **Look for side effects that exist only in custom resolvers or services.** Read each custom
      write and ask what happens if the generated mutation is called instead:
      `rg -n '::(insert|update_by_id|update_where|delete_by_id|delete_where|upsert)\(' src`.
- [ ] **Look for writes around the ORM and GraphQL strings run in-process.**
      `rg -n 'sqlx::query|\.pool\(\)|schema\.execute' src --glob '!**/tests/**'`
- [ ] **Look for `*_id` fields without relations.** Compare `rg -n '^\s*(pub(\(\w+\))? )?\w+_id:' src`
      with `rg -n '#\[relation' src`.
- [ ] **Check manual cleanup.** Compare `rg -n 'delete_where|delete_by_id' src` in hooks with the
      `on_delete` declarations. Use a declared policy where nothing observes the children, and check
      that every `cascade` has no hooks or live viewers beneath it.
- [ ] **Look for parent views that need `propagate_change`.** For each screen that shows child state,
      check the relation: `rg -n 'propagate_change' src`.
- [ ] **Confirm every entity has an explicit decision.** `rg -n -A10 'graphql_entity\(' src` and check
      each for `read_policy`, `write_policy` or `auth`. Confirm that each key has a provider:
      `rg -n '(set|with)_(entity|row|field)_policy' src`.
- [ ] **Check exposure.** Each `GraphQLEntity` should be meant for the API. Internal records, credentials
      and worker state belong in `RepositoryEntity`. On `SimpleObject` entities, confirm that `private`
      and `read = false` fields are also skipped, and that fields with a policy have
      `graphql_policy_object`: `rg -n 'graphql_policy_object|SimpleObject' src`.
- [ ] **Check identity.** Find where `AuthSubject` is built, and confirm it comes from a validated
      credential: `rg -n 'AuthSubject' src`.
- [ ] **Decide the `Repository` surface.** Confirm that the policy states what it does when `ctx` is
      `None`, and that hook writes work under it.
- [ ] **Look for races.** Read each workflow or claim for a read followed by a write outside a
      `StateMachine` transaction or `compare_and_swap`.
- [ ] **Confirm custom resolvers authorize.** Each should call `enforce_resolver_auth` and
      `ensure_entity_access` or an equivalent: `rg -n 'ensure_entity_access|ensure_writable_row|enforce_resolver' src`.
- [ ] **Look for generated operations the application doesn't need**, by comparing `schema.sdl()` with
      what clients call, and limit them with `query_shapes` and `generated_mutations`.
- [ ] **Check irreversible work is deferred.** Read each hook for network, file and process calls
      outside `hook_ctx.defer`, and confirm a `PostCommitErrorHandler` is installed.
- [ ] **Confirm clients handle continuity errors on their real delivery path**, with a test that feeds
      in `SUBSCRIPTION_CONTINUITY_LOST` and checks that the client resubscribes and then refetches.
- [ ] **Decide how clients learn of deletes:** `can_read_deleted_preimage`, a refetch, or both.

Generated GraphQL bulk writes invoke configured `notify_with` per row after commit
(maximum 1,000 rows for notified writes). Select `notificationErrors { message code }`
and the committed count; `success: true` means do not retry the write. Single-row
`notify_on_error = "report"` reports via `PostCommitErrorHandler` and returns success;
the default propagates the handler error after commit. Repository/MutationContext
paths have no GraphQL context and retain their existing notification-free behavior.

Host read policies can use `ReadPredicate::column::<Backend, Entity>(physical_name, ReadPredicateOp::Gt, SqlValue::Int(bound))` for validated private persisted columns without exposing public filters. `.not()?` preserves SQL NULL/UNKNOWN as denied; use `exists_any.not()?.or(exists_matching)?` for absent-or-matching relations.

## Exact delegated receiver requests

For private direct delegated HTTP queries, enable `receiver-admission`, register reviewed
`AgqlReceiverContract` values in an immutable `AgqlReceiverRegistry`, and admit the
bounded raw body before obtaining scopes or creating request context. Dispatch
only `AgqlAdmittedReceiverRequest::into_request`. Supply authoritative parent
sessions and current scope/tenant policy; this proof does not establish MFA or
replace row/field authorization. WebSocket is refused. See
[receiver admission](../../docs/reference/receiver-admission.md).
