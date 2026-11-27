use graphql_orm::chrono::{self, FixedOffset, SecondsFormat, Timelike};
use graphql_orm::graphql::orm::RuntimeDateTime;
use proptest::prelude::*;

fn chrono_canonical(value: chrono::DateTime<FixedOffset>) -> String {
    let utc = value.to_utc();
    let rounded = utc
        .checked_add_signed(chrono::Duration::nanoseconds(500))
        .unwrap_or(utc);
    rounded
        .with_nanosecond(rounded.nanosecond() / 1_000 * 1_000)
        .unwrap_or(rounded)
        .to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn assert_strict_parity(text: &str) {
    let parsed = chrono::DateTime::parse_from_rfc3339(text).ok();
    let canonical = parsed.map(chrono_canonical);
    let portable = canonical.as_deref().filter(|text| {
        text.len() == 27 && !text.starts_with("0000-") && !text.starts_with("9999-12-31T23:59:60")
    });
    assert_eq!(
        RuntimeDateTime::parse(text)
            .ok()
            .as_ref()
            .map(RuntimeDateTime::as_str),
        portable,
        "{text}"
    );
    assert_eq!(
        RuntimeDateTime::parse_exact(text)
            .ok()
            .as_ref()
            .map(RuntimeDateTime::as_str),
        portable.filter(|_| parsed.unwrap().nanosecond().is_multiple_of(1_000)),
        "{text}"
    );
}

#[test]
fn runtime_datetime_boundaries_match_chrono() {
    for text in [
        "0001-01-01T00:00:00Z",
        "0001-01-01T00:00:00+00:01",
        "0000-12-31T23:59:59.999999500Z",
        "9999-12-31T23:59:59.999999499Z",
        "9999-12-31T23:59:59.999999500Z",
        "9999-12-31T23:59:60Z",
        "1969-12-31T23:59:59.999999500Z",
        "1970-01-01T00:00:00Z",
        "1900-02-29T12:00:00Z",
        "2000-02-29t15:30:00.1234560001z",
        "2000-02-29 15:30:00.123456789−23:59",
        "2000-02-29X15:30:00Z",
        "2000-02-29T15:30:00+24:00",
        "2000-02-29T15:30:00+00:60",
        "2017-01-01T06:59:60.500000000+07:00",
        "2016-12-31T23:59:60.999999500Z",
        "2001-01-01T12:34:60.000000501Z",
    ] {
        assert_strict_parity(text);
    }
}

#[test]
fn stored_signed_years_match_chrono() {
    for text in [
        "-262144-12-31T23:59:59Z",
        "-262143-01-01T00:00:00Z",
        "-262142-01-01T00:00:00.000000501Z",
        "-262143-01-01T00:00:00+00:01",
        "+262142-12-31T23:59:59.999999999Z",
        "+262143-01-01T00:00:00Z",
        "-10000-02-29T00:03:00.000000501-08:00",
        "+10000-01-01T00:00:00.000000Z",
        "-9999-01-01T00:00:00+00:01",
        "-10000-12-31T23:59:59.999999500Z",
        "-10000-12-31T23:59:59-00:01",
        "-9999-12-31T23:59:60.000000499Z",
        "+9999-12-31T23:59:59.999999500Z",
        "9999-12-31T23:59:59-00:01",
        "9999-12-31T23:59:59.999999500Z",
        "+2020-1-2T3:4:5 UTC",
    ] {
        let expected = text
            .parse::<chrono::DateTime<FixedOffset>>()
            .ok()
            .map(chrono_canonical);
        let result = serde_json::from_value::<RuntimeDateTime>(serde_json::json!(text)).ok();
        assert_eq!(
            result.as_ref().map(RuntimeDateTime::as_str),
            expected.as_deref(),
            "{text}"
        );
        assert_strict_parity(text);
    }
}

proptest! {
    #[test]
    fn rfc3339_rounding_and_offset_match_chrono(
        year in 0..10_000,
        month in 1..=12,
        day in 1..=31,
        hour in 0..24,
        minute in 0..60,
        second in 0..=60,
        nanos in 0..1_000_000_000u32,
        offset in -1439i32..=1439,
    ) {
        let sign = if offset < 0 { '-' } else { '+' };
        let (offset_hour, offset_minute) = (offset.abs() / 60, offset.abs() % 60);
        assert_strict_parity(&format!(
            "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{nanos:09}{sign}{offset_hour:02}:{offset_minute:02}"
        ));
    }
}

fn chrono_kind(text: &str) -> graphql_orm::graphql::orm::TemporalKind {
    use graphql_orm::graphql::orm::TemporalKind;

    let text = text.trim();
    let naive = text.strip_suffix(" UTC").unwrap_or(text);
    if let Ok(value) = text.parse::<chrono::NaiveDate>() {
        TemporalKind::Date(value)
    } else if let Ok(value) = text.parse::<chrono::DateTime<FixedOffset>>() {
        TemporalKind::DateTimeOffset(value)
    } else if let Ok(value) = naive
        .parse::<chrono::NaiveDateTime>()
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(naive, "%Y-%m-%d %H:%M:%S%.f"))
    {
        TemporalKind::DateTime(value)
    } else {
        TemporalKind::Unparsed
    }
}

