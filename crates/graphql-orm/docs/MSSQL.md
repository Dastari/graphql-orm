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
graphql-orm = { version = "0.2.7", default-features = false, features = ["mssql"] }
```

When exactly one of `sqlite`, `postgres`, or `mssql` is enabled, the legacy implicit backend remains
available. Existing derives without a backend attribute, existing `schema_roots!` calls, and
`graphql_orm::DbPool` / `graphql_orm::DbRow` continue to work.

Multiple backend features may be enabled by Cargo feature unification in a workspace. In that mode,
each generated entity and schema root must select a backend explicitly:

```rust
#[graphql_entity(backend = "mssql", table = "dbo.Jobs", plural = "Jobs")]
pub struct Job {
    #[primary_key]
    #[graphql_orm(db_column = "JobId")]
    pub id: i32,
}

schema_roots! {
    backend: "mssql",
    query_custom_ops: [],
    entities: [Job],
}
```

If multiple backend features are enabled and an entity or schema root does not specify a backend,
the macro emits a compile-time error. In multi-backend builds, `DbPool` and `DbRow` are intentionally
not exported; use explicit backend types such as `graphql_orm::db::Database::<graphql_orm::MssqlBackend>`.

Migration and backup APIs remain limited to exactly-one SQLite or Postgres builds. SQL Server is
read-only, and backend-explicit migrations/backups for mixed-backend workspaces are not included in
this phase.

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
- read repository helpers: `query`, `find_by_id`, and `count_query`

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

let database = graphql_orm::db::Database::<graphql_orm::MssqlBackend>::new(pool);
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

Composite primary keys are not supported unless the existing ORM primary-key model is extended.

## Tests

Pure SQL rendering tests run with the default test suite:

```bash
cargo test -p graphql-orm --test query_ir
```

MSSQL compile-time read-only checks run with the MSSQL feature:

```bash
cargo test -p graphql-orm --no-default-features --features mssql --test mssql_write_unavailable_ui
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
phase. The migration path is to port one simple read-only entity first, then a relation-heavy entity,
and only then replace the old local SQL Server-specific GraphQL read path with the generic MSSQL
backend.
