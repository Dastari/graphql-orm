#![allow(
    clippy::collapsible_if,
    clippy::iter_cloned_collect,
    clippy::needless_lifetimes,
    clippy::new_without_default,
    clippy::too_many_arguments
)]

//! Runtime crate for `graphql-orm`.
//!
//! `graphql-orm` combines this runtime with the `graphql-orm-macros` derive
//! crate to generate `async-graphql` queries, mutations, subscriptions,
//! filters, ordering, pagination, relation loading, schema metadata, and
//! repository helpers from Rust structs.
//!
//! # Backends
//!
//! Enable one or more backend features:
//!
//! - `sqlite`
//! - `postgres`
//! - `mssql`
//!
//! SQLite and Postgres support reads, writes, subscriptions, schema
//! validation, planning, and explicit migrations. MSSQL is read/query-only and
//! uses Tiberius instead of SQLx because current SQLx does not support SQL
//! Server.
//!
//! When exactly one backend feature is enabled, compatibility aliases
//! `DbPool` and `DbRow` are exported. When multiple backend features are
//! enabled by Cargo feature unification, generated entities and schema roots
//! must select a backend explicitly.
//!
//! # Schema Ownership
//!
//! Backend features only compile database support. Schema ownership is selected
//! at runtime with [`graphql::orm::SchemaPolicy`].
//!
//! ```ignore
//! use graphql_orm::prelude::*;
//!
//! let database = Database::builder(pool)
//!     .schema_policy(SchemaPolicy::Managed)
//!     .build();
//!
//! let report = database
//!     .schema()
//!     .validate_against_entities(&[User::metadata()])
//!     .await?;
//! ```
//!
//! `Database::new`, `Database::builder`, and GraphQL schema construction never
//! apply schema changes. Use `database.schema()` for explicit validation,
//! planning, and migration application.
//!
//! # Spatial Fields
//!
//! PostgreSQL and SQLite entities can expose spatial values as GeoJSON through
//! `serde_json::Value` fields. PostgreSQL stores those fields as PostGIS
//! `geometry(<type>, <srid>)` columns and can create GiST spatial indexes.
//! SQLite stores canonical GeoJSON in `TEXT` columns and evaluates spatial
//! predicates in Rust, which keeps the entity API portable at the cost of
//! large-table query efficiency.
//!
//! ```ignore
//! #[graphql_orm(spatial(kind = "geometry", geometry_type = "Point", srid = 4326, index = true))]
//! #[filterable(type = "spatial")]
//! pub location: serde_json::Value;
//! ```
//!
//! Spatial filters support `equals`, `disjoint`, `intersects`, `touches`,
//! `crosses`, `within`, `contains`, `overlaps`, and `is_null`.
//!
//! # Full-Text Search
//!
//! Text fields can be marked searchable. Generated operations then expose a
//! backend-neutral search resolver and Rust query helper. PostgreSQL uses
//! managed `tsvector` search tables and GIN indexes. SQLite uses FTS5 where
//! available, with fallback scoring available through the runtime search
//! document path.
//!
//! ```ignore
//! #[graphql_orm(search(index = true, language = "english"))]
//! pub struct Article {
//!     #[primary_key]
//!     pub id: uuid::Uuid,
//!
//!     #[graphql_orm(searchable(weight = "A"))]
//!     pub title: String,
//! }
//!
//! let hits = Article::search(&pool, SearchInput {
//!     query: "melbourne park".to_string(),
//!     mode: Some(SearchMode::Web),
//!     min_score: None,
//! })
//! .limit(20)
//! .fetch_all()
//! .await?;
//! ```
//!
//! Managed migrations create search tables and indexes, but they do not
//! backfill existing data automatically. Run the generated
//! `Entity::rebuild_search_index(&database)` helper after adding or changing a
//! search index on an existing table.
//! Native PostgreSQL and SQLite FTS5 search paths push score, count, limit, and
//! offset into SQL. PostgreSQL requests that carry a database auth context still
//! use native search inside the same transaction-local context used for RLS.
//!
//! # Pagination And Relation Loading
//!
//! Generated connections use offset-style cursors. [`crate::graphql::orm::PageInput`]
//! clamps negative offsets to `0`. [`crate::graphql::orm::PaginationConfig`]
//! controls generated connection defaults and caps; the default configuration
//! applies a limit of `1000` when `page.limit` is omitted and clamps explicit
//! limits to `1000`. Configure it on [`crate::db::Database::builder`] for
//! services that need smaller pages, larger sync/export pages, or intentionally
//! unbounded generated connections. Repository-style `fetch_all` helpers remain
//! unbounded unless the caller supplies pagination. Host code that inspects
//! [`crate::graphql::orm::PageInput`] directly should use
//! [`crate::graphql::orm::PageInput::limit_with_config`] or
//! [`crate::graphql::orm::PaginationConfig::resolve_page`]; the legacy
//! `PageInput::limit()` helper is deprecated because it can only use the default
//! cap. Paged relation batches use backend window functions where available so
//! nested relation pages do not need to load every child row for every parent.
//!
//! # Generated Entity Example
//!
//! ```ignore
//! use graphql_orm::prelude::*;
//!
//! #[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
//! #[graphql_entity(table = "users", plural = "Users")]
//! pub struct User {
//!     #[primary_key]
//!     pub id: i64,
//!     #[filterable]
//!     #[sortable]
//!     pub name: String,
//! }
//! ```
//!
//! See the repository `README.md` and `docs/` directory for full examples,
//! backend notes, relation batching, schema policies, and migration guidance.

