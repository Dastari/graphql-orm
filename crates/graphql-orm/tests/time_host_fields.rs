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
use std::any::Any;
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
    lifetime(field = "expires"),
    upsert = "id"
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
    #[graphql_orm(time_offset_datetime)]
    created_at: OffsetDateTime,
    #[graphql_orm(time_offset_datetime)]
    updated_at: Option<OffsetDateTime>,
}

#[cfg(feature = "mssql")]
schema_roots! {
    backend: "mssql", schema_policy: "external_writable",
    query_custom_ops: [], entities: [TimeHostField],
}
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [TimeHostField], }
#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [TimeHostField], }

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

struct HostTimestampQuery(TimeHostField);

#[async_graphql::Object]
impl HostTimestampQuery {
    async fn records(&self) -> Vec<TimeHostField> {
        vec![self.0.clone()]
    }
}

struct HostTimestampPolicy;
struct DenyTimestamp;

impl FieldPolicy<Backend> for HostTimestampPolicy {
    fn can_read_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        _: &'a Database<Backend>,
        entity: &'static str,
        field: &'static str,
        key: Option<&'static str>,
        record: Option<&'a (dyn Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            assert_eq!(entity, "TimeHostField");
            assert!(matches!(field, "recorded" | "expires"));
            assert_eq!(key, None);
            assert_eq!(
                record.unwrap().downcast_ref::<TimeHostField>().unwrap().id,
                1
            );
            Ok(ctx.data_opt::<DenyTimestamp>().is_none())
        })
    }

    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database<Backend>,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}

async fn assert_host_timestamp_output(mut db: Database<Backend>, mut record: TimeHostField) {
    use async_graphql::{EmptyMutation, EmptySubscription, PathSegment, Pos, Request, Schema};

    db.set_field_policy(HostTimestampPolicy);
    record.recorded = datetime!(2026-04-09 12:34:56.125 +05:30);
    let schema = Schema::build(
        HostTimestampQuery(record.clone()),
        EmptyMutation,
        EmptySubscription,
    )
    .data(db.clone())
    .finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("recorded: DateTime!"), "{sdl}");
    assert!(
        sdl.lines().any(|line| line.trim() == "expires: DateTime"),
        "{sdl}"
    );
    let response = schema.execute("{ records { recorded expires } }").await;
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(
        response.data.into_json().unwrap(),
        serde_json::json!({"records": [{"recorded": "2026-04-09T07:04:56.125Z", "expires": null}]}),
    );

    // A host resolver can return a constructed entity without going through row decoding.
    record.recorded = datetime!(0001-01-01 00:00 +01:00);
    record.expires = Some(datetime!(9999-12-31 23:59:59 -01:00));
    let schema = Schema::build(HostTimestampQuery(record), EmptyMutation, EmptySubscription)
        .data(db)
        .finish();
    for field in ["recorded", "expires"] {
        let query = format!("{{\n  list: records {{ alias: {field} }}\n}}");
        for (request, code) in [
            (Request::new(&query), "INTERNAL_ERROR"),
            (Request::new(&query).data(DenyTimestamp), "FORBIDDEN"),
        ] {
            let response = schema.execute(request).await;
            assert_eq!(response.errors.len(), 1, "{response:?}");
            let error = &response.errors[0];
            if code == "INTERNAL_ERROR" {
                assert_eq!(error.message, "internal error");
            }
            assert_eq!(
                error.extensions.as_ref().unwrap().get("code"),
                Some(&async_graphql::Value::from(code)),
            );
            assert_eq!(
                error.path,
                [
                    PathSegment::Field("list".into()),
                    PathSegment::Index(0),
                    PathSegment::Field("alias".into()),
                ],
            );
            assert_eq!(
                error.locations,
                [Pos {
                    line: 2,
                    column: 19
                }]
            );
        }
    }
}

