---
title: "Microsoft SQL Server Backend"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-12
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
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.143.0", default-features = false, features = ["mssql"] }
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

Writable tables may carry enabled triggers. Generated DML never uses `OUTPUT` without `INTO`,
which SQL Server rejects on such tables. Compare-and-swap updates the row and, in the same batch
and while it holds the row's exclusive lock, reads it back by key and by the version the swap
wrote. The returned row includes changes an `AFTER` trigger made. An `INSTEAD OF UPDATE` trigger
that suppresses the update yields `Conflict` rather than the unchanged row. A trigger that rewrites
the version column yields `Conflict` even when the swap applied, and the savepoint rolls the swap
back. That outcome fails closed, so keep the version column under the ORM's control.

A write result's `rows_affected` is the executed statement's own count, so counts from statements
inside triggers that do not set `NOCOUNT` are excluded. When an `INSTEAD OF` trigger replaces the
statement, it is whatever SQL Server reports for the replaced statement. For a raw batch of several
statements it is the final statement's count, so a batch that ends in DDL or another statement
that reports no count yields 0.

CI and release validation execute the TLS, transport, writable-transaction and
query-notification regressions on test-owned loopback SQL Server containers. Each
owned server gets a certificate signed by a private root generated for it alone,
so these tests run with full-session TLS and certificate validation. The TLS
fixture checks an untrusted private CA, an explicitly trusted CA, and a
mismatched hostname. The transport fixture checks the encryption default, each
explicit opt-in, session reset between checkouts, pool timeouts and a server
default of `NOCOUNT ON`.

## Connections

Connections follow one routing redirect: under Azure SQL's Redirect connection
policy the gateway answers the login with the address of the node that hosts
the database, and the pool and the notification connection connect there under
the same transport policy. Named instances (`server=host\\INSTANCE`) are not
resolved through SQL Server Browser; give the instance's TCP port instead.

