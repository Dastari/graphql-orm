//! Owned PostgreSQL schema qualification and retention contracts.

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[allow(dead_code)]
#[path = "../support/owned_postgres.rs"]
mod owned_postgres;

mod postgres_qualified_tables;
mod retention_purge_postgres;
