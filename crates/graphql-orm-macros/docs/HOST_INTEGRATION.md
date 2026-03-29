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

## Entity Policy Hook

Host apps can attach entity-level read/write capability metadata without the shared crates inventing policy names:

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

Runtime surface:

```rust
pub trait EntityPolicy: Send + Sync {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a crate::db::Database,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>>;
}
```

Applications attach it through `Database::with_entity_policy(...)` or `set_entity_policy(...)`.

Generated surfaces consult it when configured:

- root GraphQL queries use `Read` + `GraphqlQuery`
- relation resolvers use `Read` + `GraphqlRelation`
- subscriptions use `Read` + `GraphqlSubscription`
- GraphQL mutations use `Write` + `GraphqlMutation`
- app-side `&Database` insert/update/delete helpers use `Write` + `Repository`

The shared crates do not generate or interpret application scope vocabulary. The policy key is entirely host-declared.

## Canonical Write Path

Generated entity writes are the intended default path for host-app row persistence.

Use:

- generated GraphQL mutations for GraphQL-facing writes
- generated `insert` / `update_by_id` / `update_where` / `delete_by_id` / `delete_where` helpers for repository/service writes

Attach behavior through:

- row policy for row visibility and existing-row write access
- pre-write input transforms for server-managed fields
- lifecycle hooks for transactional side effects
- deferred post-commit actions for external side effects

The shared contract explicitly favors generated writes plus hooks over bespoke CRUD resolvers.
Frontend semantic naming should prefer GraphQL aliases over backend-specific wrapper mutations.

## Row Policy

Generated row reads can be scoped through:

```rust
let mut database = graphql_orm::db::Database::new(pool);
database.set_row_policy(my_row_policy);
```

Runtime surface:

```rust
pub trait RowPolicy: Send + Sync {
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        surface: graphql_orm::graphql::orm::EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>>;

    fn can_write_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        surface: graphql_orm::graphql::orm::EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>>;
}
```

Current contract:

- generated list queries filter unauthorized rows out of the result set
- generated single-row reads resolve unauthorized rows as not found / `null`
- generated update/delete writes check row-level write access against the existing row and fail if unauthorized

This is the intended mechanism for collection/record/file scope checks and global admin bypasses.

## Pre-Write Input Transform

Server-managed field injection belongs in the generated write path through:

```rust
let mut database = graphql_orm::db::Database::new(pool);
database.set_write_input_transform(my_transform);
```

Runtime surface:

```rust
pub trait WriteInputTransform: Send + Sync {
    fn before_create<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<()>>;

    fn before_update<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        existing_row: Option<&'a (dyn std::any::Any + Send + Sync)>,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<()>>;
}
```

Use this to:

- inject owner/creator/updater actor fields
- default status or visibility
- normalize values
- override or reject forbidden client-supplied values

Generated nullable update fields preserve tri-state intent:

- omitted field: no change
- field with a value: set the new value
- field with `null`: clear the column to SQL `NULL`

Because the transform operates on the generated Rust input structs, private/write-hidden server-owned fields can still be filled in without exposing them in GraphQL mutation documents.

Example:

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

Example CreateCollection-style actor injection:

```rust
impl graphql_orm::graphql::orm::WriteInputTransform for MyTransform {
    fn before_create<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a graphql_orm::db::Database,
        entity_name: &'static str,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if entity_name == "Collection" {
                let actor_id = ctx
                    .and_then(|ctx| ctx.data_opt::<String>())
                    .cloned()
                    .ok_or_else(|| async_graphql::Error::new("missing actor"))?;
                let input = input
                    .downcast_mut::<CreateCollectionInput>()
                    .ok_or_else(|| async_graphql::Error::new("unexpected input type"))?;
                input.owner_user_id = actor_id;
            }
            Ok(())
        })
    }

    fn before_update<'a>(...) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        todo!()
    }
}
```

## Relation Delete Policy

Relations can carry explicit persistence delete semantics:

```rust
#[relation(target = "Collection", from = "collection_id", to = "id", on_delete = "cascade")]
pub collection: Option<Collection>,
```

Supported values:

- `restrict`
- `cascade`
- `set_null`

This metadata is host-declared and generic. The runtime uses it for:

