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

/// Write `number`, as serde_json prints it, the way PostgreSQL `numeric`
/// prints it: positionally, without an exponent, keeping the input's
/// fractional digits. `1e+16` becomes `10000000000000000` and `1.5e-7`
/// becomes `0.00000015`.
fn positional(number: &str, out: &mut String) {
    let (sign, unsigned) = number
        .strip_prefix('-')
        .map_or(("", number), |unsigned| ("-", unsigned));
    let (mantissa, exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or((unsigned, 0i64), |(mantissa, exponent)| {
            (mantissa, exponent.parse().unwrap_or(0))
        });
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{integer}{fraction}");
    let shift = exponent.saturating_sub(fraction.len() as i64);
    let (integer, fraction) = if shift >= 0 {
        let zeros = usize::try_from(shift).unwrap_or(usize::MAX);
        (format!("{digits}{}", "0".repeat(zeros)), String::new())
    } else {
        let scale = usize::try_from(-shift).unwrap_or(usize::MAX);
        match digits.len().checked_sub(scale) {
            Some(split) if split > 0 => (digits[..split].to_owned(), digits[split..].to_owned()),
            _ => (
                String::new(),
                format!("{}{digits}", "0".repeat(scale - digits.len().min(scale))),
            ),
        }
    };
    let integer = integer.trim_start_matches('0');
    out.push_str(sign);
    out.push_str(if integer.is_empty() { "0" } else { integer });
    if !fraction.is_empty() {
        out.push('.');
        out.push_str(&fraction);
    }
}

/// The JSON text a PostgreSQL `jsonb` column returns for `value`, which the
/// reader then parses with serde_json. Strings and structure are unchanged
/// and spacing does not matter to the reader, so only numbers are rewritten.
fn postgres_text(value: &serde_json::Value, out: &mut String) {
    use serde_json::Value as Json;
    match value {
        Json::Number(number) => positional(&number.to_string(), out),
        Json::Array(values) => {
            out.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                postgres_text(value, out);
            }
            out.push(']');
        }
        Json::Object(values) => {
            out.push('{');
            for (index, (key, value)) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Json::String(key.clone()).to_string());
                out.push(':');
                postgres_text(value, out);
            }
            out.push('}');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// Require that the document each backend returns can be read and fits
/// `limit` once re-serialized. SQLite stores the canonical text, and
/// PostgreSQL `jsonb` stores numbers as `numeric` and prints them without
/// exponents. The reader parses either text with serde_json, which reads an
/// integer token that fits `i64` or `u64` as an integer and prints every digit
/// (`1e16` returns as `10000000000000000`), reads a longer integer back as a
/// nearby `f64` (`1e42` returns as `9.999999999999999e+41`), and cannot read
/// back the largest `f64` values at all. Both read-backs are computed with the
/// same serde_json the reader uses, so they are exact rather than estimates.
fn read_back_fits(value: &serde_json::Value, limit: usize) -> Result<(), RuntimeGraphqlError> {
    fn has_float(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Number(number) => number.is_f64(),
            serde_json::Value::Array(values) => values.iter().any(has_float),
            serde_json::Value::Object(values) => values.values().any(has_float),
            _ => false,
        }
    }
    // Integers and non-numbers read back unchanged on both backends.
    if !has_float(value) {
        return Ok(());
    }
    let canonical = value.to_string();
    let mut postgres = String::new();
    postgres_text(value, &mut postgres);
    for text in [canonical, postgres] {
        let read_back: serde_json::Value = serde_json::from_str(&text).map_err(|_| invalid())?;
        bounded_json_text(&read_back, limit)?;
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
                // Bound the text SQLite and PostgreSQL read back, not only the
                // input: serde_json re-serializes `1e15` as `1000000000000000.0`,
                // and PostgreSQL returns `1e16` as `10000000000000000`.
                bounded_json_text(&json, limits.max_text_bytes)?;
                read_back_fits(&json, limits.max_text_bytes)?;
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

std::thread_local! {
    static VALIDATING_INPUT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `future` as the request's input validation step. The runtime scalar
/// validators decode strictly, with the input bounds, while it is being
/// polled. async-graphql runs one validator closure both when it validates
/// argument, variable and default values, which happens synchronously inside
/// this step, and later when it resolves output, which must accept stored
/// values beyond the input bounds. The flag is set only for the duration of
/// each poll, so it never leaks to another task on the same thread.
pub(super) fn validating_input<'a, T: 'a>(
    future: impl std::future::Future<Output = T> + Send + 'a,
) -> impl std::future::Future<Output = T> + Send + 'a {
    struct Scope<'a, T>(std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>);
    impl<T> std::future::Future for Scope<'_, T> {
        type Output = T;
        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<T> {
            struct Restore(bool);
            impl Drop for Restore {
                fn drop(&mut self) {
                    VALIDATING_INPUT.with(|flag| flag.set(self.0));
                }
            }
            let _restore = Restore(VALIDATING_INPUT.with(|flag| flag.replace(true)));
            self.0.as_mut().poll(cx)
        }
    }
    Scope(Box::pin(future))
}

/// Run `check` with the runtime scalars validating input, as they do inside
/// [`validating_input`].
fn with_input_validation<T>(check: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            VALIDATING_INPUT.with(|flag| flag.set(self.0));
        }
    }
    let _restore = Restore(VALIDATING_INPUT.with(|flag| flag.replace(true)));
    check()
}

