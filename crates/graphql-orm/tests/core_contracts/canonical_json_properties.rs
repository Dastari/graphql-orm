//! Bounded property suite for canonical JSON v1 runtime defaults (#725).
//!
//! [`RuntimeDefault::json`] writes the one canonical spelling of a value, and
//! [`RuntimeDefault::typed`] with [`RuntimeValueKind::Json`] accepts only that
//! spelling. The properties check that:
//!
//! - every written literal is accepted and reads back as the same value;
//! - equivalent values (key order, `-0.0`, escaped spellings, whitespace)
//!   produce identical bytes;
//! - any accepted text is a fixed point of read-then-write, and every other
//!   spelling of the same value is refused;
//! - arbitrary or oversized input never panics and fails closed at the
//!   documented depth, node and byte limits.
//!
//! serde_json reads a few decimal spellings one unit in the last place away
//! from the nearest float (#581). Read-back comparisons allow exactly that
//! difference and nothing else. Cases run from a fixed seed; set
//! `PROPTEST_CASES` or `PROPTEST_RNG_SEED` to explore further.

use graphql_orm::graphql::orm::{
    MAX_JSON_DEFAULT_BYTES, MAX_JSON_DEFAULT_DEPTH, MAX_JSON_DEFAULT_NODES, RuntimeDefault,
    RuntimeDefaultProblem, RuntimeTypedDefault, RuntimeValue, RuntimeValueKind,
};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence, RngSeed};
use serde_json::{Map, Number, Value};

fn config(cases: u32, seed: u64) -> Config {
    let env_u64 = |name: &str| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
    };
    Config {
        cases: env_u64("PROPTEST_CASES").map_or(cases, |value| value as u32),
        rng_seed: RngSeed::Fixed(env_u64("PROPTEST_RNG_SEED").unwrap_or(seed)),
        max_shrink_iters: 2048,
        source_file: Some(file!()),
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource("regressions"))),
        ..Config::default()
    }
}

/// Reads `text` as a canonical JSON v1 default.
fn read(text: &str) -> Result<Value, RuntimeDefaultProblem> {
    match RuntimeDefault::Literal(text.to_owned()).typed(RuntimeValueKind::Json) {
        Ok(RuntimeTypedDefault::Value(RuntimeValue::Json(value))) => Ok(value),
        Ok(other) => panic!("JSON default read as {other:?}"),
        Err(error) => Err(error.problem()),
    }
}

/// Writes `value` as a canonical JSON v1 default.
fn write(value: &Value) -> Result<String, RuntimeDefaultProblem> {
    match RuntimeDefault::json(value) {
        Ok(RuntimeDefault::Literal(text)) => Ok(text),
        Ok(other) => panic!("JSON default written as {other:?}"),
        Err(error) => Err(error.problem()),
    }
}

/// Value equality that tolerates only serde_json's last-place float reads.
fn same_value(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) if left.is_f64() && right.is_f64() => {
            let (left, right) = (left.as_f64().unwrap(), right.as_f64().unwrap());
            left == right || left.to_bits().abs_diff(right.to_bits()) <= 1
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(l, r)| same_value(l, r))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(key, l)| right.get(key).is_some_and(|r| same_value(l, r)))
        }
        _ => left == right,
    }
}

fn json_char() -> impl Strategy<Value = char> {
    prop_oneof![
        6 => proptest::char::range('a', 'z'),
        2 => proptest::sample::select(vec![
            '"', '\\', '/', '\u{8}', '\u{c}', '\n', '\r', '\t', '\u{1}', '\u{1f}', '\u{7f}', ' ',
            '{', '}', '[', ']', ':', ',',
        ]),
        2 => proptest::sample::select(vec![
            'é', '\u{301}', '雪', '🦀', '\u{2028}', '\u{2029}', '\u{feff}', '\u{fffd}', '\u{10ffff}',
        ]),
        1 => any::<char>(),
    ]
}

fn json_string() -> impl Strategy<Value = String> {
    vec(json_char(), 0..12).prop_map(|chars| chars.into_iter().collect())
}

/// Floats whose shortest decimal text serde_json reads back exactly; the
/// others are #581's documented last-place difference.
fn exact_float() -> impl Strategy<Value = f64> {
    prop_oneof![
        any::<f64>(),
        (-1.0e6f64..1.0e6f64),
        proptest::sample::select(vec![
            0.0,
            -0.0,
            0.1,
            0.5,
            -1.5,
            1.0e-7,
            2.5e-5,
            1.0e15,
            9.007_199_254_740_993e15,
            f64::MIN_POSITIVE,
            5e-324,
            f64::MAX,
            f64::MIN,
        ]),
    ]
    .prop_filter("finite, read back exactly", |value| {
        value.is_finite() && serde_json::from_str::<f64>(&format!("{value}")).ok() == Some(*value)
    })
}

