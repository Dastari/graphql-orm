//! Owned PostgreSQL runtime reads and repository contracts.
#![cfg(feature = "postgres")]

#[path = "../support/owned_postgres.rs"]
mod owned_postgres;

mod repository_only_postgres;
mod runtime_records_postgres;
mod runtime_relations_postgres;
