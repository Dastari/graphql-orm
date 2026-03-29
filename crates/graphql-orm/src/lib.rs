pub use async_graphql;
pub use futures;
pub use graphql_orm_macros::*;
pub use serde_json;
pub use sqlx;
pub use tokio;
pub use tokio_stream;
pub use uuid;

#[cfg(all(feature = "sqlite", feature = "postgres"))]
compile_error!("Enable only one backend feature for graphql-orm.");

#[cfg(not(any(feature = "sqlite", feature = "postgres")))]
compile_error!("Enable exactly one backend feature for graphql-orm.");

#[cfg(feature = "sqlite")]
pub type DbPool = sqlx::SqlitePool;
#[cfg(feature = "sqlite")]
pub type DbRow = sqlx::sqlite::SqliteRow;

#[cfg(feature = "postgres")]
pub type DbPool = sqlx::PgPool;
#[cfg(feature = "postgres")]
pub type DbRow = sqlx::postgres::PgRow;

pub mod db;
pub mod graphql;
pub mod prelude;
pub mod types;
