//! Bulk and single-row write refusal contracts.

#[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
#[allow(dead_code)]
#[path = "../support/owned_mssql.rs"]
mod owned_mssql;

#[cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]
#[allow(dead_code)]
#[path = "../support/owned_postgres.rs"]
mod owned_postgres;

mod bulk_refusals;
mod single_row_refusals;
