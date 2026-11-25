//! Internal time types and the compatibility rules for existing temporal text.

use chrono::{Datelike, Timelike};
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{Date, Duration, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

const DATE: &[time::format_description::FormatItem<'_>] =
    format_description!("[year]-[month]-[day]");
const NAIVE_T: &[time::format_description::FormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");
const NAIVE_SPACE: &[time::format_description::FormatItem<'_>] =
    format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
const NAIVE_T_FRACTION: &[time::format_description::FormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond]");
const NAIVE_SPACE_FRACTION: &[time::format_description::FormatItem<'_>] =
    format_description!("[year]-[month]-[day] [hour]:[minute]:[second].[subsecond]");

// Retain the year bounds of the existing chrono API despite time's wider range.
const MIN_YEAR: i32 = -262_143;
const MAX_YEAR: i32 = 262_142;

/// The fraction of a leap second must survive until formatting or comparison.
#[derive(Clone, Copy)]
pub(crate) struct Timestamp {
    pub(crate) value: OffsetDateTime,
    pub(crate) leap_second: bool,
}

impl Timestamp {
    pub(crate) fn from_chrono(value: chrono::DateTime<chrono::FixedOffset>) -> Self {
        let (utc, leap_second) = from_chrono_naive(value.naive_utc());
        let offset = UtcOffset::from_whole_seconds(value.offset().local_minus_utc())
            .expect("chrono offsets fit in time");
        Self {
            value: utc.assume_utc().to_offset(offset),
            leap_second,
        }
    }

    pub(crate) fn from_naive(
        value: PrimitiveDateTime,
        leap_second: bool,
        offset: UtcOffset,
    ) -> Self {
        Self {
            value: value.assume_offset(offset),
            leap_second,
        }
    }

    pub(crate) fn ordering_key(self) -> (i64, u32) {
        (
            self.value.unix_timestamp(),
            self.value.nanosecond() + if self.leap_second { 1_000_000_000 } else { 0 },
        )
    }

    pub(crate) fn to_chrono(self) -> Option<chrono::DateTime<chrono::FixedOffset>> {
        let (sec, nanos) = self.ordering_key();
        let offset = chrono::FixedOffset::east_opt(self.value.offset().whole_seconds())?;
        chrono::DateTime::from_timestamp(sec, nanos).map(|value| value.with_timezone(&offset))
    }

    pub(crate) fn canonical_utc(self) -> String {
        let utc = self.value.to_offset(UtcOffset::UTC);
        let (mut rounded, mut leap) = (utc, self.leap_second);
        if let Some(candidate) = utc.checked_add(Duration::nanoseconds(500))
            && candidate.year() <= MAX_YEAR
        {
            leap &= candidate.second() == utc.second();
            rounded = candidate;
        }
        let seconds = rounded.second() + u8::from(leap);
        format!(
            "{}T{:02}:{:02}:{seconds:02}.{:06}Z",
            display_date(rounded.date()),
            rounded.hour(),
            rounded.minute(),
            rounded.microsecond(),
        )
    }
}

/// Parse the existing RFC 3339 dialect and retain the parse error exposed by
/// the compatibility boundary.
pub(crate) fn parse_rfc3339(text: &str) -> Result<Timestamp, chrono::ParseError> {
    rfc3339(text).map_or_else(
        || chrono::DateTime::parse_from_rfc3339(text).map(Timestamp::from_chrono),
        Ok,
    )
}

fn rfc3339(text: &str) -> Option<Timestamp> {
    if !matches!(text.as_bytes().get(10), Some(b'T' | b't' | b' ')) {
        return None;
    }
    let leap_second = text.as_bytes().get(17..19) == Some(b"60");
    let unicode_minus = text.get(19..)?.find('−').map(|i| 19 + i);
    let mut normalized;
    let text = if leap_second || unicode_minus.is_some() {
        normalized = text.to_owned();
        if let Some(index) = unicode_minus {
            normalized.replace_range(index..index + '−'.len_utf8(), "-");
        }
        // Parse as the preceding ordinary second and preserve the leap flag
        // and fraction. time's RFC 3339 parser would collapse them to .999999999.
        if leap_second {
            normalized.replace_range(17..19, "59");
        }
        normalized.as_str()
    } else {
        text
    };
    let value = OffsetDateTime::parse(text, &Rfc3339).ok()?;
    Some(Timestamp { value, leap_second })
}

pub(crate) fn parse_relaxed_timestamp(text: &str) -> Result<Timestamp, chrono::ParseError> {
    if let Some(parsed) = rfc3339(text) {
        return Ok(parsed);
    }
    // Canonical stored values with a signed year can use the same time parser
    // as ordinary naive timestamps. Keep the legacy grammar for older spellings.
    if text.starts_with(['+', '-'])
        && let Some(naive) = text.strip_suffix('Z')
        && let Some(value) = parse_naive_with_year(naive, b'T')
    {
        return Ok(Timestamp::from_naive(value, false, UtcOffset::UTC));
    }
    text.parse().map(Timestamp::from_chrono)
}

pub(crate) fn from_chrono_date(date: chrono::NaiveDate) -> Date {
    let month = Month::try_from(date.month() as u8).expect("valid chrono month");
    Date::from_calendar_date(date.year(), month, date.day() as u8)
        .expect("chrono dates fit in time")
}

pub(crate) fn from_chrono_naive(value: chrono::NaiveDateTime) -> (PrimitiveDateTime, bool) {
    let leap_second = value.nanosecond() >= 1_000_000_000;
    let clock = Time::from_hms_nano(
        value.hour() as u8,
        value.minute() as u8,
        value.second() as u8,
        value.nanosecond() % 1_000_000_000,
    )
    .expect("valid chrono clock");
    (from_chrono_date(value.date()).with_time(clock), leap_second)
}

pub(crate) fn to_chrono_date(date: Date) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::from_ymd_opt(
        date.year(),
        u32::from(u8::from(date.month())),
        date.day().into(),
    )
}

pub(crate) fn to_chrono_naive(
    value: PrimitiveDateTime,
    leap: bool,
) -> Option<chrono::NaiveDateTime> {
    to_chrono_date(value.date())?.and_hms_nano_opt(
        value.hour().into(),
        value.minute().into(),
        value.second().into(),
        value.nanosecond() + if leap { 1_000_000_000 } else { 0 },
    )
}

pub(crate) fn parse_date(text: &str) -> Option<Date> {
    if text.len() != 10 {
        return None;
    }
    Date::parse(text, DATE).ok()
}

pub(crate) fn parse_naive(text: &str, separator: u8) -> Option<PrimitiveDateTime> {
    if text.as_bytes().get(4) != Some(&b'-') {
        return None;
    }
    parse_naive_with_year(text, separator)
}

fn parse_naive_with_year(text: &str, separator: u8) -> Option<PrimitiveDateTime> {
    let description = match (separator, text.contains('.')) {
        (b'T', false) => NAIVE_T,
        (b'T', true) => NAIVE_T_FRACTION,
        (_, false) => NAIVE_SPACE,
        (_, true) => NAIVE_SPACE_FRACTION,
    };
    PrimitiveDateTime::parse(text, description)
        .ok()
        .filter(|value| is_chrono_date(value.date()))
}

pub(crate) fn parse_calendar(text: &str) -> Option<Timestamp> {
    if let Ok(value) = parse_rfc3339(text) {
        return Some(value);
    }
    for (separator, legacy) in [
        (b'T', "%Y-%m-%dT%H:%M:%S%.f"),
        (b' ', "%Y-%m-%d %H:%M:%S%.f"),
    ] {
        if let Some(value) = parse_naive(text, separator) {
            return Some(Timestamp::from_naive(value, false, UtcOffset::UTC));
        }
        if let Ok(value) = chrono::NaiveDateTime::parse_from_str(text, legacy) {
            let (value, leap) = from_chrono_naive(value);
            return Some(Timestamp::from_naive(value, leap, UtcOffset::UTC));
        }
    }
    let date = parse_date(text).or_else(|| {
        chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .ok()
            .map(from_chrono_date)
    })?;
    Some(Timestamp::from_naive(
        date.midnight(),
        false,
        UtcOffset::UTC,
    ))
}

pub(crate) fn calendar_today() -> chrono::NaiveDate {
    to_chrono_date(OffsetDateTime::now_utc().date()).expect("the current year fits in chrono")
}

pub(crate) fn is_chrono_date(date: Date) -> bool {
    (MIN_YEAR..=MAX_YEAR).contains(&date.year())
}

pub(crate) fn display_date(date: Date) -> String {
    let (year, month, day) = date.to_calendar_date();
    let month = u8::from(month);
    if (0..=9999).contains(&year) {
        format!("{year:04}-{month:02}-{day:02}")
    } else {
        format!("{year:+05}-{month:02}-{day:02}")
    }
}

#[cfg(feature = "mssql")]
pub(crate) fn clock_text(time: Time) -> String {
    let (hour, minute, second, nanos) = time.as_hms_nano();
    let mut text = format!("{hour:02}:{minute:02}:{second:02}");
    if nanos != 0 {
        use std::fmt::Write as _;
        if nanos.is_multiple_of(1_000_000) {
            write!(text, ".{:03}", time.millisecond()).unwrap();
        } else if nanos.is_multiple_of(1_000) {
            write!(text, ".{:06}", time.microsecond()).unwrap();
        } else {
            write!(text, ".{nanos:09}").unwrap();
        }
    }
    text
}

#[cfg(feature = "mssql")]
pub(crate) fn mssql_rfc3339(value: OffsetDateTime) -> String {
    // SQL Server only stores offset minutes, and the established UTC spelling is Z.
    let offset = value.offset();
    let zone = if offset.is_utc() {
        "Z".to_owned()
    } else {
        let (hour, minute, _) = offset.as_hms();
        let sign = if offset.is_negative() { '-' } else { '+' };
        format!(
            "{sign}{:02}:{:02}",
            hour.unsigned_abs(),
            minute.unsigned_abs()
        )
    };
    format!(
        "{}T{}{zone}",
        display_date(value.date()),
        clock_text(value.time())
    )
}
