#![allow(
    clippy::collapsible_if,
    clippy::iter_cloned_collect,
    clippy::needless_lifetimes,
    clippy::new_without_default,
    clippy::too_many_arguments
)]

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

#[cfg(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql")
))]
compile_error!("Enable only one backend feature for graphql-orm.");

#[cfg(not(any(feature = "sqlite", feature = "postgres", feature = "mssql")))]
compile_error!("Enable exactly one backend feature for graphql-orm.");

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

#[cfg(feature = "sqlite")]
pub type DbPool = sqlx::SqlitePool;
#[cfg(feature = "sqlite")]
pub type DbRow = sqlx::sqlite::SqliteRow;

#[cfg(feature = "postgres")]
pub type DbPool = sqlx::PgPool;
#[cfg(feature = "postgres")]
pub type DbRow = sqlx::postgres::PgRow;

#[cfg(feature = "mssql")]
pub type DbPool = crate::db::mssql::MssqlPool;
#[cfg(feature = "mssql")]
pub type DbRow = crate::db::mssql::MssqlRow;

pub mod db;
pub mod graphql;
pub mod prelude;
pub mod types;
