//! `LIKE` escaping of every pattern character, on SQLite and PostgreSQL.
//! `mssql_correctness.rs` runs the same matrix on SQL Server.
#![cfg(any(feature = "sqlite", feature = "postgres"))]

#[allow(dead_code)]
#[path = "support/like_matrix.rs"]
mod like_matrix;

#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::graphql::orm::{DatabaseBackend, SqlDialect, contains_like_pattern};

/// `SELECT` of the haystack values that match one bound pattern.
fn matching_sql(dialect: DatabaseBackend, values: usize) -> String {
    let rows = (1..=values)
        .map(|index| format!("({}, {})", index, dialect.placeholder(index)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "WITH haystack(position, value) AS (VALUES {rows}) \
         SELECT value FROM haystack WHERE {} ORDER BY position",
        dialect.ci_like("value", &dialect.placeholder(values + 1))
    )
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_matches_every_pattern_character_literally() -> Result<(), Box<dyn std::error::Error>>
{
    let pool = graphql_orm::sqlx::SqlitePool::connect("sqlite::memory:").await?;
    let values = like_matrix::haystacks();
    let sql = matching_sql(DatabaseBackend::Sqlite, values.len());
    for needle in like_matrix::NEEDLES {
        let mut query = graphql_orm::sqlx::query_scalar::<_, String>(&sql);
        for value in &values {
            query = query.bind(value);
        }
        let found = query
            .bind(contains_like_pattern(needle))
            .fetch_all(&pool)
            .await?;
        assert_eq!(found, like_matrix::expected(needle), "needle {needle:?}");
    }
    Ok(())
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn postgres_matches_every_pattern_character_literally()
-> Result<(), Box<dyn std::error::Error>> {
    let mut postgres = owned_postgres::OwnedPostgres::start("like-escape-matrix")?;
    let pool = graphql_orm::sqlx::PgPool::connect(&postgres.url).await?;
    let values = like_matrix::haystacks();
    let sql = matching_sql(DatabaseBackend::Postgres, values.len());
    for needle in like_matrix::NEEDLES {
        let mut query = graphql_orm::sqlx::query_scalar::<_, String>(&sql);
        for value in &values {
            query = query.bind(value);
        }
        let found = query
            .bind(contains_like_pattern(needle))
            .fetch_all(&pool)
            .await?;
        assert_eq!(found, like_matrix::expected(needle), "needle {needle:?}");
    }
    pool.close().await;
    postgres.cleanup()?;
    Ok(())
}
