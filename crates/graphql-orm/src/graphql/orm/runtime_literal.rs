//! Typed, canonical literal defaults for runtime fields.
//!
//! [`RuntimeDefault::Literal`] keeps one owned text encoding. The field's
//! [`RuntimeValueKind`] selects the grammar, so serialized schemas and canonical
//! identities remain byte-identical for every literal that earlier releases
//! accepted. Structured kinds accept only one canonical spelling per value:
//!
//! - **JSON** uses *canonical JSON v1*: compact, unique object keys sorted by
//!   UTF-8 bytes, minimal string escapes, integers in the `i64`/`u64` range in
//!   minimal decimal form, and decimals with a fraction but no exponent, no
//!   trailing fractional zero other than `X.0`, no negative zero and at most 17
//!   significant digits. PostgreSQL `jsonb` preserves these spellings, so both
//!   backends return the same typed value.
//! - **Bytes** use padded standard base64 whose re-encoding is identical.
//! - **Date-times** use the [`RuntimeDateTime`] canonical form
//!   `YYYY-MM-DDTHH:MM:SS.ffffffZ` for years 0001 through 9999.
//!
//! Problems are reported as [`RuntimeDefaultProblem`] codes; no error or
//! diagnostic produced here contains the submitted literal.

use std::fmt;

use super::{RuntimeDateTime, RuntimeDefault, RuntimeFloat, RuntimeValue, RuntimeValueKind};

/// Maximum canonical JSON default text in bytes.
pub const MAX_JSON_DEFAULT_BYTES: usize = 16 * 1024;
/// Maximum array/object nesting of a JSON default.
pub const MAX_JSON_DEFAULT_DEPTH: usize = 32;
/// Maximum JSON values (including containers) in a JSON default.
pub const MAX_JSON_DEFAULT_NODES: usize = 1024;
/// Maximum decoded bytes of a bytes default.
pub const MAX_BYTES_DEFAULT_DECODED: usize = 8 * 1024;
const MAX_JSON_NUMBER_BYTES: usize = 40;
const MAX_JSON_DECIMAL_DIGITS: usize = 17;

/// Value-free reason a literal default was rejected.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeDefaultProblem {
    /// The text is not in the kind's grammar.
    Malformed,
    /// The value is valid but not spelled in its single canonical form.
    NonCanonical,
    /// A number or date lies outside the supported domain.
    OutOfRange,
    /// A floating-point value is NaN or infinite.
    Nonfinite,
    /// The literal exceeds a byte, node or digit bound.
    TooLarge,
    /// JSON nesting exceeds [`MAX_JSON_DEFAULT_DEPTH`].
    TooDeep,
    /// A JSON object repeats a key.
    DuplicateKey,
    /// A date-time names an impossible calendar date, time or offset.
    ImpossibleDateTime,
    /// Precision beyond what the backends store exactly.
    UnsupportedPrecision,
    /// The default form is not valid for the field's value kind.
    KindMismatch,
}

impl RuntimeDefaultProblem {
    /// Stable machine code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::NonCanonical => "non_canonical",
            Self::OutOfRange => "out_of_range",
            Self::Nonfinite => "nonfinite",
            Self::TooLarge => "too_large",
            Self::TooDeep => "too_deep",
            Self::DuplicateKey => "duplicate_key",
            Self::ImpossibleDateTime => "impossible_date_time",
            Self::UnsupportedPrecision => "unsupported_precision",
            Self::KindMismatch => "kind_mismatch",
        }
    }
}

impl fmt::Display for RuntimeDefaultProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Rejected default. Formatting never includes the submitted literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeDefaultError {
    problem: RuntimeDefaultProblem,
}

impl RuntimeDefaultError {
    const fn new(problem: RuntimeDefaultProblem) -> Self {
        Self { problem }
    }

    /// Value-free rejection reason.
    pub const fn problem(self) -> RuntimeDefaultProblem {
        self.problem
    }
}

impl fmt::Display for RuntimeDefaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "runtime default rejected ({})", self.problem)
    }
}

impl std::error::Error for RuntimeDefaultError {}

/// The typed value a backend produces for an omitted create input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeTypedDefault {
    /// A constant value.
    Value(RuntimeValue),
    /// A database-evaluated timestamp expression.
    CurrentTimestamp,
}

