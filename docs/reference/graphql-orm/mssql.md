---
title: "Microsoft SQL Server Backend"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-08-10
review_by: 2027-02-01
supersedes: []
---

# Microsoft SQL Server Backend

The `mssql` feature supports existing SQL Server schemas through Tiberius. Compatibility
constructors remain physically read-only. Applications may deliberately opt a separately
configured connection into `ExternalWritable` entity DML; managed migrations, runtime-schema
management, full-text search maintenance, and backup/restore remain unsupported.

SQL Server support uses [`tiberius`](https://crates.io/crates/tiberius). Current SQLx releases do
not provide an MSSQL driver.

## Feature Selection

For a service that only uses SQL Server, select the `mssql` backend feature:

```toml
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.22.1", default-features = false, features = ["mssql"] }
```

When exactly one of `sqlite`, `postgres`, or `mssql` is enabled, the legacy implicit backend remains
available. Existing derives without a backend attribute, existing `schema_roots!` calls, and
`graphql_orm::DbPool` / `graphql_orm::DbRow` continue to work.

Multiple backend features may be enabled by Cargo feature unification in a workspace. In that mode,
each generated entity and schema root must select a backend explicitly:

```rust
#[graphql_entity(
    backend = "mssql",
    table = "dbo.Jobs",
    plural = "Jobs",
    schema_policy = "external_read_only"
)]
pub struct Job {
    #[primary_key]
    #[graphql_orm(db_column = "JobId")]
    pub id: i32,
}

schema_roots! {
    backend: "mssql",
    schema_policy: "external_read_only",
    query_custom_ops: [],
    entities: [Job],
}
```

If multiple backend features are enabled and an entity or schema root does not specify a backend,
the macro emits a compile-time error. In multi-backend builds, schema roots must also declare
`schema_policy`. In multi-backend builds, `DbPool` and `DbRow` are intentionally not exported; use
explicit backend types such as `graphql_orm::db::Database::<graphql_orm::MssqlBackend>`.

Migration capability is backend-gated. SQLite and Postgres implement migration application; SQL
Server does not. Attempting to apply migrations through MSSQL fails at compile time when the
`MigrationBackend` bound is required, including for an `ExternalWritable` database.

The SQL Server driver dependencies are also feature-gated. `tiberius`, `tokio-util`, and the Tokio
TCP support required by Tiberius are optional dependencies and are activated only by the `mssql`
feature. SQLite and Postgres projects do not build the SQL Server runtime path.

## Capability Contract

Under `mssql`, generated GraphQL schemas contain:

- query root fields for list queries
- query root fields for single-by-primary-key queries
- filters, order-by inputs, pagination, count/page info
- relation loading for declared relations
- read row/entity/field policies
- read repository helpers: `find_all`, `find_many`, `find_by_id` for single-key entities,
  `find_by_key`, `count_all`, `count`, and compatibility raw-pool helpers such as `query`,
  `get_by_key`, and `count_query`

An entity declared with `schema_policy = "external_read_only"` does not contain:

- create, update, delete, or upsert mutations
- mutation repository helpers or subscriptions
- migration runners
- schema diffing or schema creation APIs
- backup/restore APIs

An entity declared with `schema_policy = "external_writable"` may generate the normal applicable
repository and GraphQL DML surface: insert/bulk insert, key and bounded predicate update/delete,
upsert/bulk upsert, insert-if-absent, versioned compare-and-swap, composite-key writes, transactions,
hooks, policies, change events, and subscriptions. This mode still cannot manage the physical
schema, search structures, RLS, or backups. Unsupported entity features continue to fail during
macro expansion rather than silently degrading.

## Connections

Create a read-only MSSQL database handle from a Tiberius ADO.NET-style connection string:

```rust
let database = graphql_orm::db::Database::<graphql_orm::MssqlBackend>::connect_ado(
    "server=tcp:127.0.0.1,1433;\
     database=LegacyDb;\
     user id=sa;\
     password=Your_strong_password123;\
     TrustServerCertificate=true",
)
.await?
    .with_schema_policy(graphql_orm::graphql::orm::SchemaPolicy::ExternalReadOnly);
let schema = schema_builder(database)
    .data("current-user".to_string())
    .finish();
```

The database handle reuses Tiberius connections and avoids opening one connection per resolver.
Advanced callers can still create `graphql_orm::db::mssql::MssqlPool` directly and pass it to
`Database::<MssqlBackend>::builder(pool)` when they need driver-specific setup.

Writable access requires both an explicitly writable physical pool and the external-writable schema
contract:

```rust
let database = graphql_orm::db::Database::<graphql_orm::MssqlBackend>
    ::connect_ado_external_writable(
        "server=tcp:127.0.0.1,1433;\
         database=ApplicationDb;\
         user id=application_writer;\
         password=<secret>;\
         TrustServerCertificate=true",
    )
    .await?;
```

Declare only reviewed entities as writable:

```rust
#[derive(RepositoryEntity, Clone, Debug)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.WorkItems",
    plural = "WorkItems",
    schema_policy = "external_writable",
    upsert = "external_key"
)]
struct WorkItem {
    #[primary_key]
    id: uuid::Uuid,
    #[unique]
    external_key: String,
    value: String,
    #[graphql_orm(version, default = "0")]
    version: i64,
}
```

`connect_ado`, `MssqlPool::new`, and `MssqlPool::with_max_connections` always configure Tiberius as
read-only. Changing only `SchemaPolicy` cannot turn those pools into writable connections. The
explicit `connect_ado_external_writable`, `new_external_writable`, and
`with_max_connections_external_writable` constructors are the only writable entry points.

## Mapping Existing Tables

Use schema-qualified SQL Server table names and explicit column names for legacy schemas:

```rust
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.Jobs",
    plural = "Jobs",
    schema_policy = "external_read_only",
    default_sort = "[JobId] ASC"
)]
pub struct LegacyJob {
    #[primary_key]
    #[graphql_orm(db_column = "JobId", write = false)]
    #[filterable(type = "number")]
    #[sortable]
    pub id: i64,

    #[graphql_orm(db_column = "JobName", write = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub job_name: String,

    #[graphql_orm(db_column = "IsClosed", write = false)]
    #[filterable(type = "boolean")]
    pub closed: bool,

    #[graphql_orm(db_column = "StartedAt", write = false)]
    #[filterable(type = "date")]
    #[sortable]
    pub started_at: Option<String>,
}
```

For single-backend MSSQL builds, the `backend = "mssql"` attribute is optional. Keeping it on legacy
SQL Server entities is recommended because it keeps the code valid in larger workspaces where SQLite
or Postgres may be enabled by another service.

Composite primary keys are supported for read paths by marking each key field with `#[primary_key]`:

```rust
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.LegacyLabour",
    plural = "LegacyLabourEntries",
    schema_policy = "external_read_only",
    default_sort = "[LegacyObjectType] ASC, [RefNo] ASC, [LineNum] ASC"
)]
pub struct LegacyLabourEntry {
    #[primary_key]
    #[graphql(name = "LegacyObjectType")]
    #[graphql_orm(db_column = "LegacyObjectType", write = false)]
    pub legacy_object_type: i32,

    #[primary_key]
    #[graphql(name = "RefNo")]
    #[graphql_orm(db_column = "RefNo", write = false)]
    pub ref_no: i32,

    #[primary_key]
    #[graphql(name = "LineNum")]
    #[graphql_orm(db_column = "LineNum", write = false)]
    pub line_num: i16,

    #[graphql(name = "LabourDate")]
    #[graphql_orm(db_column = "LabourDate", write = false)]
    pub labour_date: Option<String>,
}
```

The generated single lookup uses one argument per key field and binds them in declaration order:

```graphql
query {
  legacyLabourEntry(legacyObjectType: 1, refNo: 12345, lineNum: 2) {
    legacyObjectType
    refNo
    lineNum
    labourDate
  }
}
```

With Pascal-case resolver, argument, and field features, the same lookup is exposed as
`LegacyLabourEntry(LegacyObjectType: ..., RefNo: ..., LineNum: ...)`.

The generated repository key type is `LegacyLabourEntryKey`, and read helpers include `find_by_key` and
`get_by_key`. `PRIMARY_KEY` remains the first key for compatibility; use `PRIMARY_KEYS` or
`Entity::metadata().primary_keys` when code needs the full key.
Pagination cursors are offset-based today, so composite keys do not change cursor encoding.

The SQL Server dialect quotes generated identifiers as `[Name]`, renders schema-qualified tables as
`[dbo].[Jobs]`, binds parameters as `@P1`, `@P2`, and uses:

```sql
ORDER BY ... OFFSET ... ROWS FETCH NEXT ... ROWS ONLY
```

Paginated MSSQL queries require deterministic ordering. Generated list queries use explicit order
arguments or the entity default order.

## Relations

Relations are ORM metadata only. They do not require physical SQL Server foreign keys and do not
create or migrate constraints.

For renamed SQL Server columns, use the Rust source field in `from` and the target database column in
`to`. Single-column relation syntax remains unchanged:

```rust
#[graphql_orm(db_column = "CustomerId", write = false)]
#[filterable(type = "number")]
pub customer_id: i64,

#[graphql(skip)]
#[relation(
    target = "LegacyCustomer",
    from = "customer_id",
    to = "CustomerId",
    emit_fk = false
)]
pub customer: Option<LegacyCustomer>,
```

Composite relations use array syntax. `from` lists Rust source fields on the current entity, and
`to` lists target database columns on the related entity. The arity must match:

```rust
#[derive(GraphQLEntity, GraphQLRelations, GraphQLOperations, SimpleObject, Clone, Debug)]
#[graphql(rename_fields = "PascalCase")]
#[graphql(complex)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.LegacyCardFileContacts",
    plural = "LegacyCardFileContacts",
    schema_policy = "external_read_only",
    default_sort = "[CardNo] ASC, [ContNo] ASC"
)]
pub struct LegacyCardFileContact {
    #[primary_key]
    #[graphql(name = "CardNo")]
    #[graphql_orm(db_column = "CardNo", write = false)]
    pub card_no: i32,

    #[primary_key]
    #[graphql(name = "ContNo")]
    #[graphql_orm(db_column = "ContNo", write = false)]
    pub cont_no: i32,

    #[graphql(skip, name = "Details")]
    #[relation(
        target = "LegacyCardFileDetail",
        from = ["card_no", "cont_no"],
        to = ["CardNo", "ContNo"],
        multiple,
        emit_fk = false
    )]
    pub details: Vec<LegacyCardFileDetail>,
}
```

The Legacy card-file shape can be mapped as:

```rust
#[graphql(skip, name = "Contacts")]
#[relation(
    target = "LegacyCardFileContact",
    from = "card_no",
    to = "CardNo",
    multiple,
    emit_fk = false
)]
pub contacts: Vec<LegacyCardFileContact>,

#[graphql(skip, name = "Details")]
#[relation(
    target = "LegacyCardFileDetail",
    from = ["card_no", "cont_no"],
    to = ["CardNo", "ContNo"],
    multiple,
    emit_fk = false
)]
pub details: Vec<LegacyCardFileDetail>,
```

With Pascal-case feature flags, nested reads keep the expected legacy GraphQL shape:

```graphql
query {
  LegacyCardFiles {
    Edges {
      Node {
        CardNo
        CardCode
        Name
        Contacts {
          Edges {
            Node {
              CardNo
              ContNo
              Details {
                Edges {
                  Node { Type Value }
                }
              }
            }
          }
        }
      }
    }
  }
}
```

Nested relation loading is batched by layer. The query above executes as the parent card-file query,
one relation query for all contacts, and one relation query for all details. Relation-level
`Where`/`OrderBy`/`Page` arguments use the same DataLoader batching path for supported scalar key
parts, including `i16`, `i32`, `i64`, `String`, UUIDs, floats, and booleans.

SQL Server composite relation predicates are rendered with bound `@P` parameters and bracketed
identifiers:

```sql
WHERE ([CardNo] = @P1 AND [ContNo] = @P2)
   OR ([CardNo] = @P3 AND [ContNo] = @P4)
```

If any nullable source key part is `NULL`, that parent relation is skipped and resolves to an empty
connection or `None`. SQL `NULL = NULL` matching is not inferred.

For computed fields such as a derived `MicrosoftUserId`, use async-graphql’s normal complex object
pattern:

- derive the entity with `#[graphql(complex)]`
- keep generated relation fields marked `#[graphql(skip)]`
- implement a manual `#[ComplexObject]` method for the computed field
- read `Database<MssqlBackend>` or a request-scoped `DataLoader` from the GraphQL context

This keeps computed database reads explicit and lets applications batch them with their own
request-scoped loaders where needed.

## Type Notes

The MSSQL backend supports these common scalar shapes for generated reads and writes:

- integer types: `int`, `bigint`, `smallint`, `tinyint`
- `bit`
- text strings: `nvarchar`, `varchar`, and compatible text values
- binary values
- `date`, `datetime`, and `datetime2` decoded to Rust strings when mapped to `String`
- `uniqueidentifier` mapped to `uuid::Uuid`
- floating point values
- fixed decimals declared with `#[graphql_orm(decimal(precision = P, scale = S))]` and mapped to
  `rust_decimal::Decimal`; the portable declaration permits precision `1..=18` and
  `scale <= precision`
- JSON values serialized into compatible text columns and decoded with strict JSON validation

SQL Server-specific types such as `xml`, `hierarchyid`,
`geography`, `geometry`, `sql_variant`, `rowversion`, and table-valued columns are not first-class
ORM scalar types in this phase.

## Tests

Pure SQL rendering tests run with the default test suite:

```bash
cargo test -p graphql-orm --test query_ir
```

MSSQL compile-time policy checks run with the MSSQL feature:

```bash
cargo test -p graphql-orm --no-default-features --features mssql --test mssql_write_unavailable_ui
```

Composite-key read rendering and MSSQL read-only schema checks are covered by:

```bash
cargo test -p graphql-orm --no-default-features --features mssql --test composite_primary_keys
```

The owned DML and aggregate parity test is opt-in. It starts the repository-pinned SQL Server image,
publishes it on loopback only, creates a unique test database, verifies ownership before removal,
and asserts that the container is absent afterward. It never accepts an ambient database URL:

```bash
scripts/run-owned-database-lanes.sh mssql
```

It verifies physical read-only defaults, deliberate writable DML, generated keys and defaults,
native decimal/JSON/binary/date-time values, single and composite keys, bounded mutations,
concurrent upsert locking, compare-and-swap, commit/rollback, cancellation socket disposal, and
typed grouped aggregate parity. The runner rejects ambient database URL
variables, and Docker must be available to the test user. A Docker failure is
a failed required lane rather than a skip.

Do not point the owned test or schema-management APIs at application databases. Start adoption with
`ExternalReadOnly`; enable `ExternalWritable` only after the externally managed table contract,
database principal permissions, policies, concurrency semantics, and rollback behavior have been
reviewed. Writable adoption changes application behavior but performs no ORM schema migration.