/// The value [`input_value_valid`] accepts at any type: it stands for an
/// argument's variable reference, whose value is checked once against the
/// variable's declared type. A GraphQL literal is never binary, so client text
/// cannot produce it.
const VALIDATED_VARIABLE: &[u8] = b"\0graphql-orm validated variable\0";
pub(super) fn validated_variable() -> Value {
    Value::Binary(VALIDATED_VARIABLE.to_vec().into())
}

/// Whether `value` is a valid input value of the GraphQL type `ty`.
///
/// This makes the checks async-graphql's argument validation makes, with
/// runtime scalars decoded under their input bounds, and also refuses what
/// async-graphql 7.2.1 accepts: a value that is not an object for an input
/// object, and a type the registry does not define. Every visited node costs one
/// unit of `budget`; an exhausted budget refuses.
pub(super) fn input_value_valid(
    registry: &async_graphql::registry::Registry,
    ty: &str,
    value: &Value,
    budget: &mut usize,
) -> bool {
    use async_graphql::registry::{MetaType, MetaTypeName};
    let Some(remaining) = budget.checked_sub(1) else {
        return false;
    };
    *budget = remaining;
    if matches!(value, Value::Binary(bytes) if bytes.as_ref() == VALIDATED_VARIABLE) {
        return true;
    }
    match MetaTypeName::create(ty) {
        MetaTypeName::NonNull(inner) => {
            value != &Value::Null && input_value_valid(registry, inner, value, budget)
        }
        MetaTypeName::List(inner) => match value {
            Value::Null => true,
            Value::List(items) => items
                .iter()
                .all(|item| input_value_valid(registry, inner, item, budget)),
            // GraphQL coerces a single value to a one-element list.
            item => input_value_valid(registry, inner, item, budget),
        },
        MetaTypeName::Named(name) => {
            if value == &Value::Null {
                return true;
            }
            match registry.types.get(name) {
                Some(MetaType::Scalar { is_valid, .. }) => is_valid
                    .as_ref()
                    .is_none_or(|valid| with_input_validation(|| valid(value))),
                Some(MetaType::Enum { enum_values, .. }) => match value {
                    Value::Enum(item) => enum_values.contains_key(item.as_str()),
                    Value::String(item) => enum_values.contains_key(item.as_str()),
                    _ => false,
                },
                Some(MetaType::InputObject {
                    input_fields,
                    oneof,
                    ..
                }) => {
                    let Value::Object(values) = value else {
                        return false;
                    };
                    if *oneof && (values.len() != 1 || values.values().any(|v| v == &Value::Null)) {
                        return false;
                    }
                    values
                        .keys()
                        .all(|name| input_fields.contains_key(name.as_str()))
                        && input_fields.values().all(|field| {
                            match values.get(field.name.as_str()) {
                                Some(value) => {
                                    input_value_valid(registry, &field.ty, value, budget)
                                }
                                None => {
                                    !MetaTypeName::create(&field.ty).is_non_null()
                                        || field.default_value.is_some()
                                }
                            }
                        })
                }
                _ => false,
            }
        }
    }
}

/// Whether every argument default of every object, interface and subscription
/// field in `registry` is a valid input value of its argument type.
///
/// async-graphql validates request values, never a schema's argument defaults,
/// and its dynamic schema passes an omitted argument's default straight to the
/// resolver. Introspection types are left out.
pub(super) fn argument_defaults_valid(registry: &async_graphql::registry::Registry) -> bool {
    use async_graphql::registry::MetaType;
    for (name, ty) in &registry.types {
        let fields = match ty {
            MetaType::Object { fields, .. } | MetaType::Interface { fields, .. }
                if !name.starts_with("__") =>
            {
                fields
            }
            _ => continue,
        };
        for argument in fields.values().flat_map(|field| field.args.values()) {
            let Some(text) = &argument.default_value else {
                continue;
            };
            // The registry keeps defaults as GraphQL literals; read one back
            // through the parser as a variable default.
            let Ok(document) = async_graphql::parser::parse_query(format!(
                "query($v: Boolean = {text}) {{ __typename }}"
            )) else {
                return false;
            };
            let Some(value) = document
                .operations
                .iter()
                .next()
                .and_then(|(_, operation)| operation.node.variable_definitions.first())
                .and_then(|definition| definition.node.default_value.as_ref())
            else {
                return false;
            };
            let mut budget = text.len().saturating_mul(2).saturating_add(64);
            if !input_value_valid(registry, &argument.ty, &value.node, &mut budget) {
                return false;
            }
        }
    }
    true
}

