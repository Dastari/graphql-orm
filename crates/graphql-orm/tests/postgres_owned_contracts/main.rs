//! Owned PostgreSQL pagination and migration contracts.
#![cfg(feature = "postgres")]

#[path = "../support/owned_postgres.rs"]
mod owned_postgres;

mod authorized_pagination_postgres;
mod postgres_constraint_index_idempotency;