fn json_leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        exact_float().prop_map(|value| Value::Number(Number::from_f64(value).unwrap())),
        json_string().prop_map(Value::String),
    ]
}

fn json_value() -> impl Strategy<Value = Value> {
    json_leaf().prop_recursive(6, 96, 8, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..8).prop_map(Value::Array),
            vec((json_string(), inner), 0..8)
                .prop_map(|entries| Value::Object(entries.into_iter().collect())),
        ]
    })
}

/// The same value with every object's keys inserted in reverse order.
fn reinserted(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(reinserted).collect()),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .rev()
                .map(|(key, value)| (key.clone(), reinserted(value)))
                .collect::<Map<_, _>>(),
        ),
        Value::Number(number) if number.as_f64() == Some(0.0) && number.is_f64() => {
            Value::Number(Number::from_f64(-0.0).unwrap())
        }
        other => other.clone(),
    }
}

/// Rewrites one canonical text into another spelling of the same value.
fn respelled(text: &str, choice: u8, position: usize) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    match choice % 4 {
        0 => serde_json::to_string_pretty(&value).ok(),
        1 => {
            // Whitespace after one structural character outside strings.
            let mut in_string = false;
            let mut escaped = false;
            let mut structural = Vec::new();
            for (index, c) in text.char_indices() {
                match (in_string, escaped, c) {
                    (_, true, _) => escaped = false,
                    (true, false, '\\') => escaped = true,
                    (_, false, '"') => in_string = !in_string,
                    (false, false, '{' | '[' | ',' | ':') => structural.push(index),
                    _ => {}
                }
            }
            let index = *structural.get(position % structural.len().max(1))?;
            let mut text = text.to_owned();
            text.insert(index + 1, ' ');
            Some(text)
        }
        2 => {
            // One unescaped string character as a `\u` escape.
            let mut in_string = false;
            let mut escaped = false;
            let mut hex_digits = 0;
            let mut candidates = Vec::new();
            for (index, c) in text.char_indices() {
                if hex_digits > 0 {
                    hex_digits -= 1;
                    continue;
                }
                match (in_string, escaped, c) {
                    (_, true, 'u') => {
                        escaped = false;
                        hex_digits = 4;
                    }
                    (_, true, _) => escaped = false,
                    (true, false, '\\') => escaped = true,
                    (_, false, '"') => in_string = !in_string,
                    (true, false, c) if (c as u32) < 0x10000 => candidates.push((index, c)),
                    _ => {}
                }
            }
            let (index, c) = *candidates.get(position % candidates.len().max(1))?;
            Some(format!(
                "{}\\u{:04x}{}",
                &text[..index],
                c as u32,
                &text[index + c.len_utf8()..]
            ))
        }
        _ => {
            // Surrounding whitespace.
            Some(format!("{text}\n"))
        }
    }
}

proptest! {
    #![proptest_config(config(768, 0x7250_0201))]

    #[test]
    fn written_literals_are_accepted_and_read_back(value in json_value()) {
        match write(&value) {
            Ok(text) => {
                let read_back = read(&text);
                prop_assert!(read_back.is_ok(), "{} refused: {:?}", text, read_back);
                prop_assert!(same_value(&read_back.unwrap(), &value), "{}", text);
                // Writing is idempotent: the literal is its own canonical form.
                prop_assert_eq!(write(&serde_json::from_str(&text).unwrap()), Ok(text.clone()));
            }
            Err(problem) => prop_assert!(
                matches!(
                    problem,
                    RuntimeDefaultProblem::TooLarge
                        | RuntimeDefaultProblem::TooDeep
                        | RuntimeDefaultProblem::UnsupportedPrecision
                ),
                "{:?} for {}",
                problem,
                value
            ),
        }
    }

    #[test]
    fn equivalent_values_write_identical_bytes(value in json_value()) {
        prop_assert_eq!(write(&value), write(&reinserted(&value)));
    }

    #[test]
    fn other_spellings_of_a_value_are_refused(
        value in json_value(),
        choice in any::<u8>(),
        position in any::<usize>(),
    ) {
        let Ok(canonical) = write(&value) else {
            return Ok(());
        };
        if let Some(other) = respelled(&canonical, choice, position) {
            let parsed: Value = serde_json::from_str(&other).unwrap();
            // Canonical identity: the other spelling writes the same bytes.
            prop_assert_eq!(write(&parsed), Ok(canonical.clone()));
            if other != canonical {
                prop_assert!(read(&other).is_err(), "{:?} accepted beside {:?}", other, canonical);
            }
        }
    }
}

