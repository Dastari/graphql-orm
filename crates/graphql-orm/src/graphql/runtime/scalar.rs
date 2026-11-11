use super::RuntimeGraphqlError;
use crate::graphql::orm::{
    RuntimeDateTime, RuntimeFloat, RuntimeValue, RuntimeValueKind, canonical_runtime_decimal,
    parse_runtime_decimal, runtime_decimal_from_integer,
};
use async_graphql::Value;
use base64::{Engine, engine::general_purpose::STANDARD};

/// Scalar allocation and tree bounds, applied to input values.
///
/// Output renders every stored value the reader can decode, without these
/// bounds, so a row written before a limit was lowered, or by host code, is
/// returned as stored. Response size is bounded by
/// `RuntimeGraphqlLimits::max_response_bytes`.
#[derive(Clone, Copy, Debug)]
pub struct RuntimeScalarLimits {
    pub max_text_bytes: usize,
    pub max_decoded_bytes: usize,
    pub max_json_depth: usize,
    pub max_json_nodes: usize,
}
impl Default for RuntimeScalarLimits {
    fn default() -> Self {
        Self {
            max_text_bytes: 64 * 1024,
            max_decoded_bytes: 32 * 1024,
            max_json_depth: 32,
            max_json_nodes: 4096,
        }
    }
}
fn invalid() -> RuntimeGraphqlError {
    RuntimeGraphqlError::new("invalid_graphql_input")
}
// serde_json otherwise falls back to f64 for integer tokens outside i64/u64.
// Check number ranges without allocating; serde_json still owns JSON grammar.
fn json_number_bounds(text: &str) -> Result<(), RuntimeGraphqlError> {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'"' {
            index += 1;
            while index < bytes.len() {
                match bytes[index] {
                    b'\\' => index = index.saturating_add(2),
                    b'"' => {
                        index += 1;
                        break;
                    }
                    _ => index += 1,
                }
            }
        } else if bytes[index] == b'-' || bytes[index].is_ascii_digit() {
            let start = index;
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_digit()
                    || matches!(bytes[index], b'.' | b'e' | b'E' | b'+' | b'-'))
            {
                index += 1;
            }
            let number = &text[start..index];
            if number.contains(['.', 'e', 'E']) {
                if !number.parse::<f64>().map_err(|_| invalid())?.is_finite() {
                    return Err(invalid());
                }
            } else if number.starts_with('-') {
                number.parse::<i64>().map_err(|_| invalid())?;
            } else {
                number.parse::<u64>().map_err(|_| invalid())?;
            }
        } else {
            index += 1;
        }
    }
    Ok(())
}
fn json_bounds(
    value: &serde_json::Value,
    limits: RuntimeScalarLimits,
) -> Result<(), RuntimeGraphqlError> {
    let mut stack = vec![(value, 0usize)];
    let mut nodes = 0usize;
    while let Some((value, depth)) = stack.pop() {
        nodes = nodes.checked_add(1).ok_or_else(invalid)?;
        if nodes > limits.max_json_nodes || depth > limits.max_json_depth {
            return Err(invalid());
        }
        let children: Box<dyn Iterator<Item = &serde_json::Value> + '_> = match value {
            serde_json::Value::Array(values) => Box::new(values.iter()),
            serde_json::Value::Object(values) => Box::new(values.values()),
            _ => continue,
        };
        for child in children {
            if stack.len() >= limits.max_json_nodes.saturating_sub(nodes) {
                return Err(invalid());
            }
            stack.push((child, depth + 1));
        }
    }
    Ok(())
}

/// Serialize JSON compactly, stopping as soon as the text exceeds `limit`.
fn bounded_json_text(value: &serde_json::Value, limit: usize) -> Result<(), RuntimeGraphqlError> {
    struct Bounded {
        written: usize,
        limit: usize,
    }
    impl std::io::Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.written) {
                return Err(std::io::Error::other("scalar_limit"));
            }
            self.written += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(&mut Bounded { written: 0, limit }, value).map_err(|_| invalid())
}