type Checked<T> = Result<T, RuntimeDefaultError>;

fn fail<T>(problem: RuntimeDefaultProblem) -> Checked<T> {
    Err(RuntimeDefaultError::new(problem))
}

impl RuntimeDefault {
    /// Boolean literal.
    pub fn boolean(value: bool) -> Self {
        Self::Literal(if value { "true" } else { "false" }.to_owned())
    }

    /// Integer literal in minimal decimal form; the full `i64` domain is supported.
    pub fn integer(value: i64) -> Self {
        Self::Literal(value.to_string())
    }

    /// Finite float literal. The text always carries a fraction or exponent, and
    /// negative zero is written as `0.0`, matching [`RuntimeFloat`] normalization.
    ///
    /// # Errors
    ///
    /// [`RuntimeDefaultProblem::Nonfinite`] for NaN or infinities.
    pub fn float(value: f64) -> Checked<Self> {
        let value = RuntimeFloat::new(value)
            .map_err(|_| RuntimeDefaultError::new(RuntimeDefaultProblem::Nonfinite))?;
        Ok(Self::Literal(format!("{:?}", value.get())))
    }

    /// Plain string literal.
    pub fn string(value: impl Into<String>) -> Self {
        Self::Literal(value.into())
    }

    /// UUID literal in lowercase hyphenated form.
    pub fn uuid(value: uuid::Uuid) -> Self {
        Self::Literal(value.hyphenated().to_string())
    }

    /// Canonical JSON v1 literal for a JSON value.
    ///
    /// # Errors
    ///
    /// Returns the precise [`RuntimeDefaultProblem`] when the value has no
    /// canonical v1 spelling within the JSON default bounds.
    pub fn json(value: &serde_json::Value) -> Checked<Self> {
        let mut text = String::new();
        let mut nodes = 0usize;
        write_canonical_json(value, 0, &mut nodes, &mut text)?;
        check_canonical_json(&text)?;
        Ok(Self::Literal(text))
    }

    /// Canonical padded standard base64 literal.
    ///
    /// # Errors
    ///
    /// [`RuntimeDefaultProblem::TooLarge`] above [`MAX_BYTES_DEFAULT_DECODED`].
    pub fn bytes(value: &[u8]) -> Checked<Self> {
        if value.len() > MAX_BYTES_DEFAULT_DECODED {
            return fail(RuntimeDefaultProblem::TooLarge);
        }
        Ok(Self::Literal(base64_encode(value)))
    }

    /// Canonical UTC microsecond date-time literal.
    ///
    /// # Errors
    ///
    /// [`RuntimeDefaultProblem::OutOfRange`] outside years 0001 through 9999.
    pub fn datetime(value: &RuntimeDateTime) -> Checked<Self> {
        check_canonical_datetime(value.as_str())?;
        Ok(Self::Literal(value.as_str().to_owned()))
    }

    /// Validate this default against `kind` and return the typed value the
    /// backends produce when a create input omits the field.
    ///
    /// # Errors
    ///
    /// Returns the value-free [`RuntimeDefaultProblem`].
    pub fn typed(&self, kind: RuntimeValueKind) -> Checked<RuntimeTypedDefault> {
        match self {
            Self::CurrentTimestamp => match kind {
                RuntimeValueKind::DateTime | RuntimeValueKind::Integer => {
                    Ok(RuntimeTypedDefault::CurrentTimestamp)
                }
                _ => fail(RuntimeDefaultProblem::KindMismatch),
            },
            Self::Literal(text) => typed_literal(text, kind).map(RuntimeTypedDefault::Value),
        }
    }
}