async fn create_table(
    pool: &<Backend as OrmBackend>::Pool,
    #[cfg(feature = "mssql")] connection_string: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "mssql")]
    let _ = pool;
    use graphql_orm::graphql::orm::{SchemaModel, build_migration_plan};

    let model = SchemaModel::from_entities(&[TimeHostField::metadata()]);
    let empty = SchemaModel {
        extensions: vec![],
        tables: vec![],
    };
    let plan = build_migration_plan(Backend::DIALECT, &empty, &model);
    assert!(!plan.statements.is_empty());
    for statement in &plan.statements {
        #[cfg(feature = "mssql")]
        owned_mssql::execute_batch(connection_string, statement).await?;
        #[cfg(not(feature = "mssql"))]
        <Backend as WriteBackend>::execute_with_binds(pool, statement, &[]).await?;
    }

    Ok(())
}

#[tokio::test]
#[cfg_attr(not(feature = "sqlite"), ignore = "starts a disposable owned database")]
async fn repository_upsert_uses_the_timestamp_clock() -> Result<(), Box<dyn std::error::Error>> {
    exercise_upsert(false).await
}

#[tokio::test]
#[cfg_attr(not(feature = "sqlite"), ignore = "starts a disposable owned database")]
async fn graphql_upsert_uses_the_timestamp_clock() -> Result<(), Box<dyn std::error::Error>> {
    exercise_upsert(true).await
}