SQL Server uses the platform native TLS transport. Linux builds need OpenSSL
headers and `pkg-config`; runtime images need OpenSSL and CA certificates.
Windows uses Schannel and macOS uses Security Framework. Cross-compilation and
static images need an appropriate target OpenSSL configuration. See the
[0.48 migration](../../../MIGRATION.md#0470-to-0480-sql-server-platform-tls).

### Transport security

Since 0.128.0 every SQL Server connection the ORM opens uses TLS for the whole
session and validates the server certificate's chain and hostname. The driver
on its own treats a connection string without `Encrypt` as login-only TLS,
which encrypts the login packet and then sends SQL, parameters and results in
clear. The ORM treats an omitted `Encrypt` as `Encrypt=true`.

Use the server's real DNS name. To trust a private root, install it in the
platform store, or add `TrustServerCertificateCA=/absolute/path/ca.pem` to the
connection string. PEM, CRT and DER are supported, hostname validation stays
on, and the file adds to platform trust rather than pinning an exclusive root.

Before connecting, the ADO constructors (`MssqlPool::connect_ado*`,
`Database::connect_ado*` and `MssqlQueryNotificationConnection::connect_ado*`)
refuse, with `sqlx::Error::Configuration`:

| Setting | Why |
|---|---|
| `Encrypt=false` or `no` | Login-only TLS. Allowed only with an explicit opt-in. |
| `TrustServerCertificate=true` or `yes`, with or without `Encrypt=true` | Disables certificate validation, so anyone on the network path can impersonate the server. Allowed only with an explicit opt-in. |
| `Encrypt=DANGER_PLAINTEXT` | No TLS at all, including the login. Never allowed. |
| `Encrypt=strict`, `mandatory` or `optional` | Not supported by this driver. |
| `TrustServerCertificate=true` with `TrustServerCertificateCA` | Contradictory; the driver would panic. |
| `Integrated Security` or `IntegratedSecurity` set to `true`, `yes` or `SSPI` | This build supports SQL logins only; the driver would silently use SQL authentication. |
| Any other key that mentions encryption, trust, SSL, TLS, certificates or authentication, such as `Encryption`, `Trust Server Certificate` or `Authentication` | The driver ignores keys it does not know, so the setting would have no effect. |

Diagnostics name the key, never its value.

The two weaker modes are opt-ins in code, through `MssqlTransportPolicy`. A
connection string alone cannot enable them:

```rust
use graphql_orm::db::mssql::{MssqlPool, MssqlPoolOptions, MssqlTransportPolicy};

// Disposable local servers only: full-session TLS, unvalidated certificate.
let insecure = MssqlPoolOptions::new().transport(
    MssqlTransportPolicy::verified().insecure_allow_unverified_certificate(),
);
let pool = MssqlPool::connect_ado_with(
    "server=tcp:127.0.0.1,1433;database=Scratch;user id=sa;password=<secret>;\
     TrustServerCertificate=true",
    insecure,
)
.await?;
```

- `insecure_allow_unverified_certificate()` accepts `TrustServerCertificate=true`.
  The session is still encrypted, which stops passive capture only.
- `insecure_allow_login_only_encryption()` accepts `Encrypt=false`. Only the
  login is encrypted unless the server forces encryption.

Each opt-in admits only its own weakening. Never use either as a workaround for
a production connection error.

`MssqlPool::new`, `with_max_connections`, `with_options` and their
external-writable forms take a caller-built `tiberius::Config`. The pool raises
its encryption level to `Required` unless login-only encryption is allowed, in
which case it keeps the level the caller set. That level can be
`EncryptionLevel::NotSupported`, which means no TLS at all, so even the login
credentials cross the network in clear. The driver cannot report a `Config`'s
level, so the ORM cannot refuse it, and it cannot raise the level to at least
login-only without downgrading a caller who set `Required`. Only combine
`insecure_allow_login_only_encryption()` with a caller-built `Config` whose
level you set yourself. A connection string cannot ask for plaintext. The
driver also cannot report whether
a `Config` trusts every certificate, so a caller that calls `Config::trust_cert`
has chosen that in code. Build pools from connection strings to have
certificate settings checked.

### Pool sessions and bounds

`MssqlPoolOptions` configures a pool. Its defaults:

| Option | Default | Effect |
|---|---|---|
| `max_connections` | 5 | Open connections, at least 1. |
| `acquire_timeout` | 30 s | Waiting for a connection, including opening one. Exceeding it returns `sqlx::Error::PoolTimedOut`. |
| `connect_timeout` | 15 s | Opening one connection: TCP, TLS, login and session setup. Also bounds the idle health check. |
| `query_timeout` | none | Each statement. A statement that exceeds it fails with an `io::ErrorKind::TimedOut` error. The driver cannot cancel a running statement, so the connection is closed and SQL Server rolls back its open transaction. See the caveats below. |
| `idle_timeout` | 10 min | Idle connections older than this are closed. |
| `max_lifetime` | 30 min | Connections older than this are closed when next released or acquired. |
| `health_check_after_idle` | 30 s | A connection idle this long is reset and checked before reuse. |
| `transport` | `verified()` | See above. |

A statement timeout has two caveats. First, the outcome of a timed-out
autocommit write is indeterminate: the server can still finish and commit the
statement after the client has received `TimedOut`, so check before retrying
a write that is not idempotent. A timed-out statement inside a transaction is
rolled back, because the transaction never commits. Second, the timed-out
connection's permit is released as soon as the client gives up, while SQL
Server may keep the old session until it notices the closed socket and
finishes or rolls back its work. For that time the number of live server
sessions can briefly exceed `max_connections`.

The pool resets a connection's session when it opens it, in the same batch
that commits or rolls back each transaction, before returning a connection
after a non-fatal server error, and before reusing a connection that has been
idle past `health_check_after_idle`. The reset rolls back any open transaction
and sets `READ COMMITTED` isolation, `NOCOUNT OFF` and
`IMPLICIT_TRANSACTIONS OFF`. If the reset fails, the connection is closed.

Pooled statements run through `sp_executesql`, so SQL Server refuses one that
leaves its transaction count changed (error 266). For example,
`BEGIN TRANSACTION` without a `COMMIT` in the same call, or a `BEGIN
TRANSACTION` issued inside an ORM transaction, fails, and the pool then resets
the session. The work of a batch that fails while its own transaction is open
is rolled back by that reset, never committed. As a further guard, every ORM
transaction begins by rolling back any transaction still open on the session,
so it never nests inside one. If a `COMMIT` leaves `@@TRANCOUNT` above zero,
the commit fails with an error and everything is rolled back; it is never
reported as committed. So a `SERIALIZABLE` state-machine transaction no
longer leaves later autocommit reads on that connection serializable. A server,
database or login default of `NOCOUNT ON` (for example `user options` bit 512)
no longer makes every affected-row count 0. Raw SQL run through
`MssqlPool::fetch_rows` that changes session options affects that connection
until its next reset, so restore them in the same batch.

The reset is not a full `sp_reset_connection`. It does **not** clear:

- local temporary tables (`#name`) and table variables still in scope;
- session-owned application locks from `sp_getapplock` with
  `@LockOwner = 'Session'`;
- `CONTEXT_INFO` and `SESSION_CONTEXT` values;
- the current database after `USE`;
- an impersonation from `EXECUTE AS` that was not reverted;
- `SET ROWCOUNT`, `SET XACT_ABORT`, `SET LOCK_TIMEOUT`, `SET IDENTITY_INSERT`
  and other `SET` options it does not list.

Raw SQL that changes any of these must undo it in the same call. Full
`sp_reset_connection` semantics need the driver to send the TDS reset flag,
which the pinned fork does not; that is part of the fork decision in
[#516](https://github.com/Dastari/graphql-orm/issues/516).

> **Warning:** do not rely on `SESSION_CONTEXT` or `CONTEXT_INFO` that persists
> on a pooled connection, for example for SQL Server row-level security
> predicates. A value set by one borrower is still visible to the next. Set it
> at the start of every transaction that depends on it, with
> `sp_set_session_context ... @read_only = 0`, and clear it before the
> transaction ends.

An error that SQL Server reports, such as a constraint violation, resets the
connection and returns it to the pool. A transport, TLS or protocol error, a timeout, or a
dropped future closes it. Inside a transaction, an error the server reports
keeps the connection and fails the transaction: later statements and the
commit are refused and the rollback ends whatever the server left open. Any
other error closes the connection, and rolling back is then a no-op.

### Errors

An error the server reports is a `sqlx::Error::Database` holding a
`graphql_orm::db::mssql::MssqlServerError` with the error `number()`,
`state()` and `class()`. `as_database_error()` classifies it like the other
backends: 2627 and 2601 are unique violations, 547 is a foreign-key or check
violation and 515 is a not-null violation, so generated mutations report
`CONSTRAINT_VIOLATION` for the first three. `is_retryable()` is true for
1205, 1222, 3960, 41301, 41302, 41305, 41325, 41839, 40501, 40197 and 40613;
a transaction that fails with one of them is reported as retryable. I/O and
TLS failures are `sqlx::Error::Io` and `sqlx::Error::Tls`. When a transaction
callback fails, its own error is returned even if the rollback also fails.

### Database handles

Create a read-only MSSQL database handle from a Tiberius ADO.NET-style connection string:

```rust
let database = graphql_orm::db::Database::<graphql_orm::MssqlBackend>::connect_ado(
    "server=tcp:sql.internal.example,1433;\
     database=LegacyDb;\
     user id=application_reader;\
     password=<secret>",
)
.await?
    .with_schema_policy(graphql_orm::graphql::orm::SchemaPolicy::ExternalReadOnly);
let schema = schema_builder(database)
    .data(graphql_orm::graphql::auth::AuthSubject::new("current-user"))
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
        "server=tcp:sql.internal.example,1433;\
         database=ApplicationDb;\
         user id=application_writer;\
         password=<secret>",
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

`connect_ado`, `connect_ado_with`, `MssqlPool::new`, `with_max_connections` and `with_options`
always configure Tiberius as read-only. Changing only `SchemaPolicy` cannot turn those pools into
writable connections. The explicit `connect_ado_external_writable`,
`connect_ado_external_writable_with`, `new_external_writable`,
`with_max_connections_external_writable` and `with_options_external_writable` constructors are the
only writable entry points.

## Host-supplied integer primary keys

For externally allocated keys, use `#[graphql_orm(auto_generated = false)]`:

```rust
#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "mssql", table = "dbo.AllocatedRecords", schema_policy = "external_writable")]
struct AllocatedRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: i32,
    label: String,
}
```

Supply `id` through `CreateAllocatedRecordInput` and `tx.insert::<AllocatedRecord>`
inside `Database::transaction`. The host allocates the key and provisions the
external table. The ORM binds and returns the exact integer without generating
or stringifying another key. Existing policies, hooks and transaction guarantees
apply. Propagated errors roll back; catching an inner static mutation timeout and
returning `Ok` does not guarantee rollback.

Supported integer decode types remain `i8`, `i16`, `i32`, `i64`, `isize`, `u8`,
`u16`, `u32`, `u64` and `usize`, subject to SQL Server column range and the
existing signed-i64 bind domain. This fix does not add unsigned values above
`i64::MAX`. String/UUID/binary and complete composite key bindings are preserved.
See the dependency-minimal [compiled transaction example](../../../crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/examples/host_integer_key.rs)
and owned `host_integer_key` consumer test. Its MSSQL/PostgreSQL standalone
entry point is compile-only; the consumer test executes the function on owned
infrastructure. The SQLite standalone entry point executes against in-memory SQL.

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
- `date`, `datetime`, `smalldatetime`, `datetime2` and `datetimeoffset` decoded to Rust
  strings when mapped to `String`. Date-time values are RFC 3339, the text the SQLite and
  PostgreSQL backends hold: a type without an offset is read as UTC
  (`2026-11-09T12:34:56Z`, with the fraction when it is not zero) and a `datetimeoffset`
  keeps its offset (`2026-11-09T12:34:56+02:00`). `date` stays `2026-11-09`. What is read does
  not depend on the session language or `DATEFORMAT`. Values are still
  bound as text, so write ISO 8601 with a `T` to columns of the legacy `datetime` type.
- a `DECIMAL` value beyond `rust_decimal`'s range (more than 28 decimal places or a 96-bit
  mantissa) is a column decode error. A non-NULL value in a column declared with scale 38
  (`DECIMAL(38,38)`) panics inside the driver while the row is read; cast such a column in a
  view or avoid mapping it
- `uniqueidentifier` mapped to `uuid::Uuid`
- floating point values
- fixed decimals declared with `#[graphql_orm(decimal(precision = P, scale = S))]` and mapped to
  `rust_decimal::Decimal`; the portable declaration permits precision `1..=18` and
  `scale <= precision`
- native `MONEY` declared with `#[graphql_orm(mssql_money)]` on a
  `rust_decimal::Decimal` or `Option<Decimal>` field. The generated read
  projection casts the identifier-safe column to `DECIMAL(19,4)` under the
  original alias before TDS decoding. Values must have scale 4 or less and a
  signed 64-bit scaled mantissa (`-922337203685477.5808` through
  `922337203685477.5807`); input/default validation never rounds. This
  annotation is SQL Server only and cannot be combined with portable
  `decimal(...)` metadata. It neither changes source schema nor permits DDL.
- JSON values serialized into compatible text columns and decoded with strict JSON validation

SQL Server-specific types such as `xml`, `hierarchyid`,
`geography`, `geometry`, `sql_variant`, `rowversion`, and table-valued columns are not first-class
ORM scalar types in this phase.

## Tests

Every owned SQL Server test generates a private CA with OpenSSL and installs a
server certificate signed by it before the server starts, so owned tests need
the cached pinned Docker image and the `openssl` command. The certificate
regression verifies encryption, trusted private-CA access, untrusted-chain
rejection and hostname rejection. The transport regression verifies the
encryption default, the explicit opt-ins, session reset between checkouts, pool
timeouts and a `NOCOUNT ON` session default:

```bash
cargo test -p graphql-orm --no-default-features --features mssql --locked --test mssql_tls --test mssql_transport -- --ignored --test-threads=1
```

The correctness regression executes error classification, decoding, empty
pages, `LIKE` patterns, wide relation batches, the upsert batch and rendered
DDL on the owned server; it is part of `scripts/run-owned-database-lanes.sh mssql`:

```bash
cargo test -p graphql-orm --no-default-features --features mssql --locked --test mssql_correctness -- --ignored --test-threads=1
```

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
and asserts that the container is absent afterward. It never accepts an ambient database URL.
The container is bounded to 2 CPUs, 3 GiB memory with no swap, 1024 tasks and one 16 MiB
json-file log. It never pulls unless `GRAPHQL_ORM_TEST_MSSQL_PULL=missing` is set (the hosted
workflows set it), so local runs need the pinned image already cached.
`GRAPHQL_ORM_TEST_MSSQL_STORAGE_SIZE=8G` additionally bounds the writable layer through
`--storage-opt size=`; storage drivers that cannot enforce it make the run fail rather than
dropping the bound. A controller may set `GRAPHQL_ORM_TEST_RUN_TOKEN` (8-64 of `[a-z0-9-]`) to
add a `graphql-orm.test-run` label, and `GRAPHQL_ORM_TEST_MSSQL_EVIDENCE_DIR` to receive an
exclusively created `<container-id>.owned` record for every container the fixture starts:

```bash
scripts/run-owned-database-lanes.sh mssql
```

It verifies physical read-only defaults, deliberate writable DML, generated keys and defaults,
native decimal/JSON/binary/date-time values, single and composite keys, bounded mutations,
concurrent upsert locking, compare-and-swap, writes on tables with counting `AFTER` triggers,
commit/rollback, cancellation socket disposal, and
typed grouped aggregate parity. The runner rejects ambient database URL
variables, and Docker must be available to the test user. A Docker failure is
a failed required lane rather than a skip.

Do not point the owned test or schema-management APIs at application databases. Start adoption with
`ExternalReadOnly`; enable `ExternalWritable` only after the externally managed table contract,
database principal permissions, policies, concurrency semantics, and rollback behavior have been
reviewed. Writable adoption changes application behavior but performs no ORM schema migration.

## Query notifications

`MssqlQueryNotificationConnection` is a dedicated capability, separate from
`MssqlPool` and `ExternalWritable`. It registers trusted SELECT queries using
native TDS headers, receives one Broker message at a time, ends the received
conversation, and classifies bounded XML messages. It never creates or alters
schemas, queues, services, triggers, or Broker configuration. Queue consumption
and conversation completion change Broker state and require explicit authority.
Database permissions remain authoritative; SELECT-prefix validation is not a SQL
security sandbox. Deny this principal business DML and DDL.

Delivery is at-least-once, and hosts must be idempotent. `receive` returns a
message while the transaction that received it and ended its conversation is
still open; nothing is committed before the host holds the message. The
receive is acknowledged (committed) at the start of the next `receive` or
`register` call, by `acknowledge()`, or by `close()`.

- Until it is acknowledged, a message's transaction stays open and its
  conversation group is locked for other receivers. A host that handles
  messages slowly should call `acknowledge()` after handling one, or accept
  the lock.
- An unacknowledged message is delivered again after a crash, a lost
  connection, or dropping the connection value. Call `close()` on a clean
  shutdown and before a deliberate reconnect.
- A failed acknowledgement has an unknown outcome: the message was already
  delivered and is delivered once more unless the commit had landed.
  `uncertain_acknowledgements()` counts these, and the connection opens a new
  session on its next use.
- Cancelling a call is safe at every point. A cancelled wait closes its
  session at once and SQL Server rolls the receive back. Keep using the same
  connection value afterwards; it reconnects by itself.
- **Poison-message handling.** SQL Server disables a queue after five
  consecutive rolled-back receives. Each unacknowledged drop, crash, failed
  acknowledgement or cancelled receive of a waiting message is one rollback,
  so a loop that receives a message and reconnects without acknowledging it
  disables a healthy queue. Acknowledge handled messages. To recover, run
  `ALTER QUEUE ... WITH STATUS = ON`; a host that prefers endless redelivery
  can create its queue with `POISON_MESSAGE_HANDLING (STATUS = OFF)`.

Messages are invalidation hints, so handling one twice repeats a refresh; the
ORM itself has no consumer of them.

The external owner must enable Broker and provision a dedicated queue and service
accepting `http://schemas.microsoft.com/SQL/Notifications/PostQueryNotification`.
Grant the listener SELECT on the watched objects, SUBSCRIBE QUERY NOTIFICATIONS,
RECEIVE on its queue and `dbo.QueryNotificationErrorsQueue`, and permission to
end its conversations. Grant SEND on the
destination service as required by the owner configuration. Use a dedicated queue
per consuming application; receiving from a shared queue steals other consumers'
messages. No activation procedure is required.

```rust,ignore
use graphql_orm::db::mssql::{MssqlBrokerQueue, MssqlQueryNotificationConnection};
let queue = MssqlBrokerQueue::new("dbo", "ChangeQueue")?;
let mut listener = MssqlQueryNotificationConnection::connect_ado(connection_string, queue).await?;
listener.register("SELECT COUNT_BIG(*) AS Total FROM dbo.ChangeLog", "registration-id", "ChangeService", 300).await?;
let notification = listener.receive(30_000).await?;
```

Registration drains and returns the query result. SQL Server can execute an
ineligible query successfully and immediately enqueue an `Invalid` notification;
applications must consume it and report degraded status instead of claiming an
active subscription. `None` from receive means wait timeout, not expiry. Handle
`Change`, `Expired`, `Invalid`, and `Other`, re-arm one-shot registrations, and
reconcile data after reconnects or uncertain delivery. After a cancelled call
or an error, keep the connection value: it opens a new session on its next
use. Acknowledge each handled message (`acknowledge()`, the next `receive`, or
`close()` before dropping the connection); a message that is never
acknowledged is delivered again. Reads should use a separate read-only pool
while receive holds the notification connection.

The included aggregate test uses `COUNT_BIG(*)` over a whole log table; it provides
insert/delete invalidation without loading its entire history. It does not
identify rows and does not detect updates that leave that aggregate unchanged.
Applications must choose a query consistent with their source's append-only
contract, and must not assume identity order equals transaction commit order.

Run the explicitly opted-in integration test against its self-provisioned,
loopback-only, two-CPU/3-GiB disposable Docker SQL Server (no application DSN):

```bash
cargo test -p graphql-orm --no-default-features --features mssql --test mssql_notifications -- --ignored
```

On shared hosts use the host-required bounded Cargo wrapper. The test uses an
exact image digest, proves insert wakeup,
re-registration after reconnect, invalid-query delivery, and expiry, then verifies
container cleanup. The listener test principal is explicitly denied table writes
and table creation. SQL Server requires sufficient available memory to start.

Paginated batched relation reads preserve SQL expression projections, including
the exact decimal cast used for `mssql_money`, when joining ranked keys back to
full rows. SQL Server execution is covered by the owned `mssql_integration` test.