pub use async_graphql;
pub use futures;
pub use graphql_orm_macros::*;
pub use serde_json;
pub use sqlx;
#[cfg(feature = "mssql")]
pub use tiberius;
pub use tokio;
pub use tokio_stream;
#[cfg(feature = "mssql")]
pub use tokio_util;
pub use uuid;

#[cfg(not(any(feature = "sqlite", feature = "postgres", feature = "mssql")))]
compile_error!("Enable at least one backend feature for graphql-orm.");

#[cfg(any(
    all(feature = "resolver-case-pascal", feature = "resolver-case-snake"),
    all(
        feature = "resolver-case-pascal",
        feature = "resolver-case-screaming-snake"
    ),
    all(feature = "resolver-case-pascal", feature = "resolver-case-lower"),
    all(feature = "resolver-case-pascal", feature = "resolver-case-upper"),
    all(
        feature = "resolver-case-snake",
        feature = "resolver-case-screaming-snake"
    ),
    all(feature = "resolver-case-snake", feature = "resolver-case-lower"),
    all(feature = "resolver-case-snake", feature = "resolver-case-upper"),
    all(
        feature = "resolver-case-screaming-snake",
        feature = "resolver-case-lower"
    ),
    all(
        feature = "resolver-case-screaming-snake",
        feature = "resolver-case-upper"
    ),
    all(feature = "resolver-case-lower", feature = "resolver-case-upper")
))]
compile_error!("Enable at most one resolver-case-* feature for graphql-orm.");

#[cfg(any(
    all(feature = "argument-case-pascal", feature = "argument-case-snake"),
    all(
        feature = "argument-case-pascal",
        feature = "argument-case-screaming-snake"
    ),
    all(feature = "argument-case-pascal", feature = "argument-case-lower"),
    all(feature = "argument-case-pascal", feature = "argument-case-upper"),
    all(
        feature = "argument-case-snake",
        feature = "argument-case-screaming-snake"
    ),
    all(feature = "argument-case-snake", feature = "argument-case-lower"),
    all(feature = "argument-case-snake", feature = "argument-case-upper"),
    all(
        feature = "argument-case-screaming-snake",
        feature = "argument-case-lower"
    ),
    all(
        feature = "argument-case-screaming-snake",
        feature = "argument-case-upper"
    ),
    all(feature = "argument-case-lower", feature = "argument-case-upper")
))]
compile_error!("Enable at most one argument-case-* feature for graphql-orm.");

#[cfg(any(
    all(feature = "field-case-pascal", feature = "field-case-snake"),
    all(feature = "field-case-pascal", feature = "field-case-screaming-snake"),
    all(feature = "field-case-pascal", feature = "field-case-lower"),
    all(feature = "field-case-pascal", feature = "field-case-upper"),
    all(feature = "field-case-snake", feature = "field-case-screaming-snake"),
    all(feature = "field-case-snake", feature = "field-case-lower"),
    all(feature = "field-case-snake", feature = "field-case-upper"),
    all(feature = "field-case-screaming-snake", feature = "field-case-lower"),
    all(feature = "field-case-screaming-snake", feature = "field-case-upper"),
    all(feature = "field-case-lower", feature = "field-case-upper")
))]
compile_error!("Enable at most one field-case-* feature for graphql-orm.");

pub use crate::graphql::orm::{
    DefaultBackend, DefaultWriteBackend, IntrospectionBackend, MigrationBackend, MssqlBackend,
    NoDefaultBackend, OrmBackend, PostgresBackend, SqliteBackend, SqlxBackend, SubscriptionBackend,
    WriteBackend,
};

#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))),
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres")))
))]
/// Compatibility pool alias exported only when exactly one backend feature is enabled.
///
/// In multi-backend builds, use explicit backend pool types such as
/// `<SqliteBackend as OrmBackend>::Pool` or `Database<MssqlBackend>` instead.
pub type DbPool = <DefaultBackend as OrmBackend>::Pool;

#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))),
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres")))
))]
/// Compatibility row alias exported only when exactly one backend feature is enabled.
///
/// In multi-backend builds, use explicit backend row types such as
/// `<PostgresBackend as OrmBackend>::Row`.
pub type DbRow = <DefaultBackend as OrmBackend>::Row;

/// Database handle, builder, backend-specific pool helpers, and runtime configuration.
pub mod db;
/// ORM runtime traits, filters, pagination, relation loading, schema models, and migrations.
pub mod graphql;
/// Common imports for applications using generated `graphql-orm` code.
pub mod prelude;
/// Shared scalar and input helper types used by generated GraphQL surfaces.
pub mod types;
