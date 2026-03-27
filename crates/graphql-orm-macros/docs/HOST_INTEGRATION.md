# Host Integration Contract

## Purpose

`graphql-orm-macros` generates application code against the `graphql-orm` runtime crate. This document describes the current shared contract between those crates so consuming apps only need the runtime surface.

## Dependency Model

Applications should depend on:

- `graphql-orm`

Applications should not depend directly on:

- `graphql-orm-macros`

Generated code is intentionally targeted at `::graphql_orm::*`.

## Runtime Surface Expected By Generated Code

The generated code currently relies on these runtime paths:

- `::graphql_orm::db::Database`
- `::graphql_orm::graphql::auth::AuthExt`
- `::graphql_orm::graphql::filters::*`
- `::graphql_orm::graphql::pagination::*`
- `::graphql_orm::graphql::orm::*`
- `::graphql_orm::graphql::loaders::RelationLoader`

That keeps the app-facing dependency as one crate even though the proc-macros live in a separate package.

## Schema Composition

`schema_roots!` now generates:

- `QueryRoot`
- `MutationRoot`
- `SubscriptionRoot`
- `AppSchema`
- `schema_builder(database)`

`schema_builder(database)` wires the shared runtime data automatically:

- `::graphql_orm::db::Database`
- one `DataLoader<RelationLoader<T>>` per entity

Apps can then attach their own data:

```rust
let schema = schema_builder(database)
    .data(current_user_id)
    .data(app_services)
    .finish();
```

This keeps auth context, policy inputs, and other domain data in the app while hiding the repeated loader wiring.

## Mutation Lifecycle Hooks

Generated create/update/delete mutations use the runtime hook path when configured through `Database`.

Runtime types:

```rust
pub trait MutationHook: Send + Sync {
    fn on_mutation<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a crate::db::Database,
        event: &'a MutationEvent,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<()>>;
}

pub struct MutationEvent {
    pub phase: MutationPhase,
    pub action: ChangeAction,
    pub entity_name: &'static str,
    pub table_name: &'static str,
    pub id: String,
    pub changes: Vec<MutationFieldValue>,
}
```

This is the intended integration point for:

- audit trails
- version snapshots
- generic mutation observers

When the mutation comes from app-side repository helpers rather than GraphQL resolvers, `ctx` is `None`.

It is deliberately runtime-driven and not Digitise-specific.

The older `notify` / `notify_with` macro attribute remains as a compatibility path, but the runtime hook is the primary contract for shared integrations.

## Field Safety And Field Policy

Field-level safety metadata is attached directly to the Rust field.

Supported syntax:

```rust
#[graphql_orm(private)]
pub password_hash: String,

#[graphql_orm(read = false, write = false, filter = false, order = false, subscribe = false)]
pub password_hash: String,

#[graphql_orm(read_policy = "user.email.read")]
pub email: Option<String>,

#[graphql_orm(read_policy = "catalog.valuation.read", write_policy = "catalog.valuation.write")]
pub valuation: Option<String>,
```

`private` keeps the field in:

- the Rust entity
- database persistence
- ORM/runtime metadata

while excluding it from GraphQL schema exposure:

- generated object field exposure
- generated filters
- generated order inputs
- generated subscription field access

The generated Rust `Create<Entity>Input` / `Update<Entity>Input` structs still retain writable private fields as `#[graphql(skip)]` members, so app-side repository code can use the same typed inputs without exposing those fields in GraphQL SDL.

The runtime policy boundary is:

```rust
pub trait FieldPolicy: Send + Sync {
    fn can_read_field<'a>(...) -> BoxFuture<'a, async_graphql::Result<bool>>;
    fn can_write_field<'a>(...) -> BoxFuture<'a, async_graphql::Result<bool>>;
}
```

Applications attach it through `Database::with_field_policy(...)` or `set_field_policy(...)`.

Generated code consults:

- `read_policy` on generated object field reads
- `write_policy` on generated create/update mutation paths

The macro crate only generates the wiring. Policy decisions remain application-owned.

App-side typed repository helpers do not invoke field policy automatically. Host apps are expected to apply policy before calling those helpers.

## App-Side Typed Persistence Helpers

Each generated entity now exposes a typed non-GraphQL persistence surface for host repositories and services:

- `Entity::update_by_id(&database, &id, UpdateEntityInput { .. })`
- `Entity::update_where(&database, EntityWhereInput { .. }, UpdateEntityInput { .. })`
- `Entity::delete_by_id(&database, &id)`
- `Entity::delete_where(&database, EntityWhereInput { .. })`

These helpers:

- reuse the generated Rust input/filter types
- do not require `async_graphql::Context`
- preserve runtime mutation hooks
- emit the same typed entity change events used by subscriptions when a sender is registered on `Database`

Typical setup:

```rust
let database = graphql_orm::db::Database::new(pool);
database.register_event_sender::<UserChangedEvent>(user_events_tx.clone());

let updated = User::update_by_id(
    &database,
    &user_id,
    UpdateUserInput {
        password_hash: Some(new_hash),
        ..Default::default()
    },
).await?;
```

## Naming Rules

Generated field/input/filter/order names now align with serde naming instead of competing with it.

Supported naming inputs:

- Rust field name as the default GraphQL field name
- `serde(rename = "...")`
- `serde(rename_all = "...")`
- explicit `#[graphql(name = "...")]` override when needed

Root query/mutation/subscription resolver names remain PascalCase.

## UUID-First Entities

Generated code treats `uuid::Uuid` as a first-class field type across:

- row decoding
- generated create/update/get/delete operations
- `#[filterable(type = "uuid")]`
- schema metadata
- migration planning

Backend storage is normalized in the runtime:

- SQLite binds UUIDs as text and models them as `TEXT`
- Postgres binds UUIDs natively and models them as `UUID`

This allows apps to keep UUID-backed ids and foreign keys as real `uuid::Uuid` values without hand-written CRUD glue.

## Relation Batching

Argument-aware nested relation batching is now handled through `RelationQueryKey` in the runtime.

Current `RelationQueryKey` groups sibling nested relation requests by:

- relation name
- parent key
- foreign-key column
- normalized `Where` signature
- normalized `OrderBy` signature
- normalized `Page` signature

The loader currently uses grouped fetches plus in-memory per-parent slicing. That closes the common N+1 gap for nested relation queries, including argument-bearing relation selections, while keeping the contract stable for future SQL-level optimizations.

## Direction Of Travel

The runtime still exposes `DbPool` / `DbRow` aliases because the execution layer is SQLx-backed today. The next cleanup target is to push more of that shape behind typed runtime traits so applications depend even less on backend-specific details.

The intended direction is:

- keep generated code anchored to `::graphql_orm::*`
- continue moving dialect and execution concerns behind runtime-owned abstractions
- preserve SQLite/Postgres parity while leaving room for future MySQL/MSSQL backends
