# Runtime, Writes, And Policies

Generated code targets the `graphql-orm` runtime. The runtime owns database handles, backend traits, filters, pagination, relation loading, write hooks, field policies, row policies, subscriptions, backups, and schema management.

## Database Handle

Use `Database::new(pool)` for compatibility or `Database::builder(pool)` for explicit configuration.

```rust
let database = Database::builder(pool)
    .schema_policy(SchemaPolicy::Managed)
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
- `MssqlPool::connect_ado` configures Tiberius as read-only

For external databases, prefer explicit policy:

```rust
let database = Database::builder(pool)
    .schema_policy(SchemaPolicy::ExternalReadOnly)
    .build();
```

`ExternalReadOnly` rejects entity writes and schema application. `ExternalWritable` can allow entity writes on write-capable backends while still rejecting schema application.

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

## Hooks

Write-capable backends can run mutation hooks around generated creates, updates, deletes, and upserts.

Hooks are intended for application behavior such as audit rows, validation, downstream notifications, and change journals. Keep database schema changes in the schema manager instead of mutation hooks.

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
