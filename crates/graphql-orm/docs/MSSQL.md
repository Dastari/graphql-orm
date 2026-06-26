# Microsoft SQL Server Read-Only Backend

The `mssql` feature enables SQL Server as a read/query-only backend. It is intended for existing
databases where `graphql-orm` should provide the same generated entity, filter, ordering,
pagination, relation, policy, and async-graphql read paths without taking ownership of schema
management or writes.

SQL Server support uses [`tiberius`](https://crates.io/crates/tiberius). Current SQLx releases do
not provide an MSSQL driver.

## Feature Selection

For a service that only uses SQL Server, select the `mssql` backend feature:

```toml
graphql-orm = { version = "0.2.9", default-features = false, features = ["mssql"] }
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
Server does not. SQL Server is read-only, and attempting to apply migrations through MSSQL fails at
compile time when the `MigrationBackend` bound is required.

The SQL Server driver dependencies are also feature-gated. `tiberius`, `tokio-util`, and the Tokio
TCP support required by Tiberius are optional dependencies and are activated only by the `mssql`
feature. SQLite and Postgres projects do not build the SQL Server runtime path.

## Read-Only Contract

Under `mssql`, generated GraphQL schemas contain:

- query root fields for list queries
- query root fields for single-by-primary-key queries
- filters, order-by inputs, pagination, count/page info
- relation loading for declared relations
- read row/entity/field policies
- read repository helpers: `query`, `find_by_id` for single-key entities, `find_by_key` /
  `get_by_key`, and `count_query`

Under `mssql`, generated schemas do not contain:

- create, update, delete, or upsert mutations
- mutation repository helpers
- migration runners
- schema diffing or schema creation APIs
- backup/restore APIs

Attempting to use generated write helpers under `mssql` fails at compile time when the helper is not
generated. Lower-level write execution is also rejected with a clear read-only runtime error.

## Connections

Create an MSSQL pool from a Tiberius ADO.NET-style connection string:

```rust
let pool = graphql_orm::db::mssql::MssqlPool::connect_ado(
    "server=tcp:127.0.0.1,1433;\
     database=LegacyDb;\
     user id=sa;\
     password=Your_strong_password123;\
     TrustServerCertificate=true",
)
.await?;

let database = graphql_orm::db::Database::<graphql_orm::MssqlBackend>::builder(pool)
    .schema_policy(graphql_orm::graphql::orm::SchemaPolicy::ExternalReadOnly)
    .build();
let schema = schema_builder(database)
    .data("current-user".to_string())
    .finish();
```

The pool reuses Tiberius connections and avoids opening one connection per resolver.

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
    table = "dbo.JimLabour",
    plural = "JimLabourEntries",
    schema_policy = "external_read_only",
    default_sort = "[JimObjectType] ASC, [RefNo] ASC, [LineNum] ASC"
)]
pub struct JimLabourEntry {
    #[primary_key]
    #[graphql(name = "JimObjectType")]
    #[graphql_orm(db_column = "JimObjectType", write = false)]
    pub jim_object_type: i32,

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
  jimLabourEntry(jimObjectType: 1, refNo: 12345, lineNum: 2) {
    jimObjectType
    refNo
    lineNum
    labourDate
  }
}
```

With Pascal-case resolver, argument, and field features, the same lookup is exposed as
`JimLabourEntry(JimObjectType: ..., RefNo: ..., LineNum: ...)`.

The generated repository key type is `JimLabourEntryKey`, and read helpers include `find_by_key` and
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

Relation declarations currently describe single-column edges. Composite primary keys do not block
future composite relation support, but composite foreign-key relation loading is not first-class in
this phase.

For renamed SQL Server columns, use the Rust source field in `from` and the target database column in
`to`:

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

## Type Notes

The initial MSSQL backend supports the common SQL Server scalar shapes used by generated read paths:

- integer types: `int`, `bigint`, `smallint`, `tinyint`
- `bit`
- text strings: `nvarchar`, `varchar`, and compatible text values
- binary values
- `date`, `datetime`, and `datetime2` decoded to Rust strings when mapped to `String`
- `uniqueidentifier` mapped to `uuid::Uuid`
- floating point values

Decimal/numeric columns should be mapped to supported Rust numeric/string types that match the
precision needs of the application. SQL Server-specific types such as `xml`, `hierarchyid`,
`geography`, `geometry`, `sql_variant`, `rowversion`, and table-valued columns are not first-class
ORM scalar types in this phase.

## Tests

Pure SQL rendering tests run with the default test suite:

```bash
cargo test -p graphql-orm --test query_ir
```

MSSQL compile-time read-only checks run with the MSSQL feature:

```bash
cargo test -p graphql-orm --no-default-features --features mssql --test mssql_write_unavailable_ui
```

Composite-key read rendering and MSSQL read-only schema checks are covered by:

```bash
cargo test -p graphql-orm --no-default-features --features mssql --test composite_primary_keys
```

The live MSSQL integration test is opt-in. Set `MSSQL_TEST_DATABASE_URL` to an ADO.NET-style
connection string for a disposable database/user that can create and drop test tables:

```bash
MSSQL_TEST_DATABASE_URL='server=tcp:127.0.0.1,1433;database=tempdb;user id=sa;password=Your_strong_password123;TrustServerCertificate=true' \
  cargo test -p graphql-orm --no-default-features --features mssql --test mssql_integration
```

With Docker, one possible local server is:

```bash
docker run --rm -e ACCEPT_EULA=Y \
  -e MSSQL_SA_PASSWORD=Your_strong_password123 \
  -p 1433:1433 \
  mcr.microsoft.com/mssql/server:2022-latest
```

Do not run migrations or generated writes against Jim or other legacy SQL Server databases in this
phase. Use `SchemaPolicy::ExternalReadOnly` at runtime and `schema_policy = "external_read_only"`
in the entity/root macros. The migration path is to port one simple read-only entity first, then a
relation-heavy entity, and only then replace the old local SQL Server-specific GraphQL read path
with the generic MSSQL backend.