/// The document a PostgreSQL `jsonb` round trip reads back. `jsonb` stores
/// numbers as `numeric` and prints them without exponents, and serde_json
/// reads an integer token that fits `i64` or `u64` as an integer, printing
/// every digit: `1e16` (4 bytes) returns as `10000000000000000` (17 bytes).
/// Other numbers return as the same `f64`, and the reader re-serializes the
/// document compactly, so `jsonb` spacing and key order do not change its
/// length. Returns `None` when the round trip changes nothing.
fn jsonb_read_back(value: &serde_json::Value) -> Option<serde_json::Value> {
    use serde_json::Value as Json;
    fn integral(number: &serde_json::Number) -> Option<serde_json::Number> {
        let x = number.as_f64().filter(|_| number.is_f64())?;
        // 2^64 and -2^63 are exact in f64.
        if x.fract() != 0.0
            || !(-9_223_372_036_854_775_808.0..18_446_744_073_709_551_616.0).contains(&x)
        {
            return None;
        }
        Some(if x < 0.0 {
            serde_json::Number::from(x as i64)
        } else {
            serde_json::Number::from(x as u64)
        })
    }
    fn changed(value: &Json) -> bool {
        match value {
            Json::Number(n) => integral(n).is_some(),
            Json::Array(values) => values.iter().any(changed),
            Json::Object(values) => values.values().any(changed),
            _ => false,
        }
    }
    fn convert(value: &Json) -> Json {
        match value {
            Json::Number(n) => integral(n).map_or_else(|| value.clone(), Json::Number),
            Json::Array(values) => Json::Array(values.iter().map(convert).collect()),
            Json::Object(values) => Json::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), convert(value)))
                    .collect(),
            ),
            _ => value.clone(),
        }
    }
    changed(value).then(|| convert(value))
}

pub(super) fn decode(
    kind: RuntimeValueKind,
    value: &Value,
    limits: RuntimeScalarLimits,
) -> Result<RuntimeValue, RuntimeGraphqlError> {
    if value == &Value::Null {
        return Ok(RuntimeValue::Null);
    }
    match (kind, value) {
        (RuntimeValueKind::Boolean, Value::Boolean(v)) => Ok(RuntimeValue::Boolean(*v)),
        (RuntimeValueKind::Float, Value::Number(v)) => {
            RuntimeFloat::new(v.as_f64().ok_or_else(invalid)?)
                .map(RuntimeValue::Float)
                .map_err(|_| invalid())
        }
        (_, Value::String(v)) if v.len() <= limits.max_text_bytes => match kind {
            RuntimeValueKind::String => Ok(RuntimeValue::String(v.clone())),
            RuntimeValueKind::Integer => {
                if v.len() > 20 {
                    return Err(invalid());
                }
                let n: i64 = v.parse().map_err(|_| invalid())?;
                if n.to_string() != *v {
                    return Err(invalid());
                }
                Ok(RuntimeValue::Integer(n))
            }
            RuntimeValueKind::Uuid => uuid::Uuid::parse_str(v)
                .map(RuntimeValue::Uuid)
                .map_err(|_| invalid()),
            // `parse` also rejects canonical years outside 0001 to 9999. The
            // canonical text is what is stored and returned, and is 27 bytes
            // even for a shorter input, so it must fit the bound too.
            RuntimeValueKind::DateTime if v.len() <= 64 => {
                let value = RuntimeDateTime::parse(v).map_err(|_| invalid())?;
                if value.as_str().len() > limits.max_text_bytes {
                    return Err(invalid());
                }
                Ok(RuntimeValue::DateTime(value))
            }
            RuntimeValueKind::Bytes => {
                let max_encoded = limits
                    .max_decoded_bytes
                    .checked_add(2)
                    .and_then(|n| n.checked_div(3))
                    .and_then(|n| n.checked_mul(4))
                    .ok_or_else(invalid)?;
                if v.len() > max_encoded {
                    return Err(invalid());
                }
                let bytes = STANDARD.decode(v).map_err(|_| invalid())?;
                if bytes.len() > limits.max_decoded_bytes || STANDARD.encode(&bytes) != *v {
                    return Err(invalid());
                }
                Ok(RuntimeValue::Bytes(bytes))
            }
            RuntimeValueKind::Json => {
                // serde_json's recursion guard also limits parsing before walking.
                json_number_bounds(v)?;
                let json = serde_json::from_str(v).map_err(|_| invalid())?;
                json_bounds(&json, limits)?;
                // Bound the text every backend reads back, not only the input:
                // serde_json re-serializes `1e15` as `1000000000000000.0`, and
                // PostgreSQL returns `1e16` as `10000000000000000`.
                bounded_json_text(&json, limits.max_text_bytes)?;
                if let Some(read_back) = jsonb_read_back(&json) {
                    bounded_json_text(&read_back, limits.max_text_bytes)?;
                }
                Ok(RuntimeValue::Json(json))
            }
            // Exact text for the field's definition; never rounded.
            RuntimeValueKind::Decimal(definition) => parse_runtime_decimal(definition, v)
                .map(RuntimeValue::Decimal)
                .map_err(|_| invalid()),
            _ => Err(invalid()),
        },
        // Integers are exact; floating numbers are rejected rather than rounded.
        (RuntimeValueKind::Decimal(definition), Value::Number(v)) => {
            let integer = v
                .as_i64()
                .map(i128::from)
                .or_else(|| v.as_u64().map(i128::from))
                .ok_or_else(invalid)?;
            runtime_decimal_from_integer(definition, integer)
                .map(RuntimeValue::Decimal)
                .map_err(|_| invalid())
        }
        _ => Err(invalid()),
    }
}

