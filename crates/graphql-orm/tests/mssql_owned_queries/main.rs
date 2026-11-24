//! Owned SQL Server query, decoding, and backend correctness contracts.
#![cfg(feature = "mssql")]

#[path = "../support/owned_mssql.rs"]
mod owned_mssql;

mod exact_string_filters;
mod mssql_correctness;
mod mssql_integration;
