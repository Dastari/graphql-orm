//! Compare temporal parameters with the text binds used before native SQL Server support.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]

use graphql_orm::chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use graphql_orm::graphql::filters::DateFilter;
use graphql_orm::graphql::orm::{DatabaseBackend, OrmBackend, SqlValue, WriteBackend};
use graphql_orm::sqlx::Row;

#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[cfg(feature = "sqlite")]
type Backend = graphql_orm::graphql::orm::SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = graphql_orm::graphql::orm::PostgresBackend;

#[tokio::test]
#[cfg_attr(feature = "postgres", ignore = "starts disposable PostgreSQL")]
async fn temporal_parameters_keep_the_previous_text_storage_and_comparison()
-> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let pool = graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    #[cfg(feature = "postgres")]
    let mut owned = owned_postgres::OwnedPostgres::start("temporal-bind-compatibility")?;
    #[cfg(feature = "postgres")]
    let pool = graphql_orm::sqlx::PgPool::connect(&owned.url).await?;

    Backend::execute_with_binds(&pool, "CREATE TABLE temporal_values (value TEXT)", &[]).await?;
    let backend = if cfg!(feature = "sqlite") {
        DatabaseBackend::Sqlite
    } else {
        DatabaseBackend::Postgres
    };
    let insert = if cfg!(feature = "sqlite") {
        "INSERT INTO temporal_values VALUES (?)"
    } else {
        "INSERT INTO temporal_values VALUES ($1)"
    };
    let examples = [
        (
            SqlValue::from("2026-04-09".parse::<NaiveDate>()?),
            "2026-04-09",
        ),
        (
            SqlValue::from("2026-04-09T12:34:56.125".parse::<NaiveDateTime>()?),
            "2026-04-09 12:34:56.125",
        ),
        (
            SqlValue::from(DateTime::parse_from_rfc3339(
                "2026-04-09T12:34:56.125+05:30",
            )?),
            "2026-04-09 12:34:56.125 +05:30",
        ),
        (
            SqlValue::from("2026-04-09T12:34:56.125Z".parse::<DateTime<Utc>>()?),
            "2026-04-09 12:34:56.125 UTC",
        ),
        (
            SqlValue::temporal_text("2026-04-09T12:34:56.125000Z".into()),
            "2026-04-09T12:34:56.125000Z",
        ),
    ];
    for (parameter, original) in examples {
        Backend::execute_with_binds(&pool, insert, &[parameter]).await?;
        Backend::execute_with_binds(&pool, insert, &[SqlValue::String(original.to_owned())])
            .await?;
        let (conditions, values) = DateFilter {
            eq: Some(original.to_owned()),
            ..Default::default()
        }
        .render_sql(backend, "value", 1)?;
        let query = format!(
            "SELECT value FROM temporal_values WHERE {}",
            conditions.join(" AND ")
        );
        let rows = Backend::fetch_rows(&pool, &query, &values).await?;
        assert_eq!(rows.len(), 2, "{original}");
        for row in rows {
            assert_eq!(
                row.try_get::<String, _>("value")?.as_bytes(),
                original.as_bytes()
            );
        }
    }

    #[cfg(feature = "postgres")]
    {
        let original = "2026-04-09T12:34:56.125000+05:30";
        let values = [
            SqlValue::temporal_text(original.to_owned()),
            SqlValue::String(original.to_owned()),
        ];
        Backend::execute_with_binds(
            &pool,
            "CREATE TABLE temporal_instants (value TIMESTAMPTZ)",
            &[],
        )
        .await?;
        Backend::execute_with_binds(
            &pool,
            "INSERT INTO temporal_instants VALUES ($1::timestamptz), ($2::timestamptz)",
            &values,
        )
        .await?;
        let rows = Backend::fetch_rows(
            &pool,
            "SELECT value FROM temporal_instants WHERE value = $1::timestamptz",
            &values[..1],
        )
        .await?;
        assert_eq!(rows.len(), 2);
        let expected = DateTime::parse_from_rfc3339(original)?.to_utc();
        for row in rows {
            assert_eq!(row.try_get::<DateTime<Utc>, _>("value")?, expected);
        }
        pool.close().await;
        owned.cleanup()?;
    }
    Ok(())
}
