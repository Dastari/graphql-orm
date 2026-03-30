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
- `graphql_orm::graphql::orm::EntityPolicy` and `Database::with_entity_policy(...)` / `set_entity_policy(...)` for host-declared entity read/write capability checks
- `graphql_orm::graphql::orm::FieldPolicy` and `Database::with_field_policy(...)` / `set_field_policy(...)` for app-owned field visibility/editability decisions
- `graphql_orm::graphql::orm::RowPolicy` and `Database::with_row_policy(...)` / `set_row_policy(...)` for row-level read/write access checks
- `graphql_orm::graphql::orm::WriteInputTransform` and `Database::with_write_input_transform(...)` / `set_write_input_transform(...)` for pre-write server-side field injection and normalization
- generated app-side `update_by_id` / `update_where` / `delete_by_id` / `delete_where` helpers on each entity for non-GraphQL repository code
- UUID-first entity support across generated CRUD, filters, metadata, and migrations
- first-class `#[graphql_orm(json)]` persistence for typed structured fields
- tracked, transaction-wrapped migration application through `MigrationRunner::apply_migrations(...)`
- staged app-schema migration orchestration through `SchemaStage` and `SchemaStageRunner::apply_schema_stages(...)`
- `#[derive(GraphQLSchemaEntity)]` for schema-only migration metadata without GraphQL/runtime code generation

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

Generated subscriptions are operational by default through the `Database` runtime injected by `schema_builder(database)`.
Host apps do not need to register one broadcast sender per entity changed-event type.
If the schema is built without `Database` in schema data, generated subscriptions now fail explicitly instead of returning a silent empty stream.

Generated entity `create` / `update` / `delete` is the canonical write path for host apps.
The intended model is:

- row policy for row visibility/access
- pre-write input transformation for server-managed fields
- generated entity writes for row persistence
- lifecycle hooks for transactional domain side effects
- deferred post-commit hooks for external side effects
- frontend aliases over generated mutations instead of bespoke backend CRUD wrappers

## App-Side Repository Usage

Generated entities now expose a typed non-GraphQL persistence surface for host apps.

Typical repository usage:

```rust
let database = graphql_orm::db::Database::new(pool);

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
It reuses the generated typed input/filter types and preserves runtime mutation hooks plus entity subscription fanout through the runtime-owned event transport on `Database`.
Manual `register_event_sender::<T>(...)` is now optional and only needed if a host wants to override the default transport in tests or custom runtime wiring.

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

Generated names now align with serde naming and standard GraphQL casing:

- `serde(rename = "...")`
- `serde(rename_all = "...")`
- `#[graphql(rename_fields = "...")]` on the entity when GraphQL naming should diverge from serde naming
- default field/input/filter/order names are camelCase even when the Rust field is snake_case

GraphQL naming contract:

- type names stay PascalCase
- object fields are camelCase
- arguments are camelCase
- root query/mutation/subscription fields are camelCase
- wrapper/result payload fields are camelCase

Breaking change:

- generated operation names and payload field names changed from PascalCase to camelCase
- frontend codegen and handwritten GraphQL documents should be regenerated or updated after upgrade

If GraphQL needs a different field convention than serde, set it explicitly on the entity:

```rust
#[derive(GraphQLEntity, GraphQLOperations)]
#[graphql_entity(table = "gate_events", plural = "GateEvents")]
#[graphql(rename_fields = "PascalCase")]
pub struct GateEvent {
    pub gate_name: String,
    pub event_time: i64,
}
```

This applies the chosen GraphQL casing to generated object fields, create/update inputs, filters, and ordering while leaving serde behavior unchanged.

## Generated Create Defaults

Generated create mutations can now populate database-side default expressions for non-writable fields.

Use this when a column should not be client-writable but still needs an explicit insert-time SQL default:

```rust
#[derive(GraphQLEntity, GraphQLOperations)]
#[graphql_entity(table = "system_logs", plural = "SystemLogs")]
pub struct SystemLog {
    #[primary_key]
    pub id: String,

    pub message: String,

    #[graphql_orm(write = false, default = "CURRENT_TIMESTAMP")]
    pub created_at: i64,
}
```

