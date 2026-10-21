//! Owned-planner default comparison profiles.
//!
//! PostgreSQL deparses constant defaults with casts and its own spellings
//! (`'…'::uuid`, `'-1'::integer`, `'{"a": 1}'::jsonb`, session-zone timestamps).
//! The physical target keeps its existing spelling because that text feeds
//! persisted schema and plan hashes. [`DefaultEquivalence::TypedLiteralsV1`]
//! therefore compares complete constants *by value* for the target column type,
//! and only the owned/runtime planner selects it. Static planning and the
//! historical owned-planning replay recipes keep [`DefaultEquivalence::Pinned8e1`],
//! which is byte-identical to the 0.50.2 (`8e1e569`) planner.
//!
//! Recognition is closed: a side is either a bare numeric/boolean token or one
//! complete quoted literal with at most one allowed cast. Anything else, including
//! session-dependent output such as non-ISO `DateStyle` or `bytea_output=escape`,
//! is not equivalent and therefore fails closed.

/// Default comparison profile for one diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DefaultEquivalence {
    /// Exact 0.50.2 planner semantics.
    Pinned8e1,
    /// Value comparison of recognized PostgreSQL constants.
    TypedLiteralsV1,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Boolean,
    Integer,
    Float,
    Uuid,
    Jsonb,
    Bytea,
    Timestamptz,
    Numeric,
}

impl Class {
    fn of(sql_type: &str) -> Option<Self> {
        let sql_type = sql_type.trim().to_ascii_lowercase();
        // Runtime decimals target `NUMERIC(p,s)`; PostgreSQL reports `numeric(p,s)`.
        if sql_type == "numeric"
            || sql_type
                .strip_prefix("numeric(")
                .and_then(|rest| rest.strip_suffix(')'))
                .is_some_and(|args| args.bytes().all(|b| b.is_ascii_digit() || b == b','))
        {
            return Some(Self::Numeric);
        }
        Some(match sql_type.as_str() {
            "boolean" | "bool" => Self::Boolean,
            "bigint" | "integer" | "smallint" | "int" | "int8" | "int4" | "int2" => Self::Integer,
            "double precision" | "real" | "float8" | "float4" => Self::Float,
            "uuid" => Self::Uuid,
            "jsonb" => Self::Jsonb,
            "bytea" => Self::Bytea,
            "timestamptz" | "timestamp with time zone" => Self::Timestamptz,
            _ => return None,
        })
    }

    fn casts(self) -> &'static [&'static str] {
        match self {
            Self::Boolean => &["boolean"],
            Self::Integer => &["integer", "bigint", "smallint", "numeric"],
            Self::Float => &["numeric", "integer", "bigint", "double precision", "real"],
            Self::Uuid => &["uuid"],
            Self::Jsonb => &["jsonb"],
            Self::Bytea => &["bytea"],
            Self::Timestamptz => &["timestamp with time zone", "timestamptz"],
            Self::Numeric => &["numeric"],
        }
    }

    fn bare_tokens(self) -> bool {
        matches!(
            self,
            Self::Boolean | Self::Integer | Self::Float | Self::Numeric
        )
    }
}

// Generous bound on deparsed text; JSON defaults are at most 16 KiB canonically.
const MAX_COMPARED_DEFAULT_BYTES: usize = 256 * 1024;

/// Value-compare two complete PostgreSQL constant defaults for a column type.
/// `None` means at least one side is unrecognized and the caller keeps its answer.
pub(super) fn postgres_typed_defaults_equal(
    target_sql_type: &str,
    live: &str,
    target: &str,
) -> Option<bool> {
    let class = Class::of(target_sql_type)?;
    let live = constant(class, live)?;
    let target = constant(class, target)?;
    Some(match class {
        Class::Boolean => live.eq_ignore_ascii_case(&target),
        Class::Integer => live.parse::<i64>().ok()? == target.parse::<i64>().ok()?,
        Class::Float => float_bits(&live)? == float_bits(&target)?,
        // Exact value comparison; `12.5` and `12.50` are the same NUMERIC value.
        Class::Numeric => {
            rust_decimal::Decimal::from_str_exact(&live).ok()?
                == rust_decimal::Decimal::from_str_exact(&target).ok()?
        }
        Class::Uuid => uuid::Uuid::parse_str(&live).ok()? == uuid::Uuid::parse_str(&target).ok()?,
        Class::Jsonb => {
            serde_json::from_str::<serde_json::Value>(&live).ok()?
                == serde_json::from_str::<serde_json::Value>(&target).ok()?
        }
        Class::Bytea => hex_bytea(&live)? == hex_bytea(&target)?,
        Class::Timestamptz => {
            let (live, target) = (timestamptz(&live)?, timestamptz(&target)?);
            (live.timestamp(), live.timestamp_subsec_nanos())
                == (target.timestamp(), target.timestamp_subsec_nanos())
        }
    })
}