fn assert_text_kind(text: &str) {
    use graphql_orm::graphql::orm::SqlValue;

    let SqlValue::Temporal(value) = SqlValue::temporal_text(text.to_owned()) else {
        panic!("expected temporal value");
    };
    assert_eq!(value.kind(), &chrono_kind(text), "{text}");
    assert_eq!(value.as_text().as_bytes(), text.as_bytes());
}

#[test]
fn temporal_text_classification_matches_chrono() {
    for text in [
        "2024-02-29",
        "  2000-2-9 ",
        "1900-02-29",
        "0000-01-01",
        "-262143-01-01",
        "+262142-12-31",
        "+262143-01-01",
        "1969-12-31 23:59:59.999999501",
        "1970-01-01 00:00:00 UTC",
        "1970-01-01 00:00:00.0 +05:30",
        "1970-01-01T00:00:00+0530",
        "2016-12-31T23:59:60.5Z",
        "2024-02-29t23:59:59",
        "2024-02-29 23:59:60.1234567891",
        "2024-02-29X23:59:59Z",
        "legacy text",
        "",
    ] {
        assert_text_kind(text);
    }
    assert_text_kind("2024-02-29T23:59:00.1234567891234567890");
    for year in [
        -262_144, -262_143, -262_142, -10_000, -9999, 9999, 10_000, 262_142, 262_143,
    ] {
        for suffix in [
            "-01-01",
            "-01-01 00:00:00.123456789 UTC",
            "-01-01T00:00:00.123456789+00:01",
            "-01-01T00:00:00.123456789Z",
        ] {
            assert_text_kind(&format!("{year:+05}{suffix}"));
        }
    }
}

#[test]
fn chrono_values_keep_their_display_text() {
    use graphql_orm::graphql::orm::SqlValue;

    let assert_text = |value, expected: String| {
        let SqlValue::Temporal(value) = value else {
            panic!("expected temporal value");
        };
        assert_eq!(value.as_text().as_bytes(), expected.as_bytes());
    };
    for date in [chrono::NaiveDate::MIN, chrono::NaiveDate::MAX] {
        assert_text(SqlValue::from(date), date.to_string());
        let timestamp = date.and_hms_nano_opt(23, 59, 59, 1_999_999_999).unwrap();
        assert_text(SqlValue::from(timestamp), timestamp.to_string());
        let utc = timestamp.and_utc();
        assert_text(SqlValue::from(utc), utc.to_string());
    }
    let offset =
        chrono::DateTime::parse_from_rfc3339("2024-02-29T23:59:59.123456789-00:30").unwrap();
    assert_text(SqlValue::from(offset), offset.to_string());
}

fn chrono_calendar(text: &str) -> Option<chrono::NaiveDateTime> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|value| value.naive_utc())
        .or_else(|| {
            ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"]
                .iter()
                .find_map(|format| chrono::NaiveDateTime::parse_from_str(text, format).ok())
        })
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
        })
}