That default is used in both:

- generated schema metadata for migrations
- generated create inserts when the field is excluded from the GraphQL/app write surface

## Row Policy

Row-level visibility and write access can be attached through the runtime:

```rust
let mut database = graphql_orm::db::Database::new(pool);
database.set_row_policy(my_row_policy);
```

Runtime API:

- `graphql_orm::graphql::orm::RowPolicy`
- `Database::with_row_policy(...)`
- `Database::set_row_policy(...)`

Semantics:

- generated list queries filter out rows that fail `can_read_row`
- generated single-row reads resolve unauthorized rows as `null` / not found
- generated update/delete writes check `can_write_row` against the existing row and fail if unauthorized

This is the intended path for scoped entities such as collections, records, files, and media rows.

## Pre-Write Transform

Server-managed field injection and normalization live in the runtime pre-write transform:

```rust
let mut database = graphql_orm::db::Database::new(pool);
database.set_write_input_transform(my_transform);
```

Runtime API:

- `graphql_orm::graphql::orm::WriteInputTransform`
- `Database::with_write_input_transform(...)`
- `Database::set_write_input_transform(...)`

The transform runs:

- before generated create writes
- before generated update writes

Generated nullable update semantics are now tri-state:

- field omitted: leave unchanged
- field present with a value: set the new value
- field present with `null`: clear the column to SQL `NULL`

Use it to:

- inject actor-owned fields
- default status/visibility
- normalize values
- override forbidden client-supplied values

Because the transform runs on the generated Rust input structs, clients do not need to send server-owned fields in GraphQL mutation documents.

Example clear mutation:

```graphql
mutation ClearCollectionCover($id: ID!) {
  clearCollectionCover: updateCollection(
    id: $id
    input: { coverStoredFileId: null }
  ) {
    success
    collection {
      id
      coverStoredFileId
    }
  }
}
```

## Relation Delete Policy

Host apps can declare persistence-level delete behavior directly in relation metadata:

```rust
#[relation(target = "Collection", from = "collection_id", to = "id", on_delete = "cascade")]
pub collection: Option<Collection>,
```

Supported values:

- `restrict`
- `cascade`
- `set_null`

Backend mapping:

- SQLite foreign keys use `ON DELETE ...` on the generated constraint
- Postgres foreign keys use `ON DELETE ...` on the generated constraint

Semantics:

- simple ownership cleanup should prefer database-native foreign key behavior
- richer domain cleanup should still use lifecycle hooks
- `set_null` is only valid when the source foreign key field is nullable

This metadata is used by schema planning, migration execution, and schema introspection, so staged upgrades from one delete policy to another diff cleanly without repeated churn.

## Schema-Only Entities

Host apps can define metadata-only shadow structs for migration planning:

```rust
#[derive(GraphQLSchemaEntity)]
#[graphql_entity(table = "record_versions", plural = "RecordVersions")]
struct RecordVersionSchema {
    #[primary_key]
    pub id: uuid::Uuid,

    pub record_id: uuid::Uuid,
    pub version_number: i64,

    #[graphql(skip)]
    #[relation(
        target = "RecordSchema",
        from = "record_id",
        to = "id",
        on_delete = "cascade"
    )]
    pub record_relation: Option<String>,
}
```

This mode emits only schema/persistence metadata:

- table and column metadata
- db column renames
- indexes and unique metadata
- JSON field metadata
- relation metadata, including `on_delete`

It does not emit:

- GraphQL object types
- GraphQL input types
- CRUD/query helpers
- relation resolvers
- subscriptions

Use it with staged migrations when the live runtime entity should stay unchanged:

```rust
use graphql_orm::graphql::orm::{Entity, SchemaStage};

let stage = SchemaStage::from_entities(
    "2026032901",
    "record_versions_fk_policy",
    &[<RecordVersionSchema as Entity>::metadata()],
);
```

`#[derive(GraphQLEntity)]` also accepts `#[graphql_entity(schema_only = true, ...)]` when you prefer an attribute switch instead of the dedicated derive.

## Lifecycle Hooks

Generated create/update/delete mutations and app-side typed persistence helpers call the runtime lifecycle hook before and after the database write when a hook is configured.

