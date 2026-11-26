//! Write tracking and transaction contracts in the PostgreSQL legacy lane.

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[allow(dead_code)]
#[path = "../support/owned_postgres.rs"]
mod owned_postgres;

mod row_write_tracker;
mod transaction_postgres;