/// Whether `value` is a wire form this scalar can carry, input or output.
///
/// Output must pass the scalar's validator, and renders stored values without
/// the input bounds: date-times that earlier releases stored outside the years
/// 0001 to 9999, and JSON or bytes longer than the input limits. This accepts
/// those forms. Every runtime input position also decodes with [`decode`],
/// which applies the bounds.
pub(super) fn wire_form_valid(
    kind: RuntimeValueKind,
    value: &Value,
    limits: RuntimeScalarLimits,
) -> bool {
    if decode(kind, value, limits).is_ok() {
        return true;
    }
    let Value::String(text) = value else {
        return false;
    };
    match kind {
        RuntimeValueKind::DateTime => {
            text.len() <= 64
                && RuntimeDateTime::parse_stored(text).is_ok_and(|value| value.as_str() == text)
        }
        RuntimeValueKind::Json => serde_json::from_str::<serde_json::Value>(text).is_ok(),
        RuntimeValueKind::Bytes => STANDARD
            .decode(text)
            .is_ok_and(|bytes| STANDARD.encode(bytes) == *text),
        _ => false,
    }
}

/// Render a stored value. Never fails and applies no input bound, so one
/// stored value cannot fail a whole page or payload.
pub(super) fn encode(value: &RuntimeValue) -> Value {
    match value {
        RuntimeValue::Null => Value::Null,
        RuntimeValue::Boolean(v) => Value::Boolean(*v),
        RuntimeValue::Integer(v) => Value::String(v.to_string()),
        RuntimeValue::Float(v) => Value::from(v.get()),
        RuntimeValue::String(v) => Value::String(v.clone()),
        RuntimeValue::Uuid(v) => Value::String(v.to_string()),
        RuntimeValue::DateTime(v) => Value::String(v.as_str().to_owned()),
        RuntimeValue::Decimal(v) => Value::String(canonical_runtime_decimal(*v)),
        RuntimeValue::Bytes(v) => Value::String(STANDARD.encode(v)),
        // `Display` for a JSON value is its compact serialization.
        RuntimeValue::Json(v) => Value::String(v.to_string()),
    }
}

