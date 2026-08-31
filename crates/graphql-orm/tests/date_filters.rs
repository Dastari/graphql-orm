#![cfg(feature = "sqlite")]

use graphql_orm::async_graphql::{EmptyMutation, EmptySubscription, Schema};
use graphql_orm::graphql::filters::{
    DateFilter, DateRangeInput, MAX_CALENDAR_DAY_SPAN, MAX_RELATIVE_DAY_OFFSET, RelativeDateInput,
    SpatialFilter,
};
use graphql_orm::graphql::orm::spatial::date_filter_matches_at;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Deserialize, serde::Serialize)]
#[graphql_entity(
    table = "calendar_spatial_records",
    plural = "CalendarSpatialRecords",
    backend = "sqlite",
    auth = "none"
)]
struct CalendarSpatialRecord {
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    id: String,

    #[filterable(type = "date")]
    occurred_at: Option<String>,

    #[graphql_orm(spatial(kind = "geometry", geometry_type = "Point", srid = 4326))]
    #[filterable(type = "spatial")]
    location: graphql_orm::serde_json::Value,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct DateTime(String);

#[graphql_orm::async_graphql::Scalar]
impl graphql_orm::async_graphql::ScalarType for DateTime {
    fn parse(
        value: graphql_orm::async_graphql::Value,
    ) -> graphql_orm::async_graphql::InputValueResult<Self> {
        let graphql_orm::async_graphql::Value::String(value) = value else {
            return Err(graphql_orm::async_graphql::InputValueError::expected_type(
                value,
            ));
        };
        Ok(Self(value))
    }