fn typed_literal(text: &str, kind: RuntimeValueKind) -> Checked<RuntimeValue> {
    use RuntimeDefaultProblem as P;
    match kind {
        // Pre-existing kinds keep exactly the grammar earlier releases accepted.
        RuntimeValueKind::Boolean => match text {
            "true" => Ok(RuntimeValue::Boolean(true)),
            "false" => Ok(RuntimeValue::Boolean(false)),
            _ => fail(P::Malformed),
        },
        RuntimeValueKind::Integer => match text.parse::<i64>() {
            Ok(value) => Ok(RuntimeValue::Integer(value)),
            Err(error) => fail(match error.kind() {
                std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow => {
                    P::OutOfRange
                }
                _ => P::Malformed,
            }),
        },
        RuntimeValueKind::Float => {
            let value = text
                .parse::<f64>()
                .map_err(|_| RuntimeDefaultError::new(P::Malformed))?;
            RuntimeFloat::new(value)
                .map(RuntimeValue::Float)
                .map_err(|_| RuntimeDefaultError::new(P::Nonfinite))
        }
        RuntimeValueKind::String => Ok(RuntimeValue::String(text.to_owned())),
        RuntimeValueKind::Uuid => uuid::Uuid::parse_str(text)
            .map(RuntimeValue::Uuid)
            .map_err(|_| RuntimeDefaultError::new(P::Malformed)),
        RuntimeValueKind::Json => {
            check_canonical_json(text)?;
            serde_json::from_str(text)
                .map(RuntimeValue::Json)
                .map_err(|_| RuntimeDefaultError::new(P::Malformed))
        }
        RuntimeValueKind::Bytes => base64_decode_canonical(text).map(RuntimeValue::Bytes),
        RuntimeValueKind::DateTime => {
            check_canonical_datetime(text)?;
            RuntimeDateTime::parse(text)
                .map(RuntimeValue::DateTime)
                .map_err(|_| RuntimeDefaultError::new(P::ImpossibleDateTime))
        }
    }
}

// ----- canonical date-time -------------------------------------------------

const DATETIME_SHAPE: &[u8; 27] = b"dddd-dd-ddTdd:dd:dd.ddddddZ";

pub(super) fn check_canonical_datetime(text: &str) -> Checked<()> {
    use RuntimeDefaultProblem as P;
    let bytes = text.as_bytes();
    let shaped = bytes.len() == DATETIME_SHAPE.len()
        && bytes
            .iter()
            .zip(DATETIME_SHAPE)
            .all(|(byte, shape)| match shape {
                b'd' => byte.is_ascii_digit(),
                other => byte == other,
            });
    if !shaped {
        // A valid RFC 3339 instant in another spelling is non-canonical, not malformed.
        return fail(
            if text.len() <= 64 && chrono::DateTime::parse_from_rfc3339(text).is_ok() {
                P::NonCanonical
            } else {
                P::Malformed
            },
        );
    }
    if &text[..4] == "0000" {
        return fail(P::OutOfRange);
    }
    let parsed = chrono::DateTime::parse_from_rfc3339(text)
        .map_err(|_| RuntimeDefaultError::new(P::ImpossibleDateTime))?;
    let canonical = RuntimeDateTime::parse(text)
        .map_err(|_| RuntimeDefaultError::new(P::ImpossibleDateTime))?;
    if canonical.as_str() != text || parsed.timestamp_subsec_nanos() % 1_000 != 0 {
        return fail(P::NonCanonical);
    }
    Ok(())
}