fn float_bits(text: &str) -> Option<u64> {
    let value = text.parse::<f64>().ok().filter(|value| value.is_finite())?;
    Some(if value == 0.0 { 0.0f64 } else { value }.to_bits())
}

fn hex_bytea(text: &str) -> Option<Vec<u8>> {
    let hex = text.strip_prefix("\\x")?;
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(hex.get(index..index + 2)?, 16).ok())
        .collect()
}

/// Parse an RFC 3339 instant or PostgreSQL ISO `DateStyle` timestamptz output.
fn timestamptz(text: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    if let Ok(value) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(value);
    }
    // `YYYY-MM-DD HH:MM:SS[.ffffff]±HH[:MM[:SS]]`; BC years and other styles fail.
    let split = text.get(19..)?.find(['+', '-'])? + 19;
    let naive =
        chrono::NaiveDateTime::parse_from_str(text.get(..split)?, "%Y-%m-%d %H:%M:%S%.f").ok()?;
    let offset = text.get(split..)?;
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let parts = offset[1..].split(':').collect::<Vec<_>>();
    if parts.is_empty()
        || parts.len() > 3
        || parts
            .iter()
            .any(|part| part.len() != 2 || !part.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let seconds = parts
        .iter()
        .zip([3600, 60, 1])
        .map(|(part, scale)| part.parse::<i32>().map(|value| value * scale))
        .sum::<Result<i32, _>>()
        .ok()?;
    let offset = chrono::FixedOffset::east_opt(sign * seconds)?;
    naive.and_local_timezone(offset).single()
}