- target schema modeling
- migration planning and DDL generation
- schema introspection diffing on SQLite and Postgres

Backend behavior:

- SQLite emits `FOREIGN KEY (...) REFERENCES ... ON DELETE ...`
- Postgres emits `FOREIGN KEY (...) REFERENCES ... ON DELETE ...`

Use database-native delete policy for straightforward ownership semantics. Keep lifecycle hooks for richer domain cleanup that goes beyond a single foreign-key action.

`set_null` requires the source foreign key field to be nullable; invalid combinations are rejected during macro expansion.

## Schema-Only Entities

When a host app wants migration metadata without turning a struct into a live GraphQL/runtime entity, use `#[derive(GraphQLSchemaEntity)]`:

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

This is intended for shadow metadata structs used in migration planning and staged schema evolution.

Generated in schema-only mode:

- `DatabaseEntity`
- `DatabaseSchema`
- `EntityRelations`
- `Entity`

Not generated in schema-only mode:

- GraphQL object types
- GraphQL input/filter/order types
- CRUD/query/mutation/subscription helpers
- relation resolvers

That makes schema-only entities valid for:

```rust
SchemaStage::from_entities(
    "2026032901",
    "record_versions_fk_policy",
    &[<RecordVersionSchema as Entity>::metadata()],
)
```

without requiring `GraphQLRelations`, `GraphQLOperations`, `SimpleObject`, or `#[graphql(complex)]`.

If preferred, `#[derive(GraphQLEntity)]` also honors `#[graphql_entity(schema_only = true, ...)]` and emits the same metadata-only surface.

## Mutation Lifecycle Hooks

Generated create/update/delete mutations and app-side typed persistence helpers use the runtime hook path when configured through `Database`.

Runtime types:

```rust
pub trait MutationHook: Send + Sync {
    fn on_mutation<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<()>>;
}

pub struct MutationEvent {
    pub phase: MutationPhase,
    pub action: ChangeAction,
    pub entity_name: &'static str,
    pub table_name: &'static str,
    pub metadata: &'static EntityMetadata,
    pub id: String,
    pub changes: Vec<MutationFieldValue>,
    pub before_state: Option<EntityState>,
    pub after_state: Option<EntityState>,
}
```

This is the intended integration point for:

- audit trails
- version snapshots
- generic mutation observers

When the mutation comes from app-side repository helpers rather than GraphQL resolvers, `ctx` is `None`.

It is deliberately runtime-driven and not Digitise-specific.

Entity state semantics:

- create: `before = None`, `after = Some(entity)`
- update: `before = Some(old_entity)`, `after = Some(new_entity)`
- delete: `before = Some(old_entity)`, `after = None`

Typed host access:

```rust
let before = event.before::<Record>()?;
let after = event.after::<Record>()?;
```

Actor-aware hook ergonomics:

```rust
let actor_id = hook_ctx.auth_user(ctx)?;
let actor = hook_ctx.actor::<CurrentActor>(ctx);
```

Bulk behavior:

- `update_where` fires hook events per affected row
- `delete_where` fires hook events per affected row

Current transaction semantics:

- the generated entity write is held until the hook path succeeds
- `After` hooks run before the generated write commits, so returning an error aborts the main persistence change
- hooks receive a transaction-bound `MutationContext`, so related writes can use the same SQLite/Postgres transaction safely
- `hook_ctx.insert::<Entity>(...)`, `hook_ctx.update_by_id::<Entity>(...)`, `hook_ctx.update_where::<Entity>(...)`, `hook_ctx.delete_by_id::<Entity>(...)`, and `hook_ctx.delete_where::<Entity>(...)` are the intended shared paths for transactional side effects
- generated subscription event fanout runs after commit
- hook-authored side-effect writes commit or roll back with the main write

Example versioning shape:

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

Example dependent cleanup shape:

```rust
struct SessionCleanupHook;

impl graphql_orm::graphql::orm::MutationHook for SessionCleanupHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut graphql_orm::graphql::orm::MutationContext<'_>,
        event: &'a graphql_orm::graphql::orm::MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.phase == graphql_orm::graphql::orm::MutationPhase::After
                && event.action == graphql_orm::graphql::orm::ChangeAction::Deleted
                && event.entity_name == "User"
            {
                let before = event.before::<User>()?
                    .ok_or_else(|| async_graphql::Error::new("missing deleted user state"))?;

                hook_ctx
                    .delete_where::<RefreshSession>(RefreshSessionWhereInput {
                        user_id: Some(UuidFilter {
                            eq: Some(before.id),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .await
                    .map_err(|error| async_graphql::Error::new(error.to_string()))?;
            }

            Ok(())
        })
    }
}
```

Example generated create + hook-created dependent row:

```rust
struct CollectionOwnerHook;

impl graphql_orm::graphql::orm::MutationHook for CollectionOwnerHook {
    fn on_mutation<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut graphql_orm::graphql::orm::MutationContext<'_>,
        event: &'a graphql_orm::graphql::orm::MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.phase == graphql_orm::graphql::orm::MutationPhase::After
                && event.action == graphql_orm::graphql::orm::ChangeAction::Created
                && event.entity_name == "Collection"
            {
                let collection = event.after::<Collection>()?
                    .ok_or_else(|| async_graphql::Error::new("missing collection state"))?;
                let actor_id = hook_ctx.auth_user(ctx)?;

                hook_ctx
                    .insert::<CollectionMembership>(CreateCollectionMembershipInput {
                        collection_id: collection.id,
                        user_id: actor_id,
                        role: "CollectionOwner".to_string(),
                    })
                    .await
                    .map_err(|error| async_graphql::Error::new(error.to_string()))?;
            }

            Ok(())
        })
    }
}
```

Example generated update + hook-driven cleanup:

- `UpdateUser(disabled = true)` through the generated mutation or repository helper
- hook inspects `before` / `after`
- hook calls `hook_ctx.update_where::<RefreshSession>(...)` or `hook_ctx.delete_where::<RefreshSession>(...)`

This is the intended model for user disablement, token revocation, and similar dependent-row workflows.

CreateRecord-style server field injection belongs in `WriteInputTransform`, not a bespoke resolver:

- generated `CreateRecord` mutation/repository helper receives the client input
- transform injects `created_by_user_id` and `updated_by_user_id`
- lifecycle hook handles version rows or audit side effects

## Deferred Post-Commit Actions

Hooks can also queue post-commit side effects through `MutationContext`:

```rust
hook_ctx.defer(|db| async move {
    cleanup_deleted_file_from_storage().await?;
    Ok::<(), std::io::Error>(())
});
```

Use this for side effects that should happen only after a successful commit:

- storage cleanup
- webhook dispatch
- background job enqueueing
- notifications

Semantics:

- deferred actions run only after the database transaction commits
- they do not run when the mutation rolls back
- deferred-action failure does not roll back the committed mutation
- failures are reported through `Database::set_post_commit_error_handler(...)`
- if no handler is set, the runtime falls back to stderr logging

Example generated delete + deferred external cleanup:

- `DeleteStoredFile` uses the generated entity delete path
- hook reads the deleted row from `event.before::<StoredFile>()?`
- hook queues storage cleanup through `hook_ctx.defer(...)`
- blob deletion runs only after commit

Frontend aliasing example:

```graphql
mutation CreateCollectionForWorkspace($input: GraphQLCreateCollectionInput!) {
  createWorkspaceCollection: createCollection(input: $input) {
    success
    collection { id name }
  }
}
```

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

## Persisted Column Naming

If a Rust field is renamed semantically but should keep using the same database column, declare that explicitly on the field:

```rust
#[graphql_orm(json, db_column = "metadata_json")]
pub metadata: serde_json::Value,
```

The older bare attribute also remains supported:

```rust
#[db_column = "metadata_json"]
pub metadata: serde_json::Value,
```

This keeps the responsibilities clean:

- Rust and GraphQL use the semantic field name
- persistence and staged migration planning use the declared DB column name

For staged upgrades, this is the safe path for renames like `metadata_json -> metadata` without destructive column churn or SQLite table-rewrite copy failures.

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

## Migration Application Contract

Host apps should continue using:

- `build_migration_plan(...)`
- `MigrationRunner::apply_migrations(...)`

The runtime executor now provides the recovery behavior instead of pushing it into app code.