async fn exercise_upsert(graphql: bool) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let pool = graphql_orm::sqlx::SqlitePool::connect("sqlite::memory:").await?;
    #[cfg(feature = "postgres")]
    let pool = owned_postgres::OwnedPostgres::pool("time-upsert-clock", 1).await?;
    #[cfg(feature = "mssql")]
    let mut server = owned_mssql::OwnedSqlServer::start().await?;
    #[cfg(feature = "mssql")]
    let pool =
        graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(&server.connection_string)
            .await?;
    create_table(
        &pool,
        #[cfg(feature = "mssql")]
        &server.connection_string,
    )
    .await?;
    let db = Database::<Backend>::builder(pool.clone())
        .schema_policy(SchemaPolicy::ExternalWritable)
        .build();
    let schema = schema_builder(db.clone()).finish();
    let mut original_created_at = None;
    for (recorded, action) in [
        (datetime!(2026-04-09 12:00 UTC), ChangeAction::Created),
        (datetime!(2026-04-09 13:00 UTC), ChangeAction::Updated),
    ] {
        if graphql {
            let recorded = recorded.format(&time::format_description::well_known::Rfc3339)?;
            let response = schema
                .execute(
                    async_graphql::Request::new(format!(
                        r#"mutation {{ upsertTimeHostField(input: {{ id: 1, recorded: "{recorded}" }}) {{ success error action timeHostField {{ id createdAt updatedAt }} }} }}"#
                    ))
                    .data(AuthSubject::new("time-host-fields")),
                )
                .await;
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            let data = response.data.into_json()?;
            let result = &data["upsertTimeHostField"];
            assert_eq!(result["success"], true, "{result}");
            let expected = match action {
                ChangeAction::Created => "CREATED",
                ChangeAction::Updated => "UPDATED",
                ChangeAction::Deleted => unreachable!(),
            };
            assert_eq!(result["action"], expected, "{result}");
            assert!(result["timeHostField"]["updatedAt"].is_string(), "{result}");
        } else {
            let result = TimeHostField::upsert(
                &db,
                CreateTimeHostFieldInput {
                    id: 1,
                    recorded,
                    expires: None,
                },
            )
            .await?;
            assert_eq!(result.action, action);
            assert_eq!(result.entity.recorded, recorded);
        }
        let rows = Backend::fetch_rows(
            &pool,
            "SELECT id, recorded, expires, created_at, updated_at FROM time_host_fields",
            &[],
        )
        .await?;
        assert_eq!(rows.len(), 1);
        let row = TimeHostField::from_row(&rows[0])?;
        assert_eq!(row.recorded, recorded);
        assert!(row.updated_at.unwrap() > datetime!(2000-01-01 00:00 UTC));
        if let Some(created_at) = original_created_at {
            assert_eq!(row.created_at, created_at);
        } else {
            assert!(row.created_at > datetime!(2000-01-01 00:00 UTC));
            original_created_at = Some(row.created_at);
            // Force the conflict branch to prove it writes a fresh timestamp.
            <Backend as WriteBackend>::execute_with_binds(
                &pool,
                "UPDATE time_host_fields SET updated_at = NULL WHERE id = 1",
                &[],
            )
            .await?;
        }
    }

    #[cfg(feature = "mssql")]
    {
        drop(schema);
        drop(db);
        drop(pool);
        server.cleanup()?;
    }
    Ok(())
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "starts a disposable owned database"]
async fn postgres_text_and_binary_offset_decoding_agree() -> Result<(), Box<dyn std::error::Error>>
{
    use graphql_orm::sqlx::{Row, postgres::PgValueFormat};

    let pool = owned_postgres::OwnedPostgres::pool("time-protocol", 1).await?;
    let mut connection = pool.acquire().await?;
    for (zone, cases) in [
        (
            "Asia/Tokyo",
            &[
                (
                    "2026-04-09 07:04:56.123456+00",
                    Some(datetime!(2026-04-09 07:04:56.123456 UTC)),
                ),
                (
                    "9999-12-31 23:59:59.999999+00",
                    Some(datetime!(9999-12-31 23:59:59.999999 UTC)),
                ),
                ("10000-01-01 00:00:00+00", None),
            ][..],
        ),
        (
            "America/St_Johns",
            &[(
                "2026-04-09 07:04:56+00",
                Some(datetime!(2026-04-09 07:04:56 UTC)),
            )],
        ),
        (
            "Pacific/Honolulu",
            &[
                (
                    "0001-01-01 00:00:00+00",
                    Some(datetime!(0001-01-01 00:00 UTC)),
                ),
                ("0001-12-31 23:59:59.999999+00 BC", None),
            ],
        ),
        (
            "Africa/Monrovia",
            &[(
                "1900-04-09 07:04:56.123456+00",
                Some(datetime!(1900-04-09 07:04:56.123456 UTC)),
            )],
        ),
        (
            "Europe/Paris",
            &[
                (
                    "1900-04-09 07:04:56.123456+00",
                    Some(datetime!(1900-04-09 07:04:56.123456 UTC)),
                ),
                ("infinity", None),
                ("-infinity", None),
            ],
        ),
    ] {
        graphql_orm::sqlx::query("SELECT set_config('TimeZone', $1, false)")
            .bind(zone)
            .execute(&mut *connection)
            .await?;
        for (literal, expected) in cases {
            let query = format!("SELECT '{literal}'::timestamptz AS stamp");
            let binary = graphql_orm::sqlx::query(&query)
                .fetch_one(&mut *connection)
                .await?;
            let text = graphql_orm::sqlx::raw_sql(&query)
                .fetch_one(&mut *connection)
                .await?;
            for (row, format) in [
                (&binary, PgValueFormat::Binary),
                (&text, PgValueFormat::Text),
            ] {
                assert_eq!(row.try_get_raw("stamp")?.format(), format);
                let result = postgres_offset_datetime(row, "stamp");
                if let Some(expected) = expected {
                    let decoded = result?.expect("stamp is not null");
                    assert_eq!(decoded, *expected, "{zone}: {literal}");
                    assert_eq!(decoded.offset(), UtcOffset::UTC);
                } else {
                    match result {
                        Err(graphql_orm::sqlx::Error::ColumnDecode { source, .. }) => {
                            if literal.ends_with("infinity") {
                                assert_eq!(source.to_string(), "infinite PostgreSQL timestamp");
                            }
                        }
                        other => panic!("expected a decode error for {zone}: {literal}: {other:?}"),
                    }
                }
            }
        }
    }
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

    create_table(
        &pool,
        #[cfg(feature = "mssql")]
        &server.connection_string,
    )
    .await?;

    // Exercise PostgreSQL comparisons through the generated GraphQL query.
    #[cfg(feature = "postgres")]
    {
        graphql_orm::sqlx::query(
            "INSERT INTO time_host_fields (id, recorded) VALUES
             (10, '2026-04-09T07:04:56.125Z'),
             (11, '2026-04-09T08:04:56.125Z'),
             (12, '2026-04-08T15:04:56.125Z')",
        )
        .execute(&pool)
        .await?;
        let schema = schema_builder(Database::new(pool.clone())).finish();
        for (zone, filter, expected) in [
            (
                "UTC",
                r#"eq: "2026-04-09T12:34:56.125+05:30""#,
                Ok(Some((10, "2026-04-09T07:04:56.125Z"))),
            ),
            (
                "UTC",
                r#"between: {start: "2026-04-09", end: "2026-04-09T07:05:00Z"}"#,
                Ok(Some((10, "2026-04-09T07:04:56.125Z"))),
            ),
            (
                "Asia/Tokyo",
                r#"between: {start: "2026-04-09", end: "2026-04-09T00:05:00+09:00"}"#,
                Ok(Some((12, "2026-04-08T15:04:56.125Z"))),
            ),
            (
                "Asia/Tokyo",
                r#"between: {start: "2026-04-09 00:03:00", end: "2026-04-08T15:05:00Z"}"#,
                Ok(Some((12, "2026-04-08T15:04:56.125Z"))),
            ),
            (
                "Asia/Tokyo",
                r#"between: {start: "2026-04-09", end: "2026-04-09T00:05:00+10:00"}"#,
                Ok(None),
            ),
            (
                "Asia/Tokyo",
                r#"between: {start: "2026-04-09T00:00:00+09:00", end: "2026-04-09T00:05:00+10:00"}"#,
                Err("INVALID_INPUT"),
            ),
        ] {
            graphql_orm::sqlx::query("SELECT set_config('TimeZone', $1, false)")
                .bind(zone)
                .execute(&pool)
                .await?;
            let response = schema
                .execute(
                    async_graphql::Request::new(format!(
                        "{{ timeHostFields(where: {{recorded: {{{filter}}}}}) {{ edges {{ node {{ id recorded }} }} }} }}"
                    ))
                    .data(AuthSubject::new("time-host-fields")),
                )
                .await;
            match expected {
                Ok(row) => {
                    assert!(
                        response.errors.is_empty(),
                        "{filter}: {:?}",
                        response.errors
                    );
                    let edges = match row {
                        Some((id, stamp)) => {
                            serde_json::json!([{"node": {"id": id, "recorded": stamp}}])
                        }
                        None => serde_json::json!([]),
                    };
                    assert_eq!(response.data.into_json()?["timeHostFields"]["edges"], edges);
                }
                Err(code) => {
                    assert_eq!(response.errors.len(), 1, "{response:?}");
                    assert_eq!(
                        response.errors[0]
                            .extensions
                            .as_ref()
                            .and_then(|ext| ext.get("code")),
                        Some(&async_graphql::Value::String(code.into())),
                    );
                }
            }
        }
        graphql_orm::sqlx::query("DELETE FROM time_host_fields")
            .execute(&pool)
            .await?;
    }

    let stamp = datetime!(2026-04-09 12:34:56.123456789 +05:30);
    let expected_nanos = match Backend::DIALECT {
        DatabaseBackend::Postgres => 123_456_000,
        DatabaseBackend::Mssql => 123_456_700,
        DatabaseBackend::Sqlite => 123_456_789,
        DatabaseBackend::Mysql => unreachable!("fixture has no MySQL backend"),
    };
    let stored = stamp.replace_nanosecond(expected_nanos)?;
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
        "SELECT id, recorded, expires, created_at, updated_at FROM time_host_fields WHERE recorded = ? ORDER BY id",
        1,
    );
    let rows = Backend::fetch_rows(&pool, &select, &[SqlValue::from(stamp)]).await?;
    assert_eq!(rows.len(), 2);
    let first = TimeHostField::from_row(&rows[0])?;
    let second = TimeHostField::from_row(&rows[1])?;
    assert_eq!(first.expires, None);
    assert_eq!(second.expires, Some(stored));
    assert_eq!(first.recorded, stored);
    assert_eq!(second.recorded, stored);
    let offset = if cfg!(feature = "postgres") {
        UtcOffset::UTC
    } else {
        stamp.offset()
    };
    assert_eq!(first.recorded.offset(), offset);

    // Defaults must populate both clock columns on generated inserts.
    let db = Database::<Backend>::builder(pool.clone())
        .schema_policy(SchemaPolicy::ExternalWritable)
        .build();
    assert_host_timestamp_output(db.clone(), first.clone()).await;
    let created = TimeHostField::insert(
        &db,
        CreateTimeHostFieldInput {
            id: 3,
            recorded: stamp,
            expires: None,
        },
    )
    .await?;
    assert!(created.created_at > datetime!(2000-01-01 00:00 UTC));
    assert!(created.updated_at.unwrap() > datetime!(2000-01-01 00:00 UTC));
    <Backend as WriteBackend>::execute_with_binds(
        &pool,
        "UPDATE time_host_fields SET updated_at = NULL WHERE id = 3",
        &[],
    )
    .await?;
    let updated = TimeHostField::update_by_id(
        &db,
        &3,
        UpdateTimeHostFieldInput {
            expires: Some(Some(stamp)),
            ..Default::default()
        },
    )
    .await?
    .expect("generated update finds the inserted row");
    assert_eq!(updated.created_at, created.created_at);
    assert!(updated.updated_at.unwrap() > datetime!(2000-01-01 00:00 UTC));

    // These valid RFC 3339 inputs cross the supported years after conversion to UTC.
    for invalid in [
        datetime!(0001-01-01 00:00 +01:00),
        datetime!(9999-12-31 23:59:59 -01:00),
    ] {
        let result = TimeHostField::insert(
            &db,
            CreateTimeHostFieldInput {
                id: 4,
                recorded: invalid,
                expires: None,
            },
        )
        .await;
        assert!(result.is_err(), "generated write accepted {invalid}");
    }
    assert!(
        Backend::fetch_rows(&pool, "SELECT id FROM time_host_fields WHERE id = 4", &[])
            .await?
            .is_empty()
    );

    #[cfg(feature = "sqlite")]
    {
        let filter = TimeHostFieldWhereInput {
            recorded: Some(DateFilter {
                eq: Some("2026-04-09T12:34:56.123456789+05:30".into()),
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

    #[cfg(any(feature = "postgres", feature = "sqlite"))]
    {
        let schema = schema_builder(db.clone()).finish();
        #[cfg(feature = "postgres")]
        let invalid = [
            "'0002-01-01 00:00:00+00 BC'::timestamptz",
            "'10000-01-01 00:00:00+00'::timestamptz",
        ];
        #[cfg(feature = "sqlite")]
        let invalid = ["'0001-01-01T00:00:00+01:00'", "'9999-12-31T23:59:59-01:00'"];
        for literal in invalid {
            <Backend as WriteBackend>::execute_with_binds(
                &pool,
                &format!("UPDATE time_host_fields SET recorded = {literal} WHERE id = 1"),
                &[],
            )
            .await?;
            let response = schema
                .execute(
                    async_graphql::Request::new("{ timeHostField(id: 1) { recorded } }")
                        .data(AuthSubject::new("time-host-fields")),
                )
                .await;
            assert_eq!(response.errors.len(), 1, "{response:?}");
            let error = &response.errors[0];
            assert_eq!(error.message, "internal error");
            assert_eq!(
                error
                    .extensions
                    .as_ref()
                    .and_then(|extensions| extensions.get("code")),
                Some(&async_graphql::Value::String("INTERNAL_ERROR".into()))
            );
        }
    }
    #[cfg(feature = "mssql")]
    {
        drop(db);
        drop(pool);
        server.cleanup()?;
    }
    Ok(())
}
