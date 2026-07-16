# graphql-orm

`graphql-orm` generates async-graphql query/mutation types, typed filters, ordering, pagination,
relation loading, repository helpers, schema metadata, and migration plans from Rust entity structs.

It is designed for two related use cases:

- greenfield SQLite/Postgres apps where Rust entity metadata can own the database schema
- existing databases, especially legacy Microsoft SQL Server schemas, where the ORM should provide a
  safe generated GraphQL read layer without taking ownership of writes or migrations

## Highlights

- `#[derive(GraphQLEntity)]` for GraphQL object types, SQL row decoding, filters, order inputs, and
  schema metadata
- `#[derive(GraphQLOperations)]` for generated list queries, single-entity lookups, repository
  helpers, and write operations where the backend supports writes
- `#[derive(GraphQLRelations)]` for nested relation fields with batched loading
- SQLite and PostgreSQL read/write support through SQLx
- Microsoft SQL Server read/query-only support through Tiberius
- validated runtime-schema filters, deterministic keyset reads, and owned
  records on SQLite/PostgreSQL without compiled entity types
- schema-bound, least-privilege runtime relation batching with composite keys,
  per-parent keysets, and optional counts
- single and composite primary-key read support
- single and composite relation-key batching, including nested legacy shapes like
  `JimCardFiles -> Contacts -> Details`
- portable spatial fields and predicates with native PostGIS support and SQLite GeoJSON fallback
- portable per-entity full-text search with native Postgres search tables and SQLite FTS5 support
- explicit schema ownership policies for managed, external, validate-only, and plan-only schemas
- ABI-style schema migration stages for managed SQLite/Postgres schemas
- row, field, and entity policy hooks for application-owned access control
- project-agnostic `AuthSubject` and exact-scope `ScopeEntityPolicy` helpers
- opt-in PostgreSQL row-level security metadata and request-local database auth context
- backend-neutral typed read projections that omit sensitive columns from SQL and process memory
- typed composite-key, insert-if-absent, conditional, and bounded mutation APIs
- federation-composable conventional GraphQL operation roots with stable Rust root names
- dependency-owned schema modules with stable migration, backup, and restore metadata
- owned backend-neutral runtime schema IR with validation, canonical fingerprints, and static-metadata conversion
- owned runtime values, fingerprint-bound schema handles, projections, and exact SQLite/PostgreSQL row decoding
- backend-neutral fenced lease transitions for durable workers
- bounded forward and backward repository keyset windows for large timelines
- opt-in policy-gated bounded retention purge for managed append-only entities

## Install

Select exactly the backend support your service needs:

```toml
[dependencies]
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.13.0", default-features = false, features = ["sqlite"] }
```

GitHub with an exact full revision is the only supported distribution method. Neither crate is
published to crates.io. Replace the placeholder with the reviewed release commit (the version tag
is an identity aid, not a substitute for `rev`). The optional `auth-agql` bridge likewise resolves
the exact upstream revision `c92dcb441237bbe308499b26525945f60ffa394a`.

Available backend features:

- `sqlite` - activates only SQLx SQLite support
- `postgres` - activates only SQLx PostgreSQL support
- `mssql` - read/query-only SQL Server support without either SQLx database driver

Optional integration features:

- `auth-agql` - optional one-way bridge from `agql-auth` 0.10 principals into
  `AuthSubject` / `DbAuthContext`

Naming features are independent of backend features:

- `resolver-case-*`
- `argument-case-*`
- `field-case-*`

When one backend feature is enabled, existing single-backend shorthand remains available. In
multi-service workspaces, Cargo may unify backend features; in that mode each entity and
`schema_roots!` block must declare an explicit backend.

## Quick SQLite Example

```rust
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "users", plural = "Users", default_sort = "name ASC")]
pub struct User {
    #[primary_key]
    pub id: String,

    #[filterable(type = "string")]
    #[sortable]
    pub name: String,

    #[filterable(type = "boolean")]
    pub active: bool,
}

schema_roots! {
    auth: "required",
    query_custom_ops: [],
    entities: [User],
}

async fn build_schema(database_url: &str) -> graphql_orm::Result<AppSchema> {
    let database =
        graphql_orm::db::Database::<graphql_orm::SqliteBackend>::connect_sqlite(database_url)
            .await?;

    Ok(schema_builder(database)
        .data(AuthSubject::new("current-user-id"))
        .finish())
}
```

