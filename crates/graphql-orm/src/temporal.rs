//! Internal time types and the compatibility rules for existing temporal text.

use chrono::{Datelike, SecondsFormat, Timelike};
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

// Use the same internal range even when a consumer opts into time/large-dates.
pub(crate) fn is_default_time_year(year: i32) -> bool {
    (-9999..=9999).contains(&year)
}

#[cfg(feature = "postgres")]
pub(crate) fn postgres_to_primitive_datetime(micros: i64) -> Option<PrimitiveDateTime> {
    time::macros::datetime!(2000-01-01 0:00).checked_add(Duration::microseconds(micros))
}

/// Decode ISO DateStyle output, which SQLx requests at connection startup.
#[cfg(all(feature = "postgres", feature = "time"))]
pub(crate) fn postgres_text_timestamp(text: &str) -> Option<OffsetDateTime> {
    let (text, is_bc) = match text.strip_suffix(" BC") {
        Some(adjusted) => (adjusted, true),
        None => (text, false),
    };
    let (date, clock_with_offset) = text.split_once(' ')?;
    let (clock, offset) = clock_with_offset.split_at(clock_with_offset.find(['+', '-'])?);
    let clock = Time::parse(
        clock,
        format_description!("[hour]:[minute]:[second][optional [.[subsecond]]]"),
    )
    .ok()?;
    let offset = UtcOffset::parse(
        offset,
        format_description!(
            "[offset_hour sign:mandatory][optional [:[offset_minute]]][optional [:[offset_second]]]"
        ),
    )
    .ok()?;
    let utc_shift = Duration::seconds(-i64::from(offset.whole_seconds()));

    // The session-local date can lie just outside the UTC wire range.
    let utc = match (is_bc, date) {
        (true, "0001-12-31") => parse_date("0000-12-31")?
            .with_time(clock)
            .checked_add(utc_shift),
        (true, _) => None,
        (false, "10000-01-01") => parse_date("9999-12-31")?
            .with_time(clock)
            .checked_add(Duration::DAY + utc_shift),
        (false, _) => parse_date(date)?.with_time(clock).checked_add(utc_shift),
    }?;
    Some(utc.assume_utc())
}

/// Preserve the full chrono range alongside the internal time engine.
#[derive(Clone, Copy)]
pub(crate) enum Timestamp {
    Time {
        utc: PrimitiveDateTime,
        offset: UtcOffset,
        leap_second: bool,
    },
    Chrono(chrono::DateTime<chrono::FixedOffset>),
}

impl Timestamp {
    pub(crate) fn from_chrono(value: chrono::DateTime<chrono::FixedOffset>) -> Self {
        let Some((utc, leap_second)) = from_chrono_naive(value.naive_utc()) else {
            return Self::Chrono(value);
        };
        let offset = UtcOffset::from_whole_seconds(value.offset().local_minus_utc())
            .expect("chrono offsets fit in time");
        Self::Time {
            utc,
            offset,
            leap_second,
        }
    }

    pub(crate) fn from_utc(utc: PrimitiveDateTime) -> Self {
        Self::Time {
            utc,
            offset: UtcOffset::UTC,
            leap_second: false,
        }
    }

    pub(crate) fn subsec_nanos(self) -> u32 {
        match self {
            Self::Time {
                utc, leap_second, ..
            } => utc.nanosecond() + if leap_second { 1_000_000_000 } else { 0 },
            Self::Chrono(value) => value.timestamp_subsec_nanos(),
        }
    }

    pub(crate) fn ordering_key(self) -> (i64, u32) {
        let seconds = match self {
            Self::Time { utc, .. } => utc.assume_utc().unix_timestamp(),
            Self::Chrono(value) => value.timestamp(),
        };
        (seconds, self.subsec_nanos())
    }

    pub(crate) fn to_chrono(self) -> Option<chrono::DateTime<chrono::FixedOffset>> {
        match self {
            Self::Time { offset, .. } => {
                let (sec, nanos) = self.ordering_key();
                let offset = chrono::FixedOffset::east_opt(offset.whole_seconds())?;
                chrono::DateTime::from_timestamp(sec, nanos)
                    .map(|value| value.with_timezone(&offset))
            }
            Self::Chrono(value) => Some(value),
        }
    }

