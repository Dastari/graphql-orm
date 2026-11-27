//! Host-selected OffsetDateTime fields across the three backend mappings.
#![cfg(all(
    feature = "time",
    any(
        all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
        all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))),
        all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres")))
    )
))]

use graphql_orm::prelude::*;
use time::{OffsetDateTime, UtcOffset, macros::datetime};

#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;
#[cfg(feature = "mssql")]
type Backend = MssqlBackend;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "time_host_fields",
    plural = "TimeHostFields",
    schema_policy = "external_writable",
    lifetime(field = "expires")
)]
struct TimeHostField {
    #[primary_key]
    #[sortable]
    #[graphql_orm(auto_generated = false)]
    id: i64,
    #[graphql_orm(time_offset_datetime)]
    #[filterable(type = "date")]
    recorded: OffsetDateTime,
    #[graphql_orm(time_offset_datetime)]
    #[filterable(type = "date")]
    expires: Option<OffsetDateTime>,
}

#[test]
fn derive_maps_the_column_and_lifetime_metadata() {
    let expected = match Backend::DIALECT {
        DatabaseBackend::Sqlite => "TEXT",
        DatabaseBackend::Postgres => "TIMESTAMPTZ",
        DatabaseBackend::Mssql => "DATETIMEOFFSET",
        DatabaseBackend::Mysql => unreachable!("fixture has no MySQL backend"),
    };
    let columns = TimeHostField::columns();
    assert_eq!(columns[1].sql_type, expected);
    assert!(!columns[1].nullable);
    assert_eq!(columns[2].sql_type, expected);
    assert!(columns[2].nullable);
    let lifetime = TimeHostField::graphql_lifetime().unwrap();
    assert_eq!(lifetime.encoding, GraphqlLifetimeEncoding::Rfc3339Utc);
}

#[test]
fn time_scalars_use_the_existing_temporal_kind_surface() {
    for (value, expected) in [
        (
            SqlValue::from(datetime!(2026-04-09 12:34:56).date()),
            "2026-04-09",
        ),
        (
            SqlValue::from(datetime!(2026-04-09 12:34:56.125)),
            "2026-04-09 12:34:56.125",
        ),
    ] {
        let SqlValue::Temporal(value) = value else {
            panic!("expected a temporal parameter")
        };
        assert_eq!(value.as_text(), expected);
    }
    let SqlValue::Temporal(value) = SqlValue::from(datetime!(2026-04-09 12:34:56.125 +05:30))
    else {
        panic!("expected an offset timestamp parameter");
    };
    assert_eq!(value.as_text(), "2026-04-09 12:34:56.125 +05:30");
    assert!(matches!(value.kind(), TemporalKind::DateTimeOffset(_)));
}

#[test]
fn generated_update_distinguishes_omitted_and_explicit_null_timestamps() -> graphql_orm::Result<()>
{
    let mut clauses = Vec::new();
    let mut changed = Vec::new();
    let mut binds = Vec::new();
    let mut input = UpdateTimeHostFieldInput::default();
    TimeHostField::__gom_push_update_assignments(&input, &mut clauses, &mut changed, &mut binds)?;
    assert!(binds.is_empty());
    input.expires = Some(None);
    TimeHostField::__gom_push_update_assignments(&input, &mut clauses, &mut changed, &mut binds)?;
    assert_eq!(binds, [SqlValue::OffsetDateTimeNull]);
    let stamp = datetime!(2026-04-09 12:34:56.125 +05:30);
    input.expires = Some(Some(stamp));
    binds.clear();
    TimeHostField::__gom_push_update_assignments(&input, &mut clauses, &mut changed, &mut binds)?;
    assert_eq!(binds, [SqlValue::from(stamp)]);
    Ok(())
}

#[tokio::test]
#[cfg_attr(not(feature = "sqlite"), ignore = "starts a disposable owned database")]
async fn binds_and_generated_decode_round_trip_nullable_fields()
-> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let pool = graphql_orm::sqlx::SqlitePool::connect("sqlite::memory:").await?;
    #[cfg(feature = "postgres")]
    let pool = owned_postgres::OwnedPostgres::pool("time-host-fields", 1).await?;
    #[cfg(feature = "mssql")]
    let mut server = owned_mssql::OwnedSqlServer::start().await?;
    #[cfg(feature = "mssql")]
    let pool =
        graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(&server.connection_string)
            .await?;

    let sql_type = TimeHostField::columns()[1].sql_type;
    let create = format!(
        "CREATE TABLE time_host_fields (id BIGINT NOT NULL PRIMARY KEY, recorded {sql_type} NOT NULL, expires {sql_type} NULL)"
    );
    #[cfg(feature = "mssql")]
    owned_mssql::execute_batch(&server.connection_string, &create).await?;
    #[cfg(not(feature = "mssql"))]
    <Backend as WriteBackend>::execute_with_binds(&pool, &create, &[]).await?;

    let stamp = datetime!(2026-04-09 12:34:56.125 +05:30);
    let insert = Backend::DIALECT.normalize_sql(
        "INSERT INTO time_host_fields (id, recorded, expires) VALUES (?, ?, ?)",
        1,
    );
    for (id, expires) in [
        (1, SqlValue::OffsetDateTimeNull),
        (2, SqlValue::from(stamp)),
    ] {
        <Backend as WriteBackend>::execute_with_binds(
            &pool,
            &insert,
            &[SqlValue::Int(id), SqlValue::from(stamp), expires],
        )
        .await?;
    }
    let select = Backend::DIALECT.normalize_sql(
        "SELECT id, recorded, expires FROM time_host_fields WHERE recorded = ? ORDER BY id",
        1,
    );
    let rows = Backend::fetch_rows(&pool, &select, &[SqlValue::from(stamp)]).await?;
    assert_eq!(rows.len(), 2);
    let first = TimeHostField::from_row(&rows[0])?;
    let second = TimeHostField::from_row(&rows[1])?;
    assert_eq!(first.expires, None);
    assert_eq!(second.expires, Some(stamp));
    assert_eq!(first.recorded, stamp);
    assert_eq!(second.recorded, stamp);
    let offset = if cfg!(feature = "postgres") {
        UtcOffset::UTC
    } else {
        stamp.offset()
    };
    assert_eq!(first.recorded.offset(), offset);

    #[cfg(feature = "sqlite")]
    {
        let filter = TimeHostFieldWhereInput {
            recorded: Some(DateFilter {
                eq: Some("2026-04-09T12:34:56.125+05:30".into()),
                is_today: Some(true),
                ..Default::default()
            }),
            expires: Some(DateFilter {
                is_null: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let today = graphql_orm::chrono::NaiveDate::from_ymd_opt(2026, 4, 9).unwrap();
        assert!(filter.matches_entity_at(&first, today)?);
        assert!(!filter.matches_entity_at(&second, today)?);
    }

    #[cfg(feature = "postgres")]
    {
        let invalid = Backend::fetch_rows(
            &pool,
            "SELECT 1::bigint AS id, 'infinity'::timestamptz AS recorded, NULL::timestamptz AS expires",
            &[],
        )
        .await?;
        assert!(matches!(
            TimeHostField::from_row(&invalid[0]),
            Err(graphql_orm::sqlx::Error::ColumnDecode { .. })
        ));
    }
    #[cfg(feature = "mssql")]
    {
        drop(pool);
        server.cleanup()?;
    }
    Ok(())
}