Generated resolver auth can be set at the schema root or entity level with
`auth = "required" | "optional" | "none"`. The default preserves the previous fail-closed generated
auth behavior; use `auth: "none"` for public generated schemas.

Generated GraphQL includes list and single lookup queries:

```graphql
query {
  users(where: { active: { eq: true } }, orderBy: [{ name: ASC }]) {
    edges {
      node { id name active }
    }
    pageInfo { totalCount hasNextPage }
  }
}
```

SQLite/Postgres entities also get generated mutations and repository helpers unless policy/backend
settings make them unavailable.

For persisted types that must never enter the GraphQL type registry, derive
`RepositoryEntity` with `#[repository_entity(...)]`. It generates ordinary Rust
filter/order/create/update/projection types and applicable typed repository and
transaction operations, but no async-graphql object, input, resolver,
connection, payload, subscription, or schema-root implementation. Private and
sensitive fields remain available to trusted repository inputs, with generated
debug output and mutation side effects redacted. The storage model and stable
schema hash are identical to an equivalent GraphQL-enabled declaration.

`schema_roots!` can hide generated GraphQL mutations without disabling generated repository
writes. `generated_mutations` defaults to `"all"` for compatibility; use `"none"` to expose only
custom mutation roots from `extra_mutation_types`, or use `"allowlist"` with
`generated_mutation_allowlist: [Entity]` / `"denylist"` with
`generated_mutation_denylist: [Entity]` for mixed public exposure.

## SQL Server Read-Only Example

SQL Server support is intentionally read-only. It lets projects point the same entity/filter/query
system at existing databases without generating writes or migrations.

```rust
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.Jobs",
    plural = "Jobs",
    schema_policy = "external_read_only",
    default_sort = "[JobId] ASC"
)]
pub struct Job {
    #[primary_key]
    #[graphql_orm(db_column = "JobId", write = false)]
    #[filterable(type = "number")]
    #[sortable]
    pub id: i32,

    #[graphql_orm(db_column = "JobName", write = false)]
    #[filterable(type = "string")]
    #[sortable]
    pub name: String,
}

schema_roots! {
    backend: "mssql",
    schema_policy: "external_read_only",
    query_custom_ops: [],
    entities: [Job],
}
```

Create a SQL Server database handle from an ADO.NET-style connection string:

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
```

## Composite Relations

Composite relation keys use array syntax and batch efficiently across SQLite, Postgres, and MSSQL:

```rust
#[graphql(skip, name = "Details")]
#[relation(
    target = "JimCardFileDetail",
    from = ["card_no", "cont_no"],
    to = ["CardNo", "ContNo"],
    multiple,
    emit_fk = false
)]
pub details: Vec<JimCardFileDetail>,
```

A nested query such as `JimCardFiles -> Contacts -> Details` executes as one parent query plus one
batched relation query per relation layer, not N+1 or nested N*N queries.

## Documentation

- [Getting started](docs/getting-started.md)
- [Backend features and multi-backend workspaces](docs/backends.md)
- [Entities, keys, columns, naming, and relations](docs/entities-and-relations.md)
- [PostgreSQL RLS and auth-aware execution](docs/postgres.md)
- [SQL Server read-only backend](docs/mssql.md)
- [Schema ownership, validation, planning, and ABI migrations](docs/schema-management.md)
- [Writes, repository helpers, hooks, subscriptions, and policies](docs/runtime-and-writes.md)
- [Portable transactions, CAS, append-only entities, constraints, and keysets](docs/portable-persistence.md)
- [Binary keys, private repository upserts, and conditional indexes](docs/binary-keys-and-indexes.md)
- [Typed least-privilege read projections](docs/read-projections.md)
- [Repository-only persisted entities](docs/repository-only-entities.md)
- [Typed composite-key and bounded mutations](docs/composite-mutations.md)
- [Bounded append-only retention maintenance](docs/retention-maintenance.md)
- [Backup runtime API](docs/backup.md)
- [Schema modules and fenced leases](docs/schema-modules-and-leases.md)
- [Release notes](docs/release-notes.md)
- [Development and test commands](docs/development.md)

## Status

The crate is under active development. SQLite/Postgres write paths and schema management are
available for managed schemas. SQL Server is currently read/query-only by design.

## Repository Layout

- `crates/graphql-orm` - runtime crate used by applications
- `crates/graphql-orm-macros` - proc-macro crate re-exported by `graphql-orm`

Applications should depend on `graphql-orm` and use the re-exported macros from
`graphql_orm::prelude::*`.
