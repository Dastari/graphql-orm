# Backend Features

Backend features compile database support. They do not, by themselves, grant permission to mutate a
database schema. Schema ownership and migration behavior are controlled by runtime policy; see
[Schema Management](schema-management.md).

## Features

```toml
graphql-orm = { version = "0.2.11", default-features = false, features = ["sqlite"] }
```

Available backend features:

- `sqlite` - SQLite read/write/query/migration support through SQLx
- `postgres` - PostgreSQL read/write/query/migration support through SQLx
- `mssql` - Microsoft SQL Server read/query-only support through Tiberius

The `mssql` feature activates optional `tiberius`, `tokio-util`, and Tokio TCP support. Projects that
do not select `mssql` do not build the SQL Server driver path.

## Spatial Support

Spatial fields, spatial indexes, and topological `where` predicates are implemented for the
`postgres` backend with PostGIS. The `sqlite` and `mssql` backends reject `#[graphql_orm(spatial(...))]`
at compile time in this phase.

SQLite has several industry-standard spatial options, but they have different semantics and
deployment costs:

- SpatiaLite is the best fit for exact OGC-style predicates and `CreateSpatialIndex`. It requires
  extension loading, spatial metadata tables, and GEOS-backed functions.
- SQLite R*Tree is available through SQLite's R*Tree module and is useful for bounding-box
  prefiltering, but it does not provide exact topology predicates by itself.
- GeoPackage is an interoperable SQLite-based GIS container with geometry metadata and RTree
  conventions. It is useful for file exchange but heavier for ORM-managed application schemas.

The recommended future path is an optional `spatialite` feature if exact SQLite predicates are
required. Plain R*Tree should be reserved for explicit bounding-box APIs, not for exact
`contains`/`within`/`overlaps` semantics.

## Single-Backend Builds

When exactly one backend feature is enabled, the compatibility shorthand remains available:

- entities can omit `backend = "..."`
- `schema_roots!` can omit `backend`
- `graphql_orm::DbPool` and `graphql_orm::DbRow` are exported aliases

This keeps existing SQLite and PostgreSQL applications working unchanged.

## Multi-Backend Workspaces

Cargo feature unification can enable more than one backend on the same `graphql-orm` package. For
example, one workspace can have:

- `auth-service` using SQLite
- `jim-service` using SQL Server

In that mode, backend selection must be explicit:

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

In multi-backend builds, `DbPool` and `DbRow` are intentionally not exported. Use explicit backend
types:

```rust
graphql_orm::db::Database::<graphql_orm::MssqlBackend>
graphql_orm::db::Database::<graphql_orm::SqliteBackend>
graphql_orm::db::Database::<graphql_orm::PostgresBackend>
```

## Naming Feature Groups

Naming features are independent from backend features. Enable at most one feature from each group:

- `resolver-case-pascal`, `resolver-case-snake`, `resolver-case-screaming-snake`,
  `resolver-case-lower`, `resolver-case-upper`
- `argument-case-pascal`, `argument-case-snake`, `argument-case-screaming-snake`,
  `argument-case-lower`, `argument-case-upper`
- `field-case-pascal`, `field-case-snake`, `field-case-screaming-snake`, `field-case-lower`,
  `field-case-upper`

Example:

```toml
graphql-orm = {
  version = "0.2.11",
  default-features = false,
  features = [
    "mssql",
    "resolver-case-pascal",
    "argument-case-pascal",
    "field-case-pascal",
  ],
}
```