// ----- canonical base64 ----------------------------------------------------

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(super) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.len();
        let block = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            if index <= n {
                let sextet = (block >> (18 - 6 * index)) & 0x3f;
                out.push(char::from(BASE64_ALPHABET[sextet as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_value(byte: u8) -> Option<u32> {
    BASE64_ALPHABET
        .iter()
        .position(|candidate| *candidate == byte)
        .map(|index| index as u32)
}

fn base64_decode_canonical(text: &str) -> Checked<Vec<u8>> {
    use RuntimeDefaultProblem as P;
    let max_encoded = MAX_BYTES_DEFAULT_DECODED.div_ceil(3) * 4;
    if text.len() > max_encoded {
        return fail(P::TooLarge);
    }
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return fail(P::Malformed);
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (block_index, block) in bytes.chunks(4).enumerate() {
        let last = block_index + 1 == bytes.len() / 4;
        let padding = block.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && !last) {
            return fail(P::Malformed);
        }
        let mut value = 0u32;
        for byte in &block[..4 - padding] {
            value =
                (value << 6) | base64_value(*byte).ok_or(RuntimeDefaultError::new(P::Malformed))?;
        }
        value <<= 6 * padding as u32;
        let decoded = [(value >> 16) as u8, (value >> 8) as u8, value as u8];
        out.extend_from_slice(&decoded[..3 - padding]);
    }
    if out.len() > MAX_BYTES_DEFAULT_DECODED {
        return fail(P::TooLarge);
    }
    // Nonzero unused trailing bits decode, but are not the canonical spelling.
    if base64_encode(&out) != text {
        return fail(P::NonCanonical);
    }
    Ok(out)
}

// ----- canonical JSON v1 ---------------------------------------------------

fn write_canonical_json(
    value: &serde_json::Value,
    depth: usize,
    nodes: &mut usize,
    out: &mut String,
) -> Checked<()> {
    use RuntimeDefaultProblem as P;
    *nodes += 1;
    if *nodes > MAX_JSON_DEFAULT_NODES {
        return fail(P::TooLarge);
    }
    if out.len() > MAX_JSON_DEFAULT_BYTES {
        return fail(P::TooLarge);
    }
    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        serde_json::Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                out.push_str(&value.to_string());
            } else if let Some(value) = number.as_u64() {
                out.push_str(&value.to_string());
            } else {
                let value = number
                    .as_f64()
                    .filter(|value| value.is_finite())
                    .ok_or(RuntimeDefaultError::new(P::Nonfinite))?;
                // `Display` never uses exponent notation; keep the float kind.
                let mut text = format!("{}", if value == 0.0 { 0.0 } else { value });
                if !text.contains('.') {
                    text.push_str(".0");
                }
                out.push_str(&text);
            }
        }
        serde_json::Value::String(value) => write_json_string(value, out),
        serde_json::Value::Array(values) => {
            if depth >= MAX_JSON_DEFAULT_DEPTH {
                return fail(P::TooDeep);
            }
            out.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical_json(value, depth + 1, nodes, out)?;
            }
            out.push(']');
        }
        serde_json::Value::Object(values) => {
            if depth >= MAX_JSON_DEFAULT_DEPTH {
                return fail(P::TooDeep);
            }
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            out.push('{');
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_json_string(key, out);
                out.push(':');
                write_canonical_json(value, depth + 1, nodes, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// Minimal escaping shared by the canonical writer and checker.
pub(super) fn write_json_string(value: &str, out: &mut String) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if u32::from(control) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", u32::from(control)));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// Validate canonical JSON v1 text without allocating a value tree.
pub(super) fn check_canonical_json(text: &str) -> Checked<()> {
    if text.len() > MAX_JSON_DEFAULT_BYTES {
        return fail(RuntimeDefaultProblem::TooLarge);
    }
    let mut checker = JsonChecker {
        bytes: text.as_bytes(),
        text,
        index: 0,
        nodes: 0,
    };
    checker.value(0)?;
    if checker.index != checker.bytes.len() {
        return fail(RuntimeDefaultProblem::Malformed);
    }
    Ok(())
}

struct JsonChecker<'a> {
    bytes: &'a [u8],
    text: &'a str,
    index: usize,
    nodes: usize,
}

impl JsonChecker<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn expect(&mut self, byte: u8) -> Checked<()> {
        if self.peek() == Some(byte) {
            self.index += 1;
            Ok(())
        } else {
            fail(self.unexpected())
        }
    }

    // Insignificant whitespace is valid JSON but never canonical.
    fn unexpected(&self) -> RuntimeDefaultProblem {
        match self.peek() {
            Some(b' ' | b'\n' | b'\r' | b'\t') => RuntimeDefaultProblem::NonCanonical,
            _ => RuntimeDefaultProblem::Malformed,
        }
    }

    fn value(&mut self, depth: usize) -> Checked<()> {
        use RuntimeDefaultProblem as P;
        self.nodes += 1;
        if self.nodes > MAX_JSON_DEFAULT_NODES {
            return fail(P::TooLarge);
        }
        match self.peek() {
            Some(b'{') => {
                if depth >= MAX_JSON_DEFAULT_DEPTH {
                    return fail(P::TooDeep);
                }
                self.index += 1;
                let mut previous: Option<String> = None;
                if self.peek() == Some(b'}') {
                    self.index += 1;
                    return Ok(());
                }
                loop {
                    let key = self.string()?;
                    if let Some(previous) = &previous {
                        match previous.as_bytes().cmp(key.as_bytes()) {
                            std::cmp::Ordering::Less => {}
                            std::cmp::Ordering::Equal => return fail(P::DuplicateKey),
                            std::cmp::Ordering::Greater => return fail(P::NonCanonical),
                        }
                    }
                    previous = Some(key);
                    self.expect(b':')?;
                    self.value(depth + 1)?;
                    match self.peek() {
                        Some(b',') => self.index += 1,
                        Some(b'}') => {
                            self.index += 1;
                            return Ok(());
                        }
                        _ => return fail(self.unexpected()),
                    }
                }
            }
            Some(b'[') => {
                if depth >= MAX_JSON_DEFAULT_DEPTH {
                    return fail(P::TooDeep);
                }
                self.index += 1;
                if self.peek() == Some(b']') {
                    self.index += 1;
                    return Ok(());
                }
                loop {
                    self.value(depth + 1)?;
                    match self.peek() {
                        Some(b',') => self.index += 1,
                        Some(b']') => {
                            self.index += 1;
                            return Ok(());
                        }
                        _ => return fail(self.unexpected()),
                    }
                }
            }
            Some(b'"') => self.string().map(|_| ()),
            Some(b't') => self.literal("true"),
            Some(b'f') => self.literal("false"),
            Some(b'n') => self.literal("null"),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => fail(self.unexpected()),
        }
    }

    fn literal(&mut self, word: &str) -> Checked<()> {
        if self.bytes[self.index..].starts_with(word.as_bytes()) {
            self.index += word.len();
            Ok(())
        } else {
            fail(RuntimeDefaultProblem::Malformed)
        }
    }

    fn string(&mut self) -> Checked<String> {
        use RuntimeDefaultProblem as P;
        if self.peek() != Some(b'"') {
            return fail(self.unexpected());
        }
        self.index += 1;
        let mut value = String::new();
        loop {
            let rest = &self.text[self.index..];
            let character = rest
                .chars()
                .next()
                .ok_or(RuntimeDefaultError::new(P::Malformed))?;
            self.index += character.len_utf8();
            match character {
                '"' => break,
                '\\' => {
                    let escape = self.peek().ok_or(RuntimeDefaultError::new(P::Malformed))?;
                    self.index += 1;
                    let decoded = match escape {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hex = self
                                .text
                                .get(self.index..self.index + 4)
                                .ok_or(RuntimeDefaultError::new(P::Malformed))?;
                            if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                                return fail(P::Malformed);
                            }
                            self.index += 4;
                            let code = u32::from_str_radix(hex, 16)
                                .map_err(|_| RuntimeDefaultError::new(P::Malformed))?;
                            // Only controls without a short escape use `\u`, in lowercase.
                            let canonical = code < 0x20
                                && !matches!(code, 0x08 | 0x09 | 0x0a | 0x0c | 0x0d)
                                && hex == format!("{code:04x}");
                            if !canonical {
                                return fail(P::NonCanonical);
                            }
                            char::from_u32(code).ok_or(RuntimeDefaultError::new(P::Malformed))?
                        }
                        b'/' => return fail(P::NonCanonical),
                        _ => return fail(P::Malformed),
                    };
                    value.push(decoded);
                }
                control if u32::from(control) < 0x20 => return fail(P::Malformed),
                other => value.push(other),
            }
        }
        Ok(value)
    }

    fn number(&mut self) -> Checked<()> {
        use RuntimeDefaultProblem as P;
        let start = self.index;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E'))
        {
            self.index += 1;
        }
        let token = &self.text[start..self.index];
        check_json_number(token).map_err(|problem| {
            RuntimeDefaultError::new(
                if token.len() > MAX_JSON_NUMBER_BYTES && problem != P::Malformed {
                    P::TooLarge
                } else {
                    problem
                },
            )
        })
    }
}

