//! Managed row versions and update assurance contracts.

#[cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]
#[allow(dead_code)]
#[path = "../support/owned_mssql.rs"]
mod owned_mssql;

#[cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]
#[allow(dead_code)]
#[path = "../support/owned_postgres.rs"]
mod owned_postgres;

#[cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))),
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
))]
#[path = "../support/graphql_case.rs"]
mod graphql_case;

mod managed_version;
mod update_assurance;
mod update_assurance_registry;