Use this for cross-cutting behavior such as:

- audit log writes
- version/history capture
- mutation-side event fanout
- actor-aware business side effects attached to generated writes

The hook surface is runtime-driven and app-agnostic:

```rust
let database = graphql_orm::db::Database::with_mutation_hook(pool, my_hook);
```

Hook implementations receive a `MutationEvent` with:

- phase (`Before` / `After`)
- action (`Created` / `Updated` / `Deleted`)
- entity/table name
- entity metadata
- entity id as a string
- changed field/value pairs as `SqlValue`
- typed `before::<T>()` / `after::<T>()` access when entity state exists
- JSON snapshots via `before_state.as_json()` / `after_state.as_json()`

Entity state semantics:

- create: `before = None`, `after = Some(entity)`
- update: `before = Some(old_entity)`, `after = Some(new_entity)`
- delete: `before = Some(old_entity)`, `after = None`

Bulk behavior:

- `update_where` fires hooks per affected row
- `delete_where` fires hooks per affected row

Current transaction semantics:

- the main entity write is wrapped so hook failure aborts the generated create/update/delete operation
- `After` hooks run before the generated write is committed
- hook implementations receive a transaction-bound `MutationContext`, so related writes can participate in the same transaction
- hook implementations should use the transaction-bound `MutationContext` for related reads as well, rather than `db.pool()`, so SQLite/Postgres reads stay on the active mutation transaction
- `hook_ctx.insert::<Entity>(...)`, `hook_ctx.update_by_id::<Entity>(...)`, `hook_ctx.update_where::<Entity>(...)`, `hook_ctx.delete_by_id::<Entity>(...)`, and `hook_ctx.delete_where::<Entity>(...)` are the first-class host paths for related transactional writes
- `hook_ctx.query::<Entity>()` and `hook_ctx.find_by_id::<Entity>(&id)` are the first-class host paths for related transactional reads
- generated subscription fanout happens after commit
- side-effect rows queued from hooks commit or roll back with the main entity write on both SQLite and Postgres

If the mutation originated outside GraphQL, the hook receives `None` for the GraphQL context.

Example:

```rust
struct RecordVersionHook;

impl graphql_orm::graphql::orm::MutationHook for RecordVersionHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut graphql_orm::graphql::orm::MutationContext<'_>,
        event: &'a graphql_orm::graphql::orm::MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.phase == graphql_orm::graphql::orm::MutationPhase::After
                && event.action == graphql_orm::graphql::orm::ChangeAction::Updated
                && event.entity_name == "Record"
            {
                let after = event.after::<Record>()?
                    .ok_or_else(|| async_graphql::Error::new("missing record state"))?;
                hook_ctx
                    .insert::<RecordVersion>(CreateRecordVersionInput {
                        record_id: after.id,
                        title_snapshot: after.title.clone(),
                        source_action: "updated".to_string(),
                    })
                    .await
                    .map_err(|error| async_graphql::Error::new(error.to_string()))?;
            }
            Ok(())
        })
    }
}
```

Actor-aware side effects can read request data directly from the hook:

```rust
let actor_id = hook_ctx.auth_user(ctx)?;
let current_actor = hook_ctx.actor::<CurrentActor>(ctx);
```

Hook-driven dependent cleanup can use the same transaction-bound surface:

```rust
struct SessionRevocationHook;

impl graphql_orm::graphql::orm::MutationHook for SessionRevocationHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut graphql_orm::graphql::orm::MutationContext<'_>,
        event: &'a graphql_orm::graphql::orm::MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.phase == graphql_orm::graphql::orm::MutationPhase::After
                && event.action == graphql_orm::graphql::orm::ChangeAction::Updated
                && event.entity_name == "User"
            {
                let before = event.before::<User>()?
                    .ok_or_else(|| async_graphql::Error::new("missing previous user state"))?;
                let after = event.after::<User>()?
                    .ok_or_else(|| async_graphql::Error::new("missing updated user state"))?;

                if !before.disabled && after.disabled {
                    hook_ctx
                        .update_where::<RefreshSession>(
                            RefreshSessionWhereInput {
                                user_id: Some(UuidFilter {
                                    eq: Some(after.id),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            },
                            UpdateRefreshSessionInput {
                                revoked: Some(true),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                }
            }

            Ok(())
        })
    }
}
```