#[test]
fn date_filter_between_order_matches_chrono_calendar_parsing() {
    use graphql_orm::graphql::filters::{DateFilter, DateRangeInput};

    let examples = [
        "2000-2-9",
        "2000-2-9T0:0:0.0",
        "2000-02-09 00:00:00",
        "2000-02-09T01:00:00+01:00",
        "2000-2-9 1:2:3.000000001",
        "2000-02-09 23:59:59.999999999",
        "2000-2-9 23:59:60.5",
        "2000-02-10",
        "2000-02-30",
        "-10000-12-31 23:59:59.999999999",
        "-9999-01-01",
        "9999-12-31T23:00:00-01:00",
        "+10000-01-01 00:00:00",
        "-262143-01-01",
        "+262142-12-31",
        "+262143-01-01",
    ];
    for start in examples {
        for end in examples {
            let filter = DateFilter {
                between: Some(DateRangeInput {
                    start: start.into(),
                    end: end.into(),
                }),
                ..Default::default()
            };
            let expected = chrono_calendar(start)
                .zip(chrono_calendar(end))
                .is_some_and(|(start, end)| start <= end);
            assert_eq!(filter.validate().is_ok(), expected, "{start} to {end}");
        }
    }
}

#[cfg(feature = "sqlite")]
#[test]
fn sqlite_calendar_arithmetic_matches_chrono_at_boundaries() {
    use graphql_orm::graphql::filters::{DateFilter, RelativeDateInput};
    use graphql_orm::graphql::orm::spatial::date_filter_truth_at;

    for today in [
        chrono::NaiveDate::MIN,
        chrono::NaiveDate::MAX,
        chrono::NaiveDate::from_ymd_opt(-10_000, 12, 31).unwrap(),
        chrono::NaiveDate::from_ymd_opt(-9999, 1, 1).unwrap(),
        chrono::NaiveDate::from_ymd_opt(9999, 12, 31).unwrap(),
        chrono::NaiveDate::from_ymd_opt(10_000, 1, 1).unwrap(),
        chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
        chrono::NaiveDate::from_ymd_opt(2000, 2, 29).unwrap(),
        chrono::NaiveDate::from_ymd_opt(2100, 3, 1).unwrap(),
    ] {
        let mut windows = vec![
            (
                DateFilter {
                    in_past: Some(true),
                    ..Default::default()
                },
                None,
                Some(0),
            ),
            (
                DateFilter {
                    in_future: Some(true),
                    ..Default::default()
                },
                Some(1),
                None,
            ),
            (
                DateFilter {
                    is_today: Some(true),
                    ..Default::default()
                },
                Some(0),
                Some(1),
            ),
        ];
        for days in [1, 2, 366] {
            windows.extend([
                (
                    DateFilter {
                        recent_days: Some(days),
                        ..Default::default()
                    },
                    Some(1 - days),
                    Some(1),
                ),
                (
                    DateFilter {
                        within_days: Some(days),
                        ..Default::default()
                    },
                    Some(0),
                    Some(days),
                ),
            ]);
        }
        for days in [-366, -1, 0, 1, 366] {
            windows.extend([
                (
                    DateFilter {
                        gte_relative: Some(RelativeDateInput { days }),
                        ..Default::default()
                    },
                    Some(days),
                    None,
                ),
                (
                    DateFilter {
                        lte_relative: Some(RelativeDateInput { days }),
                        ..Default::default()
                    },
                    None,
                    Some(days + 1),
                ),
            ]);
        }
        let midnight_at = |days| {
            today
                .checked_add_signed(chrono::Duration::days(i64::from(days)))?
                .and_hms_opt(0, 0, 0)
        };
        for offset in [-367, -366, -365, -2, -1, 0, 1, 2, 365, 366, 367] {
            let Some(date) = midnight_at(offset).map(|value| value.date()) else {
                continue;
            };
            for text in [
                format!("{date}T00:59:59.123456789+01:00"),
                format!("{date} 23:59:60.5"),
                date.to_string(),
            ] {
                for (filter, lower_days, upper_days) in &windows {
                    let expected = (|| {
                        let value = chrono_calendar(&text)?;
                        midnight_at(1)?;
                        let after_lower = match lower_days {
                            Some(days) => value >= midnight_at(*days)?,
                            None => true,
                        };
                        let before_upper = match upper_days {
                            Some(days) => value < midnight_at(*days)?,
                            None => true,
                        };
                        Some(after_lower && before_upper)
                    })()
                    .unwrap_or(false);
                    assert_eq!(
                        date_filter_truth_at(Some(&text), filter, today),
                        Some(expected),
                        "today {today}, value {text}, bounds {lower_days:?}..{upper_days:?}"
                    );
                }
            }
        }
    }
}