    fn to_value(&self) -> graphql_orm::async_graphql::Value {
        graphql_orm::async_graphql::Value::String(self.0.clone())
    }
}

#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Deserialize, serde::Serialize)]
#[graphql_entity(table = "string_date_semantics", plural = "StringDateSemantics")]
struct StringDateSemantics {
    #[primary_key]
    id: String,
    #[date_field]
    #[filterable(type = "date")]
    observed_at: String,
}

#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Deserialize, serde::Serialize)]
#[graphql_entity(table = "datetime_date_semantics", plural = "DateTimeDateSemantics")]
struct DateTimeDateSemantics {
    #[primary_key]
    id: String,
    #[date_field]
    #[filterable(type = "date")]
    observed_at: Option<DateTime>,
}

fn anchor() -> graphql_orm::chrono::NaiveDate {
    graphql_orm::chrono::NaiveDate::from_ymd_opt(2026, 8, 31).expect("valid fixed date")
}

fn point(x: f64, y: f64) -> graphql_orm::serde_json::Value {
    graphql_orm::serde_json::json!({ "type": "Point", "coordinates": [x, y] })
}

fn point_filter(x: f64, y: f64) -> SpatialFilter {
    SpatialFilter {
        equals: Some(graphql_orm::async_graphql::Json(point(x, y))),
        ..Default::default()
    }
}

fn record(occurred_at: Option<&str>) -> CalendarSpatialRecord {
    CalendarSpatialRecord {
        id: "record-1".to_owned(),
        occurred_at: occurred_at.map(str::to_owned),
        location: point(1.0, 2.0),
    }
}

fn matches(filter: &CalendarSpatialRecordWhereInput, entity: &CalendarSpatialRecord) -> bool {
    DatabaseFilter::matches_entity_at(filter, entity, anchor()).expect("matcher succeeds")
}

#[test]
fn calendar_operators_use_half_open_fixed_anchor_ranges() {
    let today_midday = "2026-08-31T14:45:00";
    let yesterday_last = "2026-08-30T23:59:59.999999";
    let yesterday_start = "2026-08-30T00:00:00";
    let two_days_ago = "2026-08-29T23:59:59.999999";
    let tomorrow_start = "2026-09-01T00:00:00";
    let tomorrow_midday = "2026-09-01T14:45:00";
    let day_after_start = "2026-09-02T00:00:00";

    let today_filter = DateFilter {
        is_today: Some(true),
        ..Default::default()
    };
    assert!(!date_filter_matches_at(
        Some(yesterday_last),
        &today_filter,
        anchor(),
    ));
    assert!(date_filter_matches_at(
        Some("2026-08-31T00:00:00"),
        &today_filter,
        anchor(),
    ));
    assert!(!date_filter_matches_at(
        Some(tomorrow_start),
        &today_filter,
        anchor(),
    ));

    assert!(date_filter_matches_at(
        Some(today_midday),
        &today_filter,
        anchor(),
    ));
    assert!(!date_filter_matches_at(
        Some(today_midday),
        &DateFilter {
            in_future: Some(true),
            ..Default::default()
        },
        anchor(),
    ));
    assert!(date_filter_matches_at(
        Some(tomorrow_start),
        &DateFilter {
            in_future: Some(true),
            ..Default::default()
        },
        anchor(),
    ));

    for (days, included, excluded) in [
        (1, vec![today_midday], vec![yesterday_last, tomorrow_start]),
        (
            2,
            vec![yesterday_start, yesterday_last, today_midday],
            vec![two_days_ago, tomorrow_start],
        ),
    ] {
        let filter = DateFilter {
            recent_days: Some(days),
            ..Default::default()
        };
        for value in included {
            assert!(date_filter_matches_at(Some(value), &filter, anchor()));
        }
        for value in excluded {
            assert!(!date_filter_matches_at(Some(value), &filter, anchor()));
        }
    }

    for (days, included, excluded) in [
        (1, vec![today_midday], vec![yesterday_last, tomorrow_start]),
        (
            2,
            vec![today_midday, tomorrow_start, tomorrow_midday],
            vec![yesterday_last, day_after_start],
        ),
    ] {
        let filter = DateFilter {
            within_days: Some(days),
            ..Default::default()
        };
        for value in included {
            assert!(date_filter_matches_at(Some(value), &filter, anchor()));
        }
        for value in excluded {
            assert!(!date_filter_matches_at(Some(value), &filter, anchor()));
        }
    }

    let lower = DateFilter {
        gte_relative: Some(RelativeDateInput { days: 1 }),
        ..Default::default()
    };
    assert!(!date_filter_matches_at(
        Some(today_midday),
        &lower,
        anchor()
    ));
    assert!(date_filter_matches_at(
        Some(tomorrow_start),
        &lower,
        anchor()
    ));

    let upper = DateFilter {
        lte_relative: Some(RelativeDateInput { days: 0 }),
        ..Default::default()
    };
    assert!(date_filter_matches_at(Some(today_midday), &upper, anchor()));
    assert!(!date_filter_matches_at(
        Some(tomorrow_start),
        &upper,
        anchor()
    ));
}

#[test]
fn invalid_date_filter_inputs_fail_validation_and_rendering() {
    let invalid = [
        DateFilter {
            recent_days: Some(-1),
            ..Default::default()
        },
        DateFilter {
            recent_days: Some(0),
            ..Default::default()
        },
        DateFilter {
            recent_days: Some(MAX_CALENDAR_DAY_SPAN + 1),
            ..Default::default()
        },
        DateFilter {
            within_days: Some(-1),
            ..Default::default()
        },
        DateFilter {
            within_days: Some(0),
            ..Default::default()
        },
        DateFilter {
            within_days: Some(MAX_CALENDAR_DAY_SPAN + 1),
            ..Default::default()
        },
        DateFilter {
            gte_relative: Some(RelativeDateInput {
                days: MAX_RELATIVE_DAY_OFFSET + 1,
            }),
            ..Default::default()
        },
        DateFilter {
            lte_relative: Some(RelativeDateInput {
                days: -MAX_RELATIVE_DAY_OFFSET - 1,
            }),
            ..Default::default()
        },
        DateFilter {
            between: Some(DateRangeInput {
                start: "not-a-date".to_owned(),
                end: "2026-08-31".to_owned(),
            }),
            ..Default::default()
        },
        DateFilter {
            between: Some(DateRangeInput {
                start: "2026-09-01".to_owned(),
                end: "2026-08-31".to_owned(),
            }),
            ..Default::default()
        },
    ];

    for filter in invalid {
        let error = filter.validate().expect_err("invalid filter must fail");
        assert_eq!(
            OrmPublicError::from_sqlx(&error).code,
            OrmErrorCode::InvalidInput
        );
        assert!(
            filter
                .render_sql(DatabaseBackend::Mssql, "[occurred_at]", 1)
                .is_err()
        );
        assert!(!date_filter_matches_at(
            Some("2026-08-31T14:45:00"),
            &filter,
            anchor()
        ));
    }
}

#[test]
fn calendar_sql_is_exact_for_each_backend() {
    let filter = DateFilter {
        is_today: Some(true),
        in_past: Some(true),
        in_future: Some(true),
        recent_days: Some(2),
        within_days: Some(2),
        gte_relative: Some(RelativeDateInput { days: -2 }),
        lte_relative: Some(RelativeDateInput { days: 2 }),
        ..Default::default()
    };

    let cases = [
        (
            DatabaseBackend::Sqlite,
            "occurred_at",
            vec![
                "occurred_at < date('now')",
                "occurred_at >= date('now', '+1 days')",
                "occurred_at >= date('now') AND occurred_at < date('now', '+1 days')",
                "occurred_at >= date('now', '-1 days') AND occurred_at < date('now', '+1 days')",
                "occurred_at >= date('now') AND occurred_at < date('now', '+2 days')",
                "occurred_at >= date('now', '-2 days')",
                "occurred_at < date('now', '+3 days')",
            ],
        ),
        (
            DatabaseBackend::Postgres,
            "\"occurred_at\"",
            vec![
                "\"occurred_at\" < CURRENT_DATE",
                "\"occurred_at\" >= CURRENT_DATE + INTERVAL '1 days'",
                "\"occurred_at\" >= CURRENT_DATE AND \"occurred_at\" < CURRENT_DATE + INTERVAL '1 days'",
                "\"occurred_at\" >= CURRENT_DATE - INTERVAL '1 days' AND \"occurred_at\" < CURRENT_DATE + INTERVAL '1 days'",
                "\"occurred_at\" >= CURRENT_DATE AND \"occurred_at\" < CURRENT_DATE + INTERVAL '2 days'",
                "\"occurred_at\" >= CURRENT_DATE - INTERVAL '2 days'",
                "\"occurred_at\" < CURRENT_DATE + INTERVAL '3 days'",
            ],
        ),
        (
            DatabaseBackend::Mssql,
            "[occurred_at]",
            vec![
                "[occurred_at] < CAST(GETDATE() AS date)",
                "[occurred_at] >= DATEADD(day, 1, CAST(GETDATE() AS date))",
                "[occurred_at] >= CAST(GETDATE() AS date) AND [occurred_at] < DATEADD(day, 1, CAST(GETDATE() AS date))",
                "[occurred_at] >= DATEADD(day, -1, CAST(GETDATE() AS date)) AND [occurred_at] < DATEADD(day, 1, CAST(GETDATE() AS date))",
                "[occurred_at] >= CAST(GETDATE() AS date) AND [occurred_at] < DATEADD(day, 2, CAST(GETDATE() AS date))",
                "[occurred_at] >= DATEADD(day, -2, CAST(GETDATE() AS date))",
                "[occurred_at] < DATEADD(day, 3, CAST(GETDATE() AS date))",
            ],
        ),
    ];

    for (backend, column, expected) in cases {
        let (conditions, values) = filter
            .render_sql(backend, column, 1)
            .expect("valid filter renders");
        assert_eq!(conditions, expected);
        assert!(values.is_empty());
    }

    let exact = DateFilter {
        eq: Some("2026-08-31T14:45:00".to_owned()),
        between: Some(DateRangeInput {
            start: "2026-08-30".to_owned(),
            end: "2026-09-01".to_owned(),
        }),
        ..Default::default()
    };
    for (backend, expected) in [
        (
            DatabaseBackend::Sqlite,
            vec!["occurred_at = ?", "occurred_at BETWEEN ? AND ?"],
        ),
        (
            DatabaseBackend::Postgres,
            vec!["occurred_at = $4", "occurred_at BETWEEN $5 AND $6"],
        ),
        (
            DatabaseBackend::Mssql,
            vec!["occurred_at = @P4", "occurred_at BETWEEN @P5 AND @P6"],
        ),
    ] {
        let (conditions, values) = exact
            .render_sql(backend, "occurred_at", 4)
            .expect("exact comparisons render");
        assert_eq!(conditions, expected);
        assert_eq!(values.len(), 3);
    }
}

#[test]
fn sqlite_spatial_fallback_matches_date_predicates_in_boolean_trees() {
    let today = record(Some("2026-08-31T14:45:00"));
    let null_date = record(None);
    let calendar = DateFilter {
        is_today: Some(true),
        ..Default::default()
    };

    let direct = CalendarSpatialRecordWhereInput {
        occurred_at: Some(calendar.clone()),
        location: Some(point_filter(1.0, 2.0)),
        ..Default::default()
    };
    assert!(direct.requires_in_memory_filtering(DatabaseBackend::Sqlite));
    let (prefilter, prefilter_values) = direct.to_sql_prefilter_conditions(DatabaseBackend::Sqlite);
    assert!(prefilter.is_empty());
    assert!(prefilter_values.is_empty());
    assert!(matches(&direct, &today));
    assert!(!matches(&direct, &null_date));

    let and = CalendarSpatialRecordWhereInput {
        location: Some(point_filter(1.0, 2.0)),
        and: Some(vec![CalendarSpatialRecordWhereInput {
            occurred_at: Some(calendar.clone()),
            ..Default::default()
        }]),
        ..Default::default()
    };
    assert!(matches(&and, &today));
    assert!(!matches(&and, &null_date));

    let or = CalendarSpatialRecordWhereInput {
        or: Some(vec![
            CalendarSpatialRecordWhereInput {
                location: Some(point_filter(9.0, 9.0)),
                ..Default::default()
            },
            CalendarSpatialRecordWhereInput {
                occurred_at: Some(calendar.clone()),
                ..Default::default()
            },
        ]),
        ..Default::default()
    };
    assert!(matches(&or, &today));
    assert!(!matches(&or, &null_date));

    let not = CalendarSpatialRecordWhereInput {
        location: Some(point_filter(1.0, 2.0)),
        not: Some(Box::new(CalendarSpatialRecordWhereInput {
            occurred_at: Some(DateFilter {
                in_future: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        })),
        ..Default::default()
    };
    assert!(matches(&not, &today));
    assert!(!matches(&not, &null_date));
}

#[test]
fn date_fields_advertise_the_exact_filter_grammar() {
    let expected = vec![
        GraphqlSemanticFilterOperator::Equal,
        GraphqlSemanticFilterOperator::NotEqual,
        GraphqlSemanticFilterOperator::LessThan,
        GraphqlSemanticFilterOperator::LessThanOrEqual,
        GraphqlSemanticFilterOperator::GreaterThan,
        GraphqlSemanticFilterOperator::GreaterThanOrEqual,
        GraphqlSemanticFilterOperator::Between,
        GraphqlSemanticFilterOperator::IsNull,
        GraphqlSemanticFilterOperator::InPast,
        GraphqlSemanticFilterOperator::InFuture,
        GraphqlSemanticFilterOperator::IsToday,
        GraphqlSemanticFilterOperator::RecentDays,
        GraphqlSemanticFilterOperator::WithinDays,
        GraphqlSemanticFilterOperator::GteRelative,
        GraphqlSemanticFilterOperator::LteRelative,
    ];

    for metadata in [
        StringDateSemantics::graphql_semantic_metadata(),
        DateTimeDateSemantics::graphql_semantic_metadata(),
    ] {
        let metadata = metadata.expect("semantic metadata exists");
        let field = metadata
            .fields
            .iter()
            .find(|field| field.field_name == "observedAt")
            .expect("date field exists");
        assert_eq!(field.filter_operators, expected);
        assert!(
            !field
                .filter_operators
                .contains(&GraphqlSemanticFilterOperator::In)
        );
        assert!(
            !field
                .filter_operators
                .contains(&GraphqlSemanticFilterOperator::Contains)
        );
    }
}

#[derive(Default)]
struct DateSchemaQuery;

#[graphql_orm::async_graphql::Object]
impl DateSchemaQuery {
    async fn accepts_date_filter(
        &self,
        filter: DateFilter,
    ) -> graphql_orm::async_graphql::Result<bool> {
        filter
            .validate()
            .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
        Ok(true)
    }

    async fn checks_generated_filter(
        &self,
        ctx: &graphql_orm::async_graphql::Context<'_>,
        filter: CalendarSpatialRecordWhereInput,
    ) -> graphql_orm::async_graphql::Result<i64> {
        let pool = ctx.data_unchecked::<graphql_orm::sqlx::SqlitePool>();
        CalendarSpatialRecord::query(pool)
            .filter(filter)
            .count()
            .await
            .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)
    }
}

#[tokio::test]
async fn graphql_schema_requires_between_bounds_and_documents_day_limits() {
    let pool = graphql_orm::sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("in-memory SQLite pool");
    let schema = Schema::build(DateSchemaQuery, EmptyMutation, EmptySubscription)
        .data(pool)
        .finish();
    let sdl = schema.sdl();
    assert!(sdl.contains("start: String!"));
    assert!(sdl.contains("end: String!"));
    assert!(sdl.contains("maximum 36,600"));
    assert!(sdl.contains("-36,600 through 36,600"));

    let incomplete = schema
        .execute("{ acceptsDateFilter(filter: { between: { start: \"2026-08-31\" } }) }")
        .await;
    assert!(!incomplete.errors.is_empty());

    for input in [
        "{ acceptsDateFilter(filter: { recentDays: 0 }) }",
        "{ acceptsDateFilter(filter: { recentDays: 36601 }) }",
        "{ acceptsDateFilter(filter: { withinDays: -1 }) }",
        "{ acceptsDateFilter(filter: { gteRelative: { days: 36601 } }) }",
        "{ acceptsDateFilter(filter: { between: { start: \"invalid\", end: \"2026-08-31\" } }) }",
        "{ acceptsDateFilter(filter: { between: { start: \"2026-09-01\", end: \"2026-08-31\" } }) }",
    ] {
        let response = schema.execute(input).await;
        assert!(
            !response.errors.is_empty(),
            "input unexpectedly accepted: {input}"
        );
    }

    let generated = schema
        .execute(
            "{ checksGeneratedFilter(filter: { occurredAt: { gteRelative: { days: 36601 } } }) }",
        )
        .await;
    let error = generated
        .errors
        .first()
        .expect("generated execution rejects invalid filter");
    assert_eq!(
        error
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code"))
            .and_then(|value| match value {
                graphql_orm::async_graphql::Value::String(value) => Some(value.as_str()),
                _ => None,
            }),
        Some("INVALID_INPUT")
    );
}

#[tokio::test]
async fn programmatic_generated_filters_fail_before_database_work() {
    let pool = graphql_orm::sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("in-memory SQLite pool");
    let filter = CalendarSpatialRecordWhereInput {
        occurred_at: Some(DateFilter {
            within_days: Some(0),
            ..Default::default()
        }),
        ..Default::default()
    };
    let (conditions, values) = filter.to_sql_conditions();
    assert_eq!(conditions, vec!["1 = 0"]);
    assert!(values.is_empty());

    let error = CalendarSpatialRecord::query(&pool)
        .filter(filter)
        .fetch_all()
        .await
        .expect_err("invalid programmatic filter fails before querying a missing table");
    assert_eq!(
        OrmPublicError::from_sqlx(&error).code,
        OrmErrorCode::InvalidInput
    );
}
