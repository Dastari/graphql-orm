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
schema policy.

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

SQLite and Postgres support generated write helpers when the entity and root derive allow them.

MSSQL is read-only:

- no generated mutation roots
- no generated subscription roots
- no generated write helpers
- no migration application
- `Database::<MssqlBackend>::connect_ado` configures Tiberius as read-only

For external databases, prefer explicit policy:

```rust
let database = Database::builder(pool)
    .schema_policy(SchemaPolicy::ExternalReadOnly)
    .build();
```

`ExternalReadOnly` rejects entity writes and schema application. `ExternalWritable` can allow entity writes on write-capable backends while still rejecting schema application.

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

- `RowPolicy` can constrain read queries.
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

Write-capable backends can run mutation hooks around generated creates, updates, deletes, and upserts.

Hooks are intended for application behavior such as audit rows, validation, downstream notifications, and change journals. Keep database schema changes in the schema manager instead of mutation hooks.

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

Generated subscriptions are available for write-capable backends when roots include subscription support. They are not generated for MSSQL because the MSSQL backend is read-only.

The runtime uses request-local event senders attached to `Database`. Subscriptions observe changes emitted by generated write paths.

## Relation Delete Policy

Relations can describe delete behavior for managed schemas and generated writes. Existing external schemas can disable physical foreign-key emission with `emit_fk = false`.

```rust
#[graphql(skip)]
#[relation(target = "Customer", from = "customer_id", to = "CustomerId", emit_fk = false)]
pub customer: Option<Customer>,
```

For external schemas, `emit_fk = false` means the relation is a GraphQL/runtime mapping only. It does not claim ownership of the physical database constraint.

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
