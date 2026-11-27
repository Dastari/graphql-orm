//! Runtime repository actors, timestamps and dependent writes.

#[cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]
#[allow(dead_code)]
#[path = "../support/owned_postgres.rs"]
mod owned_postgres;

mod runtime_auto_actor;
mod runtime_auto_timestamps;
mod runtime_dependent_deletes;
