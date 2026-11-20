//! RFC 8785 JSON Canonicalization Scheme (JCS).
//!
//! [`rfc8785_canonical_json`] is an opt-in encoder. The fingerprints and
//! bindings that existed before catalogue 0.11.0 do not use it and keep their
//! bytes: [`crate::canonical_json_hash`], the semantic catalogue and the
//! client metadata still hash compact `serde_json` output with keys sorted by
//! UTF-8 bytes. New formats name this encoding in their algorithm identifier.
//!
//! RFC 8785 and the earlier encoding give the same bytes unless a value holds
//! a number with a fraction or exponent, an integer above 2^53 in magnitude,
//! or two keys of one object that sort differently by UTF-16 code unit than
//! by UTF-8 byte (a key above U+FFFF beside a key in U+E000..=U+FFFF).
//!
//! The encoder is exact for the double it is given. Reading JSON text into a
//! [`Value`] is the caller's step: without its `float_roundtrip` feature,
//! `serde_json` may read a long decimal as a neighbouring double.

use serde_json::Value;
use sha2::{Digest, Sha256};

/// A value RFC 8785 cannot encode.
///
/// The only such `serde_json` value is an integer that is not exactly an IEEE
/// 754 double, for example `9007199254740993`. RFC 8785 numbers are doubles,
/// so encoding it would silently change the value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rfc8785Error {
    message: &'static str,
}

impl Rfc8785Error {
    /// Returns the stable diagnostic.
    pub const fn message(&self) -> &'static str {
        self.message
    }
}

impl std::fmt::Display for Rfc8785Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for Rfc8785Error {}

/// Encodes `value` as RFC 8785 canonical JSON.
///
/// Object members are ordered by the UTF-16 code units of their names,
/// numbers use the ECMAScript `Number::toString` form, strings use the
/// `JSON.stringify` escapes, and no whitespace is written. Array order is
/// kept.
///
/// # Errors
///
/// Returns an error for an integer that is not exactly an IEEE 754 double.
pub fn rfc8785_canonical_json(value: &Value) -> Result<Vec<u8>, Rfc8785Error> {
    let mut output = String::new();
    write_value(value, &mut output)?;
    Ok(output.into_bytes())
}

/// Returns the lowercase hexadecimal SHA-256 of [`rfc8785_canonical_json`].
///
/// # Errors
///
/// Returns an error when the value has no RFC 8785 encoding.
pub fn rfc8785_json_hash(value: &Value) -> Result<String, Rfc8785Error> {
    Ok(format!(
        "{:x}",
        Sha256::digest(rfc8785_canonical_json(value)?)
    ))
}

