use super::RuntimeGraphqlError;
use crate::graphql::orm::{RuntimeDateTime, RuntimeFloat, RuntimeValue, RuntimeValueKind};
use async_graphql::Value;
use base64::{Engine, engine::general_purpose::STANDARD};

/// Scalar allocation and tree bounds, applied on input and output.
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
            RuntimeValueKind::DateTime if v.len() <= 64 => RuntimeDateTime::parse(v)
                .map(RuntimeValue::DateTime)
                .map_err(|_| invalid()),
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
                Ok(RuntimeValue::Json(json))
            }
            _ => Err(invalid()),
        },
        _ => Err(invalid()),
    }
}

pub(super) fn encode(
    value: &RuntimeValue,
    limits: RuntimeScalarLimits,
) -> Result<Value, RuntimeGraphqlError> {
    let output = match value {
        RuntimeValue::Null => Value::Null,
        RuntimeValue::Boolean(v) => Value::Boolean(*v),
        RuntimeValue::Integer(v) => Value::String(v.to_string()),
        RuntimeValue::Float(v) => Value::from(v.get()),
        RuntimeValue::String(v) => {
            if v.len() > limits.max_text_bytes {
                return Err(invalid());
            }
            Value::String(v.clone())
        }
        RuntimeValue::Uuid(v) => Value::String(v.to_string()),
        RuntimeValue::DateTime(v) => Value::String(v.as_str().to_owned()),
        RuntimeValue::Bytes(v) => {
            if v.len() > limits.max_decoded_bytes {
                return Err(invalid());
            }
            Value::String(STANDARD.encode(v))
        }
        RuntimeValue::Json(v) => {
            json_bounds(v, limits)?;
            // Bound serialization itself, not only the allocated result.
            struct Bounded {
                bytes: Vec<u8>,
                limit: usize,
            }
            impl std::io::Write for Bounded {
                fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                    if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                        return Err(std::io::Error::other("scalar_limit"));
                    }
                    self.bytes.extend_from_slice(bytes);
                    Ok(bytes.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            let mut writer = Bounded {
                bytes: Vec::new(),
                limit: limits.max_text_bytes,
            };
            serde_json::to_writer(&mut writer, v).map_err(|_| invalid())?;
            Value::String(String::from_utf8(writer.bytes).map_err(|_| invalid())?)
        }
    };
    if let Value::String(v) = &output {
        if v.len() > limits.max_text_bytes {
            return Err(invalid());
        }
    }
    Ok(output)
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
                decode(
                    RuntimeValueKind::Integer,
                    &encode(&v, limits).unwrap(),
                    limits
                )
                .unwrap(),
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
                decode(RuntimeValueKind::Json, &encode(&v, limits).unwrap(), limits).unwrap(),
                v
            );
        }
        assert_eq!(
            encode(&RuntimeValue::Json(serde_json::Value::Null), limits).unwrap(),
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
            encode(&d, limits).unwrap(),
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
                decode(
                    RuntimeValueKind::Json,
                    &encode(&value, limits).unwrap(),
                    limits
                )
                .unwrap(),
                value
            );
        }
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
            encode(&value, limits).unwrap(),
            Value::String("67e55044-10b1-426f-9247-bb680e5fe0c8".into())
        );
        let value = decode(
            RuntimeValueKind::DateTime,
            &Value::String("2026-10-09T00:00:59.9999996Z".into()),
            limits,
        )
        .unwrap();
        assert_eq!(
            encode(&value, limits).unwrap(),
            Value::String("2026-10-09T00:01:00.000000Z".into())
        );
        let value = RuntimeValue::Json(serde_json::Value::String("hello".into()));
        assert_eq!(
            decode(
                RuntimeValueKind::Json,
                &encode(&value, limits).unwrap(),
                limits
            )
            .unwrap(),
            value
        );
        assert!(decode(RuntimeValueKind::Json, &Value::List(vec![]), limits).is_err());
        assert!(
            encode(
                &RuntimeValue::Json(serde_json::json!({"long":"text"})),
                RuntimeScalarLimits {
                    max_text_bytes: 4,
                    ..limits
                }
            )
            .is_err()
        );
    }
}
