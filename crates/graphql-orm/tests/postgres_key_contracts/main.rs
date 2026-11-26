//! Portable key and parameter contracts in the PostgreSQL legacy lane.

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[allow(dead_code)]
#[path = "../support/owned_postgres.rs"]
mod owned_postgres;

mod binary_keys;
mod portable_primitives_postgres;
