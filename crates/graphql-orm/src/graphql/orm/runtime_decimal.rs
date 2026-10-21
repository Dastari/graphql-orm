//! Exact runtime decimals: one strict input grammar and one canonical spelling.
//!
//! A runtime decimal field carries the existing portable [`DecimalDef`]
//! (precision 1..=18, scale ≤ precision). Values are [`DecimalValue`]s and are
//! never rounded: input that needs more scale or precision than the definition
//! retains is rejected.
//!
//! Every text form outside the database uses the same rules:
//!
//! - **Input** is an optional `-`, integer digits without redundant leading
//!   zeros, and an optional `.` followed by one or more digits. There is no
//!   `+`, exponent, whitespace, digit separator or bare `.`. Extra trailing
//!   fractional zeros are accepted when they lose nothing (`"1.500"` for scale
//!   2). GraphQL and JSON additionally accept an integer number; floating
//!   JSON numbers are rejected because they are not exact.
//! - **Output** is the canonical spelling with exactly `scale` fractional
//!   digits, for example `"12.50"`, `"-0.05"` or `"7"` for scale 0. Zero is
//!   never signed. JSON always carries this as a string, never a number.

use std::fmt;

use super::{DecimalDef, DecimalValue, DecimalValueError};

/// Maximum accepted decimal input text in bytes.
pub const MAX_RUNTIME_DECIMAL_TEXT_BYTES: usize = 64;

/// Value-free reason a decimal input was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RuntimeDecimalProblem {
    /// The text is not in the strict decimal grammar.
    Malformed,
    /// The value has more fractional digits than the definition's scale.
    Scale,
    /// The value has more digits than the definition's precision.
    Precision,
}

impl RuntimeDecimalProblem {
    /// Stable machine code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::Scale => "scale",
            Self::Precision => "precision",
        }
    }
}

/// Rejected decimal input. Formatting never includes the submitted text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeDecimalError {
    problem: RuntimeDecimalProblem,
}

impl RuntimeDecimalError {
    const fn new(problem: RuntimeDecimalProblem) -> Self {
        Self { problem }
    }

    /// Value-free rejection reason.
    pub const fn problem(self) -> RuntimeDecimalProblem {
        self.problem
    }
}

impl fmt::Display for RuntimeDecimalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "runtime decimal rejected ({})", self.problem.as_str())
    }
}

impl std::error::Error for RuntimeDecimalError {}

impl From<DecimalValueError> for RuntimeDecimalError {
    fn from(error: DecimalValueError) -> Self {
        Self::new(match error {
            DecimalValueError::Scale => RuntimeDecimalProblem::Scale,
            DecimalValueError::Precision => RuntimeDecimalProblem::Precision,
        })
    }
}