/// The unescaped constant text, or `None` for any non-constant expression.
fn constant(class: Class, expression: &str) -> Option<String> {
    if expression.len() > MAX_COMPARED_DEFAULT_BYTES {
        return None;
    }
    let expression = super::dialect::canonicalize_column_default_expression(expression);
    let bytes = expression.as_bytes();
    let escaped = matches!(bytes.first(), Some(b'e' | b'E')) && bytes.get(1) == Some(&b'\'');
    if !escaped && bytes.first() != Some(&b'\'') {
        let bare = class.bare_tokens()
            && !expression.is_empty()
            && expression
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'));
        return bare.then_some(expression);
    }
    let mut index = usize::from(escaped) + 1;
    let mut value = String::new();
    while index < bytes.len() {
        match bytes[index] {
            b'\'' => {
                index += 1;
                if bytes.get(index) == Some(&b'\'') {
                    value.push('\'');
                    index += 1;
                    continue;
                }
                let suffix = expression[index..].trim();
                if suffix.is_empty() {
                    return Some(value);
                }
                let cast = suffix.strip_prefix("::")?.trim().to_ascii_lowercase();
                return class.casts().contains(&cast.as_str()).then_some(value);
            }
            b'\\' if escaped => {
                index += 1;
                match bytes.get(index) {
                    Some(b'\\') => value.push('\\'),
                    Some(b'\'') => value.push('\''),
                    _ => return None,
                }
                index += 1;
            }
            _ => {
                let character = expression[index..].chars().next()?;
                value.push(character);
                index += character.len_utf8();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::postgres_typed_defaults_equal as equal;

    #[test]
    fn deparsed_postgres_constants_compare_by_value() {
        for (sql_type, live, target) in [
            ("BIGINT", "'-1'::integer", "-1"),
            ("BIGINT", "'2147483648'::bigint", "2147483648"),
            (
                "BIGINT",
                "'-9223372036854775808'::numeric",
                "-9223372036854775808",
            ),
            ("BIGINT", "9223372036854775807", "9223372036854775807"),
            ("BIGINT", "7", "007"),
            ("DOUBLE PRECISION", "'-1.5'::numeric", "-1.5"),
            ("DOUBLE PRECISION", "100000", "1e5"),
            ("DOUBLE PRECISION", "0.0", "-0.0"),
            ("DOUBLE PRECISION", "1", "1.0"),
            (
                "UUID",
                "'67e55044-10b1-426f-9247-bb680e5fe0c8'::uuid",
                "'67E55044-10B1-426F-9247-BB680E5FE0C8'",
            ),
            (
                "JSONB",
                r#"'{"a": 1, "b": [true, null]}'::jsonb"#,
                r#"'{"a":1,"b":[true,null]}'::jsonb"#,
            ),
            (
                "JSONB",
                r#"'{"b": 2, "a": 1.0}'::jsonb"#,
                r#"'{"a":1.0,"b":2}'::jsonb"#,
            ),
            ("JSONB", r#"'"it''s"'::jsonb"#, r#"'"it''s"'::jsonb"#),
            ("BYTEA", r"'\x0102ff'::bytea", r"'\x0102FF'::bytea"),
            ("BYTEA", r"E'\\x0102'::bytea", r"'\x0102'::bytea"),
            ("BYTEA", r"'\x'::bytea", r"'\x'::bytea"),
            (
                "TIMESTAMPTZ",
                "'2026-10-20 22:00:00+10'::timestamp with time zone",
                "'2026-10-20T12:00:00.000000Z'::timestamptz",
            ),
            (
                "TIMESTAMPTZ",
                "'2026-10-20 17:30:00.5+05:30'::timestamp with time zone",
                "'2026-10-20T12:00:00.500000Z'::timestamptz",
            ),
            (
                "TIMESTAMPTZ",
                "'0001-01-01 10:04:52+10:04:52'::timestamp with time zone",
                "'0001-01-01T00:00:00.000000Z'::timestamptz",
            ),
            ("BOOLEAN", "true", "TRUE"),
            ("NUMERIC(12,2)", "12.50", "12.50"),
            ("NUMERIC(12,2)", "'-12.50'::numeric", "-12.50"),
            ("numeric(12,2)", "0.00", "0.00"),
            ("NUMERIC(18,0)", "999999999999999999", "999999999999999999"),
        ] {
            assert_eq!(
                equal(sql_type, live, target),
                Some(true),
                "{sql_type} {live}"
            );
        }
    }

    #[test]
    fn different_values_and_unrecognized_expressions_are_not_equal() {
        for (sql_type, live, target) in [
            ("BIGINT", "'-1'::integer", "1"),
            ("DOUBLE PRECISION", "'-1.5'::numeric", "1.5"),
            ("NUMERIC(12,2)", "'-12.50'::numeric", "12.50"),
            ("NUMERIC(12,2)", "12.51", "12.50"),
            (
                "UUID",
                "'67e55044-10b1-426f-9247-bb680e5fe0c8'::uuid",
                "'00000000-0000-0000-0000-000000000000'",
            ),
            ("JSONB", r#"'{"a": 1}'::jsonb"#, r#"'{"a":1.0}'::jsonb"#),
            ("BYTEA", r"'\x01'::bytea", r"'\x02'::bytea"),
            (
                "TIMESTAMPTZ",
                "'2026-10-20 12:00:00+00'::timestamp with time zone",
                "'2026-10-20T12:00:00.000001Z'::timestamptz",
            ),
        ] {
            assert_eq!(
                equal(sql_type, live, target),
                Some(false),
                "{sql_type} {live}"
            );
        }
        for (sql_type, live, target) in [
            ("BIGINT", "'-1'::integer::bigint", "-1"),
            ("BIGINT", "(1 + 1)", "2"),
            ("BIGINT", "'-1'::text", "-1"),
            (
                "UUID",
                "gen_random_uuid()",
                "'67e55044-10b1-426f-9247-bb680e5fe0c8'",
            ),
            ("UUID", "67e55044", "'67e55044-10b1-426f-9247-bb680e5fe0c8'"),
            ("BYTEA", r"'\001\002'::bytea", r"'\x0102'::bytea"),
            (
                "TIMESTAMPTZ",
                "'10/20/2026 12:00:00 UTC'::timestamp with time zone",
                "'2026-10-20T12:00:00.000000Z'::timestamptz",
            ),
            (
                "TIMESTAMPTZ",
                "'2026-10-20 12:00:00+00 BC'::timestamp with time zone",
                "'2026-10-20T12:00:00.000000Z'::timestamptz",
            ),
            ("TEXT", "'x'::text", "'x'"),
            ("JSONB", r#"'{"a": 1}'::json"#, r#"'{"a":1}'::jsonb"#),
            ("DOUBLE PRECISION", "'NaN'::double precision", "1.0"),
            ("NUMERIC(12,2)", "'NaN'::numeric", "1.00"),
            ("NUMERIC(12,2)", "'1.5'::text", "1.50"),
        ] {
            assert_ne!(
                equal(sql_type, live, target),
                Some(true),
                "{sql_type} {live}"
            );
        }
    }
}