Migration application guarantees:

- every successful migration version is recorded in `__graphql_orm_migrations`
- repeated `apply_migrations(...)` calls are idempotent for already-applied versions
- each migration runs inside a database transaction on SQLite and Postgres
- failed migrations roll back without recording history
- SQLite startup automatically removes stale internal rewrite tables matching `__graphql_orm_*_new` before replaying pending migrations

The migration history table contains:

- `version`
- `description`
- `applied_at`

This is automatic recovery, not a host-side cleanup chore. If a prior SQLite table rewrite failed after creating `__graphql_orm_<table>_new`, the next migration run will clear that stale internal table and retry the pending migration set cleanly.

## Staged Host-App Migrations

The intended host-facing orchestration model is now staged schema evolution.

Runtime surface:

- `SchemaStage`
- `SchemaStage::from_entities(...)`
- `SchemaStage::from_schema_model(...)`
- `SchemaStageRunner::plan_schema_stages(...)`
- `SchemaStageRunner::apply_schema_stages(...)`

Typical host usage:

```rust
use graphql_orm::graphql::orm::{Entity, SchemaStage, SchemaStageRunner};

let stages = vec![
    SchemaStage::from_entities(
        "2026032801",
        "auth_foundation",
        &[<User as Entity>::metadata(), <RefreshSession as Entity>::metadata()],
    ),
    SchemaStage::from_entities(
        "2026032802",
        "collection_foundation",
        &[
            <User as Entity>::metadata(),
            <RefreshSession as Entity>::metadata(),
            <Collection as Entity>::metadata(),
            <CollectionMembership as Entity>::metadata(),
        ],
    ),
];

database.apply_schema_stages(&stages).await?;
```

Semantics:

- each stage declares a version, description, and target schema snapshot
- the runtime skips stages already present in migration history
- missing stages are planned incrementally in declaration order
- host apps do not need to construct raw SQL or rolling synthetic migration versions
- `build_migration_plan(...)` and `MigrationRunner::apply_migrations(...)` remain the lower-level escape hatches

Internal runtime tables such as `__graphql_orm_migrations` and SQLite rewrite tables are excluded from schema-stage planning so host stages only reason about application tables.

## Naming Rules

Generated field/input/filter/order names now align with serde naming and standard GraphQL casing instead of competing with it.

Supported naming inputs:

- Rust field name as the default GraphQL field name, converted to camelCase
- `serde(rename = "...")`
- `serde(rename_all = "...")`
- explicit `#[graphql(name = "...")]` override when needed

GraphQL naming contract:

- type names remain PascalCase
- object fields are camelCase
- arguments are camelCase
- root query/mutation/subscription fields are camelCase
- wrapper/result payload fields are camelCase

Breaking change:

- generated operation names and payload field names changed from PascalCase to camelCase
- host apps should regenerate frontend codegen output and update handwritten GraphQL documents after upgrading

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

## JSON-Backed Fields

Generated entities now support typed JSON persistence through field metadata:

```rust
#[graphql_orm(json)]
pub content: Content,

#[graphql_orm(json)]
pub tags: Vec<Tag>,

#[graphql_orm(json)]
pub metadata: Option<RecordMetadata>,
```

This is a persistence mapping concern, not a GraphQL-policy shortcut. It composes with existing field controls:

```rust
#[graphql_orm(json, read = false, filter = false, order = false, subscribe = false)]
pub content: Content,
```

Behavior:

- generated insert/get/query/update helpers serialize and deserialize through serde automatically
- app-side repository code uses the real Rust field type instead of `String`
- SQLite stores JSON-backed fields as `TEXT`
- Postgres stores JSON-backed fields as `JSONB`

First-pass limits:

- JSON fields are not filterable or orderable by default
- nested JSON-path query support is not implemented yet
- for generated GraphQL surfaces, the recommended first pass is to keep JSON fields non-readable or private unless the field type already has the GraphQL representation you want

The runtime and macros treat app-side typed inputs as the source of truth. Generated GraphQL mutation inputs are a separate layer, so hidden JSON fields remain usable through app-side `Create<Entity>Input` / `Update<Entity>Input` without forcing GraphQL to expose raw JSON strings.

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