fn check_json_number(token: &str) -> Result<(), RuntimeDefaultProblem> {
    use RuntimeDefaultProblem as P;
    let unsigned = token.strip_prefix('-').unwrap_or(token);
    let negative = unsigned.len() != token.len();
    let (integer, fraction) = match unsigned.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (unsigned, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    if token.contains(['e', 'E', '+']) {
        // Exponents are valid JSON, but jsonb rewrites them; never canonical.
        let valid = serde_json::from_str::<serde_json::Number>(token).is_ok();
        return Err(if valid { P::NonCanonical } else { P::Malformed });
    }
    if !digits(integer) || fraction.is_some_and(|fraction| !digits(fraction)) {
        return Err(P::Malformed);
    }
    if integer.len() > 1 && integer.starts_with('0') {
        return Err(P::Malformed);
    }
    if token.len() > MAX_JSON_NUMBER_BYTES {
        return Err(P::TooLarge);
    }
    match fraction {
        None => {
            if negative && integer == "0" {
                return Err(P::NonCanonical);
            }
            let in_range = if negative {
                token.parse::<i64>().is_ok()
            } else {
                token.parse::<u64>().is_ok()
            };
            if in_range { Ok(()) } else { Err(P::OutOfRange) }
        }
        Some(fraction) => {
            if fraction.len() > 1 && fraction.ends_with('0') {
                return Err(P::NonCanonical);
            }
            let zero = integer == "0" && fraction.bytes().all(|b| b == b'0');
            if negative && zero {
                return Err(P::NonCanonical);
            }
            let significant = format!("{integer}{fraction}");
            let significant = significant.trim_start_matches('0');
            let significant = if fraction == "0" {
                significant.trim_end_matches('0')
            } else {
                significant
            };
            if significant.len() > MAX_JSON_DECIMAL_DIGITS {
                return Err(P::UnsupportedPrecision);
            }
            match token.parse::<f64>() {
                Ok(value) if value.is_finite() => Ok(()),
                _ => Err(P::OutOfRange),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RuntimeDefaultProblem as P;

    fn json_problem(text: &str) -> Option<RuntimeDefaultProblem> {
        check_canonical_json(text)
            .err()
            .map(RuntimeDefaultError::problem)
    }

    #[test]
    fn canonical_json_v1_accepts_one_spelling_per_value() {
        for text in [
            "null",
            "true",
            "0",
            "-1",
            "18446744073709551615",
            "-9223372036854775808",
            "1.0",
            "-1.5",
            "0.000001",
            "\"\"",
            "\"雪🦀 'quote' \\\"dq\\\" \\\\ \\n \\u0001\"",
            "[]",
            "{}",
            "{\"a\":1,\"b\":[true,null],\"é\":{}}",
        ] {
            assert_eq!(json_problem(text), None, "{text}");
            let value: serde_json::Value = serde_json::from_str(text).unwrap();
            assert_eq!(
                RuntimeDefault::json(&value).unwrap(),
                RuntimeDefault::Literal(text.to_owned()),
                "{text}"
            );
        }
        for (text, problem) in [
            ("", P::Malformed),
            (" null", P::NonCanonical),
            ("{\"a\": 1}", P::NonCanonical),
            ("{\"b\":1,\"a\":2}", P::NonCanonical),
            ("{\"a\":1,\"a\":2}", P::DuplicateKey),
            ("1e2", P::NonCanonical),
            ("1.50", P::NonCanonical),
            ("-0", P::NonCanonical),
            ("-0.0", P::NonCanonical),
            ("01", P::Malformed),
            ("18446744073709551616", P::OutOfRange),
            ("0.123456789012345678", P::UnsupportedPrecision),
            ("\"\\/\"", P::NonCanonical),
            ("\"\\u0041\"", P::NonCanonical),
            ("\"\\u001F\"", P::NonCanonical),
            ("\"\u{1}\"", P::Malformed),
            ("[1,]", P::Malformed),
            ("NaN", P::Malformed),
            ("{\"a\":1}x", P::Malformed),
        ] {
            assert_eq!(json_problem(text), Some(problem), "{text:?}");
        }
        let deep = format!("{}{}", "[".repeat(33), "]".repeat(33));
        assert_eq!(json_problem(&deep), Some(P::TooDeep));
        let wide = format!("[{}]", vec!["0"; MAX_JSON_DEFAULT_NODES].join(","));
        assert_eq!(json_problem(&wide), Some(P::TooLarge));
        let long = format!("\"{}\"", "x".repeat(MAX_JSON_DEFAULT_BYTES));
        assert_eq!(json_problem(&long), Some(P::TooLarge));
    }

    #[test]
    fn json_constructor_normalizes_floats_and_key_order() {
        let value = serde_json::json!({"z": 2.0, "a": -0.0, "m": 0.1});
        assert_eq!(
            RuntimeDefault::json(&value).unwrap(),
            RuntimeDefault::Literal("{\"a\":0.0,\"m\":0.1,\"z\":2.0}".into())
        );
        assert_eq!(
            RuntimeDefault::json(&serde_json::json!(1e300))
                .unwrap_err()
                .problem(),
            P::TooLarge
        );
    }

    #[test]
    fn base64_is_canonical_and_bounded() {
        for bytes in [&b""[..], b"\0", b"\x01\x02", b"abc", &[0xff; 4]] {
            let RuntimeDefault::Literal(text) = RuntimeDefault::bytes(bytes).unwrap() else {
                unreachable!()
            };
            assert_eq!(base64_decode_canonical(&text).unwrap(), bytes);
        }
        for (text, problem) in [
            ("AQI", P::Malformed),
            ("AQJ=", P::NonCanonical),
            ("AQ==AQ==", P::Malformed),
            ("A===", P::Malformed),
            ("AQ-=", P::Malformed),
            ("AQI\n", P::Malformed),
        ] {
            assert_eq!(
                base64_decode_canonical(text).unwrap_err().problem(),
                problem,
                "{text}"
            );
        }
        assert_eq!(
            RuntimeDefault::bytes(&[0; MAX_BYTES_DEFAULT_DECODED + 1])
                .unwrap_err()
                .problem(),
            P::TooLarge
        );
        let max = base64_encode(&[7; MAX_BYTES_DEFAULT_DECODED]);
        assert_eq!(
            base64_decode_canonical(&max).unwrap().len(),
            MAX_BYTES_DEFAULT_DECODED
        );
    }

    #[test]
    fn datetimes_are_canonical_utc_microseconds() {
        assert!(check_canonical_datetime("2026-10-20T12:00:00.000000Z").is_ok());
        assert!(check_canonical_datetime("0001-01-01T00:00:00.000000Z").is_ok());
        assert!(check_canonical_datetime("9999-12-31T23:59:59.999999Z").is_ok());
        for (text, problem) in [
            ("2026-10-20T12:00:00Z", P::NonCanonical),
            ("2026-10-20T22:00:00.000000+10:00", P::NonCanonical),
            ("2026-02-30T00:00:00.000000Z", P::ImpossibleDateTime),
            ("2026-10-20T24:00:00.000000Z", P::ImpossibleDateTime),
            ("0000-01-01T00:00:00.000000Z", P::OutOfRange),
            ("2026-10-20 12:00:00", P::Malformed),
            ("tomorrow", P::Malformed),
        ] {
            assert_eq!(
                check_canonical_datetime(text).unwrap_err().problem(),
                problem,
                "{text}"
            );
        }
    }

    #[test]
    fn floats_keep_their_kind_and_full_domain() {
        for (value, text) in [
            (1.0, "1.0"),
            (-0.0, "0.0"),
            (-1.5, "-1.5"),
            (f64::MAX, "1.7976931348623157e308"),
            (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
        ] {
            assert_eq!(
                RuntimeDefault::float(value).unwrap(),
                RuntimeDefault::Literal(text.into())
            );
        }
        assert_eq!(
            RuntimeDefault::float(f64::NAN).unwrap_err().problem(),
            P::Nonfinite
        );
        assert_eq!(
            RuntimeDefault::Literal("1e999".into())
                .typed(RuntimeValueKind::Float)
                .unwrap_err()
                .problem(),
            P::Nonfinite
        );
        assert_eq!(
            RuntimeDefault::Literal("-0.0".into())
                .typed(RuntimeValueKind::Float)
                .unwrap(),
            RuntimeTypedDefault::Value(RuntimeValue::Float(RuntimeFloat::new(0.0).unwrap()))
        );
        for value in [i64::MIN, i64::MAX, -1, 2_147_483_648] {
            assert_eq!(
                RuntimeDefault::integer(value)
                    .typed(RuntimeValueKind::Integer)
                    .unwrap(),
                RuntimeTypedDefault::Value(RuntimeValue::Integer(value))
            );
        }
        assert_eq!(
            RuntimeDefault::Literal("9223372036854775808".into())
                .typed(RuntimeValueKind::Integer)
                .unwrap_err()
                .problem(),
            P::OutOfRange
        );
    }
}