Hook-driven reads should stay on the same `MutationContext` too:

```rust
let session_count = hook_ctx
    .query::<RefreshSession>()
    .filter(RefreshSessionWhereInput {
        user_id: Some(UuidFilter {
            eq: Some(user.id),
            ..Default::default()
        }),
        ..Default::default()
    })
    .count()
    .await?;

let current_user = hook_ctx.find_by_id::<User>(&user.id).await?;
```

Using `db.pool()` inside a hook opens a separate pooled read path and can block against the in-flight mutation transaction on SQLite when the pool is constrained. Prefer `hook_ctx` for both reads and writes.

This is the intended replacement for bespoke CRUD resolvers:

- `CreateCollection`: generated `CreateCollection` mutation or `Collection::insert(...)`, then hook inserts the initial owner membership for the actor
- `DisableUser`: generated `UpdateUser` mutation or `User::update_by_id(...)`, then hook revokes refresh sessions
- ordinary dependent row cleanup should happen in hooks attached to the generated write path, not in handwritten duplicate CRUD flows

## Deferred Post-Commit Actions

`MutationContext` can also queue host-owned post-commit side effects:

- `hook_ctx.defer(|db| async move { ... })`

Deferred actions are intended for work that must not run inside the transaction:

- deleting a file from disk or object storage
- webhook dispatch
- job enqueueing
- notifications that should only happen after a committed write

Semantics:

- deferred actions run only after the main transaction commits successfully
- deferred actions do not run if the mutation rolls back
- deferred action failure does not roll back an already committed mutation
- failures are reported through `Database::set_post_commit_error_handler(...)`
- if no handler is installed, the runtime logs the failure to stderr

Example:

```rust
let mut database = graphql_orm::db::Database::new(pool);
database.set_post_commit_error_handler(my_error_handler);
```

Typical generated delete + deferred cleanup shape:

```rust
if event.phase == graphql_orm::graphql::orm::MutationPhase::After
    && event.action == graphql_orm::graphql::orm::ChangeAction::Deleted
    && event.entity_name == "StoredFile"
{
    let file = event.before::<StoredFile>()?
        .ok_or_else(|| async_graphql::Error::new("missing deleted file state"))?;
    let storage_key = file.storage_key.clone();

    hook_ctx.defer(move |_db| async move {
        delete_blob_from_storage(storage_key).await?;
        Ok::<(), std::io::Error>(())
    });
}
```

Frontend naming should prefer aliases over backend-specific wrappers:

```graphql
mutation CreateCollectionForWorkspace($input: GraphQLCreateCollectionInput!) {
  createWorkspaceCollection: createCollection(input: $input) {
    success
    collection { id name }
  }
}
```

Combined generated CRUD contract:

- `CreateCollection`: generated `createCollection` mutation/repository helper + `WriteInputTransform` injects owner actor field + lifecycle hook inserts owner membership
- `CreateRecord` / `UpdateRecord`: generated create/update + `WriteInputTransform` injects `created_by` / `updated_by`
- scoped list/get queries: generated reads + `RowPolicy`
- `DeleteStoredFile`: generated delete + deferred post-commit storage cleanup

## Field Policy Hook

Entity-level authorization metadata can be attached at the entity surface without inventing scope names in the shared crates:

```rust
#[graphql_entity(
    table = "collections",
    plural = "Collections",
    default_sort = "name ASC",
    read_policy = "collection.read",
    write_policy = "collection.write",
)]
pub struct Collection {
    pub id: uuid::Uuid,
    pub name: String,
}
```

Applications attach the runtime callback through:

```rust
let mut database = graphql_orm::db::Database::new(pool);
database.set_entity_policy(my_entity_policy);
```

The entity policy hook is host-owned and receives:

- entity name
- optional host-declared policy key such as `"collection.read"`
- access kind (`Read` or `Write`)
- access surface (`GraphqlQuery`, `GraphqlMutation`, `GraphqlSubscription`, `GraphqlRelation`, or `Repository`)