fn strict_grammar(text: &str) -> bool {
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let (integer, fraction) = match unsigned.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (unsigned, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    digits(integer) && (integer == "0" || !integer.starts_with('0')) && fraction.is_none_or(digits)
}

/// Whether text is in the strict grammar, independent of any definition.
#[cfg(feature = "runtime-graphql")]
pub(crate) fn runtime_decimal_syntax(text: &str) -> bool {
    text.len() <= MAX_RUNTIME_DECIMAL_TEXT_BYTES && strict_grammar(text)
}

/// Parse strict decimal input text for a definition, rejecting any loss.
///
/// # Errors
///
/// Returns a value-free [`RuntimeDecimalError`] for malformed text or a value
/// outside the definition's precision or scale.
pub fn parse_runtime_decimal(
    definition: DecimalDef,
    text: &str,
) -> Result<DecimalValue, RuntimeDecimalError> {
    if text.len() > MAX_RUNTIME_DECIMAL_TEXT_BYTES || !strict_grammar(text) {
        return Err(RuntimeDecimalError::new(RuntimeDecimalProblem::Malformed));
    }
    // The grammar is exact decimal text, so a parse failure means the value
    // exceeds what `rust_decimal` (and therefore any definition) can hold.
    let value = rust_decimal::Decimal::from_str_exact(text)
        .map_err(|_| RuntimeDecimalError::new(RuntimeDecimalProblem::Precision))?;
    Ok(DecimalValue::new(value, definition)?)
}

/// Exact decimal from an integer, as accepted from GraphQL and JSON numbers.
///
/// # Errors
///
/// Returns [`RuntimeDecimalProblem::Precision`] when the integer does not fit.
pub fn runtime_decimal_from_integer(
    definition: DecimalDef,
    value: i128,
) -> Result<DecimalValue, RuntimeDecimalError> {
    parse_runtime_decimal(definition, &value.to_string())
}

/// The canonical spelling: exactly `scale` fractional digits, unsigned zero.
pub fn canonical_runtime_decimal(value: DecimalValue) -> String {
    let mut value = value.value();
    if value.is_zero() {
        value.set_sign_positive(true);
    }
    value.to_string()
}

/// JSON form of [`RuntimeValue::Decimal`](super::RuntimeValue::Decimal):
/// `{"value": "<canonical>", "definition": {"precision": p, "scale": s}}`.
pub(crate) mod value_serde {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize)]
    #[serde(deny_unknown_fields)]
    struct Wire<'a> {
        value: &'a str,
        definition: DecimalDef,
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Input {
        Text(String),
        Signed(i64),
        Unsigned(u64),
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ReadWire {
        value: Input,
        definition: DecimalDef,
    }

    pub(crate) fn serialize<S: serde::Serializer>(
        value: &DecimalValue,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Wire {
            value: &canonical_runtime_decimal(*value),
            definition: value.definition(),
        }
        .serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<DecimalValue, D::Error> {
        let wire = ReadWire::deserialize(deserializer)?;
        match wire.value {
            Input::Text(text) => parse_runtime_decimal(wire.definition, &text),
            Input::Signed(value) => runtime_decimal_from_integer(wire.definition, value.into()),
            Input::Unsigned(value) => runtime_decimal_from_integer(wire.definition, value.into()),
        }
        .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(precision: u8, scale: u8) -> DecimalDef {
        DecimalDef::new(precision, scale).unwrap()
    }

    #[test]
    fn strict_input_and_canonical_output() {
        let money = def(12, 2);
        for (input, canonical) in [
            ("12.5", "12.50"),
            ("12.50", "12.50"),
            ("12.500", "12.50"),
            ("0", "0.00"),
            ("-0", "0.00"),
            ("-0.00", "0.00"),
            ("-0.05", "-0.05"),
            ("9999999999.99", "9999999999.99"),
            ("-9999999999.99", "-9999999999.99"),
        ] {
            let value = parse_runtime_decimal(money, input).unwrap();
            assert_eq!(canonical_runtime_decimal(value), canonical, "{input}");
        }
        let whole = def(18, 0);
        assert_eq!(
            canonical_runtime_decimal(parse_runtime_decimal(whole, "999999999999999999").unwrap()),
            "999999999999999999"
        );
        let fraction = def(18, 18);
        assert_eq!(
            canonical_runtime_decimal(parse_runtime_decimal(fraction, "0.5").unwrap()),
            "0.500000000000000000"
        );
    }

    #[test]
    fn rejections_never_round() {
        use RuntimeDecimalProblem::*;
        let money = def(12, 2);
        for (input, problem) in [
            ("", Malformed),
            ("+1", Malformed),
            ("1e3", Malformed),
            ("1E3", Malformed),
            (" 1", Malformed),
            ("1 ", Malformed),
            ("1_000", Malformed),
            ("1,000", Malformed),
            (".5", Malformed),
            ("5.", Malformed),
            ("01", Malformed),
            ("--1", Malformed),
            ("NaN", Malformed),
            ("12.345", Scale),
            ("0.001", Scale),
            ("10000000000.00", Precision),
            ("99999999999999999999999999999999", Precision),
        ] {
            assert_eq!(
                parse_runtime_decimal(money, input).unwrap_err().problem(),
                problem,
                "{input:?}"
            );
        }
        assert_eq!(
            parse_runtime_decimal(money, &"1".repeat(65))
                .unwrap_err()
                .problem(),
            Malformed
        );
        assert_eq!(
            runtime_decimal_from_integer(def(3, 0), 1000)
                .unwrap_err()
                .problem(),
            Precision
        );
    }
}