/// The runtime scalars' GraphQL validator: strict [`decode`] during input
/// validation, [`wire_form_valid`] otherwise.
pub(super) fn validate(kind: RuntimeValueKind, value: &Value, limits: RuntimeScalarLimits) -> bool {
    if VALIDATING_INPUT.with(std::cell::Cell::get) {
        decode(kind, value, limits).is_ok()
    } else {
        wire_form_valid(kind, value, limits)
    }
}

/// Whether `value` is a valid default for an argument of type `ty`: any value
/// typed as a runtime scalar, directly or in a list, must decode with the
/// input bounds. Other named types are the host's to check.
pub(super) fn input_default_valid(
    ty: &async_graphql::dynamic::TypeRef,
    value: &Value,
    limits: RuntimeScalarLimits,
) -> bool {
    use async_graphql::dynamic::TypeRef;
    match ty {
        TypeRef::NonNull(inner) => {
            value != &Value::Null && input_default_valid(inner, value, limits)
        }
        TypeRef::List(inner) => match value {
            Value::List(items) => items
                .iter()
                .all(|item| input_default_valid(inner, item, limits)),
            // GraphQL coerces a single value to a one-element list.
            other => input_default_valid(inner, other, limits),
        },
        TypeRef::Named(name) => {
            let kind = [
                RuntimeValueKind::Integer,
                RuntimeValueKind::Uuid,
                RuntimeValueKind::Json,
                RuntimeValueKind::Bytes,
                RuntimeValueKind::DateTime,
            ]
            .into_iter()
            .find(|kind| self::name(*kind) == name.as_ref());
            match kind {
                Some(kind) => decode(kind, value, limits).is_ok(),
                None => true,
            }
        }
    }
}

/// Whether `value` is a wire form this scalar can carry, input or output.
///
/// Output must pass the scalar's validator, and renders stored values without
/// the input bounds: date-times that earlier releases stored outside the years
/// 0001 to 9999, and JSON or bytes longer than the input limits. This accepts
/// those forms. Input validation uses [`decode`] instead; see [`validate`].
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
        // The PostgreSQL text is what `jsonb` prints, and the read back is what
        // serde_json makes of it: a long integer becomes a nearby f64.
        let postgres = |text: &str| {
            let mut out = String::new();
            postgres_text(&serde_json::from_str(text).unwrap(), &mut out);
            out
        };
        assert_eq!(
            postgres(r#"{"a":[1e16,-1.5e-7,0.5,7,1000000000000000.0,"1e16"]}"#),
            r#"{"a":[10000000000000000,-0.00000015,0.5,7,1000000000000000.0,"1e16"]}"#
        );
        assert_eq!(postgres("[1e42]"), format!("[1{}]", "0".repeat(42)));
        // `1e+42` (5 bytes) returns as `9.999999999999999e+41` (21 bytes).
        let limits = RuntimeScalarLimits {
            max_text_bytes: 46,
            ..limits
        };
        assert_round_trips(RuntimeValueKind::Json, "[1e42,-1e42]", limits);
        let limits = RuntimeScalarLimits {
            max_text_bytes: 45,
            ..limits
        };
        assert!(
            decode(
                RuntimeValueKind::Json,
                &Value::String("[1e42,-1e42]".into()),
                limits
            )
            .is_err()
        );
        // The largest f64 values cannot be read back from `jsonb` at all.
        let limits = RuntimeScalarLimits::default();
        for text in [
            "[1.7976931348623157e308]",
            "{\"a\":-1.7976931348623157e308}",
        ] {
            assert!(
                decode(RuntimeValueKind::Json, &Value::String(text.into()), limits).is_err(),
                "{text}"
            );
        }
        assert_round_trips(RuntimeValueKind::Json, "[1e300,5e-324,1.5e-7]", limits);
        // Under the default limits about 20 KB of input returns as about 74 KB.
        let limits = RuntimeScalarLimits::default();
        let text = format!("[{}]", vec!["1e16"; 4095].join(","));
        assert!(text.len() <= limits.max_text_bytes);
        assert!(decode(RuntimeValueKind::Json, &Value::String(text), limits).is_err());
        let text = format!("[{}]", vec!["1e16"; 3000].join(","));
        assert_round_trips(RuntimeValueKind::Json, &text, limits);
    }
}