    pub(crate) fn canonical_utc(self) -> String {
        if let Self::Time {
            utc, leap_second, ..
        } = self
            && let Some(rounded) = utc.checked_add(Duration::nanoseconds(500))
            && is_default_time_year(rounded.year())
        {
            let leap = leap_second && rounded.second() == utc.second();
            let seconds = rounded.second() + u8::from(leap);
            return format!(
                "{}T{:02}:{:02}:{seconds:02}.{:06}Z",
                display_date(rounded.date()),
                rounded.hour(),
                rounded.minute(),
                rounded.microsecond(),
            );
        }

        // Rounding can cross time's year bounds.
        let utc = self.to_chrono().expect("timestamps fit in chrono").to_utc();
        let rounded = utc
            .checked_add_signed(chrono::Duration::nanoseconds(500))
            .unwrap_or(utc);
        rounded
            .with_nanosecond(rounded.nanosecond() / 1_000 * 1_000)
            .unwrap_or(rounded)
            .to_rfc3339_opts(SecondsFormat::Micros, true)
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
    let offset = value.offset();
    let utc = utc_in_time(value.date().with_time(value.time()), offset)?;
    Some(Timestamp::Time {
        utc,
        offset,
        leap_second,
    })
}

// Convert through checked calendar arithmetic so an offset crossing the year
// boundary can fall back to chrono.
pub(crate) fn utc_in_time(
    local: PrimitiveDateTime,
    offset: UtcOffset,
) -> Option<PrimitiveDateTime> {
    let utc = local.checked_sub(Duration::seconds(i64::from(offset.whole_seconds())))?;
    is_default_time_year(utc.year()).then_some(utc)
}

pub(crate) fn parse_relaxed_timestamp(text: &str) -> Result<Timestamp, chrono::ParseError> {
    if let Some(parsed) = rfc3339(text) {
        return Ok(parsed);
    }
    text.parse().map(Timestamp::from_chrono)
}

pub(crate) fn from_chrono_date(date: chrono::NaiveDate) -> Option<Date> {
    if !is_default_time_year(date.year()) {
        return None;
    }
    let month = Month::try_from(date.month() as u8).expect("valid chrono month");
    Date::from_calendar_date(date.year(), month, date.day() as u8).ok()
}

pub(crate) fn from_chrono_naive(value: chrono::NaiveDateTime) -> Option<(PrimitiveDateTime, bool)> {
    let date = from_chrono_date(value.date())?;
    let leap_second = value.nanosecond() >= 1_000_000_000;
    let clock = Time::from_hms_nano(
        value.hour() as u8,
        value.minute() as u8,
        value.second() as u8,
        value.nanosecond() % 1_000_000_000,
    )
    .expect("valid chrono clock");
    Some((date.with_time(clock), leap_second))
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
    let description = match (separator, text.contains('.')) {
        (b'T', false) => NAIVE_T,
        (b'T', true) => NAIVE_T_FRACTION,
        (_, false) => NAIVE_SPACE,
        (_, true) => NAIVE_SPACE_FRACTION,
    };
    PrimitiveDateTime::parse(text, description).ok()
}

/// Parse a calendar operand and record whether it includes an explicit offset.
pub(crate) fn parse_calendar(text: &str) -> Option<(Timestamp, bool)> {
    if let Ok(value) = parse_rfc3339(text) {
        return Some((value, true));
    }
    for (separator, legacy) in [
        (b'T', "%Y-%m-%dT%H:%M:%S%.f"),
        (b' ', "%Y-%m-%d %H:%M:%S%.f"),
    ] {
        if let Some(value) = parse_naive(text, separator) {
            return Some((Timestamp::from_utc(value), false));
        }
        if let Ok(value) = chrono::NaiveDateTime::parse_from_str(text, legacy) {
            return Some((
                Timestamp::from_chrono(value.and_utc().fixed_offset()),
                false,
            ));
        }
    }
    if let Some(date) = parse_date(text) {
        return Some((Timestamp::from_utc(date.midnight()), false));
    }
    let date = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?;
    Some((
        Timestamp::from_chrono(date.and_hms_opt(0, 0, 0)?.and_utc().fixed_offset()),
        false,
    ))
}

pub(crate) fn calendar_today() -> chrono::NaiveDate {
    to_chrono_date(OffsetDateTime::now_utc().date()).expect("the current year fits in chrono")
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
