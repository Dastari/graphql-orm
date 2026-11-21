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

#[derive(
    graphql_orm::prelude::RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize,
)]
#[repository_entity(table = "temporal_rows")]
struct TemporalRow {
    #[primary_key]
    id: String,
    calendar_date: NaiveDate,
    stamp: Option<NaiveDateTime>,
}

#[tokio::test]
#[cfg_attr(feature = "postgres", ignore = "starts disposable PostgreSQL")]
async fn generated_rows_use_the_native_chrono_decoder() -> Result<(), Box<dyn std::error::Error>> {
    use graphql_orm::graphql::orm::FromSqlRow;

    #[cfg(feature = "sqlite")]
    let pool = graphql_orm::sqlx::SqlitePool::connect("sqlite::memory:").await?;
    #[cfg(feature = "postgres")]
    let pool = owned_postgres::OwnedPostgres::pool("temporal-row-decoding", 1).await?;

    #[cfg(feature = "postgres")]
    {
        Backend::execute_with_binds(
            &pool,
            "CREATE TEMP TABLE temporal_rows (id TEXT, calendar_date DATE, stamp TIMESTAMP)",
            &[],
        )
        .await?;
        Backend::execute_with_binds(
            &pool,
            "INSERT INTO temporal_rows VALUES
                ('first', DATE '2026-04-09', TIMESTAMP '2026-04-09 12:34:56.125'),
                ('second', DATE '2026-04-09', NULL)",
            &[],
        )
        .await?;
        let rows = Backend::fetch_rows(
            &pool,
            "SELECT id, calendar_date, stamp FROM temporal_rows ORDER BY id",
            &[],
        )
        .await?;
        let present = TemporalRow::from_row(&rows[0])?;
        let absent = TemporalRow::from_row(&rows[1])?;
        assert_eq!(present.calendar_date, "2026-04-09".parse::<NaiveDate>()?);
        assert_eq!(present.stamp, Some("2026-04-09T12:34:56.125".parse()?));
        assert_eq!(absent.calendar_date, present.calendar_date);
        assert_eq!(absent.stamp, None);
    }

    #[cfg(feature = "sqlite")]
    for (timestamp_sql, expected) in [
        ("'2026-04-09T12:34:56Z'", Some("2026-04-09T12:34:56")),
        ("'2026-04-09T12:34:56+05:30'", Some("2026-04-09T12:34:56")),
        ("'2026-04-09 12:34'", Some("2026-04-09T12:34:00")),
        ("0", Some("1970-01-01T00:00:00")),
        ("2440587.5", Some("1970-01-01T00:00:00")),
        ("NULL", None),
    ] {
        let rows = Backend::fetch_rows(
            &pool,
            &format!("SELECT 'row' AS id, '2026-04-09' AS calendar_date, {timestamp_sql} AS stamp"),
            &[],
        )
        .await?;
        let row = TemporalRow::from_row(&rows[0])?;
        assert_eq!(row.calendar_date, "2026-04-09".parse::<NaiveDate>()?);
        assert_eq!(
            row.stamp,
            expected.map(str::parse).transpose()?,
            "{timestamp_sql}"
        );
    }
    Ok(())
}