fn write_value(value: &Value, output: &mut String) -> Result<(), Rfc8785Error> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(true) => output.push_str("true"),
        Value::Bool(false) => output.push_str("false"),
        Value::Number(number) => write_number(number, output)?,
        Value::String(string) => write_string(string, output),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_value(value, output)?;
            }
            output.push(']');
        }
        Value::Object(members) => {
            let mut members = members.iter().collect::<Vec<_>>();
            members.sort_by(|(left, _), (right, _)| left.encode_utf16().cmp(right.encode_utf16()));
            output.push('{');
            for (index, (name, value)) in members.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_string(name, output);
                output.push(':');
                write_value(value, output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

fn write_number(number: &serde_json::Number, output: &mut String) -> Result<(), Rfc8785Error> {
    const INEXACT: Rfc8785Error = Rfc8785Error {
        message: "integer is not exactly representable as an RFC 8785 number",
    };
    let double = if let Some(integer) = number.as_u64() {
        let double = integer as f64;
        // `u64::MAX as f64` rounds up to 2^64, which saturates on the way back.
        if double >= 18_446_744_073_709_551_616.0 || double as u64 != integer {
            return Err(INEXACT);
        }
        double
    } else if let Some(integer) = number.as_i64() {
        let double = integer as f64;
        if double as i64 != integer {
            return Err(INEXACT);
        }
        double
    } else {
        // `serde_json` numbers are never NaN or infinite.
        number.as_f64().ok_or(Rfc8785Error {
            message: "number is not an IEEE 754 double",
        })?
    };
    write_double(double, output);
    Ok(())
}

/// ECMAScript `Number::toString` for a finite double.
fn write_double(value: f64, output: &mut String) {
    if value == 0.0 {
        output.push('0');
        return;
    }
    if value < 0.0 {
        output.push('-');
    }
    let (digits, exponent) = shortest_digits(value.abs());
    let count = digits.len() as i32;
    // The value is 0.<digits> * 10^point.
    let point = exponent + 1;
    if count <= point && point <= 21 {
        output.push_str(&digits);
        output.extend(std::iter::repeat_n('0', (point - count) as usize));
    } else if 0 < point && point <= 21 {
        output.push_str(&digits[..point as usize]);
        output.push('.');
        output.push_str(&digits[point as usize..]);
    } else if -6 < point && point <= 0 {
        output.push_str("0.");
        output.extend(std::iter::repeat_n('0', (-point) as usize));
        output.push_str(&digits);
    } else {
        output.push_str(&digits[..1]);
        if count > 1 {
            output.push('.');
            output.push_str(&digits[1..]);
        }
        output.push('e');
        output.push(if point > 0 { '+' } else { '-' });
        output.push_str(&(point - 1).abs().to_string());
    }
}

/// Splits Rust's exponent form into its digits and decimal exponent.
fn scientific_parts(scientific: &str) -> (String, i32) {
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("exponent form always contains an exponent");
    (
        mantissa.replace('.', ""),
        exponent
            .parse()
            .expect("exponent form always has an integer exponent"),
    )
}

/// The shortest decimal digits that read back as `value`, and the decimal
/// exponent of the first digit, as ECMAScript chooses them.
///
/// Rust's exponent form without a precision gives the shortest digits and,
/// among those, the closest to the value. ECMAScript adds one rule: when two
/// shortest candidates are equally close, the one with the even last digit
/// wins. `1424953923781206.25` is such a value: Rust writes `…206.3` and
/// ECMAScript `…206.2`.
fn shortest_digits(value: f64) -> (String, i32) {
    let (digits, exponent) = scientific_parts(&format!("{value:e}"));
    // A double has at most 767 significant decimal digits, so this is exact.
    let (exact, exact_exponent) = scientific_parts(&format!("{value:.766e}"));
    let count = digits.len();
    let tie = exact_exponent == exponent
        && exact.as_bytes()[count] == b'5'
        && exact.as_bytes()[count + 1..]
            .iter()
            .all(|digit| *digit == b'0');
    if !tie {
        return (digits, exponent);
    }
    let truncated = &exact[..count];
    let last = truncated.as_bytes()[count - 1];
    let even = if (last - b'0').is_multiple_of(2) {
        truncated.to_owned()
    } else if last == b'9' {
        // The next candidate would end in zero, so it is not a shortest one.
        return (digits, exponent);
    } else {
        format!("{}{}", &truncated[..count - 1], (last + 1) as char)
    };
    let reads_back = format!("{even}e{}", exponent - (count as i32 - 1))
        .parse::<f64>()
        .is_ok_and(|parsed| parsed == value);
    if reads_back {
        (even, exponent)
    } else {
        (digits, exponent)
    }
}

fn write_string(value: &str, output: &mut String) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{8}' => output.push_str("\\b"),
            '\u{c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            control if control < '\u{20}' => {
                use std::fmt::Write as _;
                write!(output, "\\u{:04x}", control as u32).expect("writing to String cannot fail");
            }
            other => output.push(other),
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn encode(value: &Value) -> String {
        String::from_utf8(rfc8785_canonical_json(value).unwrap()).unwrap()
    }

    fn double(bits: u64) -> String {
        let mut output = String::new();
        write_double(f64::from_bits(bits), &mut output);
        output
    }

    /// The number samples of RFC 8785 appendix B.
    #[test]
    fn appendix_b_number_samples_match() {
        for (bits, expected) in [
            (0x0000_0000_0000_0000_u64, "0"),
            (0x8000_0000_0000_0000, "0"),
            (0x0000_0000_0000_0001, "5e-324"),
            (0x8000_0000_0000_0001, "-5e-324"),
            (0x7fef_ffff_ffff_ffff, "1.7976931348623157e+308"),
            (0xffef_ffff_ffff_ffff, "-1.7976931348623157e+308"),
            (0x4340_0000_0000_0000, "9007199254740992"),
            (0xc340_0000_0000_0000, "-9007199254740992"),
            (0x4430_0000_0000_0000, "295147905179352830000"),
            (0x44b5_2d02_c7e1_4af5, "9.999999999999997e+22"),
            (0x44b5_2d02_c7e1_4af6, "1e+23"),
            (0x44b5_2d02_c7e1_4af7, "1.0000000000000001e+23"),
            (0x444b_1ae4_d6e2_ef4e, "999999999999999700000"),
            (0x444b_1ae4_d6e2_ef4f, "999999999999999900000"),
            (0x444b_1ae4_d6e2_ef50, "1e+21"),
            (0x3eb0_c6f7_a0b5_ed8c, "9.999999999999997e-7"),
            (0x3eb0_c6f7_a0b5_ed8d, "0.000001"),
            (0x41b3_de43_5555_5553, "333333333.3333332"),
            (0x41b3_de43_5555_5554, "333333333.33333325"),
            (0x41b3_de43_5555_5555, "333333333.3333333"),
            (0x41b3_de43_5555_5556, "333333333.3333334"),
            (0x41b3_de43_5555_5557, "333333333.33333343"),
            (0xbecb_f647_612f_3696, "-0.0000033333333333333333"),
            (0x4314_3ff3_c1cb_0959, "1424953923781206.2"),
        ] {
            assert_eq!(double(bits), expected, "{bits:#018x}");
        }
    }

    /// The object of RFC 8785 section 3.2.3, whose member order differs
    /// between UTF-16 code units and UTF-8 bytes.
    #[test]
    fn members_sort_by_utf16_code_units() {
        let value = json!({
            "\u{20ac}": "Euro Sign",
            "\r": "Carriage Return",
            "\u{fb33}": "Hebrew Letter Dalet With Dagesh",
            "1": "One",
            "\u{1f600}": "Emoji: Grinning Face",
            "\u{80}": "Control",
            "\u{f6}": "Latin Small Letter O With Diaeresis",
        });
        let encoded = encode(&value);
        let order = [
            "Carriage Return",
            "One",
            "Control",
            "Latin Small Letter O With Diaeresis",
            "Euro Sign",
            "Emoji: Grinning Face",
            "Hebrew Letter Dalet With Dagesh",
        ];
        let positions = order
            .iter()
            .map(|label| encoded.find(label).expect("label is encoded"))
            .collect::<Vec<_>>();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        // UTF-8 byte order would put the emoji last.
        assert!(encoded.starts_with("{\"\\r\":"));
    }

    /// The whole sample of RFC 8785 section 3.2.2 and 3.2.4.
    #[test]
    fn section_3_sample_encodes_to_the_published_bytes() {
        let mut value: Value = serde_json::from_str(
            r#"{
                "numbers": [0, 1E30, 4.50, 2e-3, 0.000000000000000000000000001],
                "string": "\u20ac$\u000F\u000aA'\u0042\u0022\u005c\\\"\/",
                "literals": [null, true, false]
            }"#,
        )
        .unwrap();
        // `serde_json` reads the sample's `333333333.33333329` as a
        // neighbouring double unless its `float_roundtrip` feature is on, so
        // the correctly rounded double is supplied here.
        value["numbers"][0] = json!("333333333.33333329".parse::<f64>().unwrap());
        assert_eq!(
            encode(&value),
            "{\"literals\":[null,true,false],\"numbers\":[333333333.3333333,1e+30,4.5,0.002,1e-27],\
             \"string\":\"\u{20ac}$\\u000f\\nA'B\\\"\\\\\\\\\\\"/\"}"
        );
    }

    #[test]
    fn integers_keep_their_digits_and_inexact_integers_are_refused() {
        assert_eq!(
            encode(&json!([0, -1, 9007199254740992_u64])),
            "[0,-1,9007199254740992]"
        );
        // Exactly a double, so encoded as ECMAScript prints that double.
        assert_eq!(encode(&json!(1_u64 << 60)), "1152921504606847000");
        for inexact in [
            json!(9007199254740993_u64),
            json!(u64::MAX),
            json!(i64::MIN + 1),
            json!({"nested": [9007199254740993_i64]}),
        ] {
            assert!(rfc8785_canonical_json(&inexact).is_err(), "{inexact}");
        }
        assert_eq!(encode(&json!(i64::MIN)), "-9223372036854776000");
    }

    /// Where the earlier sorted `serde_json` encoding and RFC 8785 disagree.
    #[test]
    fn differences_from_the_earlier_encoding_are_exactly_the_documented_ones() {
        let earlier = |value: &Value| {
            String::from_utf8(crate::semantic::canonical_json_bytes(value)).unwrap()
        };
        for same in [
            json!({"b": [1, 2, {"a": null}], "a": "text\n\u{1}\"", "c": true}),
            json!({"limit": 4294967295_u32, "name": "caf\u{e9}"}),
            json!([]),
            json!({}),
        ] {
            assert_eq!(encode(&same), earlier(&same));
        }
        for (different, expected) in [
            (json!(1.0), "1"),
            (json!(1e21), "1e+21"),
            (json!(-0.0), "0"),
            (json!(1.5e-7), "1.5e-7"),
        ] {
            assert_eq!(encode(&different), expected);
        }
        assert_ne!(encode(&json!(1.0)), earlier(&json!(1.0)));
    }

    /// Every encoded double reads back as the same double, in at most 17
    /// significant digits.
    #[test]
    fn encoded_doubles_read_back_exactly() {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut checked = 0;
        while checked < 20_000 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let value = f64::from_bits(state ^ (state >> 29));
            if !value.is_finite() {
                continue;
            }
            let mut encoded = String::new();
            write_double(value, &mut encoded);
            assert_eq!(encoded.parse::<f64>().unwrap(), value, "{encoded}");
            let digits = encoded
                .split('e')
                .next()
                .unwrap()
                .chars()
                .filter(char::is_ascii_digit)
                .collect::<String>();
            let significant = digits.trim_matches('0').len();
            assert!(significant <= 17, "{encoded}");
            checked += 1;
        }
    }

    /// Two shortest candidates at the same distance: the even one is chosen.
    #[test]
    fn ties_between_shortest_candidates_choose_the_even_digit() {
        for (value, expected) in [
            (1_424_953_923_781_206.0_f64 + 0.25, "1424953923781206.2"),
            (1_424_953_923_781_206.0 + 0.75, "1424953923781206.8"),
            (-1_424_953_923_781_206.0 - 0.25, "-1424953923781206.2"),
            // Exactly representable in fewer digits: no tie to break.
            (0.5, "0.5"),
            (2.5, "2.5"),
            (2_251_799_813_685_248.0 + 0.5, "2251799813685248.5"),
        ] {
            let mut encoded = String::new();
            write_double(value, &mut encoded);
            assert_eq!(encoded, expected);
            assert_eq!(encoded.parse::<f64>().unwrap(), value);
        }
    }

    #[test]
    fn hash_is_the_sha256_of_the_canonical_bytes() {
        assert_eq!(
            rfc8785_json_hash(&json!({})).unwrap(),
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
        assert_eq!(
            rfc8785_json_hash(&json!({"b": 2, "a": 1})).unwrap(),
            rfc8785_json_hash(&json!({"a": 1, "b": 2})).unwrap()
        );
        assert_ne!(
            rfc8785_json_hash(&json!({"x": [1, 2]})).unwrap(),
            rfc8785_json_hash(&json!({"x": [2, 1]})).unwrap()
        );
    }
}
