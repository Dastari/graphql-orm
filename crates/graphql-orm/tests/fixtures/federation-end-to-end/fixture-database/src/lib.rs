//! One test-owned, seeded database per fixture subgraph, on the backend the
//! `sqlite`, `postgres` or `mssql` feature selects.
//!
//! SQLite is in-memory. PostgreSQL and SQL Server run in test-owned
//! containers started through the ORM's own owned-database helpers; never an
//! application database. Each container is recorded here and removed by
//! [`cleanup`], which [`CleanupGuard`] runs even when a test panics.

#[cfg(feature = "mssql")]
#[path = "../../../../support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "../../../../support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::db::Database;

/// The backend this build of the fixture targets.
pub const BACKEND: &str = if cfg!(feature = "mssql") {
    "mssql"
} else if cfg!(feature = "postgres") {
    "postgres"
} else {
    "sqlite"
};

#[cfg(feature = "mssql")]
type Owner = owned_mssql::OwnedSqlServer;
#[cfg(feature = "postgres")]
type Owner = owned_postgres::OwnedPostgres;
#[cfg(any(feature = "postgres", feature = "mssql"))]
static OWNED: std::sync::Mutex<Vec<Owner>> = std::sync::Mutex::new(Vec::new());

/// Create and seed a database for one subgraph.
///
/// `statements` run in order. SQL Server databases are read only to the
/// subgraph, so the statements run on a separate administrative connection
/// first.
pub async fn seeded(label: &str, statements: &[String]) -> Database {
    #[cfg(feature = "sqlite")]
    {
        let _ = label;
        let database = Database::connect_sqlite("sqlite::memory:")
            .await
            .expect("open the fixture database");
        for statement in statements {
            graphql_orm::sqlx::query(statement)
                .execute(database.pool())
                .await
                .expect("seed the fixture database");
        }
        database
    }
    #[cfg(feature = "postgres")]
    {
        let owner =
            owned_postgres::OwnedPostgres::start(label).expect("start a test-owned PostgreSQL");
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&owner.url)
            .await
            .expect("connect to the test-owned PostgreSQL");
        for statement in statements {
            graphql_orm::sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("seed the fixture database");
        }
        OWNED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(owner);
        Database::new(pool)
    }
    #[cfg(feature = "mssql")]
    {
        let _ = label;
        let owner = owned_mssql::OwnedSqlServer::start()
            .await
            .expect("start a test-owned SQL Server");
        owned_mssql::execute_batch(&owner.connection_string, &statements.join(";\n"))
            .await
            .expect("seed the fixture database");
        let database = Database::connect_ado(&owner.connection_string)
            .await
            .expect("connect to the test-owned SQL Server");
        OWNED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(owner);
        database
    }
}

/// A `TEXT`-like key or string column type for the selected backend.
pub fn text_type() -> &'static str {
    if cfg!(feature = "mssql") {
        "NVARCHAR(64)"
    } else {
        "TEXT"
    }
}

/// Remove every container this process started for the fixture.
pub fn cleanup() {
    #[cfg(any(feature = "postgres", feature = "mssql"))]
    {
        let owners = std::mem::take(
            &mut *OWNED
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        for mut owner in owners {
            owner
                .cleanup()
                .expect("remove the test-owned database container");
        }
    }
}

/// Runs [`cleanup`] when dropped, including during a panic.
pub struct CleanupGuard;

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        cleanup();
    }
}
