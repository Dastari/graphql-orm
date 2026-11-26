//! Bulk write targeting and transaction scoping contracts.

#[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
#[allow(dead_code)]
#[path = "../support/owned_mssql.rs"]
mod owned_mssql;

#[cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]
#[allow(dead_code)]
#[path = "../support/owned_postgres.rs"]
mod owned_postgres;

mod bulk_write_targets;
mod transaction_scope_backends;