Generated root queries, relation reads, subscriptions, GraphQL mutations, and `&Database` app-side write helpers consult this hook when an entity-level policy key is configured.

The runtime does not invent scope names or interpret application scope conventions.

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

## JSON Fields

Typed structured values can be stored directly on entity fields with `#[graphql_orm(json)]`.

Example:

```rust
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Record {
    pub id: uuid::Uuid,
    pub slug: String,

    #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
    pub identity: Identity,

    #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
    pub content: Content,

    #[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
    pub tags: Vec<Tag>,
}
```

Behavior:

- entity fields keep their real Rust types such as structs, `Vec<T>`, maps, and `Option<T>`
- generated insert/get/query/update helpers serialize and deserialize automatically through serde
- host apps do not need to manually stringify or parse JSON for routine persistence
- SQLite stores JSON-backed fields as `TEXT`
- Postgres stores JSON-backed fields as `JSONB`

First-pass limits:

- JSON fields are not filterable or orderable by default
- rich JSON-path query/filter/order support is not implemented yet
- if a JSON field should stay out of generated GraphQL, keep using field controls like `read = false`, `private`, `filter = false`, `order = false`, and `subscribe = false`

## Persisted Column Names

Host apps can keep a legacy database column name while renaming the Rust field to a more semantic name.

Preferred syntax:

```rust
#[graphql_orm(json, db_column = "roles_json")]
pub roles: Vec<String>,
```

The older bare form still works too:

```rust
#[db_column = "roles_json"]
pub roles: Vec<String>,
```

Behavior:

- Rust code uses the semantic field name
- generated GraphQL fields and inputs use the semantic field name
- persistence and migration planning use the declared DB column name
- staged upgrades stay safe because the planner sees the same persisted column instead of a destructive rename

This is the intended way to clean up names like `roles_json -> roles` or `metadata_json -> metadata` without breaking existing databases.

## Migration Application

`MigrationRunner::apply_migrations(...)` is now safe to call repeatedly at startup.

Runtime behavior:

- migrations are tracked in `__graphql_orm_migrations`
- applied versions are skipped on later runs
- each migration is applied transactionally on SQLite and Postgres
- failed migrations roll back instead of leaving partially-applied statements committed
- SQLite automatically drops stale internal rewrite tables matching `__graphql_orm_*_new` before replaying migrations

Migration history table shape:

- `version`
- `description`
- `applied_at`

This keeps host apps out of the runtime's temp-table details. If a SQLite rewrite failed after creating `__graphql_orm_<table>_new`, the next `apply_migrations(...)` run recovers automatically and retries cleanly.

## Staged App Migrations

For host apps, the intended orchestration model is now staged schema evolution instead of wrapping the whole current schema snapshot in one rolling synthetic migration version.

Typical host usage:

```rust
use graphql_orm::graphql::orm::{Entity, SchemaStage, SchemaStageRunner};

let stages = vec![
    SchemaStage::from_entities(
        "2026032801",
        "auth_foundation",
        &[<User as Entity>::metadata(), <UserCredential as Entity>::metadata()],
    ),
    SchemaStage::from_entities(
        "2026032802",
        "collection_foundation",
        &[
            <User as Entity>::metadata(),
            <UserCredential as Entity>::metadata(),
            <Collection as Entity>::metadata(),
            <CollectionMembership as Entity>::metadata(),
        ],
    ),
    SchemaStage::from_entities(
        "2026032803",
        "record_foundation",
        &[
            <User as Entity>::metadata(),
            <UserCredential as Entity>::metadata(),
            <Collection as Entity>::metadata(),
            <CollectionMembership as Entity>::metadata(),
            <Record as Entity>::metadata(),
            <RecordVersion as Entity>::metadata(),
        ],
    ),
];

database.apply_schema_stages(&stages).await?;
```

Behavior:

- stages are defined in terms of entity metadata or owned `SchemaModel` snapshots, not raw SQL
- only missing stages are planned and applied
- plans are computed incrementally from the current database state to each missing stage target
- rerunning startup after stage `N` is already applied is a no-op for stages `<= N`
- the lower-level `build_migration_plan(...)` and `apply_migrations(...)` APIs remain available

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
