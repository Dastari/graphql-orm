//! Owned SQL Server generated write and trigger contracts.
#![cfg(feature = "mssql")]

#[path = "../support/owned_mssql.rs"]
mod owned_mssql;

mod mssql_trigger_writes;
mod mssql_writes;