/// Text near the JSON grammar: canonical literals, their prefixes and edits,
/// and arbitrary characters.
fn hostile_text() -> impl Strategy<Value = String> {
    let edited = (json_value(), any::<usize>(), json_char(), 0u8..3).prop_filter_map(
        "written within bounds",
        |(value, position, character, edit)| {
            let mut text: Vec<char> = write(&value).ok()?.chars().collect();
            let index = position % (text.len() + 1);
            match edit {
                0 => text.insert(index, character),
                1 if index < text.len() => {
                    text.remove(index);
                }
                _ => text.truncate(index),
            }
            Some(text.into_iter().collect())
        },
    );
    prop_oneof![
        3 => edited,
        1 => vec(json_char(), 0..48).prop_map(|chars| chars.into_iter().collect()),
        1 => vec(
            proptest::sample::select(vec![
                "0", "-", "1", "9", ".", "e", "E", "+", "\"", "\\u0000", "\\ud800", "\\udc00",
                "[", "]", "{", "}", ":", ",", "true", "null", "1e400", "-0", "00",
            ]),
            0..24,
        )
        .prop_map(|parts| parts.concat()),
    ]
}

proptest! {
    #![proptest_config(config(2048, 0x7250_0202))]

    #[test]
    fn accepted_text_is_a_fixed_point_and_never_panics(text in hostile_text()) {
        if let Ok(value) = read(&text) {
            let written = write(&value);
            prop_assert!(written.is_ok(), "accepted {:?} has no canonical spelling", text);
            let written = written.unwrap();
            if written != text {
                // Only #581's last-place float read may differ.
                let original: Value = serde_json::from_str(&text).unwrap();
                prop_assert!(
                    same_value(&original, &serde_json::from_str(&written).unwrap()),
                    "{:?} rewrote as {:?}",
                    text,
                    written
                );
            }
        }
    }
}

#[test]
fn size_limits_fail_closed_at_their_documented_bounds() {
    proptest!(config(256, 0x7250_0203), |(depth in 1usize..=48, width in 0usize..=1100, length in 0usize..=17_000)| {
        let nested = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let expected = if depth <= MAX_JSON_DEFAULT_DEPTH { Ok(()) } else { Err(RuntimeDefaultProblem::TooDeep) };
        prop_assert_eq!(read(&nested).map(|_| ()), expected);
        let value: Value = serde_json::from_str(&nested).unwrap();
        prop_assert_eq!(write(&value).map(|_| ()), expected);

        // One array node plus `width` elements.
        let wide = format!("[{}]", vec!["0"; width].join(","));
        let expected = if width < MAX_JSON_DEFAULT_NODES { Ok(()) } else { Err(RuntimeDefaultProblem::TooLarge) };
        prop_assert_eq!(read(&wide).map(|_| ()), expected);

        let long = format!("\"{}\"", "x".repeat(length));
        let expected = if long.len() <= MAX_JSON_DEFAULT_BYTES { Ok(()) } else { Err(RuntimeDefaultProblem::TooLarge) };
        prop_assert_eq!(read(&long).map(|_| ()), expected);
    });
}

/// Minimized and boundary cases retained as explicit regressions.
#[test]
fn retained_canonical_json_cases() {
    for (text, expected) in [
        ("{\"\":null}", Ok(())),
        ("{\"a\":0.0}", Ok(())),
        ("\"\\u007f\"", Err(RuntimeDefaultProblem::NonCanonical)),
        ("\"\u{7f}\"", Ok(())),
        ("\"\\ud800\"", Err(RuntimeDefaultProblem::NonCanonical)),
        ("[1.0e400]", Err(RuntimeDefaultProblem::Malformed)),
        (
            "{\"b\":1,\"a\":1}",
            Err(RuntimeDefaultProblem::NonCanonical),
        ),
        (
            "{\"a\":1,\"a\":1}",
            Err(RuntimeDefaultProblem::DuplicateKey),
        ),
        ("[\n]", Err(RuntimeDefaultProblem::NonCanonical)),
    ] {
        assert_eq!(read(text).map(|_| ()), expected, "{text:?}");
    }
}
