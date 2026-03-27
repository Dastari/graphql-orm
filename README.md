# `graphql-orm`

Cargo workspace for the `graphql-orm` runtime crate and the paired `graphql-orm-macros` proc-macro crate.

## Layout

- `crates/graphql-orm` - application-facing runtime crate
- `crates/graphql-orm-macros` - internal proc-macro crate re-exported by `graphql-orm`

Applications should depend on `graphql-orm` and use the re-exported macros from there.

## App Contract

Generated code targets `::graphql_orm::*` directly. Applications should depend only on `graphql-orm`, not on `graphql-orm-macros`.

The shared app-facing contract now includes:

- `schema_roots!` generating `QueryRoot`, `MutationRoot`, `SubscriptionRoot`, `AppSchema`, and `schema_builder(database)`
- `graphql_orm::db::Database` as the runtime handle passed into GraphQL
- `graphql_orm::graphql::orm::MutationHook` and `Database::with_mutation_hook(...)` for audit/versioning integrations
- `graphql_orm::graphql::orm::FieldPolicy` and `Database::with_field_policy(...)` / `set_field_policy(...)` for app-owned field visibility/editability decisions
- generated app-side `update_by_id` / `update_where` / `delete_by_id` / `delete_where` helpers on each entity for non-GraphQL repository code
- UUID-first entity support across generated CRUD, filters, metadata, and migrations

Typical setup:

```rust
use graphql_orm::prelude::*;

schema_roots! {
    query_custom_ops: [],
    entities: [User, Post],
}

let database = graphql_orm::db::Database::new(pool);
let schema = schema_builder(database)
    .data(current_user_id)
    .finish();
```

Apps can still attach extra app-specific data after `schema_builder(...)`. Auth, policy, and domain rules remain app concerns.

## App-Side Repository Usage

Generated entities now expose a typed non-GraphQL persistence surface for host apps.

Typical repository usage:

```rust
let database = graphql_orm::db::Database::new(pool);
database.register_event_sender::<UserChangedEvent>(user_events_tx.clone());

let user = User::update_by_id(
    &database,
    &user_id,
    UpdateUserInput {
        password_hash: Some(new_password_hash),
        disabled: Some(false),
        ..Default::default()
    },
).await?;

let revoked = RefreshToken::delete_where(
    &database,
    RefreshTokenWhereInput {
        family_id: Some(UuidFilter {
            eq: Some(family_id),
            ..Default::default()
        }),
        ..Default::default()
    },
).await?;
```

This is the intended non-GraphQL persistence surface for host applications.
It reuses the generated typed input/filter types and preserves runtime mutation hooks plus entity subscription fanout when a sender is registered on `Database`.

## Field Safety

Field-safety and field-policy metadata live on the field itself.

Baseline private field:

```rust
#[graphql_orm(private)]
pub password_hash: String,
```

Granular controls:

```rust
#[graphql_orm(read = false, write = false, filter = false, order = false, subscribe = false)]
pub password_hash: String,
```

Policy-gated fields:

```rust
#[graphql_orm(read_policy = "user.email.read")]
pub email: Option<String>,

#[graphql_orm(read_policy = "catalog.valuation.read", write_policy = "catalog.valuation.write")]
pub valuation: Option<String>,
```

`private` excludes the field from generated GraphQL object fields, create/update inputs, filters, ordering, and subscription access while keeping it in the Rust struct and ORM persistence model.
Private/write-only fields remain present on the generated Rust input structs, so app-side repository code can still write them without exposing them in the GraphQL schema.

Generated names now align with serde naming:

- `serde(rename = "...")`
- `serde(rename_all = "...")`

Root query/mutation/subscription names remain PascalCase.

## Lifecycle Hooks

Generated create/update/delete mutations and app-side typed update/delete helpers call the runtime mutation hook before and after the database write when a hook is configured.

Use this for cross-cutting behavior such as:

- audit log writes
- version/history capture
- mutation-side event fanout

The hook surface is runtime-driven and app-agnostic:

```rust
let database = graphql_orm::db::Database::with_mutation_hook(pool, my_hook);
```

Hook implementations receive a `MutationEvent` with:

- phase (`Before` / `After`)
- action (`Created` / `Updated` / `Deleted`)
- entity/table name
- entity id as a string
- changed field/value pairs as `SqlValue`

If the mutation originated outside GraphQL, the hook receives `None` for the GraphQL context.

## Field Policy Hook

Applications can attach field-level policy decisions at the runtime boundary:

```rust
let mut database = graphql_orm::db::Database::new(pool);
database.set_field_policy(my_policy);
```

The policy hook is app-owned and receives:

- entity name
- field name
- optional policy key such as `"user.email.read"`
- optional record/value context

Generated read paths consult `read_policy` when present. Generated create/update paths consult `write_policy` when present.

## UUID Support

`uuid::Uuid` fields are supported as first-class generated fields, including:

- primary keys
- filters via `#[filterable(type = "uuid")]`
- generated CRUD operations
- schema metadata and migration planning

Backend behavior:

- SQLite stores UUID-backed fields as `TEXT`
- Postgres stores UUID-backed fields as native `UUID`

## Development

```bash
cargo test -p graphql-orm
```

## Postgres Tests

The live Postgres tests default to:

```text
postgres://postgres:postgres@127.0.0.1:55432/postgres
```

Start the local test database with Docker:

```bash
docker compose up -d postgres-test
```

Then run the Postgres runtime tests:

```bash
cargo test -p graphql-orm --no-default-features --features postgres
```

Or override the database target explicitly:

```bash
TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:55432/postgres cargo test -p graphql-orm --no-default-features --features postgres
```
