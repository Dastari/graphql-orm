//! SQL Server transport, TLS and notification contracts.
#![cfg(feature = "mssql")]

#[path = "../support/owned_mssql.rs"]
mod owned_mssql;

mod mssql_notifications;
mod mssql_tls;
mod mssql_transport;