pub(super) const fn name(kind: RuntimeValueKind) -> &'static str {
    match kind {
        RuntimeValueKind::Boolean => "Boolean",
        RuntimeValueKind::Integer => "RuntimeInt64",
        RuntimeValueKind::Float => "Float",
        RuntimeValueKind::String => "String",
        RuntimeValueKind::Uuid => "RuntimeUuid",
        RuntimeValueKind::Json => "RuntimeJson",
        RuntimeValueKind::Bytes => "RuntimeBytes",
        RuntimeValueKind::DateTime => "RuntimeDateTime",
        // The same scalar name as static `rust_decimal::Decimal` fields.
        RuntimeValueKind::Decimal(_) => "Decimal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lossless_wire_values_and_strict_integer_forms() {
        let limits = RuntimeScalarLimits::default();
        for n in [
            i64::MIN,
            i64::MAX,
            9_007_199_254_740_993,
            -9_007_199_254_740_993,
            0,
        ] {
            let v = RuntimeValue::Integer(n);
            assert_eq!(
                decode(RuntimeValueKind::Integer, &encode(&v), limits).unwrap(),
                v
            );
        }
        for s in ["+1", "01", "-0", "1.0", "1e2", "9223372036854775808"] {
            assert!(decode(RuntimeValueKind::Integer, &Value::String(s.into()), limits).is_err());
        }
        for v in [
            RuntimeValue::Null,
            RuntimeValue::Json(serde_json::Value::Null),
            RuntimeValue::Json(serde_json::json!({"π": [true, "hello", 9007199254740993u64]})),
        ] {
            assert_eq!(
                decode(RuntimeValueKind::Json, &encode(&v), limits).unwrap(),
                v
            );
        }
        assert_eq!(
            encode(&RuntimeValue::Json(serde_json::Value::Null)),
            Value::String("null".into())
        );
        assert!(decode(RuntimeValueKind::Integer, &Value::from(1), limits).is_err());
    }
    #[test]
    fn bytes_datetime_and_json_bounds() {
        let limits = RuntimeScalarLimits {
            max_decoded_bytes: 2,
            ..Default::default()
        };
        assert_eq!(
            decode(
                RuntimeValueKind::Bytes,
                &Value::String("AQI=".into()),
                limits
            )
            .unwrap(),
            RuntimeValue::Bytes(vec![1, 2])
        );
        for s in ["AQI", "AQJ=", "AQID"] {
            assert!(decode(RuntimeValueKind::Bytes, &Value::String(s.into()), limits).is_err());
        }
        let d = decode(
            RuntimeValueKind::DateTime,
            &Value::String("2026-10-09T01:00:00.123456+01:00".into()),
            limits,
        )
        .unwrap();
        assert_eq!(
            encode(&d),
            Value::String("2026-10-09T00:00:00.123456Z".into())
        );
        let limits = RuntimeScalarLimits {
            max_json_depth: 1,
            ..limits
        };
        assert!(
            decode(
                RuntimeValueKind::Json,
                &Value::String("[[[null]]]".into()),
                limits
            )
            .is_err()
        );
    }
    #[test]
    fn json_integer_overflow_is_not_silently_reinterpreted_as_float() {
        let limits = RuntimeScalarLimits::default();
        for text in ["18446744073709551616", "-9223372036854775809", "1e999"] {
            assert!(decode(RuntimeValueKind::Json, &Value::String(text.into()), limits).is_err());
        }
        for text in [
            "18446744073709551615",
            "-9223372036854775808",
            "1e200",
            r#""18446744073709551616""#,
        ] {
            let value =
                decode(RuntimeValueKind::Json, &Value::String(text.into()), limits).unwrap();
            assert_eq!(
                decode(RuntimeValueKind::Json, &encode(&value), limits).unwrap(),
                value
            );
        }
    }
    /// Every accepted input value must encode, because its canonical form is
    /// what is stored and later returned.
    fn assert_round_trips(kind: RuntimeValueKind, text: &str, limits: RuntimeScalarLimits) {
        let value = decode(kind, &Value::String(text.into()), limits).unwrap();
        let encoded = encode(&value);
        assert_eq!(decode(kind, &encoded, limits).unwrap(), value, "{text}");
    }
    #[test]
    fn datetime_inputs_whose_canonical_year_leaves_0001_to_9999_are_rejected() {
        let limits = RuntimeScalarLimits::default();
        for text in [
            "0001-01-01T00:00:00Z",
            "9999-12-31T23:59:59.9999994Z",
            "9999-12-31T23:59:59.999999Z",
        ] {
            assert_round_trips(RuntimeValueKind::DateTime, text, limits);
        }
        for text in [
            "9999-12-31T23:59:59.9999995Z",
            "9999-12-31T23:59:59.9999999Z",
            "9999-12-31T23:00:00-01:00",
            "0001-01-01T00:00:00+00:01",
            "0000-01-01T00:00:00Z",
            "9999-12-31T23:59:60Z",
        ] {
            assert!(
                decode(
                    RuntimeValueKind::DateTime,
                    &Value::String(text.into()),
                    limits
                )
                .is_err(),
                "{text}"
            );
        }
        // The 20-byte input is canonicalized to 27 bytes, which must also fit.
        let short = RuntimeScalarLimits {
            max_text_bytes: 26,
            ..limits
        };
        assert!(
            decode(
                RuntimeValueKind::DateTime,
                &Value::String("2026-01-01T00:00:00Z".into()),
                short
            )
            .is_err()
        );
    }
    #[test]
    fn json_inputs_are_bounded_by_their_canonical_serialization() {
        // `1e15` is re-serialized as `1000000000000000.0`.
        let limits = RuntimeScalarLimits {
            max_text_bytes: 39,
            ..Default::default()
        };
        assert_round_trips(RuntimeValueKind::Json, "[1e15,1e15]", limits);
        let limits = RuntimeScalarLimits {
            max_text_bytes: 38,
            ..limits
        };
        assert_round_trips(RuntimeValueKind::Json, "[1e15]", limits);
        assert!(
            decode(
                RuntimeValueKind::Json,
                &Value::String("[1e15,1e15]".into()),
                limits
            )
            .is_err()
        );
        // Under the default limits a 20 KiB input expands past 64 KiB.
        let limits = RuntimeScalarLimits::default();
        let text = format!("[{}]", vec!["1e15"; 4000].join(","));
        assert!(text.len() <= limits.max_text_bytes);
        assert!(decode(RuntimeValueKind::Json, &Value::String(text), limits).is_err());
        let text = format!("[{}]", vec!["1e15"; 3000].join(","));
        assert_round_trips(RuntimeValueKind::Json, &text, limits);
    }
    #[test]
    fn uuid_datetime_carry_and_json_text_are_canonical() {
        let limits = RuntimeScalarLimits::default();
        let value = decode(
            RuntimeValueKind::Uuid,
            &Value::String("67E55044-10B1-426F-9247-BB680E5FE0C8".into()),
            limits,
        )
        .unwrap();
        assert_eq!(
            encode(&value),
            Value::String("67e55044-10b1-426f-9247-bb680e5fe0c8".into())
        );
        let value = decode(
            RuntimeValueKind::DateTime,
            &Value::String("2026-10-09T00:00:59.9999996Z".into()),
            limits,
        )
        .unwrap();
        assert_eq!(
            encode(&value),
            Value::String("2026-10-09T00:01:00.000000Z".into())
        );
        let value = RuntimeValue::Json(serde_json::Value::String("hello".into()));
        assert_eq!(
            decode(RuntimeValueKind::Json, &encode(&value), limits).unwrap(),
            value
        );
        assert!(decode(RuntimeValueKind::Json, &Value::List(vec![]), limits).is_err());
    }
    /// Output renders stored values without the input bounds, and the shared
    /// scalar validator accepts those forms; input decoding stays strict.
    #[test]
    fn stored_values_beyond_input_bounds_are_rendered_and_pass_the_validator() {
        let limits = RuntimeScalarLimits {
            max_text_bytes: 8,
            max_decoded_bytes: 2,
            ..Default::default()
        };
        let long_json = RuntimeValue::Json(serde_json::json!({"long": "text"}));
        assert_eq!(
            encode(&long_json),
            Value::String(r#"{"long":"text"}"#.into())
        );
        let bytes = RuntimeValue::Bytes(vec![1, 2, 3, 4]);
        assert_eq!(encode(&bytes), Value::String("AQIDBA==".into()));
        for (kind, value) in [
            (RuntimeValueKind::Json, encode(&long_json)),
            (RuntimeValueKind::Bytes, encode(&bytes)),
        ] {
            assert!(wire_form_valid(kind, &value, limits));
            assert!(decode(kind, &value, limits).is_err());
        }
        let limits = RuntimeScalarLimits::default();
        for stored in [
            "+10000-01-01T00:00:00.000000Z",
            "0000-12-31T23:00:00.000000Z",
            "9999-12-31T23:59:60.000000Z",
        ] {
            let value = Value::String(stored.into());
            assert!(
                wire_form_valid(RuntimeValueKind::DateTime, &value, limits),
                "{stored}"
            );
            assert!(
                decode(RuntimeValueKind::DateTime, &value, limits).is_err(),
                "{stored}"
            );
        }
        for invalid in ["+10000-01-01T00:00:00Z", "not a date", "2026-01-01"] {
            assert!(!wire_form_valid(
                RuntimeValueKind::DateTime,
                &Value::String(invalid.into()),
                limits
            ));
        }
        assert!(!wire_form_valid(
            RuntimeValueKind::Json,
            &Value::String("{".into()),
            limits
        ));
        assert!(!wire_form_valid(
            RuntimeValueKind::Bytes,
            &Value::String("AQI".into()),
            limits
        ));
    }
    #[test]
    fn json_inputs_are_bounded_by_their_postgresql_read_back() {
        // `jsonb` returns `1e16` as `10000000000000000`, read back as an integer.
        let limits = RuntimeScalarLimits {
            max_text_bytes: 38,
            ..Default::default()
        };
        assert_round_trips(RuntimeValueKind::Json, "[1e16,-1e16]", limits);
        let limits = RuntimeScalarLimits {
            max_text_bytes: 37,
            ..limits
        };
        assert!(
            decode(
                RuntimeValueKind::Json,
                &Value::String("[1e16,-1e16]".into()),
                limits
            )
            .is_err()
        );
        // Outside i64/u64 the read back is the same f64, so nothing grows:
        // the canonical text is `[1e+20,-1e+19,0.5]`.
        let limits = RuntimeScalarLimits {
            max_text_bytes: 18,
            ..limits
        };
        assert_round_trips(RuntimeValueKind::Json, "[1e20,-1e19,0.5]", limits);
        assert_eq!(
            jsonb_read_back(&serde_json::json!([1e20, -1e19, 0.5, 7])),
            None
        );
        assert_eq!(
            jsonb_read_back(&serde_json::json!({"a": [1e16, -9.223372036854776e18]})),
            Some(serde_json::json!({"a": [10_000_000_000_000_000u64, i64::MIN]}))
        );
        // Under the default limits about 20 KB of input returns as about 74 KB.
        let limits = RuntimeScalarLimits::default();
        let text = format!("[{}]", vec!["1e16"; 4095].join(","));
        assert!(text.len() <= limits.max_text_bytes);
        assert!(decode(RuntimeValueKind::Json, &Value::String(text), limits).is_err());
        let text = format!("[{}]", vec!["1e16"; 3000].join(","));
        assert_round_trips(RuntimeValueKind::Json, &text, limits);
    }
}
