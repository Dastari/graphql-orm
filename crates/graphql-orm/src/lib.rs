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
