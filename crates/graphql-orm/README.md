---
title: "graphql-orm"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-08-31
review_by: 2027-02-01
supersedes: []
---

# `graphql-orm`

`graphql-orm` turns annotated Rust structs into typed database metadata,
`async-graphql` objects, query/filter/order inputs, and—when requested—CRUD
resolver types. It supports managed SQLite and PostgreSQL schemas plus
read-only or deliberately `ExternalWritable` SQL Server integration for
externally owned tables.

It is not a database server, migration runner, HTTP framework, authorization
system, or a promise that a generated field is safe to expose. Applications
own connection lifecycle, schema-application decisions, request identity,
field/row policy, and HTTP transport.

## Install from Git

Packages are distributed from this repository at reviewed full revisions. Pin
the runtime and disable the default SQLite feature when selecting another
backend:

```toml
[dependencies]
graphql-orm = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.30.0", default-features = false, features = ["sqlite"] }
```

This unpublished package has no docs.rs release. Use this Git README and the
repository reference, or build matching local rustdoc with
`cargo doc -p graphql-orm --no-deps`.

| Feature | Enables | Important limit |
| --- | --- | --- |
| `sqlite` (default) | SQLx SQLite, Tokio runtime, migrations, GeoJSON spatial support | spatial predicates run in Rust; no SQLite spatial index is created |
| `postgres` | SQLx PostgreSQL + Rustls, migrations, native PostGIS/FTS paths | requires a PostgreSQL deployment and compatible extensions for spatial use |
| `mssql` | Tiberius SQL Server reads and deliberate external-schema DML | compatibility constructors remain physically read-only; no managed migrations, backup/restore, search maintenance, or runtime-schema row decoding |
| `change-journal` | change-journal API surface | does not itself configure a journal |
| `auth-agql` | one-way `agql-auth` bridge | does not install authentication or grant scopes |
| `router-protocol` | project-neutral generated operation export | does not run a router |

The naming features `resolver-case-*`, `argument-case-*`, and `field-case-*`
are independent groups; enable at most one feature in each group. In a
multi-backend dependency graph, select `backend = "sqlite" | "postgres" |
"mssql"` on each entity and in `schema_roots!`.

Companion capabilities remain separate packages rather than core features.
Depending on `graphql-orm` does not compile or link `graphql-orm-ai`,
`graphql-orm-storage`, `graphql-orm-backup`, or `graphql-orm-router`; add only
the companion crates an application uses. For the core crate itself, set
`default-features = false` and enable one database backend plus only the
optional bridges needed by the application.

Generated update, delete, predicate-write, and upsert helpers keep the
authoritative preimage, row/field policy, input transformation, hooks, and DML
on one pinned transaction. Predicate writes materialize the authorized primary
keys and never re-run a broad predicate after authorization. Top-level upserts
select `TransactionMode::StateMachine` automatically; an upsert composed inside
`Database::transaction` must explicitly select that mode so an absent-key
decision is fenced across every backend. See [runtime writes and repository
operations](../../docs/reference/graphql-orm/runtime-and-writes.md#repository-helpers).

## Minimum entity

```rust
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(
    table = "accounts",
    plural = "Accounts",
    schema_policy = "managed",
    auth = "required"
)]
pub struct Account {
    #[primary_key]
    pub id: i64,

    #[filterable(type = "string")]
    #[sortable]
    #[graphql_orm(description = "Human-readable account label")]
    pub label: String,
}

schema_roots! {
    backend: "sqlite",
    schema_policy: "managed",
    auth: "required",
    entities: [Account],
}
```

This declares the types and schema builders. Connect a
`Database::<SqliteBackend>`, choose whether to validate/plan/apply schema, and
put the resulting `AppSchema` behind an `async-graphql` HTTP integration. A
complete runnable path belongs in the workspace quickstart; this package
README remains project-neutral.

## Core concepts

- **Entity vs. repository entity:** `GraphQLEntity` creates a GraphQL surface;
  `RepositoryEntity` creates typed persistence APIs with no GraphQL types.
- **Schema policy:** managed SQLite/PostgreSQL schemas may be planned and
  explicitly applied. An externally owned SQL Server table uses
  `external_read_only` by default or deliberate `external_writable` DML;
  neither mode receives generated schema mutation.
- **Generated operations:** `GraphQLOperations` emits potential root fields;
  `schema_roots!` chooses which generated mutations are exposed and creates the
  schema builders plus discovery catalog.
- **Authorization:** generated resolver authentication is `required`,
  `optional`, or `none`; row, field, repository, and database policy remain
  separate authoritative checks.
- **Operation metadata:** generated descriptors and fingerprints are discovery
  and drift evidence. They neither authorize a resolver nor disclose a field.
- **Semantic catalogue:** `graphql_orm_semantic_catalog()` is the canonical,
  versioned public API graph for descriptions, typed fields, relationships,
  capabilities, classification, export disposition, and root coordinates.
  It omits physical and policy internals and remains non-authoritative.
- **Typed aggregates:** every public persisted readable field participates in a
  closed generated aggregate-field enum. Repository code can group and compute
  multiple `COUNT`, `MIN`, `MAX`, and `SUM` expressions in the database;
  `aggregate = true` separately opts a schema into a generated bounded
  aggregate query root.
- **Conditional relations:** externally managed polymorphic references can use
  compile-time source or target discriminator conditions. Every generated
  resolver and batching path enforces the same bound-value predicate, and the
  declaration must disable physical foreign-key emission.
- **Server-defined ordering:** computed expressions and opt-in relation counts
  add direction-only fields to generated order inputs. Named expression binds
  can be supplied by an entity-owned function reading GraphQL server context;
  clients never provide SQL, identifiers, values, or aggregate functions.
  Missing primary-key columns are appended as deterministic pagination
  tie-breakers.
- **Checked calendar filters:** generated date predicates use half-open
  calendar ranges and recursively reject invalid spans, offsets, and ranges
  before database work. PostgreSQL uses session `CURRENT_DATE`, SQL Server uses
  server-local `GETDATE()`, and SQLite uses UTC `date('now')`.

## Errors and security boundaries

Public GraphQL errors use stable codes; database errors are converted through
the runtime rather than exposing backend details. Do not treat entity
descriptions, generated operation metadata, `auth = "required"`, or a tool
manifest as authorization. Install an `AuthSubject`/policy integration and
enforce row/field/database limits appropriate to the application.

## Documentation

- [Core reference index](../../docs/reference/graphql-orm/README.md)
- [Macro and attribute reference](../../docs/reference/graphql-orm/macros-and-attributes.md)
- [Backends and multi-backend workspaces](../../docs/reference/graphql-orm/backends.md)
- [Entities and relations](../../docs/reference/graphql-orm/entities-and-relations.md)
- [Typed grouped aggregates](../../docs/reference/graphql-orm/typed-aggregates.md)
- [Runtime writes and repository operations](../../docs/reference/graphql-orm/runtime-and-writes.md)
- [Schema management](../../docs/reference/graphql-orm/schema-management.md)
- [Strict authorization](../../docs/reference/graphql-orm/strict-authorization.md)
- [SQL Server integration](../../docs/reference/graphql-orm/mssql.md)
