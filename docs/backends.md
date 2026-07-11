# Backend Features

Backend features compile database support. They do not, by themselves, grant permission to mutate a
database schema. Schema ownership and migration behavior are controlled by runtime policy; see
[Schema Management](schema-management.md).

## Features

```toml
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.5.0", default-features = false, features = ["sqlite"] }
```

Available backend features:

- `sqlite` - SQLite read/write/query/migration support using SQLX internally
- `postgres` - PostgreSQL read/write/query/migration support using SQLX internally
- `mssql` - Microsoft SQL Server read/query-only support through Tiberius

Optional non-backend features:

- `auth-agql` - optional one-way bridge from upstream `agql-auth` 0.7
  (`git` rev `5e7f230b96350f55496477c11f8a0505e6438779`) into `AuthSubject` /
  `DbAuthContext`.

The `mssql` feature activates optional `tiberius`, `tokio-util`, and Tokio TCP support. Projects that
do not select `mssql` do not build the SQL Server driver path.

Normal application setup can stay on `graphql-orm` types:

```rust
let sqlite = graphql_orm::db::Database::<graphql_orm::SqliteBackend>::connect_sqlite(
    "sqlite://app.db",
)
.await?;

let postgres = graphql_orm::db::Database::<graphql_orm::PostgresBackend>::connect_postgres(
    std::env::var("DATABASE_URL")?,
)
.await?;
```

`graphql_orm::sqlx` remains re-exported for compatibility and advanced pool customization, but
generated repository helpers and runtime query APIs now return `graphql_orm::Result<T>`.

## Spatial Support

Spatial fields and topological `where` predicates are supported by the `postgres` and `sqlite`
backends with different execution models.

Postgres uses native PostGIS `geometry(<type>, <srid>)` columns. When `index = true`, managed
migrations create a GiST spatial index and predicates render to PostGIS functions.

SQLite stores spatial fields as GeoJSON in `TEXT` columns. Writes validate that the value is a
GeoJSON geometry of the declared type, and spatial predicates are evaluated in Rust after rows are
loaded. Other SQL-able filters in the same `where` input are still pushed into SQLite first so the
Rust topology check sees a smaller candidate set. This keeps application code portable, but it is not
spatial-indexed and can be inefficient on large tables. `index = true` is accepted on SQLite for
schema portability, but no SQLite index is created.

The `mssql` backend still rejects `#[graphql_orm(spatial(...))]` at compile time in this phase.

SQLite has several industry-standard spatial options, but they have different semantics and
deployment costs:

- SpatiaLite is the best fit for exact OGC-style predicates and `CreateSpatialIndex`. It requires
  extension loading, spatial metadata tables, and GEOS-backed functions.
- SQLite R*Tree is available through SQLite's R*Tree module and is useful for bounding-box
  prefiltering, but it does not provide exact topology predicates by itself.
- GeoPackage is an interoperable SQLite-based GIS container with geometry metadata and RTree
  conventions. It is useful for file exchange but heavier for ORM-managed application schemas.

The recommended future path for indexed exact SQLite predicates is an optional `spatialite` feature.
Plain R*Tree should be reserved for explicit bounding-box APIs or as a prefilter, not as the source
of exact `contains`/`within`/`overlaps` semantics.

## Full-Text Search Support

Full-text search is exposed through the same generated API on supported backends:

- Postgres uses a managed shadow table with `tsvector`, `tsquery`, `ts_rank_cd`, and a GIN index on
  `document_vector`. Native search is used for authenticated requests with `DbAuthContext` as well
  as anonymous requests.
- SQLite creates an FTS5 virtual table with the `unicode61` tokenizer by default. If native FTS
  execution is unavailable at runtime and fallback is enabled, query helpers can fall back to the
  deterministic Rust scorer over loaded entities.
- MSSQL has metadata and diagnostics for future support, but managed execution is not implemented in
  this pass.

Search documents are denormalized per entity. Local `searchable(...)` fields are maintained by
generated ORM writes. Writes made outside `graphql-orm` require an explicit rebuild before native
search indexes reflect the new data.

Native Postgres and SQLite FTS5 search paths push scoring, ordering, limit, offset, and count into
SQL. The Rust fallback path is reserved for missing native structures or explicit fallback use; it is
not used to mask arbitrary native search errors.

Postgres SQL shape:

```sql
CREATE TABLE __graphql_orm_search_articles (
  entity_pk TEXT PRIMARY KEY,
  entity_pk_json JSONB NOT NULL,
  document_text TEXT NOT NULL,
  document_vector TSVECTOR NOT NULL,
  updated_at BIGINT NOT NULL
);

CREATE INDEX idx_gom_search_articles_vector
ON __graphql_orm_search_articles
USING GIN (document_vector);
```

SQLite SQL shape:

```sql
CREATE VIRTUAL TABLE __graphql_orm_fts_articles
USING fts5(
  entity_pk UNINDEXED,
  weight_a,
  weight_b,
  weight_c,
  weight_d,
  document_text,
  tokenize = 'unicode61'
);
```

Future MySQL support should use `FULLTEXT` indexes for local fields and the same denormalized shadow
table strategy when related fields are included. Future MSSQL support should target full-text
catalogs and `CONTAINSTABLE`/`FREETEXTTABLE`; current MSSQL support remains read/query-only.

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
  version = "0.2.21",
  default-features = false,
  features = [
    "mssql",
    "resolver-case-pascal",
    "argument-case-pascal",
    "field-case-pascal",
  ],
}
```
